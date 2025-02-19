//! All code related to handling storage manager anb ledger.
use std::collections::{BTreeMap, HashMap, HashSet};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_db::schema::{DeltaReader, SchemaBatch};
use sov_rollup_interface::common::SlotNumber;
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

const MAX_REORG_FINDING_ATTEMPTS: u8 = 30;

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
    post_state_root: StateRoot,
}

impl<Da: DaSpec, StateRoot: AsRef<[u8]>> std::fmt::Debug for StateOnBlock<Da, StateRoot> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateOnBlock")
            .field("block_header", &self.block_header)
            .field("pre_state_root", &hex::encode(self.pre_state_root.as_ref()))
            .field(
                "post_state_root",
                &hex::encode(self.post_state_root.as_ref()),
            )
            .finish()
    }
}

impl<Da: DaSpec, StateRoot: Clone> StateOnBlock<Da, StateRoot> {
    fn from_transition_witness<W>(
        transition_witness: &StateTransitionWitness<StateRoot, W, Da>,
    ) -> Self {
        StateOnBlock {
            block_header: transition_witness.da_block_header.clone(),
            pre_state_root: transition_witness.initial_state_root.clone(),
            post_state_root: transition_witness.final_state_root.clone(),
        }
    }
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
    // We record all seen transitions at the given height.
    state_on_block:
        HashMap<<<Da as DaService>::Spec as DaSpec>::SlotHash, StateOnBlock<Da::Spec, StateRoot>>,
    // Helper for faster iteration over fork tree.
    seen_on_height: BTreeMap<u64, HashSet<<Da::Spec as DaSpec>::SlotHash>>,
    state_update_sender: watch::Sender<StateUpdateInfo<Sm::StfState>>,
    st_info_sender: Option<StfInfoSender<StateRoot, Witness, Da::Spec>>,
    max_provable_slot_number_tracker: Box<dyn ProvableHeightTracker>,
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
            state_on_block: Default::default(),
            seen_on_height: Default::default(),
            state_update_sender: state_update_channel,
            st_info_sender,
            max_provable_slot_number_tracker: state_height_tracker,
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
                    &*self.max_provable_slot_number_tracker,
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
    #[tracing::instrument(skip_all)]
    pub(crate) async fn prepare_storage(
        &mut self,
        mut filtered_block: Da::FilteredBlock,
        da_service: &Da,
    ) -> anyhow::Result<(Sm::StfState, Da::FilteredBlock)> {
        let start = std::time::Instant::now();
        if !self.is_initialized {
            anyhow::bail!(
                "StateManager wasn't initialized. Please call `.startup()` method before using"
            );
        }
        let reorg_happened = self
            .has_reorg_happened(filtered_block.header(), da_service)
            .await?;
        tracing::trace!(reorg_happened, "Checked if reorg happened");

        if reorg_happened {
            let ForkPoint {
                block: new_block,
                pre_state_root,
            } = self.choose_fork_point(da_service).await?;
            tracing::trace!(
                old_block = %filtered_block.header().display(),
                new_block = %new_block.header().display(),
                old_pre_state_root = hex::encode(self.state_root.as_ref()),
                new_pre_state_root = hex::encode(pre_state_root.as_ref()),
                "Reorg happened, updating variables");
            filtered_block = new_block;
            self.state_root = pre_state_root;
            tracing::info!(
                header = %filtered_block.header().display(),
                time = ?start.elapsed(),
                "Chosen fork point"
            );
        }

        let (stf_pre_state, ledger_state) = self
            .storage_manager
            .create_state_for(filtered_block.header())?;
        // Second condition, we only update channels with new state before returning in case of reorg
        if reorg_happened {
            tracing::trace!(
                "Reorg has happened, updating API and Ledger storage before returning Stf state"
            );
            // In case if reorg happened, we want to keep ledger and API storages in sync.
            // Otherwise, the API storage and LedgerDb have been updated in [`Self::update_api_and_ledger_storage`]
            self.update_channels(stf_pre_state.clone(), ledger_state)
                .await?;
        }

        tracing::trace!(
            block_header = %filtered_block.header().display(),
            reorg_happened,
            time = ?start.elapsed(),
            "Returning STF state for block");
        Ok((stf_pre_state, filtered_block))
    }

    /// Performs all necessary operations on data that has been processed by the rollup.
    /// Returns vector of finalized state transitions, so the caller can do anything on top of that.
    /// All necessary data for these finalized transitions have been saved on disk.
    /// Now: First ever call to this method should be with either finalized block or with the block next to finalized.
    /// Otherwise follow up calls to prepare storage can panic if a chain reorgs.
    pub(crate) async fn process_stf_changes<
        S: SlotData,
        B: serde::Serialize,
        T: TxReceiptContents,
    >(
        &mut self,
        da_service: &Da,
        da_height_at_genesis: u64,
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
        let slot_number = self.get_slot_number()?;
        let new_state_root = transition_witness.final_state_root.clone();
        let block_header: <<Da as DaService>::Spec as DaSpec>::BlockHeader =
            transition_witness.da_block_header.clone();
        if self.state_on_block.contains_key(&block_header.hash()) {
            anyhow::bail!(
                "Attempt to process already processed block: {}, probably a bug in a caller",
                block_header.display()
            );
        }
        tracing::debug!(
            %slot_number,
            current_state_root = hex::encode(self.get_state_root().as_ref()),
            next_state_root = hex::encode(new_state_root.as_ref()),
            aggregated_proofs = aggregated_proofs.len(),
            "Saving changes after applying slot"
        );

        // ---
        let seen_state_on_block =
            StateOnBlock::from_transition_witness::<Witness>(&transition_witness);
        tracing::trace!(block_header = %block_header.display(), "Adding transition to the list of the seen");
        self.state_on_block
            .insert(block_header.hash(), seen_state_on_block);
        self.seen_on_height
            .entry(block_header.height())
            .or_default()
            .insert(block_header.hash());
        // ----

        let (last_finalized_header, finalized_transitions) =
            self.process_finalized_state_transitions(da_service).await?;
        tracing::trace!(
            finalized_transitions = finalized_transitions.len(),
            "Processed finalized transitions"
        );

        let mut ledger_change_set = self
            .ledger_db
            .materialize_slot(slot_commit, new_state_root.as_ref())?;
        tracing::trace!("Initial Ledger ChangeSet is materialized");

        // TODO: Review this, does not look nice.
        let last_finalized_slot_number = SlotNumber::new_dangerous(
            last_finalized_header
                .height()
                .saturating_sub(da_height_at_genesis),
        );
        tracing::trace!(
            ?last_finalized_slot_number,
            "Going to materialize last finalized slot number"
        );
        let last_finalized_slot_update = self
            .ledger_db
            .materialize_latest_finalize_slot(last_finalized_slot_number)?;

        ledger_change_set.merge(last_finalized_slot_update);
        tracing::trace!(
            ?last_finalized_slot_number,
            "Last finalized slot is materialized into LedgerDb ChangeSet"
        );

        if let Some(st_info_sender) = &self.st_info_sender {
            tracing::trace!("Going to materialize StateTransitionInfo");
            let stf_info = StateTransitionInfo {
                data: transition_witness,
                slot_number,
            };
            let stf_info_schema = st_info_sender
                .materialize_stf_info(&stf_info, &self.ledger_db)
                .await?;
            ledger_change_set.merge(stf_info_schema);
            tracing::trace!("StateTransitionInfo is materialized into Ledger ChangeSet");
        }

        for aggregated_proof in aggregated_proofs {
            let this_height_data = self
                .ledger_db
                .materialize_aggregated_proof(aggregated_proof)?;
            ledger_change_set.merge(this_height_data);
            tracing::trace!("Aggregated Proof is materialized into Ledger ChangeSet");
        }

        self.storage_manager
            .save_change_set(&block_header, stf_changes, ledger_change_set)?;

        self.update_api_and_ledger_storage(&block_header).await?;
        tracing::trace!("API and Ledger storage updated");

        for finalized_transition in &finalized_transitions {
            self.storage_manager
                .finalize(&finalized_transition.block_header)?;
        }
        tracing::trace!("All finalized transitions are marked as finalized");

        if let Some(st_info_sender) = &mut self.st_info_sender {
            // Notify `StateTransitionInfo` consumers that the data is saved in the Db.
            let max_provable_slot_number = self
                .max_provable_slot_number_tracker
                .max_provable_slot_number();
            st_info_sender
                .notify(max_provable_slot_number, &self.ledger_db)
                .await?;
        }

        self.state_root = new_state_root;
        // API storage and Ledger have all data from this iteration,
        // now it is safe to submit notifications.
        self.ledger_db.send_notifications();
        tracing::trace!("Notifications sent, state manager is completed its task");

        Ok(())
    }

