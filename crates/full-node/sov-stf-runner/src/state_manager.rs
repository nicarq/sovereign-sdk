//! All code related to handling storage manager anb ledger.
use std::collections::VecDeque;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_db::schema::{DeltaReader, SchemaBatch};
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec};
use sov_rollup_interface::node::da::{DaService, SlotData};
use sov_rollup_interface::stf::TxReceiptContents;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_rollup_interface::zk::StateTransitionWitness;
use sov_rollup_interface::{ProvableHeightTracker, StateUpdateInfo};
use tokio::sync::watch;

use crate::processes::{Sender as StfInfoSender, StateTransitionInfo};
use crate::query_state_update_info;

/// Point where rollup execution can be resumed after DA fork happened.
struct ForkPoint<Da: DaService, StateRoot> {
    /// The next block in a new fork, following the last seen transition by the rollup.
    block: Da::FilteredBlock,
    /// Last observed state root before the fork.
    pre_state_root: StateRoot,
}

/// Structure that holds a block header and a pre-state root that was on this block header
struct StateOnBlock<Da: DaSpec, StateRoot> {
    block_header: Da::BlockHeader,
    pre_state_root: StateRoot,
}

/// StateManager controls storage lifecycle for [`StateTransitionFunction`],
/// [`LedgerDb`] and API endpoints in case of DA-reorgs.
/// It needs [`DaService`] so it can backtrack to the last seen transition in new fork.
pub struct StateManager<StateRoot, Witness, Sm, Da>
where
    Da: DaService,
    Sm: HierarchicalStorageManager<Da::Spec>,
{
    storage_manager: Sm,
    ledger_db: LedgerDb,
    // `state_root` is tracked so [`StateTransitionWitness`] can have proper `prev_state_root`.
    // Probably it can be saved in variable before "apply_slot" is called,
    // But then the runner needs to know about it and carry it over.
    state_root: StateRoot,
    seen_state_transitions: VecDeque<StateOnBlock<Da::Spec, StateRoot>>,
    state_update_sender: watch::Sender<StateUpdateInfo<Sm::StfState>>,
    st_info_sender: Option<StfInfoSender<StateRoot, Witness, Da::Spec>>,
    maximum_provable_height_tracker: Box<dyn ProvableHeightTracker>,
    is_initialized: bool,
}

