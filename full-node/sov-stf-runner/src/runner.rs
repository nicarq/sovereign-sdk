use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use jsonrpsee::RpcModule;
use sov_db::ledger_db::{LedgerDB, SlotCommit};
use sov_db::schema::{CacheDb, ChangeSet};
use sov_rollup_interface::da::{BlobReaderTrait, BlockHeaderTrait, DaSpec};
use sov_rollup_interface::services::da::{DaService, SlotData};
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::{StateTransitionData, Zkvm, ZkvmHost};
use tokio::sync::oneshot;
use tracing::{debug, info};

use crate::{ProofAggregationStatus, ProverService, RunnerConfig};

type StateRoot<ST, Vm, Da> = <ST as StateTransitionFunction<Vm, Da>>::StateRoot;
type GenesisParams<ST, Vm, Da> = <ST as StateTransitionFunction<Vm, Da>>::GenesisParams;

/// Combines `DaService` with `StateTransitionFunction` and "runs" the rollup.
pub struct StateTransitionRunner<Stf, Sm, Da, Vm, Ps>
where
    Da: DaService,
    Vm: ZkvmHost,
    Sm: HierarchicalStorageManager<Da::Spec>,
    Stf: StateTransitionFunction<Vm, Da::Spec, Condition = <Da::Spec as DaSpec>::ValidityCondition>,
    Ps: ProverService,
{
    start_height: u64,
    da_service: Da,
    stf: Stf,
    storage_manager: Sm,
    rpc_storage: Arc<RwLock<Sm::StfState>>,
    ledger_db: LedgerDB,
    state_root: StateRoot<Stf, Vm, Da::Spec>,
    listen_address: SocketAddr,
    prover_service: Ps,
}

/// How [`StateTransitionRunner`] is initialized
pub enum InitVariant<Stf: StateTransitionFunction<Vm, Da>, Vm: Zkvm, Da: DaSpec> {
    /// From give state root
    Initialized(Stf::StateRoot),
    /// From empty state root
    Genesis {
        /// Genesis block header should be finalized at init moment
        block_header: Da::BlockHeader,
        /// Genesis params for Stf::init
        genesis_params: GenesisParams<Stf, Vm, Da>,
    },
}

