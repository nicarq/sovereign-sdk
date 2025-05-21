//! Prover side of NOMT-based Storage implementation
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fmt::Formatter;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use nomt::hasher::BinaryHasher;
use nomt::proof::MultiProof;
use nomt::FinishedSession;
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::reexports::digest::Digest;

use crate::storage::ReadType;
use crate::{
    CompileTimeNamespace, MerkleProofSpec, Namespace, NodeLeaf, NodeLeafAndMaybeValue,
    OrderedReadsAndWrites, ProvableCompileTimeNamespace, ProvableNamespace, SlotKey, SlotValue,
    StateAccesses, StateRoot, StateUpdate, Storage, StorageProof, StorageRoot, Witness,
};

type NomtSession<H> = nomt::Session<BinaryHasher<H>>;

/// A [`Storage`] implementation to be used by the prover in a native execution based on NOMT.
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = "S: MerkleProofSpec"))]
pub struct NomtProverStorage<S: MerkleProofSpec> {
    user_session: Arc<Mutex<Option<NomtSession<S::Hasher>>>>,
    kernel_session: Arc<Mutex<Option<NomtSession<S::Hasher>>>>,
}

impl<S: MerkleProofSpec> core::fmt::Debug for NomtProverStorage<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NomtProverStorage::<{}>", std::any::type_name::<S>())
    }
}

impl<S: MerkleProofSpec> NomtProverStorage<S> {
    /// Create the new instance of [`NomtProverStorage`] with the given sessions.
    pub fn new(
        user_session: NomtSession<S::Hasher>,
        kernel_session: NomtSession<S::Hasher>,
    ) -> Self {
        Self {
            kernel_session: Arc::new(Mutex::new(Some(kernel_session))),
            user_session: Arc::new(Mutex::new(Some(user_session))),
        }
    }

    fn read_value<N: CompileTimeNamespace>(
        &self,
        key: &SlotKey,
        version: Option<SlotNumber>,
    ) -> Option<SlotValue> {
        if version.is_some() {
            unimplemented!(
                "NomProverStorage does not support versioned data yet. Key: {}, version: {:?}.",
                key,
                version
            );
        }
        let key_path = S::Hasher::digest(key.as_ref()).into();
        let session_lock = match N::NAMESPACE {
            Namespace::User => self
                .user_session
                .lock()
                .expect("Failed to acquire lock on user session"),
            Namespace::Kernel => self
                .kernel_session
                .lock()
                .expect("Failed to acquire lock on kernel session"),
            Namespace::Accessory => {
                unimplemented!(
                    "NomProverStorage does not support accessory data yet. Key: {}, version: {:?}.",
                    key,
                    version
                );
            }
        };
        let session = session_lock.as_ref().expect("Session is None");
        session.warm_up(key_path);
        let value = session.read(key_path).expect("Underlying I/O failed");
        value.map(Into::into)
    }
}

fn to_nomt_accesses<S: MerkleProofSpec>(
    session: &NomtSession<S::Hasher>,
    sov_accesses: OrderedReadsAndWrites,
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

        let write_value = write_val.map(|v| v.value().to_vec());

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
    accesses: OrderedReadsAndWrites,
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

#[allow(missing_docs)]
pub struct NomtStateUpdate {
    pub user: FinishedSession,
    pub kernel: FinishedSession,
    pub accessory: OrderedReadsAndWrites,
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
    // These 2 are effectively the same thing, `StateUpdate` is not materialized, change is materialized.
    type StateUpdate = NomtStateUpdate;
    type ChangeSet = ();
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

    fn get_accessory(&self, _key: &SlotKey, _version: Option<SlotNumber>) -> Option<SlotValue> {
        unimplemented!("The NomtProverStorage does not have the accessory state yet.")
    }

    fn compute_state_update(
        &self,
        state_accesses: StateAccesses,
        witness: &Self::Witness,
        prev_state_root: Self::Root,
    ) -> anyhow::Result<(Self::Root, Self::StateUpdate)> {
        let StateAccesses { user, kernel } = state_accesses;

        let prev_user_root = prev_state_root.namespace_root(ProvableNamespace::User);

        // User
        let user_finished_session = {
            let mut user_session = self
                .user_session
                .lock()
                .expect("Failed to acquire lock on user session");
            // Note: It will be solved later: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2634
            let session = user_session
                .take()
                .expect("user session has been taken already");
            compute_state_update_namespace::<S>(session, user, witness).context("user state")?
        };

        assert_eq!(
            user_finished_session.prev_root().as_ref(),
            &prev_user_root,
            "User state root is not equal to the previous state root"
        );

        // Kernel
        let prev_kernel_root = prev_state_root.namespace_root(ProvableNamespace::Kernel);
        let kernel_finished_session = {
            let mut kernel_session = self
                .kernel_session
                .lock()
                .expect("Failed to acquire lock on kernel session");
            // Note: It will be solved later: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2634
            let session = kernel_session
                .take()
                .expect("kernel session has been taken already");
            compute_state_update_namespace::<S>(session, kernel, witness).context("kernel state")?
        };
        assert_eq!(
            kernel_finished_session.prev_root().as_ref(),
            &prev_kernel_root,
            "User state root is not equal to the previous state root"
        );

        let user_root = user_finished_session.root();
        let kernel_root = kernel_finished_session.root();

        let state_update = NomtStateUpdate {
            user: user_finished_session,
            kernel: kernel_finished_session,
            accessory: Default::default(),
        };

        let root = StorageRoot::new(user_root.into_inner(), kernel_root.into_inner());
        Ok((root, state_update))
    }

    fn materialize_changes(self, _state_update: Self::StateUpdate) -> Self::ChangeSet {
        unimplemented!("The NomtProverStorage does not support `materialize_changes` yet.")
    }

    fn open_proof(
        _state_root: Self::Root,
        _proof: StorageProof<Self::Proof>,
    ) -> anyhow::Result<(SlotKey, Option<SlotValue>)> {
        unimplemented!("The NomtProverStorage does not support `open_proof` yet.")
    }
}
