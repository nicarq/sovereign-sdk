use std::marker::PhantomData;

use jmt::storage::{NodeBatch, TreeWriter};
use jmt::{JellyfishMerkleTree, KeyHash, Version};
use sov_db::namespaces;
use sov_db::namespaces::{KernelNamespace as DBKernelNamespace, UserNamespace as DBUserNamespace};
use sov_db::native_db::NativeDB;
use sov_db::schema::ChangeSet;
use sov_db::state_db::{JmtHandler, StateDB};
use sov_modules_core::namespaces::{Accessory, CompileTimeNamespace, ProvableCompileTimeNamespace};
use sov_modules_core::{
    Namespace, NativeStorage, OrderedReadsAndWrites, ProvableNamespace, SlotKey, SlotValue,
    StateAccesses, StateUpdate, Storage, StorageProof, Witness,
};

use crate::config::Config;
use crate::storage_internals::{SparseMerkleProof, StorageRoot};
use crate::MerkleProofSpec;

/// A [`Storage`] implementation to be used by the prover in a native execution
/// environment (outside of the zkVM).
#[derive(derivative::Derivative)]
#[derivative(
    Clone(bound = "S: MerkleProofSpec"),
    Debug(bound = "S: MerkleProofSpec")
)]
pub struct ProverStorage<S: MerkleProofSpec> {
    db: StateDB,
    native_db: NativeDB,
    _phantom_hasher: PhantomData<S::Hasher>,
}

impl<S: MerkleProofSpec> ProverStorage<S> {
    /// Creates a new [`ProverStorage`] instance from specified db handles
    pub fn with_db_handles(db: StateDB, native_db: NativeDB) -> Self {
        Self {
            db,
            native_db,
            _phantom_hasher: Default::default(),
        }
    }

