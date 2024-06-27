use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use jsonrpsee::RpcModule;
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_db::schema::{CacheDb, SchemaBatch};
use sov_rollup_interface::da::{BlobReaderTrait, BlockHeaderTrait, DaSpec};
use sov_rollup_interface::services::da::{DaService, SlotData};
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::{StateTransitionWitness, Zkvm, ZkvmGuest, ZkvmHost};
use tokio::sync::watch;
use tracing::{debug, error, info};

use crate::{ProofManager, ProverService, RunnerConfig, StateTransitionInfo};

type StateRoot<ST, InnerVm, OuterVm, Da> =
    <ST as StateTransitionFunction<InnerVm, OuterVm, Da>>::StateRoot;
type GenesisParams<ST, InnerVm, OuterVm, Da> =
    <ST as StateTransitionFunction<InnerVm, OuterVm, Da>>::GenesisParams;

type Verifier<Host> = <<Host as ZkvmHost>::Guest as ZkvmGuest>::Verifier;

/// Combines `DaService` with `StateTransitionFunction` and "runs" the rollup.
pub struct StateTransitionRunner<Stf, Sm, Da, InnerVm, OuterVm, Ps>
where
    Da: DaService,
    InnerVm: ZkvmHost,
    OuterVm: ZkvmHost,
    Sm: HierarchicalStorageManager<Da::Spec>,
    Stf: StateTransitionFunction<
        Verifier<InnerVm>,
        Verifier<OuterVm>,
        Da::Spec,
        Condition = <Da::Spec as DaSpec>::ValidityCondition,
    >,
    Ps: ProverService,
{
    first_unprocessed_height_at_startup: u64,
    da_polling_interval_ms: u64,
    da_service: Arc<Da>,
    stf: Stf,
    storage_manager: Sm,
    rpc_storage_sender: watch::Sender<Sm::StfState>,
    ledger_db: LedgerDb,
    state_root: StateRoot<Stf, Verifier<InnerVm>, Verifier<OuterVm>, Da::Spec>,
    listen_address_rpc: SocketAddr,
    listen_address_axum: SocketAddr,
    proof_manager: ProofManager<Ps>,
    sync_state: Arc<DaSyncState>,
}

/// The state necessary to track the sync status of the node
#[derive(Debug, Default)]
pub struct DaSyncState {
    synced_da_height: AtomicU64,
    target_da_height: AtomicU64,
}

/// The status of the current sync
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncStatus {
    /// The node has caught up to the chain tip
    Synced {
        /// The current height through which we've synced
        synced_da_height: u64,
    },
    /// The node is currently syncing
    Syncing {
        /// The current height through which we've synced
        synced_da_height: u64,
        /// The height to which we're syncing. This reflects the current view of the DA chain tip
        target_da_height: u64,
    },
}

impl SyncStatus {
    /// Returns true if the sync status is `Synced`
    pub fn is_synced(&self) -> bool {
        match self {
            SyncStatus::Synced { .. } => true,
            SyncStatus::Syncing { .. } => false,
        }
    }
}

