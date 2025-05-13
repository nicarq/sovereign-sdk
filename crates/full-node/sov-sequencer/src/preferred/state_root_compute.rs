//! Defines the logic for computing state roots for the preferred sequencer.

use std::collections::BTreeMap;

use sov_modules_api::capabilities::RollupHeight;
use sov_modules_api::{Spec, Storage};
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
    pub witness: <S::Storage as Storage>::Witness,
    pub storage: S::Storage,
    pub rollup_height: RollupHeight,
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
        witness: <S::Storage as Storage>::Witness,
        storage: S::Storage,
        rollup_height: RollupHeight,
    ) -> (RollupHeight, <S::Storage as Storage>::Root) {
        let prev_root = storage
            .get_root_hash(storage.latest_version())
            .expect("Failed to get root hash");

        let new_writes = state_accesses.user.ordered_writes.clone();
        let new_root =
            compute_state_root::<S>(state_accesses, witness, storage, rollup_height, prev_root)
                .await;
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

#[derive(Debug, thiserror::Error)]
pub enum StateRootComputeError {
    #[error("State roots computations were requested out of order. This is a bug in the rollup block executor. Last known height: {last_known_height}, requested height: {requested_height}")]
    MissingPreviousRoots {
        last_known_height: RollupHeight,
        requested_height: RollupHeight,
    },
}

pub(super) struct StateRootBackgroundTaskState<S: Spec> {
    pub request_sender: mpsc::Sender<StateRootComputeRequest<S>>,
}

async fn compute_state_root<S: Spec>(
    state_accesses: StateAccesses,
    witness: <S::Storage as Storage>::Witness,
    storage: S::Storage,
    rollup_height: RollupHeight,
    prev_root: <S::Storage as Storage>::Root,
) -> <S::Storage as Storage>::Root {
    // TODO: avoid blocking the runtime here
    let handle = tokio::runtime::Handle::current().spawn_blocking(move ||{
		tracing::trace!(%rollup_height, "Computing sequencer state root for height");
		let (root, _) =
		tracing::span!(tracing::Level::DEBUG,  "sequencer_compute_state_update")
			.in_scope(|| {
				storage
					.compute_state_update(state_accesses, &witness, prev_root)
					.unwrap_or_else(|_| {
								tracing::error!(
						rollup_height = %rollup_height,
						"failed to compute valid state update. This is a bug, please report it"
					);
					panic!("Failed to compute valid state for height {} in sequencer. This is a bug, please report it", rollup_height);
				})
			});
		root
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
                    witness,
                    storage,
                    rollup_height,
                    response_channel,
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

                // Sanity check: New requests should always be submitted in order (i.e. we should never request block 10 before block 9)
                if cached_results
                    .last_key_value()
                    .is_some_and(|(last_key, _)| rollup_height > last_key.saturating_add(1))
                {
                    tracing::error!("State root computation requested for height {}, but intermediate state roots are not available. This is a bug, please report it.", rollup_height);
                    // Usually, we delegate error handling to the receiver - but the receiver was dropped panic to ensure the error is noticed.
                    tracing::error!(err=%StateRootComputeError::MissingPreviousRoots {
                        last_known_height: cached_results.last_key_value().map(|(k, _)| *k).unwrap_or(RollupHeight::GENESIS),
                        requested_height: rollup_height,
                    }, "State root computation returned an error");
                    panic!("Uncaught state-root mismatch. See logs for details");
                }

                // If the entry is in cache, check that the state root is consistent and return early
                if let Some(cached_entry) = cached_results.get(&rollup_height) {
                    // If we're checking that the state roots are equal, we have some work to do.
                    let result = if check_state_roots {
                        cached_entry
                            .assert_consistency(state_accesses, witness, storage, rollup_height)
                            .await
                    } else {
                        (rollup_height, cached_entry.root.clone())
                    };
                    let _ = response_channel.send(result);
                    continue;
                }

                // If the entry wasn't in cache, we'll need to add it. Check if we should save the write set.
                let writes = &state_accesses.user.ordered_writes;
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
                let prev_root = storage
                    .get_root_hash(storage.latest_version())
                    .expect("Failed to get root hash");
                let root = compute_state_root::<S>(
                    state_accesses,
                    witness,
                    storage,
                    rollup_height,
                    prev_root.clone(),
                )
                .await;

                // Add the new entry to the cache
                tracing::trace!(%rollup_height, %prev_root, %root, "Adding new state root to cache");
                cached_results.insert(
                    rollup_height,
                    StateRootCacheEntry {
                        root: root.clone(),
                        writes: writes_to_save,
                        size,
                    },
                );
                cached_results_size += size;

                // Prune the cache if necessary
                Self::prune_cache(&mut cached_results, &mut cached_results_size);
                let _ = response_channel.send((rollup_height, root.clone()));
            }
            drop(shutdown_notifier);
            tracing::info!("State root background task shutdown");
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
