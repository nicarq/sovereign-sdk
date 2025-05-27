//! Prover side of NOMT-based Storage implementation
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fmt::Formatter;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use nomt::hasher::BinaryHasher;
use nomt::proof::MultiProof;
use nomt::FinishedSession;
use sov_db::accessory_db::AccessoryDb;
use sov_db::historical_state::HistoricalStateReader;
use sov_db::namespaces::{KernelNamespace, UserNamespace};
use sov_db::state_db_nomt::StateSession;
use sov_db::storage_manager::{
    InitializableNativeNomtStorage, NomtChangeSet, StateFinishedSession,
};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::reexports::digest::Digest;

use crate::storage::ReadType;
use crate::{
    Accessory, CompileTimeNamespace, MerkleProofSpec, Namespace, NativeStorage, NodeLeaf,
    NodeLeafAndMaybeValue, OrderedReadsAndWrites, ProvableCompileTimeNamespace, ProvableNamespace,
    SlotKey, SlotValue, StateAccesses, StateRoot, StateUpdate, Storage, StorageProof, StorageRoot,
    Witness,
};

type NomtSession<H> = nomt::Session<BinaryHasher<H>>;

/// A [`Storage`] implementation to be used by the prover in a native execution based on NOMT.
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = "S: MerkleProofSpec"))]
pub struct NomtProverStorage<S: MerkleProofSpec> {
    state_session: Arc<Mutex<Option<StateSession<S::Hasher>>>>,
    historical_state: HistoricalStateReader,
    accessory: AccessoryDb,
}

impl<S: MerkleProofSpec> core::fmt::Debug for NomtProverStorage<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NomtProverStorage::<{}>", std::any::type_name::<S>())
    }
}

impl<S: MerkleProofSpec> NomtProverStorage<S> {
    /// Create the new instance of [`NomtProverStorage`] with the given sessions.
    pub fn new(
        state: StateSession<S::Hasher>,
        historical_state: HistoricalStateReader,
        accessory: AccessoryDb,
    ) -> Self {
        Self {
            state_session: Arc::new(Mutex::new(Some(state))),
            historical_state,
            accessory,
        }
    }
    /// Utility method for checking if storage is empty.
    /// Does not guarantee 100% that it actually is.
    pub fn is_empty(&self) -> bool {
        self.historical_state.get_next_version() == SlotNumber::GENESIS
    }

    fn get_version_to_use(&self, version: Option<SlotNumber>) -> Option<SlotNumber> {
        if self.is_empty() {
            return None;
        }
        let next_version = self.historical_state.get_next_version();
        match version {
            None => Some(
                next_version
                    .checked_sub(1)
                    .expect("Next version for non empty storage should be above 0"),
            ),
            Some(passed_version) => {
                if passed_version >= next_version {
                    None
                } else {
                    Some(passed_version)
                }
            }
        }
    }

    fn read_value<N: CompileTimeNamespace>(
        &self,
        key: &SlotKey,
        version: Option<SlotNumber>,
    ) -> Option<SlotValue> {
        match (N::NAMESPACE, version) {
            (Namespace::User, None) => {
                let key_path = S::Hasher::digest(key.as_ref()).into();
                let session_lock = self
                    .state_session
                    .lock()
                    .expect("Failed to acquire lock on state session");
                let session = &session_lock.as_ref().expect("Session is None").user;

                session.warm_up(key_path);
                session.read(key_path).expect("Underlying I/O failed")
            }
            (Namespace::Kernel, None) => {
                let key_path = S::Hasher::digest(key.as_ref()).into();
                let session_lock = self
                    .state_session
                    .lock()
                    .expect("Failed to acquire lock on state session");
                let session = &session_lock.as_ref().expect("Session is None").kernel;

                session.warm_up(key_path);
                session.read(key_path).expect("Underlying I/O failed")
            }
            (Namespace::User, Some(version)) => {
                let version = self.get_version_to_use(Some(version))?;
                self.historical_state
                    .get_value_option_by_key::<UserNamespace>(version, key.as_ref())
                    .expect("Underlying I/O failed")
            }
            (Namespace::Kernel, Some(version)) => {
                let version = self.get_version_to_use(Some(version))?;
                self.historical_state
                    .get_value_option_by_key::<KernelNamespace>(version, key.as_ref())
                    .expect("Underlying I/O failed")
            }
            (Namespace::Accessory, version) => {
                let version = self.get_version_to_use(version)?;
                self.accessory
                    .get_value_option(key.as_ref(), version)
                    .expect("Unable to read from AccessoryDb")
            }
        }
        .map(Into::into)
    }
}

