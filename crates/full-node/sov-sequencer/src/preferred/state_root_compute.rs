//! Defines the logic for computing state roots for the preferred sequencer.

use std::collections::BTreeMap;

use sov_modules_api::capabilities::RollupHeight;
use sov_modules_api::{Spec, Storage};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::{future_or_shutdown, FutureOrShutdownOutput};
use sov_state::{NativeStorage, SlotValue, StateAccesses, StateRoot};
use tokio::sync::{mpsc, oneshot, watch};

/// The memory limit for old write sets that we keep around. These old sets are useful because they tell us what keys have been changed.
/// Which lets us output a much handier error message when the state root computation changes.
const MEMORY_LIMIT_FOR_STATE_ROOT_COMPUTATION: usize = 100_000_000; // 100MB
const MAX_STATE_ROOTS_TO_CACHE: usize = 100;
const NUM_STATE_ROOT_COMPUTE_REQUESTS: usize = 50;

pub(crate) struct StateRootComputeRequest<S: Spec> {
    pub state_accesses: StateAccesses,
    pub storage: S::Storage,
    pub rollup_height: RollupHeight,
    pub max_slot_number: SlotNumber,
    pub response_channel: oneshot::Sender<(RollupHeight, <S::Storage as Storage>::Root)>,
}

struct StateRootCacheEntry<S: Spec> {
    root: <S::Storage as Storage>::Root,
    writes: Option<Vec<(sov_state::SlotKey, Option<SlotValue>)>>,
    size: usize,
}

fn user_roots_match<S: StateRoot>(old_root: &S, new_root: &S) -> bool {
    let old_user_root = old_root.namespace_root(sov_state::ProvableNamespace::User);
    let new_user_root = new_root.namespace_root(sov_state::ProvableNamespace::User);
    old_user_root == new_user_root
}

impl<S: Spec> StateRootCacheEntry<S> {
    // Assert that the state root is consistent with the write set.
    async fn assert_consistency(
        &self,
        state_accesses: StateAccesses,
        storage: S::Storage,
        rollup_height: RollupHeight,
        slot_number: SlotNumber,
    ) -> (RollupHeight, <S::Storage as Storage>::Root) {
        let new_writes = state_accesses.user.ordered_writes.clone();
        let new_root =
            compute_state_root::<S>(state_accesses, storage, rollup_height, slot_number).await;
        if user_roots_match(&self.root, &new_root) {
            return (rollup_height, new_root);
        }

        tracing::error!(%rollup_height, initial_root = %self.root, new_root = %new_root, description = %Self::describe_mismatch(&new_writes, &self.writes), "State root computation has changed. This is a bug.");
        panic!("State root computation has changed. This is a bug.");
    }

    fn describe_mismatch(
        new_write_set: &Vec<(sov_state::SlotKey, Option<SlotValue>)>,
        old_write_set: &Option<Vec<(sov_state::SlotKey, Option<SlotValue>)>>,
    ) -> String {
        use std::fmt::Write;
        let Some(old_write_set) = old_write_set else {
            return String::new();
        };
        let mut description = String::new();
        description.push_str("New writes:\n");
        for (key, value) in new_write_set {
            if description.len() > 100_000 {
                description.push_str("...\n");
                break;
            }
            let _ = description.write_fmt(format_args!(
                "{} => {}\n",
                key,
                SlotValue::debug_show(value.as_ref())
            ));
        }
        description.push_str("Old writes:\n");
        for (key, value) in old_write_set {
            if description.len() > 200_000 {
                description.push_str("...\n");
                break;
            }
            let _ = description.write_fmt(format_args!(
                "{} => {}\n",
                key,
                SlotValue::debug_show(value.as_ref())
            ));
        }

        description
    }
}

pub(super) struct StateRootBackgroundTaskState<S: Spec> {
    pub request_sender: mpsc::Sender<StateRootComputeRequest<S>>,
}