impl DaSyncState {
    async fn update_target<Da: DaService<Error = anyhow::Error>>(
        &self,
        da_service: &Da,
    ) -> Result<(), anyhow::Error> {
        let target_da_height = da_service.get_head_block_header().await?.height();
        self.target_da_height
            .store(target_da_height, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    fn status(&self) -> SyncStatus {
        let current = self
            .synced_da_height
            .load(std::sync::atomic::Ordering::Acquire);
        let target = self
            .target_da_height
            .load(std::sync::atomic::Ordering::Acquire);

        if current == target {
            SyncStatus::Synced {
                synced_da_height: current,
            }
        } else {
            SyncStatus::Syncing {
                synced_da_height: current,
                target_da_height: target,
            }
        }
    }
}

/// How [`StateTransitionRunner`] is initialized
pub enum InitVariant<
    Stf: StateTransitionFunction<InnerVm, OuterVm, Da::Spec>,
    InnerVm: Zkvm,
    OuterVm: Zkvm,
    Da: DaService,
> {
    /// From give state root
    Initialized(Stf::StateRoot),
    /// From empty state root
    Genesis {
        /// Genesis block header should be finalized at initialization moment.
        block: Da::FilteredBlock,
        /// Genesis params for Stf::init.
        genesis_params: GenesisParams<Stf, InnerVm, OuterVm, Da::Spec>,
    },
}

impl<Stf, Sm, Da, InnerVm, OuterVm, Ps> StateTransitionRunner<Stf, Sm, Da, InnerVm, OuterVm, Ps>
where
    Da: DaService<Error = anyhow::Error> + Clone,
    InnerVm: ZkvmHost,
    OuterVm: ZkvmHost,
    Sm: HierarchicalStorageManager<Da::Spec, LedgerChangeSet = SchemaBatch, LedgerState = CacheDb>,
    Sm::StfState: Clone,
    Stf: StateTransitionFunction<
        <InnerVm::Guest as ZkvmGuest>::Verifier,
        <OuterVm::Guest as ZkvmGuest>::Verifier,
        Da::Spec,
        Condition = <Da::Spec as DaSpec>::ValidityCondition,
        PreState = Sm::StfState,
        ChangeSet = Sm::StfChangeSet,
    >,
    Ps: ProverService<StateRoot = Stf::StateRoot, Witness = Stf::Witness, DaService = Da>,
{
    /// Creates a new [`StateTransitionRunner`].
    ///
    /// If a previous state root is provided, uses that as the starting point
    /// for execution. Otherwise, initializes the chain using the provided
    /// genesis config.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runner_config: RunnerConfig,
        da_service: Arc<Da>,
        mut ledger_db: LedgerDb,
        stf: Stf,
        mut storage_manager: Sm,
        rpc_storage_sender: watch::Sender<Sm::StfState>,
        init_variant: InitVariant<
            Stf,
            <InnerVm::Guest as ZkvmGuest>::Verifier,
            <OuterVm::Guest as ZkvmGuest>::Verifier,
            Da,
        >,
        proof_manager: ProofManager<Ps>,
    ) -> Result<Self, anyhow::Error> {
        let rpc_config = runner_config.rpc_config;
        let axum_config = runner_config.axum_config;

        let prev_state_root = match init_variant {
            InitVariant::Initialized(state_root) => {
                debug!("Chain is already initialized; skipping initialization");
                state_root
            }
            InitVariant::Genesis {
                block,
                genesis_params: params,
            } => {
                let block_header = block.header().clone();
                info!(
                    header = %block_header.display(),
                    "No history detected. Initializing chain on the block header..."
                );
                let (stf_state, ledger_state) = storage_manager.create_state_for(&block_header)?;
                ledger_db.replace_db(ledger_state)?;
                let (genesis_root, initialized_storage) = stf.init_chain(stf_state, params);
                let data_to_commit: SlotCommit<
                    _,
                    Stf::BatchReceiptContents,
                    Stf::TxReceiptContents,
                > = SlotCommit::new(block);
                let mut ledger_change_set =
                    ledger_db.materialize_slot(data_to_commit, genesis_root.as_ref())?;
                let finalized_slot_changes = ledger_db.materialize_latest_finalize_slot(0)?;
                ledger_change_set.merge(finalized_slot_changes);
                storage_manager.save_change_set(
                    &block_header,
                    initialized_storage,
                    ledger_change_set,
                )?;

                storage_manager.finalize(&block_header)?;
                ledger_db.send_notifications();
                info!(
                    genesis_root = hex::encode(genesis_root.as_ref()),
                    "Chain initialization is done"
                );
                genesis_root
            }
        };

        let listen_address_rpc =
            SocketAddr::new(rpc_config.bind_host.parse()?, rpc_config.bind_port);
        let listen_address_axum =
            SocketAddr::new(axum_config.bind_host.parse()?, axum_config.bind_port);

        // Start the main rollup loop
        let next_item_numbers = ledger_db.get_next_items_numbers()?;
        let last_slot_processed_before_shutdown = next_item_numbers.slot_number.saturating_sub(1);

        let da_height_processed =
            runner_config.genesis_height + last_slot_processed_before_shutdown;

        let first_unprocessed_height_at_startup = da_height_processed + 1;
        debug!(%last_slot_processed_before_shutdown, %runner_config.genesis_height, %first_unprocessed_height_at_startup, "Initializing StfRunner");

        Ok(Self {
            first_unprocessed_height_at_startup,
            da_polling_interval_ms: runner_config.da_polling_interval_ms,
            da_service: da_service.clone(),
            stf,
            storage_manager,
            rpc_storage_sender,
            ledger_db,
            state_root: prev_state_root,
            listen_address_rpc,
            listen_address_axum,
            proof_manager,

            sync_state: Arc::new(DaSyncState {
                synced_da_height: AtomicU64::new(da_height_processed),
                target_da_height: AtomicU64::new(u64::MAX),
            }),
        })
    }

    /// Starts a RPC server with provided rpc methods.
    ///  # Arguments:
    ///   * methods: [`RpcModule`] with all RPC methods.
    ///   * channel: If `Some`, notification with actual [`SocketAddr`] where RPC server listens to.
    pub async fn start_rpc_server(&self, methods: RpcModule<()>) -> anyhow::Result<SocketAddr> {
        let server = jsonrpsee::server::ServerBuilder::default()
            .build([self.listen_address_rpc].as_ref())
            .await?;
        let rpc_address = server.local_addr()?;

        let _handle = tokio::spawn(async move {
            info!(%rpc_address, "Starting RPC server");
            let _server_handle = server.start(methods);

            futures::future::pending::<()>().await;
        });

        Ok(rpc_address)
    }

    /// Starts an Axum server with provided router.
    pub async fn start_axum_server(&self, router: axum::Router<()>) -> anyhow::Result<SocketAddr> {
        let listener = tokio::net::TcpListener::bind(self.listen_address_axum).await?;
        let rest_address = listener.local_addr()?;

        tokio::spawn(async move {
            info!(%rest_address, "Starting REST API server");
            axum::serve(listener, router).await.unwrap();
        });

        Ok(rest_address)
    }

    /// Spawn a [`tokio::task`] that updates the sync status every `polling_interval`.
    pub fn spawn_sync_status_updater(&self, polling_interval: Duration) {
        let sync_state = self.sync_state.clone();
        let da_service = self.da_service.clone();

        tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(polling_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await; // Tick the interval once because it starts at 0ms. <https://docs.rs/tokio/latest/src/tokio/time/interval.rs.html#427>

            loop {
                if let Err(error) = sync_state.update_target(da_service.as_ref()).await {
                    error!(
                        ?error,
                        "Failed to update the sync status; will retry in ~{}ms",
                        polling_interval.as_millis()
                    );
                }

                if let SyncStatus::Syncing {
                    synced_da_height,
                    target_da_height,
                } = sync_state.status()
                {
                    info!(synced_da_height, target_da_height, "Sync in progress");
                }
                interval.tick().await;
            }
        });
    }

