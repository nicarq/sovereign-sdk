use std::marker::PhantomData;

use jmt::storage::NodeBatch;
use jmt::{JellyfishMerkleTree, KeyHash, Version};
use sov_db::accessory_db::AccessoryDb;
use sov_db::namespaces;
use sov_db::namespaces::{KernelNamespace as DBKernelNamespace, UserNamespace as DBUserNamespace};
use sov_db::state_db::{JmtHandler, StateDb};
use sov_db::storage_manager::{InitializableNativeStorage, NativeChangeSet, StfStorageHandlers};

use crate::cache::{OrderedReadsAndWrites, StateAccesses};
use crate::config::Config;
use crate::namespaces::{
    Accessory, CompileTimeNamespace, Namespace, ProvableCompileTimeNamespace, ProvableNamespace,
};
use crate::storage::{NativeStorage, SlotKey, SlotValue, StateUpdate, Storage, StorageProof};
use crate::storage_internals::{SparseMerkleProof, StorageRoot};
use crate::{MerkleProofSpec, StateRoot, Witness};

/// A [`Storage`] implementation to be used by the prover in a native execution
/// environment (outside of the zkVM).
#[derive(derivative::Derivative)]
#[derivative(
    Clone(bound = "S: MerkleProofSpec"),
    Debug(bound = "S: MerkleProofSpec")
)]
pub struct ProverStorage<S: MerkleProofSpec> {
    db: StateDb,
    accessory_db: AccessoryDb,
    _phantom_hasher: PhantomData<S::Hasher>,
}

impl<S: MerkleProofSpec> From<StfStorageHandlers> for ProverStorage<S> {
    fn from(value: StfStorageHandlers) -> Self {
        ProverStorage::with_db_handles(value.state, value.accessory)
    }
}

impl<S: MerkleProofSpec> InitializableNativeStorage for ProverStorage<S> {
    fn new(db: StateDb, accessory_db: AccessoryDb) -> Self {
        Self {
            db,
            accessory_db,
            _phantom_hasher: Default::default(),
        }
    }
}

impl<S: MerkleProofSpec> ProverStorage<S> {
    /// Creates a new [`ProverStorage`] instance from specified db handles
    pub fn with_db_handles(db: StateDb, accessory_db: AccessoryDb) -> Self {
        Self {
            db,
            accessory_db,
            _phantom_hasher: Default::default(),
        }
    }

    fn read_value_namespace<N: namespaces::Namespace>(
        &self,
        key: &SlotKey,
        version: Version,
    ) -> Option<SlotValue> {
        match self.db.get_value_option_by_key::<N>(version, key.as_ref()) {
            Ok(value) => value.map(Into::into),
            // It is ok to panic here, we assume the db is available and consistent.
            Err(e) => panic!("Unable to read value from db: {e}"),
        }
    }

    fn read_value<N: CompileTimeNamespace>(
        &self,
        key: &SlotKey,
        version: Option<Version>,
    ) -> Option<SlotValue> {
        if self.is_empty() {
            return None;
        }
        let version_to_use = version.unwrap_or_else(|| {
            self.db
                .get_next_version()
                .checked_sub(1)
                .expect("If storage is not `is_empty` it should have next version to be above 0")
        });

        match N::NAMESPACE {
            Namespace::User => self.read_value_namespace::<DBUserNamespace>(key, version_to_use),
            Namespace::Kernel => {
                self.read_value_namespace::<DBKernelNamespace>(key, version_to_use)
            }
            Namespace::Accessory => self
                .accessory_db
                .get_value_option(key.as_ref(), version_to_use)
                .expect("Unable to read from AccessoryDb")
                .map(Into::into),
        }
    }

    fn get_root_hash_namespace_helper<N: namespaces::Namespace>(
        &self,
        version: Version,
    ) -> anyhow::Result<jmt::RootHash> {
        let state_db_handler: JmtHandler<N> = self.db.get_jmt_handler();
        let merkle = JellyfishMerkleTree::<JmtHandler<N>, S::Hasher>::new(&state_db_handler);
        merkle.get_root_hash(version)
    }

    /// Return the root hash for a given namespace and version
    pub fn get_root_hash_namespace(
        &self,
        namespace: ProvableNamespace,
        version: Version,
    ) -> anyhow::Result<jmt::RootHash> {
        match namespace {
            ProvableNamespace::User => {
                self.get_root_hash_namespace_helper::<DBUserNamespace>(version)
            }
            ProvableNamespace::Kernel => {
                self.get_root_hash_namespace_helper::<DBKernelNamespace>(version)
            }
        }
    }