impl<StateRoot, Witness, Sm, Da> StateManager<StateRoot, Witness, Sm, Da>
where
    Da: DaService<Error = anyhow::Error>,
    StateRoot: Clone + AsRef<[u8]> + Serialize + DeserializeOwned,
    Witness: Serialize + DeserializeOwned,
    Sm: HierarchicalStorageManager<
        Da::Spec,
        LedgerChangeSet = SchemaBatch,
        LedgerState = DeltaReader,
    >,
    Sm::StfState: Clone,
{
    pub(crate) fn new(
        storage_manager: Sm,
        ledger_db: LedgerDb,
        initial_state_root: StateRoot,
        state_update_channel: watch::Sender<StateUpdateInfo<Sm::StfState>>,
        st_info_sender: Option<StfInfoSender<StateRoot, Witness, Da::Spec>>,
        state_height_tracker: Box<dyn ProvableHeightTracker>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            storage_manager,
            ledger_db,
            state_root: initial_state_root,
            seen_state_transitions: Default::default(),
            state_update_sender: state_update_channel,
            st_info_sender,
            maximum_provable_height_tracker: state_height_tracker,
            is_initialized: false,
        })
    }

    pub(crate) async fn startup(&mut self) -> anyhow::Result<()> {
        if let Some(sender) = &mut self.st_info_sender {
            // If this state manager uses a channel, it MUST be correctly
            // initialized before usage.
            sender
                .startup_notify_about_infos_from_db(
                    &self.ledger_db,
                    &*self.maximum_provable_height_tracker,
                )
                .await?;
        }
        self.is_initialized = true;
        Ok(())
    }

    /// Updates both the [`LedgerDb`] and the [`StateUpdateInfo`]
    /// states.
    ///
    /// ## Potential synchronization issues
    /// Note that we are not using strong synchronization primitives here.
    /// However, we always have the guarantee that the [`LedgerDb`] is
    /// updated before the [`StateUpdateInfo`]. This means that, given the rollup height
    /// accessible from the [`StateUpdateInfo`] channel, we can safely query data from the [`LedgerDb`] at this height.
    async fn update_channels(
        &mut self,
        stf_state: Sm::StfState,
        ledger_state: DeltaReader,
    ) -> anyhow::Result<()> {
        self.ledger_db.replace_reader(ledger_state);

        let state_update_info = query_state_update_info(&self.ledger_db, stf_state).await?;

        // `send_replace` is superior to `send` for our use case. It never fails
        // because it doesn't need to notify all receivers, unlike `send`, which
        // we don't need. It will also keep working even if there are no
        // receivers currently alive, which makes it easier to reason about the
        // code.
        self.state_update_sender.send_replace(state_update_info);

        Ok(())
    }

    /// Returns an [`HierarchicalStorageManager::StfState`] and a [`DaService::FilteredBlock`] that can be used to continue execution.
    /// If a caller relies on some data from `filtered_block`,
    /// it should be updated after the call of this method.
    /// If a given block continues in the current fork, it is simply returned to the caller.
    /// If reorg happened, it will return block following the last seen transition.
    pub(crate) async fn prepare_storage(
        &mut self,
        mut filtered_block: Da::FilteredBlock,
        da_service: &Da,
    ) -> anyhow::Result<(Sm::StfState, Da::FilteredBlock)> {
        if !self.is_initialized {
            anyhow::bail!(
                "StateManager wasn't initialized. Please call `.startup()` method before using"
            );
        }
        let reorg_happened = if let Some(ForkPoint {
            block: new_block,
            pre_state_root,
        }) = self
            .has_reorg_happened(filtered_block.header(), da_service)
            .await?
        {
            filtered_block = new_block;
            self.state_root = pre_state_root;
            tracing::info!(
                header = %filtered_block.header().display(),
                "Resuming execution at fork point's height"
            );
            true
        } else {
            false
        };

        let (stf_pre_state, ledger_state) = self
            .storage_manager
            .create_state_for(filtered_block.header())?;
        if reorg_happened {
            tracing::trace!(
                "Reorg has happened, updating API and Ledger storage before returning Stf state"
            );
            // In case if reorg happened, we want to keep ledger and API storages in sync.
            // Otherwise, the API storage and LedgerDb have been updated in [`Self::update_api_and_ledger_storage`]
            self.update_channels(stf_pre_state.clone(), ledger_state)
                .await?;
        }

        tracing::trace!(block_header = %filtered_block.header().display(), "Returning STF state for block");
        Ok((stf_pre_state, filtered_block))
    }

    /// Performs all necessary operations on data that has been processed by the rollup.
    /// Returns vector of finalized state transitions, so the caller can do anything on top of that.
    /// All necessary data for these finalized transitions have been saved on disk.
    pub(crate) async fn process_stf_changes<
        S: SlotData,
        B: serde::Serialize,
        T: TxReceiptContents,
    >(
        &mut self,
        last_finalized_height: u64,
        stf_changes: Sm::StfChangeSet,
        transition_witness: StateTransitionWitness<StateRoot, Witness, Da::Spec>,
        slot_commit: SlotCommit<S, B, T>,
        aggregated_proofs: Vec<SerializedAggregatedProof>,
    ) -> anyhow::Result<()> {
        if !self.is_initialized {
            anyhow::bail!(
                "StateManager wasn't initialized. Please call `.startup()` method before using"
            );
        }
        let rollup_height = self.get_rollup_height()?;
        let new_state_root = transition_witness.final_state_root.clone();
        let block_header: <<Da as DaService>::Spec as DaSpec>::BlockHeader =
            transition_witness.da_block_header.clone();
        tracing::debug!(
            rollup_height,
            block_header = %block_header.display(),
            current_state_root = hex::encode(self.get_state_root().as_ref()),
            next_state_root = hex::encode(new_state_root.as_ref()),
            aggregated_proofs = aggregated_proofs.len(),
            "Saving changes after applying slot"
        );

        self.seen_state_transitions.push_back(StateOnBlock {
            block_header: block_header.clone(),
            pre_state_root: transition_witness.initial_state_root.clone(),
        });

        let finalized_transitions = self
            .process_finalized_state_transitions(last_finalized_height)
            .await?;

        let mut ledger_change_set = self
            .ledger_db
            .materialize_slot(slot_commit, new_state_root.as_ref())?;

        let last_finalized_slot = self
            .ledger_db
            .materialize_latest_finalize_slot(last_finalized_height)?;
        ledger_change_set.merge(last_finalized_slot);

        if let Some(st_info_sender) = &self.st_info_sender {
            let stf_info = StateTransitionInfo {
                data: transition_witness,
                rollup_height,
            };
            let stf_info_schema = st_info_sender
                .materialize_stf_info(&stf_info, &self.ledger_db)
                .await?;
            ledger_change_set.merge(stf_info_schema);
        }

        for aggregated_proof in aggregated_proofs {
            let this_height_data = self
                .ledger_db
                .materialize_aggregated_proof(aggregated_proof)?;
            ledger_change_set.merge(this_height_data);
        }

        self.storage_manager
            .save_change_set(&block_header, stf_changes, ledger_change_set)?;

        self.update_api_and_ledger_storage(&block_header).await?;

        for finalized_transition in &finalized_transitions {
            self.storage_manager
                .finalize(&finalized_transition.block_header)?;
        }

        if let Some(st_info_sender) = &mut self.st_info_sender {
            // Notify `StateTransitionInfo` consumers that the data is saved in the Db.
            let maximum_provable_height = self
                .maximum_provable_height_tracker
                .maximum_provable_height();
            st_info_sender
                .notify(maximum_provable_height, &self.ledger_db)
                .await?;
        }

        self.state_root = new_state_root;
        // API storage and Ledger have all data from this iteration,
        // now it is safe to submit notifications.
        self.ledger_db.send_notifications();

        Ok(())
    }

    /// Checks if passed [`DaSpec::BlockHeader`] is a continuation of seen state transition.
    /// If not, traverses back to the latest-seen transition, that belongs to the fork of passed header.
    async fn has_reorg_happened(
        &mut self,
        block_header: &<Da::Spec as DaSpec>::BlockHeader,
        da_service: &Da,
    ) -> anyhow::Result<Option<ForkPoint<Da, StateRoot>>> {
        if let Some(state_transition) = self.seen_state_transitions.back() {
            if state_transition.block_header.hash() != block_header.prev_hash() {
                tracing::warn!(
                    current_header = %block_header.display(),
                    prev_seen_header = %state_transition.block_header.display(),
                    "Block does not belong in current chain. Chain has forked. Traversing seen headers backwards"
                );
                while let Some(state_transition) = self.seen_state_transitions.pop_back() {
                    let block = da_service
                        .get_block_at(state_transition.block_header.height())
                        .await?;
                    tracing::debug!(
                        fetched = %block.header().display(),
                        seen = %state_transition.block_header.display(),
                        "Checking seen header vs fetched from DA"
                    );
                    if block.header().prev_hash() == state_transition.block_header.prev_hash() {
                        return Ok(Some(ForkPoint {
                            block,
                            pre_state_root: state_transition.pre_state_root,
                        }));
                    }
                }
                anyhow::bail!("Could not match any seen block with the current chain. Could rollup start from non-finalized block?");
            }
        }
        Ok(None)
    }

    /// Returns all [`StateTransitionInfo`] which are below finalized height
    /// and relevant LedgerDb changes.
    async fn process_finalized_state_transitions(
        &mut self,
        last_finalized_height: u64,
    ) -> anyhow::Result<Vec<StateOnBlock<Da::Spec, StateRoot>>> {
        tracing::trace!(
            last_finalized_height,
            seen_transitions = self.seen_state_transitions.len(),
            "Start processing finalized state transitions"
        );

        let mut finalized_transitions = Vec::new();
        // Checking all seen blocks, in case if there was delay in getting last finalized header.
        while let Some(earliest_seen_transition) = self.seen_state_transitions.front() {
            let earliest_header = &earliest_seen_transition.block_header;
            tracing::trace!(header = %earliest_header.display(), last_finalized_height, "Checking seen header");
            let height = earliest_header.height();

            if height <= last_finalized_height {
                let transition_data = self.seen_state_transitions.pop_front().expect(
                    "There should be seen transition, as observed by previous call to .front()",
                );

                finalized_transitions.push(transition_data);
                continue;
            }

            break;
        }
        tracing::trace!(
            finalized_transitions = finalized_transitions.len(),
            "Completed check for finalized transitions"
        );
        Ok(finalized_transitions)
    }

    async fn update_api_and_ledger_storage(
        &mut self,
        block_header: &<<Da as DaService>::Spec as DaSpec>::BlockHeader,
    ) -> anyhow::Result<()> {
        tracing::trace!(after_block = %block_header.display(), "Updating Ledger and API storage");
        let (api_storage, ledger_state) = self.storage_manager.create_state_after(block_header)?;

        self.update_channels(api_storage, ledger_state).await?;
        Ok(())
    }

    /// Allows reading current state root.
    pub fn get_state_root(&self) -> &StateRoot {
        &self.state_root
    }

    fn get_rollup_height(&self) -> anyhow::Result<u64> {
        Ok(self.ledger_db.get_next_items_numbers()?.rollup_height)
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::Ordering;
    use std::num::NonZero;

    use sov_db::storage_manager::{NativeChangeSet, NativeStorageManager};
    use sov_mock_da::{
        MockAddress, MockBlock, MockBlockHeader, MockDaService, MockDaSpec, MockFee,
        MockValidityCond, PlannedFork,
    };
    use sov_mock_zkvm::MockZkvm;
    use sov_rollup_interface::node::ledger_api::LedgerStateProvider;
    use sov_rollup_interface::stf::StateTransitionFunction;
    use sov_state::{
        ArrayWitness, NativeStorage, ProverStorage, SlotKey, SlotValue, StateAccesses, Storage,
    };

    use super::*;
    use crate::mock::MockStf;
    type Da = MockDaService;
    type Vm = MockZkvm;
    type Stf = MockStf<MockValidityCond>;
    type S = sov_state::DefaultStorageSpec<sha2::Sha256>;
    type StateRoot = <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::StateRoot;
    type TestBatchReceiptContents =
        <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::BatchReceiptContents;
    type TestTxReceiptContents =
        <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::TxReceiptContents;
    type Witness = <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::Witness;
    type MockSlotCommit = SlotCommit<MockBlock, Witness, TestTxReceiptContents>;
    type TestStateManager =
        StateManager<StateRoot, Witness, NativeStorageManager<MockDaSpec, ProverStorage<S>>, Da>;
    use sov_modules_api::provable_height_tracker::InfiniteHeight;

    const SEQUENCER_ADDRESS: MockAddress = MockAddress::new([0; 32]);

    async fn setup_state_manager(path: &std::path::Path) -> anyhow::Result<TestStateManager> {
        let mut storage_manager: NativeStorageManager<MockDaSpec, ProverStorage<S>> =
            NativeStorageManager::new(path)?;
        let genesis_block = MockBlock::default_at_height(0);
        let genesis_header = genesis_block.header().clone();
        let (genesis_storage, ledger_state) = storage_manager.create_state_for(&genesis_header)?;
        let ledger_db = LedgerDb::with_reader(ledger_state)?;

        let (state_root, change_set) = produce_synthetic_changes(&genesis_storage, 0);

        let data_to_commit: SlotCommit<_, TestBatchReceiptContents, TestTxReceiptContents> =
            SlotCommit::new(genesis_block);
        let mut ledger_change_set =
            ledger_db.materialize_slot(data_to_commit, state_root.as_ref())?;
        let finalized_slot_changes = ledger_db.materialize_latest_finalize_slot(0)?;
        ledger_change_set.merge(finalized_slot_changes);

        storage_manager.save_change_set(&genesis_header, change_set, ledger_change_set)?;
        storage_manager.finalize(&genesis_header)?;

        let (stf_state, ledger_state) = storage_manager.create_bootstrap_state().unwrap();

        let ledger_db = LedgerDb::with_reader(ledger_state)?;

        let update_info = query_state_update_info(&ledger_db, stf_state).await?;

        let (state_update_sender, _state_update_recv) = watch::channel(update_info);

        let mut state_manager = StateManager::new(
            storage_manager,
            ledger_db,
            state_root,
            state_update_sender,
            None,
            Box::new(InfiniteHeight),
        )?;

        state_manager.startup().await?;

        Ok(state_manager)
    }

    fn produce_synthetic_changes(
        prover_storage: &ProverStorage<S>,
        height: u64,
    ) -> (StateRoot, NativeChangeSet) {
        let w = ArrayWitness::default();
        let mut accesses = StateAccesses::default();
        accesses.user.ordered_writes.push((
            SlotKey::from(vec![height as u8]),
            Some(SlotValue::from(vec![height as u8])),
        ));
        let (state_root, state_update) = prover_storage.compute_state_update(accesses, &w).unwrap();
        let change_set = prover_storage.materialize_changes(&state_update);

        (state_root.root_hash().0.to_vec(), change_set)
    }

    async fn process_normal_transition(
        state_manager: &mut TestStateManager,
        filtered_block: MockBlock,
        finalized_height: u64,
        da_service: &Da,
    ) -> anyhow::Result<()> {
        let (prover_storage, returned_block) = state_manager
            .prepare_storage(filtered_block.clone(), da_service)
            .await?;

        assert_eq!(filtered_block, returned_block);

        let (state_root, change_set) =
            produce_synthetic_changes(&prover_storage, filtered_block.header().height());
        let (relevant_blobs, relevant_proofs) = da_service
            .extract_relevant_blobs_with_proof(&filtered_block)
            .await;

        let transition_witness = StateTransitionWitness {
            initial_state_root: state_manager.get_state_root().to_owned(),
            final_state_root: state_root,
            da_block_header: filtered_block.header().clone(),
            relevant_proofs,
            relevant_blobs,
            witness: (),
        };

        let slot_commit: MockSlotCommit = SlotCommit::new(filtered_block);
        state_manager
            .process_stf_changes(
                finalized_height,
                change_set,
                transition_witness,
                slot_commit,
                Vec::new(),
            )
            .await?;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn state_manager_on_empty_transitions_non_instant_finalization() -> anyhow::Result<()> {
        // Checks that the same block returned when storage requested on new
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let filtered_block = MockBlock {
            header: MockBlockHeader::from_height(1),
            ..Default::default()
        };
        let da_service = MockDaService::new(SEQUENCER_ADDRESS);

        process_normal_transition(&mut state_manager, filtered_block, 0, &da_service).await?;

        // LedgerDb storage should be updated by that point, so correct height is returned
        assert_eq!(
            0,
            state_manager
                .ledger_db
                .get_latest_finalized_rollup_height()
                .await?
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_instant_finality() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let (sender, mut receiver) = crate::processes::new_stf_info_channel(
            state_manager.ledger_db.clone(),
            NonZero::new(40).unwrap(),
            NonZero::new(40).unwrap(),
        )
        .await?;
        state_manager.st_info_sender = Some(sender);
        let da_service = MockDaService::new(SEQUENCER_ADDRESS);

        let mut state_root = state_manager.get_state_root().clone();
        for height in 1..4 {
            da_service
                .send_transaction(&[height as u8; 10], MockFee::zero())
                .await
                .await??;
            let filtered_block = da_service.get_block_at(height).await?;
            process_normal_transition(
                &mut state_manager,
                filtered_block.clone(),
                height,
                &da_service,
            )
            .await?;
            let finalized = receiver.read_next().await?.unwrap();

            if let Some(sender) = state_manager.st_info_sender.as_ref() {
                sender.next_height_to_receive.fetch_add(1, Ordering::SeqCst);
            };

            assert_eq!(height, finalized.rollup_height);
            assert_eq!(filtered_block.header, finalized.data.da_block_header);
            assert_eq!(state_root, finalized.data.initial_state_root);
            state_root.clone_from(&finalized.data.final_state_root);
            assert_eq!(
                height,
                state_manager
                    .ledger_db
                    .get_latest_finalized_rollup_height()
                    .await?
            );
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_reorg_happened_correct_block_returned() -> anyhow::Result<()> {
        // The idea of the test is
        // to ensure that the state manager returns the correct block and storage after reorg.
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let fork_point = 3;
        let last_block = 6;

        let state_update_receiver = state_manager.state_update_sender.subscribe();

        let mut da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(5);
        da_service
            .set_planned_fork(PlannedFork::new(
                last_block,
                fork_point,
                vec![vec![11], vec![22], vec![33], vec![44]],
            ))
            .await?;

        let mut state_roots = Vec::with_capacity(last_block as usize);

        for rollup_height in 1..=last_block {
            // Not used anywhere, `process_normal_transition` relies on da header to produce changes.
            let blob_data = [rollup_height as u8; 10];
            da_service
                .send_transaction(&blob_data, MockFee::zero())
                .await
                .await??;
            let filtered_block = da_service.get_block_at(rollup_height).await?;
            if rollup_height < last_block {
                process_normal_transition(&mut state_manager, filtered_block, 0, &da_service)
                    .await?;
                let current_state_root = state_manager.get_state_root().clone();
                let received_storage = state_update_receiver.borrow().storage.clone();
                let received_storage_root = received_storage.get_root_hash(rollup_height)?;
                assert_eq!(
                    current_state_root,
                    received_storage_root.root_hash().0.to_vec()
                );
                state_roots.push(current_state_root);
            } else {
                let (prover_storage, returned_block) = state_manager
                    .prepare_storage(filtered_block.clone(), &da_service)
                    .await?;

                assert_ne!(filtered_block, returned_block);
                // Because we get filtered block via submission, first height we get is 1, thus rollup height > da_height by 1.
                assert_eq!(fork_point + 1, returned_block.header().height());

                let expected_state_root = &state_roots[fork_point as usize - 1];
                assert_eq!(expected_state_root, state_manager.get_state_root());

                let returned_storage_root = prover_storage.get_root_hash(fork_point)?;
                let received_update_info = state_update_receiver.borrow().clone();
                let received_storage_root =
                    received_update_info.storage.get_root_hash(fork_point)?;
                assert_eq!(returned_storage_root, received_storage_root);
            }
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_no_seen_block_has_been_tracked() -> anyhow::Result<()> {
        // The idea of the test is that the state manager receives a request for storage for a block
        // That is not a part of the current chain.
        // But it cannot back-track to the last known block in the new chain
        // because it hasn't seen transition in the new chain.
        // We simulate that by removing seen transitions before actual finalization happens.

        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let chain_length = 5;
        let finality = 3;
        let da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);

        for height in 1..=chain_length {
            da_service
                .send_transaction(&[height as u8; 10], MockFee::zero())
                .await
                .await??;
            let filtered_block = da_service.get_block_at(height).await?;
            let finalized_height = height.saturating_sub(finality as u64);
            process_normal_transition(
                &mut state_manager,
                filtered_block.clone(),
                finalized_height,
                &da_service,
            )
            .await?;
        }

        // Reinitialize new da_service with completely different blocks.
        let da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);
        for height in 1..=chain_length {
            da_service
                .send_transaction(&[(height * 10) as u8; 10], MockFee::zero())
                .await
                .await??;
        }

        let alien_block = da_service.get_block_at(chain_length).await?;

        let result = state_manager
            .prepare_storage(alien_block, &da_service)
            .await;
        assert!(result.is_err());
        assert_eq!("Could not match any seen block with the current chain. Could rollup start from non-finalized block?", result.unwrap_err().to_string());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_save_last_finalized_larger_than_seen_transitions() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;
        let da_service = MockDaService::new(SEQUENCER_ADDRESS);

        let chain_length = 5;
        // Fill some seen transitions without finalizing.
        for height in 1..chain_length {
            da_service
                .send_transaction(&[height as u8; 10], MockFee::zero())
                .await
                .await??;
            let filtered_block = da_service.get_block_at(height).await?;

            process_normal_transition(&mut state_manager, filtered_block, 0, &da_service).await?;
            assert_eq!(
                0,
                state_manager
                    .ledger_db
                    .get_latest_finalized_rollup_height()
                    .await?
            );
        }
        da_service
            .send_transaction(&[chain_length as u8; 10], MockFee::zero())
            .await
            .await??;
        let filtered_block = da_service.get_block_at(chain_length).await?;

        process_normal_transition(&mut state_manager, filtered_block, u64::MAX, &da_service)
            .await?;
        // Last finalized height written to LedgerDb as it passed.
        assert_eq!(
            u64::MAX,
            state_manager
                .ledger_db
                .get_latest_finalized_rollup_height()
                .await?
        );
        Ok(())
    }
}
