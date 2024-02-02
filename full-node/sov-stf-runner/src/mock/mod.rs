use std::marker::PhantomData;

use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::stf::{
    ApplySlotOutput, BatchReceipt, SlotResult, StateTransitionFunction,
};
use sov_rollup_interface::zk::{ValidityCondition, Zkvm};

/// A mock implementation of the [`StateTransitionFunction`]
#[derive(PartialEq, Debug, Clone, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct MockStf<Cond> {
    phantom_data: PhantomData<Cond>,
}

impl<Vm: Zkvm, Cond: ValidityCondition, Da: DaSpec> StateTransitionFunction<Vm, Da>
    for MockStf<Cond>
{
    type StateRoot = Vec<u8>;
    type GenesisParams = ();
    type PreState = ();
    type ChangeSet = ();
    type TxReceiptContents = ();
    type BatchReceiptContents = ();
    type Witness = ();
    type Condition = Cond;

    // Perform one-time initialization for the genesis block.
    fn init_chain(
        &self,
        _base_state: Self::PreState,
        _params: Self::GenesisParams,
    ) -> (Self::StateRoot, ()) {
        (Vec::default(), ())
    }

    fn apply_slot<'a, I>(
        &self,
        _pre_state_root: &Self::StateRoot,
        _base_state: Self::PreState,
        _witness: Self::Witness,
        _slot_header: &Da::BlockHeader,
        _validity_condition: &Da::ValidityCondition,
        _blobs: I,
    ) -> ApplySlotOutput<Vm, Da, Self>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        SlotResult {
            state_root: Vec::default(),
            change_set: (),
            batch_receipts: vec![BatchReceipt {
                batch_hash: [0; 32],
                tx_receipts: vec![],
                inner: (),
                gas_price: vec![],
            }],
            witness: (),
        }
    }
}