    /// Returns true, if passing block_header is not an incremental continuation of the current canonical chain.
    async fn has_reorg_happened(
        &self,
        block_header: &<Da::Spec as DaSpec>::BlockHeader,
        da_service: &Da,
    ) -> anyhow::Result<bool> {
        // Reorg: if passed block header a new and it is not a continuation of any of the previous height transitions.
        tracing::trace!(
            block_header = %block_header.display(),
            "Checking if reorg happened");
        // 0. Short circuit
        if self.state_on_block.is_empty() {
            tracing::trace!("empty state_on_block => checking if passed block is finalized or direct descendant of finalized");
            let finalized = da_service.get_last_finalized_block_header().await?;
            // Simple case
            if block_header.prev_hash() == finalized.hash()
                || block_header.hash() == finalized.hash()
            {
                return Ok(false);
            }
            if block_header.height() >= finalized.height() {
                tracing::trace!(
                    block_header = %block_header.display(),
                    last_finalzied = %finalized.display(),
                    "passed block header is higher than finalized and not direct descendant of finalized => reorg happened");
                return Ok(true);
            }
            // If it is not last finalized, but finalized in the past
            let past_finalized_block = da_service.get_block_at(block_header.height()).await?;

            if block_header.hash() == past_finalized_block.header().hash() {
                tracing::trace!("Passed block header has been finalized in the past => no reorg");
                return Ok(false);
            }
            tracing::trace!(
                "This block header has not been finalized in the past => reorg happened"
            );
            return Ok(false);
        }

        let preceding_state_root = self.get_preceding_state_root_if_new_transition(block_header);
        if preceding_state_root.is_none() {
            return Ok(true);
        }

        // 3. Continuation of **existing** state of state manager.
        let predecessor_state_root = preceding_state_root.unwrap();
        let is_fork = self.state_root.as_ref() != predecessor_state_root.as_ref();
        tracing::trace!(block_header = %block_header.display(), is_fork, "current state matches predecessor");
        Ok(is_fork)
    }

    fn get_preceding_state_root_if_new_transition(
        &self,
        block_header: &<Da::Spec as DaSpec>::BlockHeader,
    ) -> Option<StateRoot> {
        // 1. Does not have a predecessor: not continuous transition
        let predecessor = self.state_on_block.get(&block_header.prev_hash());
        if predecessor.is_none() {
            tracing::trace!(block_header = %block_header.display(), "has no predecessor => fork");
            return None;
        }
        // 2. Has been seen: not a new transition
        if self.state_on_block.contains_key(&block_header.hash()) {
            tracing::trace!(block_header = %block_header.display(), "has been seen => fork");
            return None;
        }
        predecessor.map(|state| state.post_state_root.clone())
    }

    fn get_earliest_seen_height(&self) -> Option<u64> {
        self.seen_on_height.first_key_value().map(|(k, _)| *k)
    }

    // The highest seen does not mean the latest in the current chain.
    fn get_highest_seen_height(&self) -> Option<u64> {
        self.seen_on_height.last_key_value().map(|(k, _)| *k)
    }

    // If reorg happened,
    // the next incremental continuation of that fork that hasn't been processed should be found.
    async fn choose_fork_point(&self, da_service: &Da) -> anyhow::Result<ForkPoint<Da, StateRoot>> {
        if self.state_on_block.is_empty() {
            let last_finalized = da_service.get_last_finalized_block_header().await?;
            let adjacent = da_service.get_block_at(last_finalized.height() + 1).await?;
            // reorg can happen between these 2 calls, right now just panic, improve handling in the future.
            assert!(adjacent.header().prev_hash() == last_finalized.hash());
            return Ok(ForkPoint {
                block: adjacent,
                pre_state_root: self.state_root.clone(),
            });
        }

        // What if we already saw the head?
        // Then we will call the next height anyway and will wait.
        // What else can we do?
        let earliest_seen_height = self
            .get_earliest_seen_height()
            .expect("Choosing fork point only possible if some transitions have been seen");
        let highest_seen_height = self
            .get_highest_seen_height()
            .expect("Choosing fork point only possible if some transitions have been seen");

        let mut head = da_service.get_head_block_header().await?;

        'new_fork_search: for attempt in 0..MAX_REORG_FINDING_ATTEMPTS {
            let mut low = earliest_seen_height;
            let mut high = std::cmp::min(highest_seen_height, head.height()).saturating_add(1);
            tracing::trace!(
                highest_seen_height,
                low_height = low,
                high_height = high,
                this_fork_head = %head.display(),
                attempt,
                "Start choosing fork point"
            );

            // Start looking for a candidate for the fork point.
            // Candidate should be the next block after the latest seen transition that belongs to this chain.
            while low <= high {
                let mid = low + (high - low) / 2;
                tracing::trace!(
                    candidate_height = mid,
                    low,
                    high,
                    earliest_seen_height,
                    "Checking height"
                );
                let (candidate, this_head) = tokio::try_join!(
                    da_service.get_block_at(mid),
                    da_service.get_head_block_header()
                )?;
                if this_head.hash() != head.hash() {
                    tracing::warn!("Reorg happened during fork point selection, trying again");
                    head = this_head;
                    continue 'new_fork_search;
                }
                // Found our guy!
                if let Some(pre_state_root) =
                    self.get_preceding_state_root_if_new_transition(candidate.header())
                {
                    return Ok(ForkPoint {
                        block: candidate,
                        pre_state_root,
                    });
                }

                if self.state_on_block.contains_key(&candidate.header().hash()) {
                    // Seen this block, moving right.
                    low = mid.saturating_add(1);
                    tracing::trace!(
                    candidate = %candidate.header().display(),
                    new_low = low,
                    high = high,
                    "Seen this candidate, trying to find a later one");
                } else {
                    // Haven't seen this block, moving left.
                    high = mid.saturating_sub(1);
                    tracing::trace!(
                        candidate = %candidate.header().display(),
                        low = low,
                        new_high = high,
                        "Block is not a continuation of any seen transitions, checking earlier blocks"
                    );
                }
            }
            tracing::trace!("Haven't found candidate for fork point on seen transitions. It means candidate should be the next after last finalized height");
            // The difference in this case with the loop above,
            // is that we check that block at earliest seen transition height also points to last finalized height.

            // All earliest transitions point to the last known finalized state
            let any_earliest_seen_hash = self
                .seen_on_height
                .first_key_value()
                .expect("Choosing fork point only possible if some transitions have been seen")
                .1
                .iter()
                .next()
                .expect("There should be no entries without values");

            // This relies on an assumption, that a candidate hasn't been seen.
            let (candidate, this_head) = tokio::try_join!(
                da_service.get_block_at(earliest_seen_height),
                da_service.get_head_block_header()
            )?;
            if this_head.hash() != head.hash() {
                tracing::warn!("Reorg happened during fork point selection, trying again");
                head = this_head;
                continue 'new_fork_search;
            }
            if self.get_prev_hash(any_earliest_seen_hash) != candidate.header().prev_hash() {
                tracing::trace!(candidate = %candidate.header().display(), any_earliest = %any_earliest_seen_hash, "There should be block after last finalized");
                panic!("Finalized header changed");
            }

            let state_on_the_same_block = self
                .state_on_block
                .get(any_earliest_seen_hash)
                .expect("Internal inconsistency in maps");
            // As they point to the same previous block, it is safe to return pre_state_root
            let seen_prev_state_root = state_on_the_same_block.pre_state_root.clone();
            return Ok(ForkPoint {
                block: candidate,
                pre_state_root: seen_prev_state_root,
            });
        }