    pub(crate) fn compute_state_update_namespace<N: namespaces::Namespace>(
        &self,
        state_accesses: OrderedReadsAndWrites,
        witness: &<ProverStorage<S> as Storage>::Witness,
    ) -> anyhow::Result<(jmt::RootHash, ProverStateUpdate)> {
        let jmt_handler: JmtHandler<N> = self.db.get_jmt_handler();
        let jmt = JellyfishMerkleTree::<JmtHandler<N>, S::Hasher>::new(&jmt_handler);

        match self.db.get_next_version().checked_sub(1) {
            // If next_version is zero it means genesis
            None => (),
            // Previous root and reads are not witnessed during genesis.
            Some(latest_version) => {
                let root_hash = jmt
                    .get_root_hash(latest_version)
                    .expect("Previous root hash was not populated");
                witness.add_hint(root_hash.0);
                // For each value that's been read from the tree, read it from the logged JMT to populate hints
                for (key, read_value) in &state_accesses.ordered_reads {
                    let key_hash = KeyHash::with::<S::Hasher>(key.key().as_ref());
                    // TODO: Switch to the batch read API once it becomes available
                    let (result, proof) = jmt.get_with_proof(key_hash, latest_version)?;
                    if result != read_value.as_ref().map(|f| f.value().to_vec()) {
                        anyhow::bail!("Bug! Incorrect value read from jmt");
                    }
                    witness.add_hint(proof);
                }
            }
        };

        let mut key_preimages = Vec::with_capacity(state_accesses.ordered_writes.len());

        // Compute the JMT update from the batch of write operations.
        let batch = state_accesses
            .ordered_writes
            .into_iter()
            .map(|(key, value)| {
                let key_hash = KeyHash::with::<S::Hasher>(key.key().as_ref());
                key_preimages.push((key_hash, key.clone()));
                (key_hash, value.as_ref().map(|v| v.value().to_vec()))
            });

        let next_version = self.db.get_next_version();

        let (new_root, update_proof, tree_update) = jmt
            .put_value_set_with_proof(batch, next_version)
            .expect("JMT update must succeed");

        witness.add_hint(update_proof);
        witness.add_hint(new_root.0);

        let new_state_update = ProverStateUpdate {
            node_batch: tree_update.node_batch,
            key_preimages,
        };

        Ok((new_root, new_state_update))
    }

    fn materialize_accessory(
        &self,
        accessory_writes: &OrderedReadsAndWrites,
    ) -> sov_db::schema::SchemaBatch {
        let next_version = self.db.get_next_version();
        AccessoryDb::materialize_values(
            accessory_writes
                .ordered_writes
                .iter()
                .map(|(k, v_opt)| (k.key().to_vec(), v_opt.as_ref().map(|v| v.value().to_vec()))),
            next_version,
        )
        .expect("accessory db write must succeed")
    }

    fn get_with_proof_namespace<N: namespaces::Namespace>(
        &self,
        namespace: ProvableNamespace,
        key: SlotKey,
        version: Option<u64>,
    ) -> StorageProof<<ProverStorage<S> as Storage>::Proof> {
        let version_to_use = version.unwrap_or_else(|| {
            self.db
                .get_next_version()
                .checked_sub(1)
                .expect("If storage is not `is_empty` it should have next version to be above 0")
        });
        let state_db_handler: JmtHandler<N> = self.db.get_jmt_handler();
        let merkle = JellyfishMerkleTree::<JmtHandler<N>, S::Hasher>::new(&state_db_handler);
        // We should've checked all input before this point, so any error means a bug.
        let (val_opt, proof) = merkle
            .get_with_proof(KeyHash::with::<S::Hasher>(key.as_ref()), version_to_use)
            .expect("Corrupted JMT state");
        StorageProof {
            key,
            value: val_opt.map(SlotValue::from),
            proof: SparseMerkleProof::<S::Hasher>::from(proof),
            namespace,
        }
    }

    /// Utility method for checking if storage is empty.
    /// Does not guarantee 100% that it actually is.
    pub fn is_empty(&self) -> bool {
        self.db.get_next_version() == 0
    }
}

#[derive(Default)]
pub struct ProverStateUpdate {
    pub(crate) node_batch: NodeBatch,
    pub(crate) key_preimages: Vec<(KeyHash, SlotKey)>,
}

pub struct NamespacedStateUpdate {
    pub user: ProverStateUpdate,
    pub kernel: ProverStateUpdate,
    pub accessory: OrderedReadsAndWrites,
}

impl StateUpdate for NamespacedStateUpdate {
    fn add_accessory_item(&mut self, key: SlotKey, value: Option<SlotValue>) {
        self.accessory.ordered_writes.push((key, value));
    }
}