async fn compute_state_root<S: Spec>(
    state_accesses: StateAccesses,
    storage: S::Storage,
    rollup_height: RollupHeight,
    slot_number: SlotNumber,
) -> <S::Storage as Storage>::Root {
    let handle = tokio::runtime::Handle::current().spawn_blocking(move || {
        tracing::trace!(%rollup_height, "Computing sequencer state root for height");
        tracing::span!(tracing::Level::DEBUG, "compute_state_update", scope = "sequencer", %rollup_height, %slot_number)
            .in_scope(|| {
                let prev_root = storage
                    .get_latest_root_hash()
                    .expect("Failed to get root hash");
                let compute_result = storage
                    .compute_state_update(state_accesses, &Default::default(), prev_root);
                match compute_result {
                    Ok((root, _state_update)) => root,
                    Err(error) => {
                        // TODO: Use better error matching when sov-state uses this error. See issues:
                        //    * https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2634
                        //    * https://github.com/Sovereign-Labs/sovereign-sdk/issues/473
                        if error.to_string().contains("stale") {
                            tracing::trace!(
                                %slot_number,
                                %rollup_height,
                                "Stale storage detected, going to fetch unbound root hash");
                            storage.get_root_hash_unbound(slot_number).unwrap_or_else(|error| {
                                tracing::error!(%rollup_height, %error, "Slot number {} was detected to be stale during state root computation, but the corresponding state root could not be found in storage. This is a bug, please report it", slot_number);
                                panic!("Failed to fetch target root hash at slot number = {} after staled attempted to compute state update: {}. This is a bug, please report it",
                                       slot_number, error
                                );
                            })
                        } else {
                            tracing::error!(%rollup_height, %error, "failed to compute valid state update. This is a bug, please report it");
                            panic!("Failed to compute valid state for height {} in sequencer. This is a bug, please report it", rollup_height);
                        }
                    }
                }
            })
    });
    handle.await.unwrap()
}

impl<S: Spec> StateRootBackgroundTaskState<S> {
    pub(super) fn create(
        shutdown_notifier: mpsc::Sender<()>,
        shutdown_receiver: watch::Receiver<()>,
        check_state_roots: bool,
    ) -> (tokio::task::JoinHandle<()>, StateRootBackgroundTaskState<S>) {
        tracing::info!("Starting sequencer state root computation background task");
        let (request_sender, mut request_receiver) = mpsc::channel(NUM_STATE_ROOT_COMPUTE_REQUESTS);

        let mut cached_results: BTreeMap<RollupHeight, StateRootCacheEntry<S>> = BTreeMap::new();
        let mut cached_results_size = 0;
        let handle = tokio::spawn(async move {
            loop {
                // Wait for a new request, or shutdown.
                let StateRootComputeRequest::<S> {
                    state_accesses,
                    storage,
                    rollup_height,
                    max_slot_number,
                    response_channel,
                    ..
                } = match future_or_shutdown(request_receiver.recv(), &shutdown_receiver).await {
                    FutureOrShutdownOutput::Output(Some(request)) => request,
                    FutureOrShutdownOutput::Shutdown => {
                        tracing::info!(
                            "Sequencer state root background task shutdown in response to signal",
                        );
                        break;
                    }
                    FutureOrShutdownOutput::Output(None) => {
                        tracing::info!(
                            "All state root compute request senders were dropped, shutting down sequencer state root computation loop",
                        );
                        break;
                    }
                };

                // If the entry is in cache, check that the state root is consistent and return early
                if let Some(cached_entry) = cached_results.get(&rollup_height) {
                    tracing::trace!(%rollup_height, "Known state root");
                    // If we're checking that the state roots are equal, we have some work to do.
                    let result = if check_state_roots {
                        cached_entry
                            .assert_consistency(
                                state_accesses,
                                storage,
                                rollup_height,
                                max_slot_number,
                            )
                            .await
                    } else {
                        (rollup_height, cached_entry.root.clone())
                    };
                    let _ = response_channel.send(result);
                    continue;
                }

                // If the entry wasn't in cache, we'll need to add it. Check if we should save the write set.
                let writes = &state_accesses.user.ordered_writes;
                tracing::trace!(%rollup_height, user_space_writes = writes.len(), "New state root");
                let (writes_to_save, size) = if check_state_roots {
                    let mut size = 0;
                    for (key, value) in writes {
                        size += key.as_ref().len()
                            + value.as_ref().map(|v| v.value().len()).unwrap_or(0);
                    }
                    // Skip caching the write set if it would take up too much of our available memory. This prevents just a few large write
                    // sets from hogging the cache.
                    if size > MEMORY_LIMIT_FOR_STATE_ROOT_COMPUTATION / 5 {
                        (None, 0)
                    } else {
                        (Some(writes.clone()), size)
                    }
                } else {
                    (None, 0)
                };

                // Compute the new root
                let root = compute_state_root::<S>(
                    state_accesses,
                    storage,
                    rollup_height,
                    max_slot_number,
                )
                .await;

                // Add the new entry to the cache
                cached_results.insert(
                    rollup_height,
                    StateRootCacheEntry {
                        root: root.clone(),
                        writes: writes_to_save,
                        size,
                    },
                );
                tracing::trace!(%rollup_height, %root, %size, "Added new state root to cache");
                cached_results_size += size;

                // Prune the cache if necessary
                Self::prune_cache(&mut cached_results, &mut cached_results_size);
                let _ = response_channel.send((rollup_height, root.clone()));
            }
            drop(shutdown_notifier);
            tracing::info!(%cached_results_size, "State root background task shutdown");
        });

        (handle, StateRootBackgroundTaskState { request_sender })
    }