    fn read_value_namespace<N: namespaces::Namespace>(
        &self,
        key: &SlotKey,
        version: Option<Version>,
    ) -> Option<SlotValue> {
        let version_to_use = version.unwrap_or_else(|| self.db.get_next_version());

        match self
            .db
            .get_value_option_by_key::<N>(version_to_use, key.as_ref())
        {
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
        match N::NAMESPACE {
            Namespace::User => self.read_value_namespace::<DBUserNamespace>(key, version),
            Namespace::Kernel => self.read_value_namespace::<DBKernelNamespace>(key, version),
            Namespace::Accessory => self
                .native_db
                .get_value_option(key.as_ref(), version.unwrap_or(u64::MAX))
                .expect("Unable to read from nativeDB")
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
    ) -> Result<(jmt::RootHash, ProverStateUpdate), anyhow::Error> {
        let state_db_handler: JmtHandler<N> = self.db.get_jmt_handler();
        let jmt = JellyfishMerkleTree::<JmtHandler<N>, S::Hasher>::new(&state_db_handler);
        let latest_version = self.db.get_next_version() - 1;

        // Handle empty jmt
        // TODO: Fix this before introducing snapshots!
        if jmt.get_root_hash_option(latest_version)?.is_none() {
            assert_eq!(latest_version, 0);
            let empty_batch = Vec::default().into_iter();
            let (_, tree_update) = jmt
                .put_value_set(empty_batch, latest_version)
                .expect("JMT update must succeed");

            self.db
                .get_jmt_handler::<N>()
                .write_node_batch(&tree_update.node_batch)
                .expect("db write must succeed");
        }
        let prev_root = jmt
            .get_root_hash(latest_version)
            .expect("Previous root hash was just populated");
        witness.add_hint(prev_root.0);

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

        let mut key_preimages = Vec::with_capacity(state_accesses.ordered_writes.len());

        // Compute the jmt update from the write batch
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

    fn commit_namespace<N: namespaces::Namespace>(&self, state_update: &ProverStateUpdate) {
        self.db
            .put_preimages::<N>(
                state_update
                    .key_preimages
                    .iter()
                    .map(|(key_hash, key)| (*key_hash, key.key_ref())),
            )
            .expect("Preimage put must succeed");

        // Write the state values last, since we base our view of what has been touched
        // on state. If the node crashes between the `native_db` update and this update,
        // then the whole `commit` will be re-run later so no data can be lost.
        self.db
            .get_jmt_handler::<N>()
            .write_node_batch(&state_update.node_batch)
            .expect("db write must succeed");
    }

    fn commit_accessory(&self, accessory_writes: &OrderedReadsAndWrites) {
        let latest_version = self.db.get_next_version() - 1;
        self.native_db
            .set_values(
                accessory_writes.ordered_writes.iter().map(|(k, v_opt)| {
                    (k.key().to_vec(), v_opt.as_ref().map(|v| v.value().to_vec()))
                }),
                latest_version,
            )
            .expect("native db write must succeed");
    }

    fn get_with_proof_namespace<N: namespaces::Namespace>(
        &self,
        namespace: ProvableNamespace,
        key: SlotKey,
        version: Option<u64>,
    ) -> StorageProof<<ProverStorage<S> as Storage>::Proof> {
        let state_db_handler: JmtHandler<N> = self.db.get_jmt_handler();
        let merkle = JellyfishMerkleTree::<JmtHandler<N>, S::Hasher>::new(&state_db_handler);
        let (val_opt, proof) = merkle
            .get_with_proof(
                KeyHash::with::<S::Hasher>(key.as_ref()),
                version.unwrap_or_else(|| self.db.get_next_version() - 1),
            )
            .unwrap();
        StorageProof {
            key,
            value: val_opt.map(SlotValue::from),
            proof: SparseMerkleProof::<S::Hasher>::from(proof),
            namespace,
        }
    }
}

/// Changeset extracted from [`ProverStorage`]
pub struct ProverChangeSet {
    /// [`ChangeSet`] associated with provable state updates.
    pub state_change_set: ChangeSet,
    /// [`ChangeSet`] associated with non-provable accessory updates.
    pub accessory_change_set: ChangeSet,
}

impl<S: MerkleProofSpec> TryFrom<ProverStorage<S>> for ProverChangeSet {
    type Error = anyhow::Error;

    fn try_from(prover_storage: ProverStorage<S>) -> Result<Self, Self::Error> {
        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/122
        let ProverStorage { db, native_db, .. } = prover_storage;
        let state_change_set = db.freeze()?;
        let accessory_change_set = native_db.freeze()?;
        Ok(ProverChangeSet {
            state_change_set,
            accessory_change_set,
        })
    }
}

#[derive(Default)]
pub struct ProverStateUpdate {
    pub(crate) node_batch: NodeBatch,
    pub key_preimages: Vec<(KeyHash, SlotKey)>,
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
    type ChangeSet = ProverChangeSet;

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
    ) -> Result<(Self::Root, Self::StateUpdate), anyhow::Error> {
        let (user_root, user_state_update) =
            self.compute_state_update_namespace::<DBUserNamespace>(state_accesses.user, witness)?;

        let (kernel_root, kernel_state_update) = self
            .compute_state_update_namespace::<DBKernelNamespace>(state_accesses.kernel, witness)?;

        Ok((
            StorageRoot::<S>::new(user_root, kernel_root),
            NamespacedStateUpdate::new(user_state_update, kernel_state_update, Default::default()),
        ))
    }

    fn commit(&self, state_update: &Self::StateUpdate) {
        self.commit_namespace::<DBUserNamespace>(&state_update.user);
        self.commit_namespace::<DBKernelNamespace>(&state_update.kernel);
        self.commit_accessory(&state_update.accessory);

        // Finally, update our in-memory view of the current item numbers
        self.db.inc_next_version();
    }

    fn open_proof(
        state_root: Self::Root,
        state_proof: StorageProof<Self::Proof>,
    ) -> Result<(SlotKey, Option<SlotValue>), anyhow::Error> {
        let StorageProof {
            key,
            value,
            proof,
            namespace,
        } = state_proof;
        let key_hash = KeyHash::with::<S::Hasher>(key.as_ref());

        // We need to verify the proof against the correct root hash
        // Hence we match the key against its namespace
        match namespace {
            ProvableNamespace::User => proof.inner().verify(
                state_root.user_hash(),
                key_hash,
                value.as_ref().map(|v| v.value()),
            )?,
            ProvableNamespace::Kernel => proof.inner().verify(
                state_root.kernel_hash(),
                key_hash,
                value.as_ref().map(|v| v.value()),
            )?,
        }

        Ok((key, value))
    }

    // Based on assumption `validate_and_commit` increments version.
    fn is_empty(&self) -> bool {
        self.db.get_next_version() <= 1
    }

    fn to_change_set(self) -> Self::ChangeSet {
        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/122
        ProverChangeSet::try_from(self)
            .expect("Failed to convert, another RC must exists somewhere")
    }
}

impl<S: MerkleProofSpec> NativeStorage for ProverStorage<S> {
    fn get_with_proof<N: ProvableCompileTimeNamespace>(
        &self,
        key: SlotKey,
        version: Option<u64>,
    ) -> StorageProof<Self::Proof> {
        let namespace = N::PROVABLE_NAMESPACE;
        match namespace {
            ProvableNamespace::User => {
                self.get_with_proof_namespace::<DBUserNamespace>(namespace, key, version)
            }
            ProvableNamespace::Kernel => {
                self.get_with_proof_namespace::<DBKernelNamespace>(namespace, key, version)
            }
        }
    }

    fn get_root_hash(&self, version: Version) -> anyhow::Result<Self::Root> {
        let user_root = self.get_root_hash_namespace_helper::<DBUserNamespace>(version)?;
        let kernel_root = self.get_root_hash_namespace_helper::<DBKernelNamespace>(version)?;

        Ok(StorageRoot::<S>::new(user_root, kernel_root))
    }
}