impl NamespacedStateUpdate {
    pub fn new(
        user: ProverStateUpdate,
        kernel: ProverStateUpdate,
        accessory: OrderedReadsAndWrites,
    ) -> Self {
        Self {
            user,
            kernel,
            accessory,
        }
    }
}

impl<S: MerkleProofSpec> Storage for ProverStorage<S> {
    type Witness = S::Witness;
    type RuntimeConfig = Config;
    type Proof = SparseMerkleProof<S::Hasher>;
    type Root = StorageRoot<S>;
    type StateUpdate = NamespacedStateUpdate;
    type ChangeSet = NativeChangeSet;

    fn get<N: ProvableCompileTimeNamespace>(
        &self,
        key: &SlotKey,
        version: Option<Version>,
        witness: &Self::Witness,
    ) -> Option<SlotValue> {
        let val = self.read_value::<N>(key, version);
        witness.add_hint(val.clone());
        val
    }

    fn get_accessory(&self, key: &SlotKey, version: Option<Version>) -> Option<SlotValue> {
        self.read_value::<Accessory>(key, version)
    }

    fn compute_state_update(
        &self,
        state_accesses: StateAccesses,
        witness: &Self::Witness,
    ) -> anyhow::Result<(Self::Root, Self::StateUpdate)> {
        let (user_root, user_state_update) =
            self.compute_state_update_namespace::<DBUserNamespace>(state_accesses.user, witness)?;

        let (kernel_root, kernel_state_update) = self
            .compute_state_update_namespace::<DBKernelNamespace>(state_accesses.kernel, witness)?;

        Ok((
            StorageRoot::<S>::new(user_root, kernel_root),
            NamespacedStateUpdate::new(user_state_update, kernel_state_update, Default::default()),
        ))
    }

    fn materialize_changes(&self, state_update: &Self::StateUpdate) -> Self::ChangeSet {
        let preimages_batch = StateDb::materialize_preimages(
            state_update
                .kernel
                .key_preimages
                .iter()
                .map(|(key_hash, key)| (*key_hash, key.key_ref())),
            state_update
                .user
                .key_preimages
                .iter()
                .map(|(key_hash, key)| (*key_hash, key.key_ref())),
        )
        .expect("collecting preimages must succeed");

        let state_change_set = self
            .db
            .materialize_node_batches(
                &state_update.kernel.node_batch,
                &state_update.user.node_batch,
                Some(preimages_batch),
            )
            .expect("collecting node batch must succeed");

        let accessory_batch = self.materialize_accessory(&state_update.accessory);

        NativeChangeSet {
            state_change_set,
            accessory_change_set: accessory_batch,
        }
    }

    fn open_proof(
        state_root: Self::Root,
        state_proof: StorageProof<Self::Proof>,
    ) -> anyhow::Result<(SlotKey, Option<SlotValue>)> {
        let StorageProof {
            key,
            value,
            proof,
            namespace,
        } = state_proof;
        let key_hash = KeyHash::with::<S::Hasher>(key.as_ref());

        // We need to verify the proof against the correct root hash.
        // Hence we match the key against its namespace
        proof.inner().verify(
            jmt::RootHash(state_root.namespace_root(namespace)),
            key_hash,
            value.as_ref().map(|v| v.value()),
        )?;

        Ok((key, value))
    }
}

impl<S: MerkleProofSpec> NativeStorage for ProverStorage<S> {
    fn get_with_proof<N: ProvableCompileTimeNamespace>(
        &self,
        key: SlotKey,
        version: Option<u64>,
    ) -> anyhow::Result<StorageProof<Self::Proof>> {
        if self.is_empty() {
            anyhow::bail!("Empty storage, no proofs available yet");
        }
        let namespace = N::PROVABLE_NAMESPACE;
        Ok(match namespace {
            ProvableNamespace::User => {
                self.get_with_proof_namespace::<DBUserNamespace>(namespace, key, version)
            }
            ProvableNamespace::Kernel => {
                self.get_with_proof_namespace::<DBKernelNamespace>(namespace, key, version)
            }
        })
    }

    fn get_root_hash(&self, version: Version) -> anyhow::Result<Self::Root> {
        if self.is_empty() {
            anyhow::bail!("Empty storage does not have root hash");
        }
        let user_root = self.get_root_hash_namespace_helper::<DBUserNamespace>(version)?;
        let kernel_root = self.get_root_hash_namespace_helper::<DBKernelNamespace>(version)?;

        Ok(StorageRoot::<S>::new(user_root, kernel_root))
    }
}