    /// Runs the rollup.
    pub async fn run_in_process(&mut self) -> anyhow::Result<()> {
        let mut seen_state_transition = VecDeque::new();
        let mut next_da_height = self.first_unprocessed_height_at_startup;
        let target_height = self.da_service.get_head_block_header().await?.height();
        self.sync_state
            .target_da_height
            .store(target_height, std::sync::atomic::Ordering::Release);

        self.spawn_sync_status_updater(Duration::from_millis(self.da_polling_interval_ms));

        loop {
            debug!(next_da_height, "Requesting DA block");
            sov_metrics::update_metrics(|metrics| {
                metrics.current_da_height.set(next_da_height as i64);
            });

            let mut transaction_count = 0;
            let mut batch_count = 0;
            let mut filtered_block = self.da_service.get_block_at(next_da_height).await?;
            debug!(header = %filtered_block.header().display(), "fetched block header");

            // ----------------  Checking if reorg happened or not.
            let reorg_happened = if let Some(ForkPoint {
                height: new_height,
                block: new_block,
                pre_state_root,
            }) = has_reorg_happened::<
                Stf,
                Da,
                <InnerVm::Guest as ZkvmGuest>::Verifier,
                <OuterVm::Guest as ZkvmGuest>::Verifier,
            >(
                &filtered_block,
                &mut seen_state_transition,
                &self.da_service,
            )
            .await?
            {
                next_da_height = new_height;
                filtered_block = new_block;
                self.state_root = pre_state_root;
                // TODO: Prune stale entries from ledger here <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/746>!
                info!(
                    next_da_height,
                    header = %filtered_block.header().display(),
                    "Resuming execution at fork point's height"
                );
                true
            } else {
                false
            };

            // Prepare storage
            let filtered_block_header = filtered_block.header().clone();
            let (stf_pre_state, ledger_state) = self
                .storage_manager
                .create_state_for(&filtered_block_header)?;
            if reorg_happened {
                // In case if reorg happened, we want to keep ledger and RPC storages in sync.
                // Otherwise, the RPC storage and LedgerDb have been updated in [`StfRunner::update_rpc_and_ledger_storage`]
                self.rpc_storage_sender.send_replace(stf_pre_state.clone());
                self.ledger_db.replace_db(ledger_state)?;
            }

            // STF execution
            let mut relevant_blobs = self.da_service.extract_relevant_blobs(&filtered_block);
            let batch_blobs = &mut relevant_blobs.batch_blobs;
            info!(
                batch_blobs_count = batch_blobs.len(),
                next_da_height,
                batch_blobs = ?batch_blobs
                    .iter()
                    .map(|b| format!(
                        "sequencer={} blob_hash=0x{}",
                        b.sender(),
                        hex::encode(b.hash())
                    ))
                    .collect::<Vec<_>>(),
                "Extracted relevant blobs"
            );

            let slot_result = self.stf.apply_slot(
                &self.state_root,
                stf_pre_state,
                Default::default(),
                &filtered_block_header,
                &filtered_block.validity_condition(),
                relevant_blobs.as_iters(),
            );
            let next_state_root = slot_result.state_root.clone();

            // Getting relevant proofs
            let relevant_proofs = self
                .da_service
                .get_extraction_proof(&filtered_block, &relevant_blobs)
                .await;

            // Handling executed data
            let mut data_to_commit = SlotCommit::new(filtered_block);
            for receipt in slot_result.batch_receipts {
                batch_count += 1;
                transaction_count += receipt.tx_receipts.len();
                data_to_commit.add_batch(receipt);
            }
            let transition_data: StateTransitionWitness<Stf::StateRoot, Stf::Witness, Da::Spec> =
                StateTransitionWitness {
                    initial_state_root: self.state_root.clone(),
                    final_state_root: slot_result.state_root.clone(),
                    da_block_header: filtered_block_header.clone(),
                    relevant_proofs,
                    relevant_blobs,
                    witness: slot_result.witness,
                };

            // Processing finalized header
            let last_finalized = self.da_service.get_last_finalized_block_header().await?;
            debug!(header = %last_finalized.display(), "Got last finalized header");
            let last_finalized_height = last_finalized.height();
            let slot_number = self.ledger_db.get_next_items_numbers()?.slot_number;
            debug!(slot_number, "Last slot number");
            seen_state_transition.push_back(StateTransitionInfo {
                data: transition_data,
                slot_number,
            });

            let (finalization_ledger_changes, finalized_headers) = self
                .process_finalized_state_transitions(
                    &mut seen_state_transition,
                    last_finalized_height,
                )
                .await?;

            // Handling change sets and storage updates.
            let mut ledger_change_set = self
                .ledger_db
                .materialize_slot(data_to_commit, slot_result.state_root.as_ref())?;

            let zk_proofs_from_stf = slot_result
                .proof_receipts
                .into_iter()
                .map(|proof_receipt| proof_receipt.raw_proof);

            let proof_change_set = self
                .proof_manager
                .materialize_aggregated_proofs(zk_proofs_from_stf)
                .await?;

            ledger_change_set.merge(proof_change_set);
            ledger_change_set.merge(finalization_ledger_changes);

            self.storage_manager.save_change_set(
                &filtered_block_header,
                slot_result.change_set,
                ledger_change_set,
            )?;
            self.update_rpc_and_ledger_storage(&filtered_block_header)?;
            for finalized_header in finalized_headers {
                self.storage_manager.finalize(&finalized_header)?;
            }
            // RPC storage and Ledger have all data from this iteration,
            // now it is safe to submit notifications.
            self.ledger_db.send_notifications();

            // Updating counters and metrics
            self.sync_state
                .synced_da_height
                .store(next_da_height, std::sync::atomic::Ordering::Release);
            next_da_height += 1;
            self.state_root = next_state_root;

            sov_metrics::update_metrics(|metrics| {
                metrics.rollup_batches_processed.inc_by(batch_count);
                metrics.rollup_txns_processed.inc_by(transaction_count as _);
            });
        }
    }