fn to_nomt_accesses<S: MerkleProofSpec>(
    session: &NomtSession<S::Hasher>,
    sov_accesses: &OrderedReadsAndWrites,
) -> anyhow::Result<Vec<(nomt::trie::KeyPath, nomt::KeyReadWrite)>> {
    let mut merged_accesses: BTreeMap<nomt::trie::KeyPath, nomt::KeyReadWrite> = BTreeMap::new();

    let OrderedReadsAndWrites {
        ordered_reads,
        ordered_writes,
    } = sov_accesses;

    // First, put all the reads into merged accesses, so later we can distingiush `Write` from `ReadThenWrite`
    for (key, read_node_leaf) in ordered_reads {
        // Reads are warmed up during normal `get/get_leaf`
        let key_hash: nomt::trie::KeyPath = S::Hasher::digest(key.as_ref()).into();
        // From documentation:
        // > This should be called for every logical write within the session, as well as every
        // > logical read if you expect to generate a merkle proof for the session.
        // So warming up all reads.
        session.warm_up(key_hash);

        let combined_hash_and_size =
            read_node_leaf.map(|node_leaf| node_leaf.combine_val_hash_and_size());

        let nomt_read = nomt::KeyReadWrite::Read(combined_hash_and_size);

        if merged_accesses.insert(key_hash, nomt_read).is_some() {
            anyhow::bail!("Duplicate key read in state: {:?}", key_hash);
        };
    }

    // Writes
    for (key, write_val) in ordered_writes {
        let key_hash: nomt::trie::KeyPath = S::Hasher::digest(key.as_ref()).into();
        session.warm_up(key_hash);

        let write_value = write_val.as_ref().map(|v| v.value().to_vec());

        match merged_accesses.entry(key_hash) {
            Entry::Vacant(vacant) => {
                // Also warming up all writes. `ReadThenWrite` has been warmed up during reads collection.
                session.warm_up(key_hash);
                vacant.insert(nomt::KeyReadWrite::Write(write_value));
            }
            Entry::Occupied(occupied) => match occupied.remove() {
                nomt::KeyReadWrite::Read(read_value) => {
                    merged_accesses.insert(
                        key_hash,
                        nomt::KeyReadWrite::ReadThenWrite(read_value, write_value),
                    );
                }
                _ => {
                    anyhow::bail!("Duplicate key write in kernel state: {:?}", key_hash);
                }
            },
        }
    }

    Ok(merged_accesses.into_iter().collect())
}

fn compute_state_update_namespace<S: MerkleProofSpec>(
    session: NomtSession<S::Hasher>,
    accesses: &OrderedReadsAndWrites,
    witness: &S::Witness,
) -> anyhow::Result<FinishedSession> {
    let nomt_accesses = to_nomt_accesses::<S>(&session, accesses)?;
    let mut finished = session.finish(nomt_accesses)?;
    let nomt_witness = finished.take_witness().expect("Witness cannot be missing");
    let nomt::Witness {
        path_proofs,
        operations: nomt::WitnessedOperations { .. },
    } = nomt_witness;
    // Note, we discard `p.path`, but maybe there's a way to use to have more efficient verification?
    let path_proofs_inner = path_proofs.into_iter().map(|p| p.inner).collect::<Vec<_>>();

    let multi_proof = MultiProof::from_path_proofs(path_proofs_inner);
    witness.add_hint(&multi_proof);
    Ok(finished)
}

impl<S: MerkleProofSpec> InitializableNativeNomtStorage<S::Hasher> for NomtProverStorage<S> {
    fn new(
        state_db: sov_db::state_db_nomt::StateSession<S::Hasher>,
        historical_state: HistoricalStateReader,
        accessory_db: AccessoryDb,
    ) -> Self {
        Self::new(state_db, historical_state, accessory_db)
    }
}

