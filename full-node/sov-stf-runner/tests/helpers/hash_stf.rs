use sha2::Digest;
use sov_mock_da::{
    MockAddress, MockBlob, MockBlock, MockBlockHeader, MockDaSpec, MockValidityCond,
};
use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::namespaces::User;
use sov_prover_storage_manager::SimpleStorageManager;
use sov_rollup_interface::da::{BlobReaderTrait, BlockHeaderTrait, DaSpec};
use sov_rollup_interface::stf::{ApplySlotOutput, SlotResult, StateTransitionFunction};
use sov_rollup_interface::zk::{ValidityCondition, Zkvm};
use sov_state::storage::{NativeStorage, SlotKey, SlotValue, StateAccesses};
use sov_state::{
    ArrayWitness, DefaultStorageSpec, OrderedReadsAndWrites, Prefix, ProverChangeSet,
    ProverStorage, Storage,
};

pub type S = DefaultStorageSpec;

#[derive(Default, Clone)]
pub struct HashStf<Cond> {
    phantom_data: std::marker::PhantomData<Cond>,
}

impl<Cond> HashStf<Cond> {
    pub fn new() -> Self {
        Self {
            phantom_data: std::marker::PhantomData,
        }
    }

    fn hash_key() -> SlotKey {
        let prefix = Prefix::new(b"root".to_vec());
        SlotKey::singleton(&prefix)
    }

    fn save_from_hasher(
        hasher: sha2::Sha256,
        storage: ProverStorage<S>,
        witness: &ArrayWitness,
    ) -> ([u8; 32], ProverChangeSet) {
        let result = hasher.finalize();

        let hash_key = HashStf::<Cond>::hash_key();
        let hash_value = SlotValue::from(result.as_slice().to_vec());

        let ordered_reads_writes = OrderedReadsAndWrites {
            ordered_reads: Vec::default(),
            ordered_writes: vec![(hash_key, Some(hash_value))],
        };
        let state_accesses = StateAccesses {
            user: ordered_reads_writes,
            kernel: Default::default(),
        };

        let (jmt_root_hash, state_update) = storage
            .compute_state_update(state_accesses, witness)
            .unwrap();

        storage.commit(&state_update);

        (jmt_root_hash.into(), storage.to_change_set())
    }
}

impl<Vm: Zkvm, Cond: ValidityCondition, Da: DaSpec> StateTransitionFunction<Vm, Da>
    for HashStf<Cond>
{
    type StateRoot = [u8; 32];
    type GenesisParams = Vec<u8>;
    type PreState = ProverStorage<S>;
    type ChangeSet = ProverChangeSet;
    type TxReceiptContents = ();
    type BatchReceiptContents = [u8; 32];
    type Witness = ArrayWitness;
    type Condition = Cond;

    fn init_chain(
        &self,
        genesis_state: Self::PreState,
        params: Self::GenesisParams,
    ) -> (Self::StateRoot, Self::ChangeSet) {
        let mut hasher = sha2::Sha256::new();
        hasher.update(params);

        HashStf::<Cond>::save_from_hasher(hasher, genesis_state, &ArrayWitness::default())
    }

    #[tracing::instrument(name = "HashStf::apply_slot", skip_all)]
    fn apply_slot<'a, I>(
        &self,
        pre_state_root: &Self::StateRoot,
        pre_state: Self::PreState,
        witness: Self::Witness,
        slot_header: &Da::BlockHeader,
        _validity_condition: &Da::ValidityCondition,
        blobs: I,
    ) -> ApplySlotOutput<Vm, Da, Self>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        tracing::debug!("Getting the root hash...");

        let storage_root_hash = pre_state.get_root_hash(slot_header.height()).unwrap();
        assert_eq!(
            pre_state_root,
            &storage_root_hash.root_hash().0,
            "Incorrect pre_state_root has been passed"
        );

        let mut hasher = sha2::Sha256::new();

        let hash_key = HashStf::<Cond>::hash_key();
        let existing_cache = pre_state.get::<User>(&hash_key, None, &witness).unwrap();
        tracing::trace!(
            pre_state_root = hex::encode(pre_state_root),
            existing_cache = hex::encode(existing_cache.value()),
            "Fetched existing cache value from pre_state"
        );
        hasher.update(existing_cache.value());

        for blob in blobs {
            let data = blob.verified_data();
            hasher.update(data);
        }

        let (state_root, storage) = HashStf::<Cond>::save_from_hasher(hasher, pre_state, &witness);
        SlotResult {
            state_root,
            change_set: storage,
            // TODO: Add batch receipts to inspection
            batch_receipts: vec![],
            witness,
        }
    }
}

#[test]
fn compare_output() {
    let genesis_params: Vec<u8> = vec![1, 2, 3, 4, 5];

    let raw_blobs: Vec<Vec<Vec<u8>>> = vec![
        // Block A
        vec![vec![1, 1, 1], vec![2, 2, 2]],
        // Block B
        vec![vec![3, 3, 3], vec![4, 4, 4], vec![5, 5, 5]],
        // Block C
        vec![vec![6, 6, 6]],
        // Block D
        vec![vec![7, 7, 7], vec![8, 8, 8]],
    ];

    let mut blocks = Vec::new();

    for (idx, raw_block) in raw_blobs.iter().enumerate() {
        let mut blobs = Vec::new();
        for raw_blob in raw_block.iter() {
            let blob = MockBlob::new(
                raw_blob.clone(),
                MockAddress::new([11u8; 32]),
                [idx as u8; 32],
            );
            blobs.push(blob);
        }

        let block = MockBlock {
            header: MockBlockHeader::from_height(idx as u64 + 1),
            validity_cond: MockValidityCond::default(),
            blobs,
        };
        blocks.push(block);
    }

    let (state_root, root_hash) = get_result_from_blocks(&genesis_params, &blocks);

    assert!(root_hash.is_some());

    let recorded_state_root: [u8; 32] = [
        187, 159, 56, 140, 156, 64, 1, 22, 185, 241, 95, 11, 247, 87, 147, 191, 133, 14, 225, 235,
        94, 135, 23, 253, 145, 74, 94, 23, 128, 233, 18, 32,
    ];

    assert_eq!(recorded_state_root, state_root);
}

// Returns final data hash and root hash
pub fn get_result_from_blocks(
    genesis_params: &[u8],
    blocks: &[MockBlock],
) -> ([u8; 32], Option<<ProverStorage<S> as Storage>::Root>) {
    let tmpdir = tempfile::tempdir().unwrap();

    let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
    let storage = storage_manager.create_storage();

    let stf = HashStf::<MockValidityCond>::new();

    let (genesis_state_root, change_set) = <HashStf<MockValidityCond> as StateTransitionFunction<
        MockZkVerifier,
        MockDaSpec,
    >>::init_chain(
        &stf, storage, genesis_params.to_vec()
    );
    storage_manager.commit(change_set);

    let mut state_root = genesis_state_root;

    let l = blocks.len();

    for block in blocks {
        let mut blobs = block.blobs.clone();

        let storage = storage_manager.create_storage();
        let result = <HashStf<MockValidityCond> as StateTransitionFunction<
            MockZkVerifier,
            MockDaSpec,
        >>::apply_slot::<&mut Vec<MockBlob>>(
            &stf,
            &state_root,
            storage,
            ArrayWitness::default(),
            &block.header,
            &block.validity_cond,
            &mut blobs,
        );

        state_root = result.state_root;
        storage_manager.commit(result.change_set);
    }

    let storage = storage_manager.create_storage();
    let root_hash = storage.get_root_hash(l as u64).ok();
    (state_root, root_hash)
}
