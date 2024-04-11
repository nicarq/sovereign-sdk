use std::marker::PhantomData;

use sov_rollup_interface::da::{BlockHeaderTrait, DaVerifier};
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::zk::{StateTransitionPublicData, StateTransitionWitness, ZkvmGuest};
/// Verifies a state transition
pub struct StateTransitionVerifier<ST, Da, Zk>
where
    Da: DaVerifier,
    Zk: ZkvmGuest,
    ST: StateTransitionFunction<Zk::Verifier, Da::Spec>,
{
    app: ST,
    da_verifier: Da,
    phantom: PhantomData<Zk>,
}

impl<Stf, Da, Zk> StateTransitionVerifier<Stf, Da, Zk>
where
    Da: DaVerifier,
    Zk: ZkvmGuest,
    Stf: StateTransitionFunction<Zk::Verifier, Da::Spec>,
{
    /// Create a [`StateTransitionVerifier`]
    pub fn new(app: Stf, da_verifier: Da) -> Self {
        Self {
            app,
            da_verifier,
            phantom: Default::default(),
        }
    }

    /// Verify the next block
    pub fn run_block(&self, zkvm: Zk, pre_state: Stf::PreState) -> Result<(), Da::Error> {
        let mut data: StateTransitionWitness<_, _, Da::Spec> = zkvm.read_from_host();
        let validity_condition = self.da_verifier.verify_relevant_tx_list(
            &data.da_block_header,
            &data.relevant_blobs,
            data.relevant_proofs,
        )?;

        let result = self.app.apply_slot(
            &data.initial_state_root,
            pre_state,
            data.witness,
            &data.da_block_header,
            &validity_condition,
            data.relevant_blobs.as_iters(),
        );

        let out: StateTransitionPublicData<Da::Spec, _> = StateTransitionPublicData {
            initial_state_root: data.initial_state_root,
            final_state_root: result.state_root,
            slot_hash: data.da_block_header.hash(),
            validity_condition,
        };

        zkvm.commit(&out);
        Ok(())
    }
}