#[allow(missing_docs)]
pub struct NomtStateUpdate {
    user: FinishedSession,
    kernel: FinishedSession,
    accessory: OrderedReadsAndWrites,
    state_accesses: StateAccesses,
}

impl StateUpdate for NomtStateUpdate {
    fn add_accessory_item(&mut self, key: SlotKey, value: Option<SlotValue>) {
        self.accessory.ordered_writes.push((key, value));
    }

    fn get_accessory_items(&self) -> impl Iterator<Item = &(SlotKey, Option<SlotValue>)> {
        self.accessory.ordered_writes.iter()
    }
}

impl<S: MerkleProofSpec> Storage for NomtProverStorage<S> {
    type Hasher = S::Hasher;
    type Witness = S::Witness;
    type Proof = ();
    type Root = StorageRoot<S>;
    // These 2 are effectively the same thing, `StateUpdate` is not materialized, `ChangeSet` is materialized.
    type StateUpdate = NomtStateUpdate;
    type ChangeSet = NomtChangeSet;
    const PRE_GENESIS_ROOT: Self::Root =
        StorageRoot::new(nomt::trie::TERMINATOR, nomt::trie::TERMINATOR);

    fn put_in_witness(&self, value: Option<SlotValue>, witness: &Self::Witness) {
        witness.add_hint(&value);
    }

    fn get_leaf<N: ProvableCompileTimeNamespace>(
        &self,
        key: &SlotKey,
        version: Option<SlotNumber>,
        witness: &Self::Witness,
    ) -> Option<NodeLeafAndMaybeValue> {
        let val = self.read_value::<N>(key, version);

        // First, we create a node that we put in the cache. This one contains the value.
        let node_leaf_with_fetched_value = val.map(|v| {
            let leaf = NodeLeaf::make_leaf::<S::Hasher>(&v);
            NodeLeafAndMaybeValue {
                leaf,
                value: ReadType::GetSizeValueFetched(v),
            }
        });

        // Second, we create a node that we put in the witness. This one doesn't contain the value.
        let node_leaf_without_value =
            node_leaf_with_fetched_value
                .clone()
                .map(|node| NodeLeafAndMaybeValue {
                    leaf: node.leaf,
                    value: ReadType::GetSizeValueNotFetched,
                });

        witness.add_hint(&node_leaf_without_value);
        node_leaf_with_fetched_value
    }

    fn get<N: ProvableCompileTimeNamespace>(
        &self,
        key: &SlotKey,
        version: Option<SlotNumber>,
        witness: &Self::Witness,
    ) -> Option<SlotValue> {
        let val = self.read_value::<N>(key, version);
        witness.add_hint(&val);
        val
    }

    fn get_accessory(&self, key: &SlotKey, version: Option<SlotNumber>) -> Option<SlotValue> {
        self.read_value::<Accessory>(key, version)
    }

    fn compute_state_update(
        &self,
        state_accesses: StateAccesses,
        witness: &Self::Witness,
        prev_state_root: Self::Root,
    ) -> anyhow::Result<(Self::Root, Self::StateUpdate)> {
        let mut state_session = self
            .state_session
            .lock()
            .expect("Failed to acquire lock on state session");
        let StateSession {
            user: user_session,
            kernel: kernel_session,
        } = state_session
            .take()
            .expect("user session has been taken already");

        // User
        let prev_user_root = prev_state_root.namespace_root(ProvableNamespace::User);
        let user_finished_session =
            compute_state_update_namespace::<S>(user_session, &state_accesses.user, witness)
                .context("user state")?;
        assert_eq!(
            user_finished_session.prev_root().as_ref(),
            &prev_user_root,
            "User state root is not equal to the previous state root"
        );

        // Kernel
        let prev_kernel_root = prev_state_root.namespace_root(ProvableNamespace::Kernel);
        let kernel_finished_session =
            compute_state_update_namespace::<S>(kernel_session, &state_accesses.kernel, witness)
                .context("user state")?;
        assert_eq!(
            kernel_finished_session.prev_root().as_ref(),
            &prev_kernel_root,
            "Kernel state root is not equal to the previous state root"
        );

        let user_root = user_finished_session.root();
        let kernel_root = kernel_finished_session.root();

        let state_update = NomtStateUpdate {
            user: user_finished_session,
            kernel: kernel_finished_session,
            accessory: Default::default(),
            state_accesses,
        };

        let root = StorageRoot::new(user_root.into_inner(), kernel_root.into_inner());
        Ok((root, state_update))
    }