impl<Stf, Sm, Da, Vm, Ps> StateTransitionRunner<Stf, Sm, Da, Vm, Ps>
where
    Da: DaService<Error = anyhow::Error> + Clone + Send + Sync + 'static,
    Vm: ZkvmHost,
    Sm: HierarchicalStorageManager<Da::Spec, LedgerChangeSet = ChangeSet, LedgerState = CacheDb>,
    Stf: StateTransitionFunction<
        Vm,
        Da::Spec,
        Condition = <Da::Spec as DaSpec>::ValidityCondition,
        PreState = Sm::StfState,
        ChangeSet = Sm::StfChangeSet,
    >,
    Ps: ProverService<StateRoot = Stf::StateRoot, Witness = Stf::Witness, DaService = Da>,
{
    /// Creates a new `StateTransitionRunner`.
    ///
    /// If a previous state root is provided, uses that as the starting point
    /// for execution. Otherwise, initializes the chain using the provided
    /// genesis config.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runner_config: RunnerConfig,
        da_service: Da,
        mut ledger_db: LedgerDB,
        stf: Stf,
        mut storage_manager: Sm,
        rpc_storage: Arc<RwLock<Sm::StfState>>,
        init_variant: InitVariant<Stf, Vm, Da::Spec>,
        prover_service: Ps,
    ) -> Result<Self, anyhow::Error> {
        let rpc_config = runner_config.rpc_config;

        let prev_state_root = match init_variant {
            InitVariant::Initialized(state_root) => {
                debug!("Chain is already initialized. Skipping initialization.");
                state_root
            }
            InitVariant::Genesis {
                block_header,
                genesis_params: params,
            } => {
                info!(
                    "No history detected. Initializing chain on block_header={:?}...",
                    block_header
                );
                let (stf_state, ledger_state) = storage_manager.create_state_for(&block_header)?;
                ledger_db.replace_db(ledger_state)?;
                let (genesis_root, initialized_storage) = stf.init_chain(stf_state, params);
                let ledger_change_set = ledger_db.clone_change_set();
                storage_manager.save_change_set(
                    &block_header,
                    initialized_storage,
                    ledger_change_set,
                )?;
                storage_manager.finalize(&block_header)?;
                info!(
                    "Chain initialization is done. Genesis root: 0x{}",
                    hex::encode(genesis_root.as_ref()),
                );
                genesis_root
            }
        };

        let listen_address = SocketAddr::new(rpc_config.bind_host.parse()?, rpc_config.bind_port);

        // Start the main rollup loop
        let item_numbers = ledger_db.get_next_items_numbers();
        let last_slot_processed_before_shutdown = item_numbers.slot_number - 1;
        let start_height = runner_config.start_height + last_slot_processed_before_shutdown;

        Ok(Self {
            start_height,
            da_service,
            stf,
            storage_manager,
            rpc_storage,
            ledger_db,
            state_root: prev_state_root,
            listen_address,
            prover_service,
        })
    }

    /// Starts a RPC server with provided rpc methods.
    ///  # Arguments:
    ///   * methods: [`RpcModule`] with all RPC methods.
    ///   * channel: If `Some`, notification with actual [`SocketAddr`] where RPC server listens
    pub async fn start_rpc_server(
        &self,
        methods: RpcModule<()>,
        channel: Option<oneshot::Sender<SocketAddr>>,
    ) {
        let listen_address = self.listen_address;
        let _handle = tokio::spawn(async move {
            let server = jsonrpsee::server::ServerBuilder::default()
                .build([listen_address].as_ref())
                .await
                .unwrap();

            let bound_address = server.local_addr().unwrap();
            info!("Starting RPC server at {} ", &bound_address);
            let _server_handle = server.start(methods);

            if let Some(channel) = channel {
                channel.send(bound_address).unwrap();
            }
            futures::future::pending::<()>().await;
        });
    }

    /// Runs the rollup.
    pub async fn run_in_process(&mut self) -> Result<(), anyhow::Error> {
        let mut seen_state_transition_data: VecDeque<
            StateTransitionData<Stf::StateRoot, Stf::Witness, Da::Spec>,
        > = VecDeque::new();
        let mut height = self.start_height;

        let mut agg_block_hashes = Vec::default();
        loop {
            debug!("Requesting DA block for height={}", height);
            let mut filtered_block = self.da_service.get_block_at(height).await?;

            // Checking if reorg happened or not.
            if let Some(ForkPoint {
                height: new_height,
                block: new_block,
                pre_state_root,
            }) = has_reorg_happened::<Stf, Da, Vm>(
                &filtered_block,
                &mut seen_state_transition_data,
                &self.da_service,
            )
            .await?
            {
                height = new_height;
                filtered_block = new_block;
                self.state_root = pre_state_root;
                info!("Resuming execution on height={}", height);
            }
            let mut blobs = self.da_service.extract_relevant_blobs(&filtered_block);

            info!(
                "Extracted {} relevant blobs at height {}: {:?}",
                blobs.len(),
                height,
                blobs
                    .iter()
                    .map(|b| format!(
                        "sequencer={} blob_hash=0x{}",
                        b.sender(),
                        hex::encode(b.hash())
                    ))
                    .collect::<Vec<_>>()
            );

            let mut data_to_commit = SlotCommit::new(filtered_block.clone());

            let (stf_pre_state, ledger_state) = self
                .storage_manager
                .create_state_for(filtered_block.header())?;
            self.ledger_db.replace_db(ledger_state)?;

            let slot_result = self.stf.apply_slot(
                &self.state_root,
                stf_pre_state,
                Default::default(),
                filtered_block.header(),
                &filtered_block.validity_condition(),
                &mut blobs,
            );

            for receipt in slot_result.batch_receipts {
                data_to_commit.add_batch(receipt);
            }

            let (inclusion_proof, completeness_proof) = self
                .da_service
                .get_extraction_proof(&filtered_block, &blobs)
                .await;

            let transition_data: StateTransitionData<Stf::StateRoot, Stf::Witness, Da::Spec> =
                StateTransitionData {
                    // TODO(https://github.com/Sovereign-Labs/sovereign-sdk/issues/1247): incorrect pre-state root in case of re-org
                    initial_state_root: self.state_root.clone(),
                    final_state_root: slot_result.state_root.clone(),
                    da_block_header: filtered_block.header().clone(),
                    inclusion_proof,
                    completeness_proof,
                    blobs,
                    state_transition_witness: slot_result.witness,
                };

            // Post apply slot machinery
            let next_state_root = slot_result.state_root;
            self.state_root = next_state_root;
            seen_state_transition_data.push_back(transition_data);
            height += 1;
            self.ledger_db.commit_slot(data_to_commit)?;

            // Save data back to StorageManager
            let ledger_change_set = self.ledger_db.clone_change_set();
            self.storage_manager.save_change_set(
                filtered_block.header(),
                slot_result.change_set,
                ledger_change_set,
            )?;

            // Update RPC storage
            {
                let (new_rpc_storage, _) = self
                    .storage_manager
                    .create_state_after(filtered_block.header())?;
                let mut rpc_storage = self
                    .rpc_storage
                    .write()
                    .expect("RPC Storage RwLock poisoned");
                *rpc_storage = new_rpc_storage;
            }

            // ----------------
            // Finalization. Done after seen block for proper handling of instant finality
            // Can be moved to another thread to improve throughput
            let last_finalized = self.da_service.get_last_finalized_block_header().await?;
            // For safety we finalize blocks one by one
            info!(
                "Last finalized header height is {}, ",
                last_finalized.height()
            );

            // Checking all seen blocks, in case if there was delay in getting last finalized header.
            while let Some(earliest_seen_state_transition_data) = seen_state_transition_data.front()
            {
                debug!(
                    "Checking seen header height={}",
                    earliest_seen_state_transition_data.da_block_header.height()
                );
                if earliest_seen_state_transition_data.da_block_header.height()
                    <= last_finalized.height()
                {
                    debug!(
                        "Finalizing seen header height={}",
                        earliest_seen_state_transition_data.da_block_header.height()
                    );
                    self.storage_manager
                        .finalize(&earliest_seen_state_transition_data.da_block_header)?;

                    let transition_data = seen_state_transition_data.pop_front().unwrap();
                    agg_block_hashes.push(transition_data.da_block_header.hash());

                    // Create ZK proof.
                    self.create_aggregated_proof(transition_data, &mut agg_block_hashes)
                        .await;

                    continue;
                }

                break;
            }
        }
    }

    /// Allows to read current state root
    pub fn get_state_root(&self) -> &Stf::StateRoot {
        &self.state_root
    }

    async fn create_aggregated_proof(
        &self,
        transition_data: StateTransitionData<Stf::StateRoot, Stf::Witness, <Da as DaService>::Spec>,
        agg_block_hashes: &mut Vec<<Da::Spec as DaSpec>::SlotHash>,
    ) {
        let header_hash = transition_data.da_block_header.hash();

        self.prover_service.submit_witness(transition_data).await;
        self.prover_service
            .prove(header_hash.clone())
            .await
            .expect("The proof creation should succeed");

        if agg_block_hashes.len() >= self.prover_service.aggregated_proof_block_jump() {
            loop {
                let status = self
                    .prover_service
                    .create_aggregated_proof(agg_block_hashes.as_slice())
                    .await;

                match status {
                    Ok(ProofAggregationStatus::Success(_)) => {
                        agg_block_hashes.clear();
                        break;
                    }
                    // TODO(https://github.com/Sovereign-Labs/sovereign-sdk/issues/1185): Add timeout handling.
                    Ok(ProofAggregationStatus::ProofGenerationInProgress) => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                    // TODO(https://github.com/Sovereign-Labs/sovereign-sdk/issues/1185): Add handling for DA submission errors.
                    Err(e) => panic!("{:?}", e),
                }
            }
        }
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
// This cab indicate that rollup started from non-finalized block.
// Also can error if da_service returns error.
async fn has_reorg_happened<Stf, Da, Vm>(
    filtered_block: &Da::FilteredBlock,
    seen_state_transition_data: &mut VecDeque<
        StateTransitionData<Stf::StateRoot, Stf::Witness, Da::Spec>,
    >,
    da_service: &Da,
) -> anyhow::Result<Option<ForkPoint<Da, Stf::StateRoot>>>
where
    Da: DaService<Error = anyhow::Error> + Clone + Send + Sync + 'static,
    Vm: Zkvm,
    Stf: StateTransitionFunction<Vm, Da::Spec>,
{
    if let Some(prev_block_header) = seen_state_transition_data.back() {
        if prev_block_header.da_block_header.hash() != filtered_block.header().prev_hash() {
            tracing::warn!(
                "Block {:?} does not belong in current chain. Chain has forked. Traversing seen headers backwards", filtered_block.header()
            );
            while let Some(seen_transition_data) = seen_state_transition_data.pop_back() {
                let block = da_service
                    .get_block_at(seen_transition_data.da_block_header.height())
                    .await?;
                if block.header().prev_hash() == seen_transition_data.da_block_header.prev_hash() {
                    return Ok(Some(ForkPoint {
                        height: seen_transition_data.da_block_header.height(),
                        block,
                        pre_state_root: seen_transition_data.initial_state_root,
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
    use std::default::Default;

    use sov_mock_da::{
        MockAddress, MockBlob, MockBlock, MockBlockHeader, MockDaService, MockDaSpec,
        MockValidityCond,
    };
    use sov_mock_zkvm::MockZkvm;

    use super::*;
    use crate::mock::MockStf;

    type Da = MockDaService;
    type Vm = MockZkvm<MockValidityCond>;
    type Stf = MockStf<MockValidityCond>;
    type StateRoot =
        <MockStf<MockValidityCond> as StateTransitionFunction<Vm, MockDaSpec>>::StateRoot;
    type StfWitness =
        <MockStf<MockValidityCond> as StateTransitionFunction<Vm, MockDaSpec>>::Witness;

    #[tokio::test]
    async fn test_reorg_happened_empty_seen() {
        let mut seen_state_transition_data: VecDeque<
            StateTransitionData<StateRoot, StfWitness, MockDaSpec>,
        > = VecDeque::new();
        let filtered_block = MockBlock::default();
        let da_service = MockDaService::new(MockAddress::new([0; 32]));
        let result = has_reorg_happened::<Stf, Da, Vm>(
            &filtered_block,
            &mut seen_state_transition_data,
            &da_service,
        )
        .await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_reorg_happened_correct_block_returned() {
        let sequencer_address = MockAddress::new([0; 32]);
        let da_service = MockDaService::new(sequencer_address).with_finality(5);
        // seen blocks are 1, 2, 3, 4, 5
        let mut seen_state_transition_data: VecDeque<
            StateTransitionData<StateRoot, StfWitness, MockDaSpec>,
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
            // first byte means "fork id", second byte means initial or final
            let initial_state_root = vec![0, 0, height as u8];
            let final_state_root = vec![0, 1, height as u8];

            da_service.send_transaction(&raw_blob).await.unwrap();
            if height < fork_point {
                // Just take block from the service
                let block = da_service.get_block_at(height).await.unwrap();
                seen_state_transition_data.push_back(StateTransitionData {
                    initial_state_root,
                    final_state_root,
                    da_block_header: block.header.clone(),
                    inclusion_proof: Default::default(),
                    completeness_proof: Default::default(),
                    blobs: block.blobs,
                    state_transition_witness: Default::default(),
                });
            } else {
                let prev_header = if height == fork_point {
                    let block = da_service.get_block_at(height - 1).await.unwrap();
                    block.header
                } else {
                    header_from_height(height - 1)
                };
                // Just double it from "canonical" chain
                let raw_blob = vec![height as u8; 64];
                let blob = MockBlob::new_with_hash(raw_blob, Default::default(), sequencer_address);
                let mut header = header_from_height(height);
                header.prev_hash = prev_header.hash;
                seen_state_transition_data.push_back(StateTransitionData {
                    initial_state_root,
                    final_state_root,
                    da_block_header: header,
                    inclusion_proof: Default::default(),
                    completeness_proof: Default::default(),
                    blobs: vec![blob],
                    state_transition_witness: Default::default(),
                });
            }
        }

        let block_head = da_service.get_block_at(last_block).await.unwrap();
        let result = has_reorg_happened::<Stf, Da, Vm>(
            &block_head,
            &mut seen_state_transition_data,
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
        let da_service = MockDaService::new(sequencer_address).with_finality(5);
        // seen blocks are 1, 2, 3, 4, 5
        let mut seen_state_transition_data: VecDeque<
            StateTransitionData<StateRoot, StfWitness, MockDaSpec>,
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
            // first byte means "fork id", second byte means initial or final
            let initial_state_root = vec![0, 0, height as u8];
            let final_state_root = vec![0, 1, height as u8];

            da_service.send_transaction(&raw_blob).await.unwrap();

            let prev_header = header_from_height(height - 1);
            // Just double it from "canonical" chain
            let raw_blob = vec![height as u8; 64];
            let blob = MockBlob::new_with_hash(raw_blob, Default::default(), sequencer_address);
            let mut header = header_from_height(height);
            header.prev_hash = prev_header.hash;
            seen_state_transition_data.push_back(StateTransitionData {
                initial_state_root,
                final_state_root,
                da_block_header: header,
                inclusion_proof: Default::default(),
                completeness_proof: Default::default(),
                blobs: vec![blob],
                state_transition_witness: Default::default(),
            });
        }

        let block_head = da_service.get_block_at(last_block).await.unwrap();
        let result = has_reorg_happened::<Stf, Da, Vm>(
            &block_head,
            &mut seen_state_transition_data,
            &da_service,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            "Could not match any seen block with the current chain. Could rollup start from non-finalized block?",
            result.err().unwrap().to_string()
        );
    }
}