    fn prune_cache(
        cached_results: &mut BTreeMap<RollupHeight, StateRootCacheEntry<S>>,
        cached_results_size: &mut usize,
    ) {
        // If the cache is too big, prune the oldest writes sets
        let mut entries = cached_results.values_mut();
        while *cached_results_size > MEMORY_LIMIT_FOR_STATE_ROOT_COMPUTATION {
            let next_entry = entries.next().unwrap(); // Safety: We just checked that the cache is not empty
            *cached_results_size -= next_entry.size;
            next_entry.size = 0;
            next_entry.writes = None;
        }
        // If the cache has too many entries, prune the oldest one.
        if cached_results.len() > MAX_STATE_ROOTS_TO_CACHE {
            cached_results.pop_first().unwrap(); // Safety: We just checked that the cache is not empty
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_state::{OrderedReadsAndWrites, SlotKey};
    use sov_test_utils::storage::{
        ForklessStorageManager, SimpleNomtStorageManager, SimpleStorageManager,
    };
    use sov_test_utils::{TestNomtSpec, TestSpec, TestStorageSpec};
    use tokio::task::JoinHandle;

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_jmt_new_rollup_height_state_root_on_stale_storage() {
        let storage_manager = SimpleStorageManager::<TestStorageSpec>::new();
        new_rollup_height_state_root_on_stale_storage::<TestSpec, _>(storage_manager).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_jmt_known_rollup_height_state_root_on_stale_storage() {
        let storage_manager = SimpleStorageManager::<TestStorageSpec>::new();
        known_rollup_height_state_root_on_stale_storage::<TestSpec, _>(storage_manager).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_nomt_new_rollup_height_state_root_on_stale_storage() {
        let storage_manager = SimpleNomtStorageManager::<TestStorageSpec>::new();
        new_rollup_height_state_root_on_stale_storage::<TestNomtSpec, _>(storage_manager).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_nomt_known_rollup_height_state_root_on_stale_storage() {
        let storage_manager = SimpleNomtStorageManager::<TestStorageSpec>::new();
        known_rollup_height_state_root_on_stale_storage::<TestNomtSpec, _>(storage_manager).await;
    }

    // Helpers go below
    fn writes_only_kernel() -> StateAccesses {
        StateAccesses {
            user: Default::default(),
            kernel: OrderedReadsAndWrites {
                ordered_reads: Vec::new(),
                ordered_writes: vec![(
                    SlotKey::from_slice(&b"kernel_key"[..]),
                    Some(SlotValue::from(b"value_1".to_vec())),
                )],
            },
        }
    }

    fn sample_batch() -> StateAccesses {
        StateAccesses {
            user: OrderedReadsAndWrites {
                ordered_reads: Vec::new(),
                ordered_writes: vec![(
                    SlotKey::from_slice(&b"user_key"[..]),
                    Some(SlotValue::from(b"value_a".to_vec())),
                )],
            },
            kernel: OrderedReadsAndWrites {
                ordered_reads: Vec::new(),
                ordered_writes: vec![(
                    SlotKey::from_slice(&b"kernel_key"[..]),
                    Some(SlotValue::from(b"value_2".to_vec())),
                )],
            },
        }
    }

    fn start_background_task<S: Spec>() -> (
        StateRootBackgroundTaskState<S>,
        JoinHandle<()>,
        watch::Sender<()>,
    ) {
        let (shutdown_notifier, _shutdown_rx) = mpsc::channel(1);
        let (shutdown_sender, mut shutdown_receiver) = watch::channel(());
        shutdown_receiver.mark_unchanged();

        let (handle, task) = StateRootBackgroundTaskState::<S>::create(
            shutdown_notifier.clone(),
            shutdown_receiver.clone(),
            true,
        );

        (task, handle, shutdown_sender)
    }

    async fn get_root_from_background_task<S: Spec>(
        task: &StateRootBackgroundTaskState<S>,
        storage: S::Storage,
        state_accesses: StateAccesses,
        rollup_height: RollupHeight,
        slot_number: SlotNumber,
    ) -> <S::Storage as Storage>::Root {
        let (response_channel, response_receiver) = oneshot::channel();
        task.request_sender
            .send(StateRootComputeRequest {
                state_accesses,
                storage,
                rollup_height,
                max_slot_number: slot_number,
                response_channel,
            })
            .await
            .unwrap();

        let (received_rollup_height, received_root) =
            tokio::time::timeout(std::time::Duration::from_secs(10), response_receiver)
                .await
                .expect("Timed out waiting for state root computation")
                .expect("State root computation failed");

        assert_eq!(received_rollup_height, rollup_height);
        received_root
    }

    // Start background task, sender compute request, shutdown, return root computed.
    async fn one_off_check_state_root_computation<S: Spec>(
        storage: S::Storage,
        state_accesses: StateAccesses,
        rollup_height: RollupHeight,
        slot_number: SlotNumber,
    ) -> <S::Storage as Storage>::Root {
        let (task, handle, shutdown_sender) = start_background_task::<S>();

        let received_root = get_root_from_background_task::<S>(
            &task,
            storage,
            state_accesses,
            rollup_height,
            slot_number,
        )
        .await;

        shutdown_sender.send(()).unwrap();
        handle.await.unwrap();
        received_root
    }

    fn genesis<S, Sm>(storage_manager: &mut Sm)
    where
        S: Spec,
        Sm: ForklessStorageManager<Storage = S::Storage>,
        S::Storage: NativeStorage,
    {
        let node_storage = storage_manager.create_prover_storage();
        let writes_on_the_node = writes_only_kernel();
        let prev_root = <S::Storage as Storage>::PRE_GENESIS_ROOT;
        let (node_new_root, changes) = node_storage
            .compute_state_update(writes_on_the_node, &Default::default(), prev_root)
            .unwrap();
        storage_manager.commit_state_update(node_storage, changes, node_new_root);
    }

    async fn new_rollup_height_state_root_on_stale_storage<S, Sm>(mut storage_manager: Sm)
    where
        S: Spec,
        Sm: ForklessStorageManager<Storage = S::Storage>,
        S::Storage: NativeStorage,
        <S::Storage as Storage>::Root: Copy,
    {
        genesis::<S, Sm>(&mut storage_manager);

        let node_storage = storage_manager.create_prover_storage();
        let writes_on_the_node = sample_batch();

        let prev_root = node_storage
            .get_root_hash(node_storage.latest_version())
            .unwrap();
        let (node_new_root, changes) = node_storage
            .compute_state_update(writes_on_the_node, &Default::default(), prev_root)
            .unwrap();

        // Important detail: storage for the background task is created before node changes are committed.
        let storage_for_background = storage_manager.create_prover_storage();
        storage_manager.commit_state_update(node_storage, changes, node_new_root);

        let task_new_root = one_off_check_state_root_computation::<S>(
            storage_for_background,
            sample_batch(),
            RollupHeight::new(1),
            SlotNumber::new(1),
        )
        .await;

        assert_eq!(node_new_root, task_new_root);
    }

    async fn known_rollup_height_state_root_on_stale_storage<S, Sm>(mut storage_manager: Sm)
    where
        S: Spec,
        Sm: ForklessStorageManager<Storage = S::Storage>,
        S::Storage: NativeStorage,
        <S::Storage as Storage>::Root: Copy,
    {
        // Genesis
        genesis::<S, Sm>(&mut storage_manager);

        // Starting background task
        let (task, handle, shutdown_sender) = start_background_task::<S>();

        let node_storage = storage_manager.create_prover_storage();
        let writes_on_the_node = sample_batch();
        let rollup_height = RollupHeight::new(1);
        let slot_number = SlotNumber::new(1);
        let prev_root = node_storage
            .get_root_hash(node_storage.latest_version())
            .unwrap();
        let (node_new_root, changes) = node_storage
            .compute_state_update(writes_on_the_node, &Default::default(), prev_root)
            .unwrap();

        let storage_for_background_1 = storage_manager.create_prover_storage();
        let storage_for_background_2 = storage_manager.create_prover_storage();

        // Normal, not stalled
        let received_root_1 = get_root_from_background_task::<S>(
            &task,
            storage_for_background_1,
            sample_batch(),
            rollup_height,
            slot_number,
        )
        .await;

        storage_manager.commit_state_update(node_storage, changes, node_new_root);

        let received_root_2 = get_root_from_background_task::<S>(
            &task,
            storage_for_background_2,
            sample_batch(),
            rollup_height,
            slot_number,
        )
        .await;

        assert_eq!(node_new_root, received_root_1);
        assert_eq!(received_root_1, received_root_2);

        shutdown_sender.send(()).unwrap();
        handle.await.unwrap();
    }
}