    fn materialize_changes(self, state_update: Self::StateUpdate) -> Self::ChangeSet {
        let next_version = self.historical_state.get_next_version();
        let NomtStateUpdate {
            state_accesses:
                StateAccesses {
                    user: user_versioned,
                    kernel: kernel_versioned,
                },
            accessory: accessory_writes,
            user,
            kernel,
        } = state_update;
        let user_to_materialize = user_versioned.ordered_writes.into_iter().map(|(k, v)| {
            // TODO: Clone now, figure out how to optimize later
            (k.as_ref().clone(), v.map(|x| x.value().to_vec()))
        });
        let kernel_to_materialize = kernel_versioned.ordered_writes.into_iter().map(|(k, v)| {
            // TODO: Clone now, figure out how to optimize later
            (k.as_ref().clone(), v.map(|x| x.value().to_vec()))
        });
        let historical_schema_batch = HistoricalStateReader::materialize_values(
            user_to_materialize,
            kernel_to_materialize,
            next_version,
        )
        .expect("historical state db materialization must succeed");
        let accessory_batch = AccessoryDb::materialize_values(
            accessory_writes
                .ordered_writes
                .iter()
                .map(|(k, v_opt)| (k.key().to_vec(), v_opt.as_ref().map(|v| v.value().to_vec()))),
            next_version,
        )
        .expect("accessory db materialization must succeed");
        NomtChangeSet {
            state: StateFinishedSession::new(user, kernel),
            historical_state: historical_schema_batch,
            accessory: accessory_batch,
        }
    }

    fn open_proof(
        _state_root: Self::Root,
        _proof: StorageProof<Self::Proof>,
    ) -> anyhow::Result<(SlotKey, Option<SlotValue>)> {
        unimplemented!("The NomtProverStorage does not support `open_proof` yet.")
    }
}

impl<S: MerkleProofSpec> NativeStorage for NomtProverStorage<S> {
    fn latest_version(&self) -> SlotNumber {
        self.historical_state.get_next_version().saturating_sub(1)
    }

    fn get_with_proof<N: ProvableCompileTimeNamespace>(
        &self,
        key: SlotKey,
        slot_number: Option<SlotNumber>,
    ) -> anyhow::Result<StorageProof<Self::Proof>> {
        let version_to_use = match self.get_version_to_use(slot_number) {
            None => {
                anyhow::bail!(
                    "Proof is not available at version {:?}. Empty storage or future version",
                    slot_number,
                )
            }
            Some(v) => v,
        };
        let namespace = N::PROVABLE_NAMESPACE;
        let value = match namespace {
            ProvableNamespace::User => self.read_value::<crate::User>(&key, Some(version_to_use)),
            ProvableNamespace::Kernel => {
                self.read_value::<crate::Kernel>(&key, Some(version_to_use))
            }
        };

        // self.read_value::<N>(&key, Some(version_to_use));
        Ok(StorageProof {
            key,
            value,
            // TODO: Proof is empty now, will be fixed in follow
            proof: (),
            namespace,
        })
    }

    fn get_root_hash(&self, version: SlotNumber) -> anyhow::Result<Self::Root> {
        // TODO: Support for historical root hash
        let _version_to_use = match self.get_version_to_use(Some(version)) {
            None => {
                // Mimic error from jmt.
                // TODO: Address this in the future.
                anyhow::bail!("Root node not found for version {}.", version)
            }
            Some(v) => v,
        };
        let state_session_lock = self
            .state_session
            .lock()
            .expect("Failed to acquire lock on state session");
        let state_session = state_session_lock
            .as_ref()
            .expect("Session is None, Storage is not usable anymore");
        let kernel_root = state_session.kernel.prev_root();
        let user_root = state_session.user.prev_root();
        drop(state_session_lock);
        Ok(StorageRoot::new(
            user_root.into_inner(),
            kernel_root.into_inner(),
        ))
    }
}