    // Processes `seen_state_transitions`, removes finalized and posts aggregates DA proofs.
    async fn process_finalized_state_transitions(
        &mut self,
        seen_state_transition: &mut VecDeque<
            StateTransitionInfo<Stf::StateRoot, Stf::Witness, Da::Spec>,
        >,
        last_finalized_height: u64,
    ) -> anyhow::Result<(SchemaBatch, Vec<<Da::Spec as DaSpec>::BlockHeader>)> {
        let mut ledger_change_set = SchemaBatch::new();
        let mut finalized_headers = Vec::new();
        // Checking all seen blocks, in case if there was delay in getting last finalized header.
        while let Some(earliest_seen_state_transition_info) = seen_state_transition.front() {
            let earliest_header = earliest_seen_state_transition_info.da_block_header();
            debug!(header = %earliest_header.display(), last_finalized_height, "Checking seen header");
            let height = earliest_header.height();

            if height <= last_finalized_height {
                ledger_change_set = self.ledger_db.materialize_latest_finalize_slot(
                    earliest_seen_state_transition_info.slot_number,
                )?;

                let transition_data = seen_state_transition.pop_front().unwrap();

                finalized_headers.push(transition_data.da_block_header().clone());

                // Post ZK proof to DA.
                self.proof_manager
                    .post_aggregated_proof_to_da_when_ready(transition_data)
                    .await?;
                continue;
            }

            break;
        }
        Ok((ledger_change_set, finalized_headers))
    }