        anyhow::bail!("Could find fork point after {MAX_REORG_FINDING_ATTEMPTS} attempts")
    }

    fn get_prev_hash(
        &self,
        hash: &<Da::Spec as DaSpec>::SlotHash,
    ) -> <Da::Spec as DaSpec>::SlotHash {
        self.state_on_block
            .get(hash)
            .map(|state| state.block_header.prev_hash())
            .expect("Internal inconsistency in maps")
    }

    /// Returns all [`StateTransitionInfo`] which are below finalized height
    /// and relevant LedgerDb changes.
    /// Also returns the latest finalized block header, so caller shouldn't do another call.
    async fn process_finalized_state_transitions(
        &mut self,
        da_service: &Da,
    ) -> anyhow::Result<(
        <Da::Spec as DaSpec>::BlockHeader,
        Vec<StateOnBlock<Da::Spec, StateRoot>>,
    )> {
        let last_finalized_header = da_service.get_last_finalized_block_header().await?;
        let earliest_seen_transition = self
            .get_earliest_seen_height()
            .expect("Should be called after at least single transition added");
        let highest_seen_transition = self
            .get_highest_seen_height()
            .expect("Should be called after at least single transition added");

        tracing::trace!(
            last_finalized_header = %last_finalized_header.display(),
            highest_seen_transition,
            "Compare truly last finalized header with highest seen transition");

        let last_seen_finalized_header = if last_finalized_header.height() > highest_seen_transition
        {
            da_service
                .get_block_at(highest_seen_transition)
                .await?
                .header()
                .clone()
        } else {
            last_finalized_header.clone()
        };

        tracing::trace!(
            last_seen_finalized_header = %last_seen_finalized_header.display(),
            seen_transitions = self.state_on_block.len(),
            "Start processing finalized state transitions"
        );

        // Start with eliminating all non-finalized transitions
        // that does not originate from a finalized header.
        // But do we need this? Won't they be cleared on the next iteration, when finalized height rises?
        // Yes, 2 reasons:
        //   1. Not all of them will be removed, so we might have many orphaned transitions in memory.
        //   2. We rely on check on clean-seen state to check if reorg happened or not.
        {
            // We start from height after the last finalized header
            let start_height = last_seen_finalized_header
                .height()
                .checked_add(1)
                .expect("end of chain");
            let mut survivors = vec![last_seen_finalized_header.hash()];

            let range = start_height..=highest_seen_transition;
            tracing::trace!(
                 last_seen_finalized_header = % last_seen_finalized_header.display(),
                ?range,
                "Going to eliminate all future transitions which are not derived from last seen finalized header");
            for height in range {
                let new_survivors: Vec<_> = {
                    let this_height_blocks = self
                        .seen_on_height
                        .get(&height)
                        .expect("Continuity broken, inconsistent internal state");
                    this_height_blocks
                        .iter()
                        .filter(|block_hash| survivors.contains(&self.get_prev_hash(block_hash)))
                        .cloned()
                        .collect()
                };

                {
                    self.seen_on_height.get_mut(&height)
                        .expect("Continuity broken, inconsistent internal state")
                        .retain(|block_hash| {
                        if new_survivors.contains(block_hash) {
                            true
                        } else {
                            tracing::trace!(
                                %block_hash,
                                ?new_survivors,
                                "Removing block header from seen_on_height, because it does not originate from last seen finalized header"
                            );
                            self.state_on_block.remove(block_hash);
                            false
                        }
                    });
                }

                survivors = new_survivors;
            }
        }

        let mut finalized_transitions = Vec::with_capacity(
            last_seen_finalized_header
                .height()
                .saturating_sub(earliest_seen_transition) as usize,
        );

        // Going backwards does not mean there's a connection between earliest and fetched last seen finalized header.
        // TO BE 100% sure, we need to do N queries from earliest to latest seen finalized header.
        // We can offload that into a background task, that is subscribed and read it via channel.
        let range = (earliest_seen_transition..=last_seen_finalized_header.height()).rev();
        tracing::trace!(
            ?range,
            "Going to extract finalized transitions from previously seen transitions"
        );
        for height in range {
            // Slow, but can be improved. For now let's take care only about correctness.
            // TODO: If all CI passes, this can be tracked via subcription as mentioned above.
            let finalized_at_that_height = da_service.get_block_at(height).await?;

            tracing::trace!(height, "Going to extract finalized transitions from height");
            let blocks_on_height = self
                .seen_on_height
                .remove(&height)
                .expect("Should be at least one seen transition on each height");
            tracing::trace!(
                ?blocks_on_height,
                "Going to extract finalized transitions from height"
            );
            let mut pushed_for_this_height = false;
            for block_hash in blocks_on_height {
                let transition = self
                    .state_on_block
                    .remove(&block_hash)
                    .expect("Should be there");
                if block_hash == finalized_at_that_height.header().hash() {
                    assert!(
                        !pushed_for_this_height,
                        "Should be only one finalized transition per height"
                    );
                    finalized_transitions.push(transition);
                    pushed_for_this_height = true;
                }
            }
        }

        // Remove all entries in `seen_on_height` that have empty vectors.
        self.seen_on_height.retain(|_, entries| !entries.is_empty());

        finalized_transitions.reverse();
        tracing::trace!(
            finalized_transitions = finalized_transitions.len(),
            "Completed check for finalized transitions"
        );
        Ok((last_finalized_header, finalized_transitions))
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

    fn get_slot_number(&self) -> anyhow::Result<SlotNumber> {
        Ok(self.ledger_db.get_next_items_numbers()?.slot_number)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::num::NonZero;

    use futures::StreamExt;
    use proptest::prelude::*;
    use rand::SeedableRng;
    use sov_db::storage_manager::{NativeChangeSet, NativeStorageManager};
    use sov_mock_da::storable::layer::StorableMockDaLayer;
    use sov_mock_da::storable::service::StorableMockDaService;
    use sov_mock_da::{
        BlockProducingConfig, MockAddress, MockBlock, MockDaConfig, MockDaService, MockDaSpec,
        MockFee, MockHash, PlannedFork, RandomizationBehaviour, RandomizationConfig,
    };
    use sov_mock_zkvm::MockZkvm;
    use sov_modules_api::provable_height_tracker::InfiniteHeight;
    use sov_rollup_interface::common::{HexHash, SlotNumber};
    use sov_rollup_interface::node::ledger_api::LedgerStateProvider;
    use sov_rollup_interface::stf::StateTransitionFunction;
    use sov_state::{
        ArrayWitness, NativeStorage, ProverStorage, SlotKey, SlotValue, StateAccesses, Storage,
    };

    use super::*;
    use crate::mock::MockStf;

    type Vm = MockZkvm;
    type Stf = MockStf;
    type S = sov_state::DefaultStorageSpec<sha2::Sha256>;
    type StateRoot = <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::StateRoot;
    type TestBatchReceiptContents =
        <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::BatchReceiptContents;
    type TestTxReceiptContents =
        <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::TxReceiptContents;
    type Witness = <Stf as StateTransitionFunction<Vm, Vm, MockDaSpec>>::Witness;
    type MockSlotCommit = SlotCommit<MockBlock, Witness, TestTxReceiptContents>;
    type TestStateManager<Da> = StateManager<
        StateRoot,
        Witness,
        NativeStorageManager<<Da as DaService>::Spec, ProverStorage<S>>,
        Da,
    >;
    type TestStateManagerInMemory = TestStateManager<MockDaService>;

    const SEQUENCER_ADDRESS: MockAddress = MockAddress::new([0; 32]);
    const SEED_1: [u8; 32] = [1; 32];
    const SEED_2: [u8; 32] = [2; 32];
    const SEED_3: [u8; 32] = [3; 32];

    #[tokio::test(flavor = "multi_thread")]
    async fn test_empty_state_manager_returns_last_finalized_height() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let finality = 1000;
        let da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);
        da_service
            .send_transaction(&[10; 10], MockFee::zero())
            .await
            .await??;
        let filtered_block = da_service.get_block_at(1).await?;

        process_continuous_transition(&mut state_manager, filtered_block, &da_service, finality)
            .await?;

        // LedgerDb storage should be updated by that point, so the correct height is returned
        assert_eq!(
            SlotNumber::GENESIS,
            state_manager
                .ledger_db
                .get_latest_finalized_slot_number()
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
            process_continuous_transition(
                &mut state_manager,
                filtered_block.clone(),
                &da_service,
                0,
            )
            .await?;
            // TODO: Check how state manager internal state looks like on instant finality.
            let finalized = receiver.read_next().await?.unwrap();

            if let Some(sender) = state_manager.st_info_sender.as_ref() {
                sender.inc_next_height_to_receive();
            };

            assert_eq!(height, finalized.slot_number.get());
            assert_eq!(filtered_block.header, finalized.data.da_block_header);
            assert_eq!(state_root, finalized.data.initial_state_root);
            state_root.clone_from(&finalized.data.final_state_root);
            assert_eq!(
                height,
                state_manager
                    .ledger_db
                    .get_latest_finalized_slot_number()
                    .await?
                    .get()
            );
        }

        Ok(())
    }

    // Basic test for single reorg, but detailed check of what state root hash is returned.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_reorg_happened_correct_block_returned() -> anyhow::Result<()> {
        // The idea of the test is
        // to ensure that the state manager returns the correct block and storage aftera single reorg.
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let fork_point = 3;
        let fork_happens_at = 6;
        let finality = 5;

        let state_update_receiver = state_manager.state_update_sender.subscribe();

        let mut da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);
        da_service
            .set_planned_fork(PlannedFork::new(
                fork_happens_at,
                fork_point,
                vec![vec![11], vec![22], vec![33], vec![44]],
            ))
            .await?;

        // State root after executing i-th transition
        let mut post_state_roots = Vec::with_capacity(fork_happens_at as usize);
        let mut hash_to_post_state_root: HashMap<MockHash, StateRoot> = HashMap::new();

        for da_height in 1..=fork_happens_at {
            // Not used anywhere, `process_normal_transition` relies on da header to produce changes.
            let blob_data = [da_height as u8; 10];
            da_service
                .send_transaction(&blob_data, MockFee::zero())
                .await
                .await??;
            let filtered_block = da_service.get_block_at(da_height).await?;
            if da_height < fork_happens_at {
                let block_hash = filtered_block.header().hash();
                process_continuous_transition(
                    &mut state_manager,
                    filtered_block,
                    &da_service,
                    finality,
                )
                .await?;
                let current_state_root = state_manager.get_state_root().clone();
                let received_storage = state_update_receiver.borrow().storage.clone();
                let received_storage_root = get_last_storage_root_hash(&received_storage)?;
                assert_eq!(
                    current_state_root,
                    received_storage_root.root_hash().0.to_vec().into()
                );
                post_state_roots.push(current_state_root.clone());
                hash_to_post_state_root.insert(block_hash, current_state_root);
            } else {
                let (prover_storage, returned_block) = state_manager
                    .prepare_storage(filtered_block.clone(), &da_service)
                    .await?;
                assert_ne!(filtered_block, returned_block);
                // First non seen block:
                assert_eq!(fork_point + 1, returned_block.header().height());

                assert!(!hash_to_post_state_root.contains_key(&returned_block.header.hash));
                let expected_pre_state_root = hash_to_post_state_root
                    .get(&returned_block.header().prev_hash())
                    .expect("Should be there");
                assert_eq!(
                    expected_pre_state_root,
                    state_manager.get_state_root(),
                    "Expected (left) state root does not match actual(right) set in StateManager. All state roots: {:?}",
                    post_state_roots);

                let returned_storage_root = get_last_storage_root_hash(&prover_storage)?;
                let received_update_info = state_update_receiver.borrow().clone();
                let received_storage_root =
                    get_last_storage_root_hash(&received_update_info.storage)?;
                assert_eq!(returned_storage_root, received_storage_root);
            }
        }
        Ok(())
    }

    /// This test checks that process_stf_changes goes normally,
    /// even when the finalized block progressed above the passed block header.
    /// Important invariant, that ledger db receives "true" last finalized height,
    /// not height of the last **seen** finalized transition.
    /// Basically this test covers the case of "syncing node",
    /// and it is an important invariant that LedgerDb gets true finalized height
    #[tokio::test(flavor = "multi_thread")]
    async fn test_save_last_finalized_larger_than_seen_latest_seen_transition() -> anyhow::Result<()>
    {
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;
        let finality = 10;
        let da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);

        let chain_length = 5;
        // Fill some seen transitions without finalizing.
        for height in 1..chain_length {
            da_service
                .send_transaction(&[height as u8; 10], MockFee::zero())
                .await
                .await??;
            let filtered_block = da_service.get_block_at(height).await?;

            process_continuous_transition(
                &mut state_manager,
                filtered_block,
                &da_service,
                finality,
            )
            .await?;
            assert_eq!(
                0,
                state_manager
                    .ledger_db
                    .get_latest_finalized_slot_number()
                    .await?
                    .get()
            );
        }

        // Here we are going to finalize all things between
        da_service
            .send_transaction(&[chain_length as u8; 10], MockFee::zero())
            .await
            .await??;

        let filtered_block = da_service.get_block_at(chain_length).await?;
        let (prover_storage, returned_block) = state_manager
            .prepare_storage(filtered_block.clone(), &da_service)
            .await?;

        assert_eq!(filtered_block, returned_block);

        let produce_between = (finality * 3) as u64;
        for _ in 0..produce_between {
            da_service
                .send_transaction(&[10; 10], MockFee::zero())
                .await
                .await??;
        }

        let last_finalized_height = da_service.get_last_finalized_block_header().await?.height();

        let (change_set, transition_witness) = produce_synthetic_state_transition_witness(
            state_manager.get_state_root().to_owned(),
            &prover_storage,
            &da_service,
            filtered_block.clone(),
        )
        .await;

        let slot_commit: MockSlotCommit = SlotCommit::new(filtered_block);
        state_manager
            .process_stf_changes(
                &da_service,
                0,
                change_set,
                transition_witness,
                slot_commit,
                Vec::new(),
            )
            .await?;
        check_internal_consistency(&state_manager, finality as usize);

        // Last finalized height written to LedgerDb as it passed.
        assert_eq!(
            last_finalized_height,
            state_manager
                .ledger_db
                .get_latest_finalized_slot_number()
                .await?
                .get()
        );
        Ok(())
    }

    // Test simulates usage of StateManager by StfRunner
    // DaLayer is set up with finality, some empty blocks are padded, and some batches are submitted.
    // Then it iterates for `loop_blocks` producing a new block on every loop.
    async fn test_progressing_with_shuffle(
        finality: u32,
        empty_padding: u32,
        batches: usize,
        loop_blocks: usize,
        shuffle_after: usize,
        seed: [u8; 32],
    ) -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let da_layer = std::sync::Arc::new(tokio::sync::RwLock::new(
            StorableMockDaLayer::new_in_memory(finality).await?,
        ));
        let da_service = StorableMockDaService::new(
            SEQUENCER_ADDRESS,
            da_layer.clone(),
            BlockProducingConfig::OnBatchSubmit {
                block_wait_timeout_ms: Some(3_000),
            },
        );
        let mut rng = rand::rngs::SmallRng::from_seed(seed);

        // Empty padding
        da_service
            .produce_n_blocks_now(empty_padding as usize)
            .await?;

        // Blobs
        let blob_data = [10; 10];
        for _ in 0..batches {
            da_service
                .send_transaction(&blob_data, MockFee::zero())
                .await
                .await??;
        }

        if empty_padding == 0 && batches == 0 {
            // Producing height=1, so the main loop can kick in.
            da_service.produce_block_now().await?;
        }

        let mut max_seen_height = 0;
        let mut non_finalized_batches = batches.saturating_sub(finality as usize);
        let mut last_finalized_header = da_service.get_last_finalized_block_header().await?;
        let mut height = match last_finalized_header.height {
            0 => 1,
            h => h,
        };

        let mut seen_transitions: HashMap<MockHash, StateRoot> = HashMap::new();
        let mut finalized_hashes: HashSet<MockHash> = HashSet::new();
        for h in 0..=last_finalized_header.height() {
            finalized_hashes.insert(da_service.get_block_at(h).await?.header().hash());
        }

        // This is a simplified version of `StfRunner
        //  - Track height, adjusts it based on StateManager results
        //  - Produce some changes based on a given block
        //  - Moves on the next height
        for i in 0..loop_blocks {
            // Start with getting block
            let filtered_block = da_service.get_block_at(height).await?;

            let (prover_storage, returned_block) = state_manager
                .prepare_storage(filtered_block, &da_service)
                .await?;

            // Always a new non-seen block
            assert!(
                !seen_transitions.contains_key(&returned_block.header().hash()),
                "Already seen: {}",
                returned_block.header().display()
            );

            let prev_hash = returned_block.header().prev_hash();
            assert!(
                seen_transitions.contains_key(&prev_hash) || finalized_hashes.contains(&prev_hash),
                "prev hash of returned block should be seen or in finalized {} SEEN: {:?} FINALIZED {:?}",
                returned_block.header().display(),
                seen_transitions,
                finalized_hashes,
            );

            let (change_set, transition_witness) = produce_synthetic_state_transition_witness(
                state_manager.get_state_root().to_owned(),
                &prover_storage,
                &da_service,
                returned_block.clone(),
            )
            .await;

            let slot_commit: MockSlotCommit = SlotCommit::new(returned_block.clone());

            let state_root_hash = transition_witness.final_state_root.clone();
            state_manager
                .process_stf_changes(
                    &da_service,
                    0,
                    change_set,
                    transition_witness,
                    slot_commit,
                    Vec::new(),
                )
                .await?;
            check_internal_consistency(&state_manager, finality as usize);

            seen_transitions.insert(returned_block.header().hash(), state_root_hash);

            if returned_block.header().height() > max_seen_height {
                max_seen_height = returned_block.header().height();
            }

            height = returned_block.header().height() + 1;

            if let Some(earliest_seen_height) = state_manager.get_earliest_seen_height() {
                assert!(
                    earliest_seen_height >= last_finalized_header.height(),
                    "older finalized heights are not erased: {} {}: {:?}",
                    earliest_seen_height,
                    last_finalized_header.height(),
                    state_manager.seen_on_height,
                );
                let highest_seen_height =
                    state_manager.seen_on_height.keys().copied().max().unwrap();
                assert!(
                    highest_seen_height <= max_seen_height,
                    "Inconsistent state transitions, highest seen hight is too large"
                );
            }

            // Check is done, moving the chain forward

            if i > 0 && i % shuffle_after == 0 {
                let mut da_layer = da_layer.write().await;
                da_layer.shuffle_non_finalized_blobs(&mut rng, 0).await?;
            }
            // First, check if we need to submit a blob, so it will keep floating
            // TO
            // last_finalized_header = da_service.get_last_finalized_block_header().await?;

            // New block should always be created with a batch
            if batches >= finality as usize {
                da_service
                    .send_transaction(&blob_data, MockFee::zero())
                    .await
                    .await??;
            } else {
                let next_finalized_block = da_service
                    .get_block_at(last_finalized_header.height().saturating_add(1))
                    .await?;
                // All batches in next block are going to be finalized, so it won't be possible to shuffle them anymore
                non_finalized_batches =
                    non_finalized_batches.saturating_sub(next_finalized_block.batch_blobs.len());
                // We try to maintain number of non finalized batches closer to the original number.
                if non_finalized_batches < batches {
                    da_service
                        .send_transaction(&blob_data, MockFee::zero())
                        .await
                        .await??;
                    non_finalized_batches += 1;
                } else {
                    da_service.produce_block_now().await?;
                }
            }
            last_finalized_header = da_service.get_last_finalized_block_header().await?;
            finalized_hashes.insert(last_finalized_header.hash());
        }
        Ok(())
    }

    // This test check that a chain always returns the non-executed block, even if chain forks are restored.
    // We emulate the return of the chain by having only a single blob "floating" between a number of empty blocks.
    // Empty blocks have the same root hash, so we can check that we don't execute empty blocks several times.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_double_reorg_chain_restored() -> anyhow::Result<()> {
        let finality = 20;
        let empty_blocks_padding = 15;
        let batches = 1;
        let loop_blocks = 100;
        for seed in [SEED_1, SEED_2, SEED_3] {
            test_progressing_with_shuffle(
                finality,
                empty_blocks_padding,
                batches,
                loop_blocks,
                3,
                seed,
            )
            .await?;
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_shuffle_with_multiple_blobs() -> anyhow::Result<()> {
        let finality = 20;
        let empty_blocks_padding = 0;
        let batches = 5;
        let loop_blocks = 50;
        for seed in [SEED_1, SEED_2, SEED_3] {
            test_progressing_with_shuffle(
                finality,
                empty_blocks_padding,
                batches,
                loop_blocks,
                2,
                seed,
            )
            .await?;
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_shuffle_with_deeper_reorgs() -> anyhow::Result<()> {
        let finality = 20;
        let empty_blocks_padding = 10;
        let batches = 5;
        let loop_blocks = 50;
        for seed in [SEED_1, SEED_2, SEED_3] {
            test_progressing_with_shuffle(
                finality,
                empty_blocks_padding,
                batches,
                loop_blocks,
                10,
                seed,
            )
            .await?;
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_with_frequent_periodic_batch_production() -> anyhow::Result<()> {
        // sov_test_utils::initialize_logging();
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let finality = 50;
        let (sender, mut receiver) = tokio::sync::watch::channel(());
        receiver.mark_unchanged();

        let da_service = StorableMockDaService::from_config(
            MockDaConfig {
                connection_string: "sqlite::memory:".to_string(),
                sender_address: SEQUENCER_ADDRESS,
                finalization_blocks: finality,
                block_producing: BlockProducingConfig::Periodic { block_time_ms: 100 },
                da_layer: None,
                randomization: Some(RandomizationConfig {
                    seed: HexHash::from(SEED_1),
                    // At every new block
                    reorg_interval: 1..2,
                    behaviour: RandomizationBehaviour::only_shuffle(0),
                }),
            },
            receiver,
        )
        .await;
        {
            let spammer = da_service.clone();
            let _handle: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
                let mut finalized_blocks = spammer.subscribe_finalized_header().await?;
                let blob = vec![10, 10];
                while let Some(res) = finalized_blocks.next().await {
                    let _ = match res {
                        Ok(b) => b,
                        Err(_err) => {
                            break;
                        }
                    };
                    spammer
                        .send_transaction(&blob, MockFee::zero())
                        .await
                        .await??;
                }
                Ok(())
            });
        }

        let mut height = match da_service.get_last_finalized_block_header().await?.height() {
            0 => 1,
            h => h,
        };
        let final_height = 100;

        let mut seen_transitions: HashMap<MockHash, StateRoot> = HashMap::new();

        while height < final_height {
            let filtered_block = da_service.get_block_at(height).await?;
            let (prover_storage, returned_block) = state_manager
                .prepare_storage(filtered_block, &da_service)
                .await?;

            assert!(
                !seen_transitions.contains_key(&returned_block.header().hash()),
                "Already seen: {}",
                returned_block.header().display()
            );
            // TODO: Check prev_hash connected to something already seen.

            let (change_set, transition_witness) = produce_synthetic_state_transition_witness(
                state_manager.get_state_root().to_owned(),
                &prover_storage,
                &da_service,
                returned_block.clone(),
            )
            .await;

            let slot_commit: MockSlotCommit = SlotCommit::new(returned_block.clone());

            let state_root_hash = transition_witness.final_state_root.clone();
            state_manager
                .process_stf_changes(
                    &da_service,
                    0,
                    change_set,
                    transition_witness,
                    slot_commit,
                    Vec::new(),
                )
                .await?;
            check_internal_consistency(&state_manager, finality as usize);
            seen_transitions.insert(returned_block.header().hash(), state_root_hash);

            height = returned_block.header().height() + 1;
        }

        sender.send(())?;
        Ok(())
    }

    // After each "prepare_storage" there are (empty_blobs + batch_blobs) number of blocks produced.
    // `shuffle_after` controls how often shuffle happens, based on number of blocks last shuffle happened
    async fn test_chain_progress_between_prepare_storage_and_save_changes(
        finality: u32,
        // Total number of iterations.
        loop_blocks: usize,
        // Progression parameters.
        empty_blobs: usize,
        batch_blobs: usize,
        shuffle_after: u64,
        seed: [u8; 32],
    ) -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let mut rng = rand::rngs::SmallRng::from_seed(seed);

        let da_layer = std::sync::Arc::new(tokio::sync::RwLock::new(
            StorableMockDaLayer::new_in_memory(finality).await?,
        ));
        let da_service = StorableMockDaService::new(
            SEQUENCER_ADDRESS,
            da_layer.clone(),
            BlockProducingConfig::OnBatchSubmit {
                block_wait_timeout_ms: Some(3_000),
            },
        );
        // To kick start things.
        da_service.produce_block_now().await?;

        let mut seen_transitions: HashMap<MockHash, StateRoot> = HashMap::new();
        let mut height = 1;
        let mut last_shuffled_height = 0;

        for _ in 0..loop_blocks {
            let filtered_block = da_service.get_block_at(height).await?;
            let (prover_storage, returned_block) = state_manager
                .prepare_storage(filtered_block, &da_service)
                .await?;

            assert!(
                !seen_transitions.contains_key(&returned_block.header().hash()),
                "Already seen: {}",
                returned_block.header().display()
            );
            // TODO: Check prev_hash connected to something already seen.

            // Here we do some progression of the chain
            {
                da_service.produce_n_blocks_now(empty_blobs).await?;
                for i in 0..batch_blobs {
                    let blob_data = [i as u8, i as u8];
                    da_service
                        .send_transaction(&blob_data, MockFee::zero())
                        .await
                        .await??;
                }
                let head = da_service.get_head_block_header().await?;
                let head_height = head.height().saturating_sub(last_shuffled_height);
                if head_height > shuffle_after {
                    let mut da_layer = da_layer.write().await;
                    da_layer.shuffle_non_finalized_blobs(&mut rng, 0).await?;
                    last_shuffled_height = head_height;
                }
            }

            // Then saving
            let (change_set, transition_witness) = produce_synthetic_state_transition_witness(
                state_manager.get_state_root().to_owned(),
                &prover_storage,
                &da_service,
                returned_block.clone(),
            )
            .await;

            let slot_commit: MockSlotCommit = SlotCommit::new(returned_block.clone());

            let state_root_hash = transition_witness.final_state_root.clone();
            state_manager
                .process_stf_changes(
                    &da_service,
                    0,
                    change_set,
                    transition_witness,
                    slot_commit,
                    Vec::new(),
                )
                .await?;
            check_internal_consistency(&state_manager, finality as usize);

            seen_transitions.insert(returned_block.header().hash(), state_root_hash);

            height = returned_block.header().height() + 1;
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_chain_progress_between_prepare_and_save_instant_finality() -> anyhow::Result<()> {
        for seed in [SEED_1, SEED_2, SEED_3] {
            // With empty blobs
            test_chain_progress_between_prepare_storage_and_save_changes(0, 60, 3, 3, 6, seed)
                .await?;
            // Without empty blobs
            test_chain_progress_between_prepare_storage_and_save_changes(0, 60, 0, 3, 6, seed)
                .await?;
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_chain_progress_between_prepare_and_save_non_instant_finality(
    ) -> anyhow::Result<()> {
        let finality = 5;

        for seed in [SEED_1, SEED_2, SEED_3] {
            // With empty blobs
            test_chain_progress_between_prepare_storage_and_save_changes(
                finality, 60, 1, 2, 6, seed,
            )
            .await?;
            // Shuffle every time
            test_chain_progress_between_prepare_storage_and_save_changes(
                finality, 60, 1, 2, 3, seed,
            )
            .await?;
            // Without empty blobs
            test_chain_progress_between_prepare_storage_and_save_changes(
                finality, 60, 0, 3, 6, seed,
            )
            .await?;
        }

        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn proptest_shuffling_with_different_params(
            finality in prop_oneof![
                Just(0u32),
                Just(1u32),
                Just(5u32)
            ],
            loop_blocks in 1..=20usize,
            batches in prop_oneof![
                Just(0usize),
                Just(2usize),
                Just(5usize)
            ],
            reshuffle_after in prop_oneof![
                Just(1usize),
                Just(3usize),
                Just(5usize)
            ],
            seed in prop_oneof![
                Just(SEED_1),
                Just(SEED_2),
                Just(SEED_3),
            ]
            ) {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on( async {
                        let test_future = test_progressing_with_shuffle(
                            finality,
                            0,
                            batches,
                            loop_blocks,
                            reshuffle_after,
                            seed,
                        );
                        tokio::time::timeout(std::time::Duration::from_secs(5), test_future).await.unwrap().unwrap();
                });
            }

        #[test]
        fn proptest_chain_prorgress_between(
            finality in prop_oneof![
                Just(0u32),
                Just(1u32),
                Just(5u32)
            ],
            loop_blocks in 1..=20usize,
            batches in prop_oneof![
                Just(1usize),
                Just(2usize),
                Just(5usize)
            ],
            reshuffle_after in prop_oneof![
                Just(1u64),
                Just(3u64),
                Just(5u64)
            ],
            seed in prop_oneof![
                Just(SEED_1),
                Just(SEED_2),
                Just(SEED_3),
            ]
            ) {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on( async {
                        let test_future = test_chain_progress_between_prepare_storage_and_save_changes(
                            finality,
                            loop_blocks,
                            0,
                            batches,
                            reshuffle_after,
                            seed,
                        );
                        tokio::time::timeout(std::time::Duration::from_secs(5), test_future).await.unwrap().unwrap();
                });
            }
    }

    // Fail case tests
    /// Normal changes tracked in state manager, some of them finalized.
    /// Then new [`MockDaService`] is initialized and new blocks are submitted, so new different header is finalized.
    /// This way we can have a case where [`StateManager`] cannot backtrack to continuous transition,
    /// because finalized were eliminated. This behaviour is similar as starting from a non-finalized block and then whole chain switches.
    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "Finalized header changed")]
    async fn test_change_in_finalized_header() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut state_manager = setup_state_manager(tempdir.path()).await.unwrap();

        let chain_length = 5;
        let finality = 3;

        let da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);

        for height in 1..=chain_length {
            da_service
                .send_transaction(&[height as u8; 10], MockFee::zero())
                .await
                .await
                .unwrap()
                .unwrap();
            let filtered_block = da_service.get_block_at(height).await.unwrap();
            process_continuous_transition(
                &mut state_manager,
                filtered_block.clone(),
                &da_service,
                finality,
            )
            .await
            .unwrap();
        }

        let da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);
        for height in 1..=chain_length {
            da_service
                .send_transaction(&[(height * 10) as u8; 10], MockFee::zero())
                .await
                .await
                .unwrap()
                .unwrap();
        }

        let alien_block = da_service
            .get_block_at(da_service.get_head_block_header().await.unwrap().height())
            .await
            .unwrap();

        state_manager
            .prepare_storage(alien_block, &da_service)
            .await
            .unwrap();
    }

    // On empty internal state, state manager should check if passed block is finalized
    // And return last finalized.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_state_manager_starts_from_non_finalized_height() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let mut state_manager = setup_state_manager(tempdir.path()).await?;

        let chain_length = 7;
        let finality = 5;

        let da_service = MockDaService::new(SEQUENCER_ADDRESS).with_finality(finality);
        for height in 1..=chain_length {
            da_service
                .send_transaction(&[(height * 10) as u8; 10], MockFee::zero())
                .await
                .await??;
        }

        let last_finalized_header = da_service.get_last_finalized_block_header().await?;
        // Should be allowed, because storage has continuous data
        let next_to_finalized = da_service
            .get_block_at(last_finalized_header.height() + 1)
            .await?;
        // Should not be allowed
        let not_next_to_finalized = da_service
            .get_block_at(last_finalized_header.height() + 2)
            .await?;

        let (_prover_storage, returned_block_1) = state_manager
            .prepare_storage(next_to_finalized.clone(), &da_service)
            .await?;

        assert_eq!(returned_block_1, next_to_finalized);

        let (_prover_storage, returned_block_2) = state_manager
            .prepare_storage(not_next_to_finalized.clone(), &da_service)
            .await?;

        assert_ne!(returned_block_2, not_next_to_finalized);
        assert_eq!(returned_block_2, next_to_finalized);

        Ok(())
    }

    // TODO: Add tests that verification of finalized transitions only contains finalized blocks

    // TODO: Test state manager starts from non finalized height, then chain forks and all transitions are obliterated.
    // prepare_storage will panic probably
    // But process storage might just eliminate all transitions and it will start from finalized height.
    // Is it bad? Probably yes, because

    // ----------------
    // Helper functions
    async fn setup_storage_manager(
        path: &std::path::Path,
    ) -> anyhow::Result<(
        StateRoot,
        NativeStorageManager<MockDaSpec, ProverStorage<S>>,
    )> {
        let mut storage_manager: NativeStorageManager<MockDaSpec, ProverStorage<S>> =
            NativeStorageManager::new(path)?;
        let genesis_block = MockBlock::default_at_height(0);
        let genesis_header = genesis_block.header().clone();
        let (genesis_storage, ledger_state) = storage_manager.create_state_for(&genesis_header)?;
        let ledger_db = LedgerDb::with_reader(ledger_state)?;

        let (state_root, change_set) =
            produce_synthetic_changes::<MockDaSpec>(&genesis_storage, &genesis_header);

        let data_to_commit: SlotCommit<_, TestBatchReceiptContents, TestTxReceiptContents> =
            SlotCommit::new(genesis_block);
        let mut ledger_change_set =
            ledger_db.materialize_slot(data_to_commit, state_root.as_ref())?;
        let finalized_slot_changes =
            ledger_db.materialize_latest_finalize_slot(SlotNumber::GENESIS)?;
        ledger_change_set.merge(finalized_slot_changes);

        storage_manager.save_change_set(&genesis_header, change_set, ledger_change_set)?;
        storage_manager.finalize(&genesis_header)?;

        Ok((state_root, storage_manager))
    }

    async fn setup_state_manager<Da>(path: &std::path::Path) -> anyhow::Result<TestStateManager<Da>>
    where
        Da: DaService<Error = anyhow::Error, Spec = MockDaSpec>,
    {
        let (state_root, mut storage_manager) = setup_storage_manager(path).await?;

        let (stf_state, ledger_state) = storage_manager.create_bootstrap_state()?;
        let ledger_db = LedgerDb::with_reader(ledger_state)?;
        let update_info = query_state_update_info(&ledger_db, stf_state).await?;

        // Update channel, receiver does not need to be alive
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

    // Writes to user space concatenation of block height bytes and block hash
    fn produce_synthetic_changes<Da: DaSpec>(
        prover_storage: &ProverStorage<S>,
        block_header: &Da::BlockHeader,
    ) -> (StateRoot, NativeChangeSet) {
        let mut data = block_header.height().to_le_bytes().to_vec();
        data.extend_from_slice(block_header.hash().as_ref());
        let mut accesses = StateAccesses::default();
        accesses
            .user
            .ordered_writes
            .push((SlotKey::from(data.clone()), Some(SlotValue::from(data))));
        let (state_root, state_update) = prover_storage
            .compute_state_update(accesses, &ArrayWitness::default())
            .unwrap();
        let change_set = prover_storage.materialize_changes(&state_update);

        (state_root.root_hash().0.to_vec().into(), change_set)
    }

    async fn produce_synthetic_state_transition_witness<Da: DaService>(
        initial_state_root: StateRoot,
        prover_storage: &ProverStorage<S>,
        da_service: &Da,
        filtered_block: Da::FilteredBlock,
    ) -> (
        NativeChangeSet,
        StateTransitionWitness<StateRoot, Witness, Da::Spec>,
    ) {
        let (state_root, change_set) =
            produce_synthetic_changes::<Da::Spec>(prover_storage, filtered_block.header());
        let (relevant_blobs, relevant_proofs) = da_service
            .extract_relevant_blobs_with_proof(&filtered_block)
            .await;

        let transition_witness = StateTransitionWitness {
            initial_state_root,
            final_state_root: state_root,
            da_block_header: filtered_block.header().clone(),
            relevant_proofs,
            relevant_blobs,
            witness: (),
        };

        (change_set, transition_witness)
    }

    fn get_last_storage_root_hash(
        prover_storage: &ProverStorage<S>,
    ) -> anyhow::Result<sov_state::StorageRoot<S>> {
        let latest_slot = prover_storage.latest_version();
        prover_storage.get_root_hash(latest_slot)
    }

    // Passed `filtered_block` supposed to be a continuation of the current chain,
    // So this helper function performs transition and checks that there is no error
    async fn process_continuous_transition(
        state_manager: &mut TestStateManagerInMemory,
        filtered_block: MockBlock,
        da_service: &MockDaService,
        finality: u32,
    ) -> anyhow::Result<()> {
        let (prover_storage, returned_block) = state_manager
            .prepare_storage(filtered_block.clone(), da_service)
            .await?;

        assert_eq!(filtered_block, returned_block);

        let (change_set, transition_witness) = produce_synthetic_state_transition_witness(
            state_manager.get_state_root().to_owned(),
            &prover_storage,
            da_service,
            filtered_block.clone(),
        )
        .await;

        let slot_commit: MockSlotCommit = SlotCommit::new(filtered_block);
        state_manager
            .process_stf_changes(
                da_service,
                0,
                change_set,
                transition_witness,
                slot_commit,
                Vec::new(),
            )
            .await?;
        check_internal_consistency(state_manager, finality as usize);

        Ok(())
    }

    fn check_internal_consistency<Da>(state_manager: &TestStateManager<Da>, finality: usize)
    where
        Da: DaService<Error = anyhow::Error>,
    {
        // Ensure consistency between seen_on_height and state_on_block
        for (height, seen_blocks) in &state_manager.seen_on_height {
            assert!(
                !seen_blocks.is_empty(),
                "empty seen blocks at height: {}. Dirty!",
                height
            );
            for seen_hash in seen_blocks {
                assert_eq!(
                    state_manager
                        .state_on_block
                        .get(seen_hash)
                        .map(|state| state.block_header.hash())
                        .as_ref(),
                    Some(seen_hash)
                );
                if let Some(state) = state_manager.state_on_block.get(seen_hash) {
                    assert_eq!(
                        height, &state.block_header.height(),
                        "Inconsistency found: height in seen_on_height ({}) does not match state_on_block ({})",
                        height, state.block_header.height()
                    );
                    assert_eq!(
                        &state.block_header.prev_hash(),
                        &state_manager.get_prev_hash(seen_hash),
                        "Inconsistency found: prev_hash in seen_on_height ({}) does not match state_on_block ({})",
                        height, state.block_header.prev_hash()
                    );
                } else {
                    panic!(
                        "Block {} from seen_on_height is missing in state_on_block",
                        seen_hash
                    );
                }
            }
        }

        // Check if all blocks in state_on_block are present in seen_on_height
        for (block_hash, state) in &state_manager.state_on_block {
            let block_header = &state.block_header;
            assert_eq!(&block_header.hash(), block_hash);
            if !state_manager
                .seen_on_height
                .get(&block_header.height())
                .expect("Block is missing from seen_on_height")
                .iter()
                .any(|seen_hash| seen_hash == block_hash)
            {
                panic!(
                    "Block {} from state_on_block is missing in seen_on_height",
                    block_header.display(),
                );
            }
        }

        // We should not observe more hights than there are non-finalized blocks possible.
        let seen_on_height_size = state_manager.seen_on_height.len();
        assert!(
            seen_on_height_size <= finality,
            "Size of seen_on_height={} is more than finality={}",
            seen_on_height_size,
            finality
        );

        let earliest_seen_height = state_manager.get_earliest_seen_height();
        let highest_seen_height = state_manager.get_highest_seen_height();

        // There should be no gaps between heights of observed blocks.
        let expected_continuous_size = match (earliest_seen_height, highest_seen_height) {
            (Some(earliest), Some(latest)) => latest
                .saturating_sub(earliest)
                .checked_add(1)
                .expect("bug in test") as usize,
            (None, None) => 0,
            _ => panic!("Impossible, both values derived from same map"),
        };

        assert_eq!(seen_on_height_size, expected_continuous_size);
    }
}
