//! Testing utilities.

use borsh::{BorshDeserialize, BorshSerialize};
use sov_rollup_interface::da::{DaSpec, RelevantBlobIters};
use sov_rollup_interface::stf::{
    ApplySlotOutput, BatchReceipt, ExecutionContext, StateTransitionFunction,
};
use sov_rollup_interface::zk::Zkvm;

/// A mock implementation of the [`StateTransitionFunction`]
#[derive(PartialEq, Debug, Clone, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct MockStf;

#[derive(
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    BorshSerialize,
    BorshDeserialize,
    derive_more::Display,
    derive_more::Debug,
    derive_more::From,
    derive_more::AsRef,
)]
#[display("{}", hex::encode(&self.0))]
#[debug("MockRoot({})", hex::encode(&self.0))]
/// A mock state root
pub struct MockRoot(Vec<u8>);

impl AsRef<[u8]> for MockRoot {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl<InnerVm: Zkvm, OuterVm: Zkvm, Da: DaSpec> StateTransitionFunction<InnerVm, OuterVm, Da>
    for MockStf
{
    type Address = Vec<u8>;
    type StateRoot = MockRoot;
    type GasPrice = ();
    type GenesisParams = ();
    type PreState = ();
    type ChangeSet = ();
    type StorageProof = ();
    type TxReceiptContents = ();
    type BatchReceiptContents = ();
    type Witness = ();

    // Perform one-time initialization for the genesis block.
    fn init_chain(
        &self,
        _genesis_rollup_header: &Da::BlockHeader,
        _base_state: Self::PreState,
        _params: Self::GenesisParams,
    ) -> (Self::StateRoot, ()) {
        (Vec::default().into(), ())
    }

    fn apply_slot(
        &self,
        _pre_state_root: &Self::StateRoot,
        _base_state: Self::PreState,
        _witness: Self::Witness,
        _slot_header: &Da::BlockHeader,
        _relevant_blobs: RelevantBlobIters<&mut [<Da as DaSpec>::BlobTransaction]>,
        _execution_context: ExecutionContext,
    ) -> ApplySlotOutput<InnerVm, OuterVm, Da, Self> {
        ApplySlotOutput::<InnerVm, OuterVm, Da, Self> {
            state_root: Vec::default().into(),
            change_set: (),
            proof_receipts: vec![],
            batch_receipts: vec![BatchReceipt {
                batch_hash: [0; 32],
                tx_receipts: vec![],
                ignored_tx_receipts: vec![],
                inner: (),
            }],
            witness: (),
        }
    }
}