    fn update_rpc_and_ledger_storage(
        &mut self,
        filtered_block_header: &<<Da as DaService>::Spec as DaSpec>::BlockHeader,
    ) -> Result<(), anyhow::Error> {
        let (new_rpc_storage, ledger_state) = self
            .storage_manager
            .create_state_after(filtered_block_header)?;

        // `send_replace` is superior to `send` for our use case. It never fails
        // because it doesn't need to notify all receivers, unlike `send`, which
        // we don't need. It will also keep working even if there are no
        // receivers currently alive, which makes it easier to reason about the
        // code.
        self.rpc_storage_sender.send_replace(new_rpc_storage);
        self.ledger_db.replace_db(ledger_state)?;
        Ok(())
    }

    /// Allows to read current state root
    pub fn get_state_root(&self) -> &Stf::StateRoot {
        &self.state_root
    }
}

struct ForkPoint<Da: DaService, StateRoot> {
    // Height when reorg happened
    height: u64,
    // new block at [Self::height]`
    block: Da::FilteredBlock,
    // State root of the rollup at the beginning of this block
    pre_state_root: StateRoot,
}

// Returns None if no reorg happened, otherwise returns block at which reorg happened
// Errors if reorg happened, but it cannot backtrack to the seen block from the current chain.
// This can indicate that rollup started from non-finalized block.
// Also can error if da_service returns error.
async fn has_reorg_happened<Stf, Da, InnerVm, OuterVm>(
    filtered_block: &Da::FilteredBlock,
    seen_state_transition: &mut VecDeque<
        StateTransitionInfo<Stf::StateRoot, Stf::Witness, Da::Spec>,
    >,
    da_service: &Da,
) -> anyhow::Result<Option<ForkPoint<Da, Stf::StateRoot>>>
where
    Da: DaService<Error = anyhow::Error> + Clone,
    InnerVm: Zkvm,
    OuterVm: Zkvm,
    Stf: StateTransitionFunction<InnerVm, OuterVm, Da::Spec>,
{
    if let Some(state_transition) = seen_state_transition.back() {
        if state_transition.da_block_header().hash() != filtered_block.header().prev_hash() {
            tracing::warn!(
                current_header = %filtered_block.header().display(),
                prev_seen_header = %state_transition.da_block_header().display(),
                "Block does not belong in current chain. Chain has forked. Traversing seen headers backwards"
            );
            while let Some(state_transition) = seen_state_transition.pop_back() {
                let block = da_service
                    .get_block_at(state_transition.da_block_header().height())
                    .await?;
                debug!(
                    fetched = %block.header().display(),
                    seen = %state_transition.da_block_header().display(),
                    "Checking seen header vs fetched from DA"
                );
                if block.header().prev_hash() == state_transition.da_block_header().prev_hash() {
                    return Ok(Some(ForkPoint {
                        height: state_transition.da_block_header().height(),
                        block,
                        pre_state_root: state_transition.initial_state_root().clone(),
                    }));
                }
            }
            //
            anyhow::bail!("Could not match any seen block with the current chain. Could rollup start from non-finalized block?");
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use sov_mock_da::{
        MockAddress, MockBlob, MockBlock, MockBlockHeader, MockDaService, MockDaSpec,
        MockValidityCond,
    };
    use sov_mock_zkvm::{MockZkVerifier, MockZkvm};
    use sov_rollup_interface::da::{DaProof, RelevantBlobs, RelevantProofs};
    use sov_rollup_interface::services::da::DaServiceWithRetries;

    use super::*;
    use crate::mock::MockStf;

    type Da = DaServiceWithRetries<MockDaService>;
    type Vm = MockZkvm;
    type Stf = MockStf<MockValidityCond>;
    type StateRoot = <MockStf<MockValidityCond> as StateTransitionFunction<
        <<Vm as ZkvmHost>::Guest as ZkvmGuest>::Verifier,
        <<Vm as ZkvmHost>::Guest as ZkvmGuest>::Verifier,
        MockDaSpec,
    >>::StateRoot;
    type StfWitness = <MockStf<MockValidityCond> as StateTransitionFunction<
        <<Vm as ZkvmHost>::Guest as ZkvmGuest>::Verifier,
        <<Vm as ZkvmHost>::Guest as ZkvmGuest>::Verifier,
        MockDaSpec,
    >>::Witness;

    #[tokio::test]
    async fn test_reorg_happened_empty_seen() {
        let mut seen_state_transition_info: VecDeque<
            StateTransitionInfo<StateRoot, StfWitness, MockDaSpec>,
        > = VecDeque::new();
        let filtered_block = MockBlock::default();
        let da_service =
            DaServiceWithRetries::new_fast(MockDaService::new(MockAddress::new([0; 32])));
        let result = has_reorg_happened::<Stf, Da, MockZkVerifier, MockZkVerifier>(
            &filtered_block,
            &mut seen_state_transition_info,
            &da_service,
        )
        .await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_reorg_happened_correct_block_returned() {
        let sequencer_address = MockAddress::new([0; 32]);
        let da_service =
            DaServiceWithRetries::new_fast(MockDaService::new(sequencer_address).with_finality(5));
        // seen blocks are 1, 2, 3, 4, 5
        let mut seen_state_transition_info: VecDeque<
            StateTransitionInfo<StateRoot, StfWitness, MockDaSpec>,
        > = VecDeque::new();

        let header_from_height = |height| -> MockBlockHeader {
            let mut header = MockBlockHeader::from_height(height);
            // Just magic number to prevent collision
            header.hash.0[0] = 255;
            header
        };

        let fork_point = 3;
        let last_block = 5;
        // Filling the seen data and da service
        for height in 1..=last_block {
            let raw_blob: Vec<u8> = vec![height as u8; 32];
            let fee = da_service.estimate_fee(raw_blob.len()).await.unwrap();
            da_service.send_transaction(&raw_blob, fee).await.unwrap();
            if height < fork_point {
                // Just take a block from the service
                let block = da_service.get_block_at(height).await.unwrap();
                seen_state_transition_info.push_back(make_transition_info(
                    block.header.clone(),
                    block.batch_blobs,
                    height,
                ));
            } else {
                let prev_header = if height == fork_point {
                    let block = da_service.get_block_at(height - 1).await.unwrap();
                    block.header
                } else {
                    header_from_height(height - 1)
                };
                // Double it from "canonical" chain
                let raw_blob = vec![height as u8; 64];
                let blob = MockBlob::new_with_hash(raw_blob, sequencer_address);
                let mut header = header_from_height(height);
                header.prev_hash = prev_header.hash;

                seen_state_transition_info.push_back(make_transition_info(
                    header,
                    vec![blob],
                    height,
                ));
            }
        }

        let block_head = da_service.get_block_at(last_block).await.unwrap();
        let result = has_reorg_happened::<Stf, Da, MockZkVerifier, MockZkVerifier>(
            &block_head,
            &mut seen_state_transition_info,
            &da_service,
        )
        .await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_some());
        let actual_fork_point = result.unwrap();
        let block_at_fork_point = da_service.get_block_at(fork_point).await.unwrap();
        let expected_fork_point = ForkPoint::<Da, StateRoot> {
            height: fork_point,
            block: block_at_fork_point,
            pre_state_root: vec![0, 0, fork_point as u8],
        };

        assert_eq!(expected_fork_point.height, actual_fork_point.height);
        assert_eq!(
            expected_fork_point.pre_state_root,
            actual_fork_point.pre_state_root
        );
        assert_eq!(expected_fork_point.block, actual_fork_point.block);
    }

    #[tokio::test]
    async fn test_no_seen_block_has_been_tracked() {
        // Idea of the test is data in "seen blocks" is completely different from the data in the da service
        // This means, that caller started from non-finalized block, and reorg happened while runner was stopped
        let sequencer_address = MockAddress::new([0; 32]);
        let da_service =
            DaServiceWithRetries::new_fast(MockDaService::new(sequencer_address).with_finality(5));
        // seen blocks are 1, 2, 3, 4, 5
        let mut seen_state_transition_info: VecDeque<
            StateTransitionInfo<StateRoot, StfWitness, MockDaSpec>,
        > = VecDeque::new();

        let header_from_height = |height| -> MockBlockHeader {
            let mut header = MockBlockHeader::from_height(height);
            // Just magic number to prevent collision
            header.hash.0[0] = 255;
            header
        };

        let last_block = 5;
        // Filling the seen data and da service
        for height in 1..=last_block {
            let raw_blob: Vec<u8> = vec![height as u8; 32];
            let fee = da_service.estimate_fee(raw_blob.len()).await.unwrap();
            da_service.send_transaction(&raw_blob, fee).await.unwrap();

            let prev_header = header_from_height(height - 1);
            // Double it from "canonical" chain
            let raw_blob = vec![height as u8; 64];
            let blob = MockBlob::new_with_hash(raw_blob, sequencer_address);
            let mut header = header_from_height(height);
            header.prev_hash = prev_header.hash;
            seen_state_transition_info.push_back(make_transition_info(header, vec![blob], height));
        }

        let block_head = da_service.get_block_at(last_block).await.unwrap();
        let result = has_reorg_happened::<Stf, Da, MockZkVerifier, MockZkVerifier>(
            &block_head,
            &mut seen_state_transition_info,
            &da_service,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            "Could not match any seen block with the current chain. Could rollup start from non-finalized block?",
            result.err().unwrap().to_string()
        );
    }

    fn make_transition_info(
        header: MockBlockHeader,
        blobs: Vec<MockBlob>,
        height: u64,
    ) -> StateTransitionInfo<Vec<u8>, (), MockDaSpec> {
        // first byte means "fork id", second byte means initial or final
        let initial_state_root = vec![0, 0, height as u8];
        let final_state_root = vec![0, 1, height as u8];

        StateTransitionInfo {
            data: StateTransitionWitness {
                initial_state_root,
                final_state_root,
                da_block_header: header,
                relevant_proofs: RelevantProofs {
                    batch: DaProof {
                        inclusion_proof: Default::default(),
                        completeness_proof: Default::default(),
                    },
                    proof: DaProof {
                        inclusion_proof: Default::default(),
                        completeness_proof: Default::default(),
                    },
                },
                relevant_blobs: RelevantBlobs {
                    proof_blobs: vec![],
                    batch_blobs: blobs,
                },

                witness: Default::default(),
            },
            slot_number: height,
        }
    }
}
