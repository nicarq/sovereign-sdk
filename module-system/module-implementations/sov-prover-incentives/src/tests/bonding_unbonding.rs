use sov_bank::GAS_TOKEN_ID;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::Context;

use crate::tests::helpers::{setup, BOND_AMOUNT, S};
use crate::ProverIncentiveError;

#[test]
/// Tests that the prover can unbond correctly
fn test_unbonding() {
    let (module, prover_address, sequencer, mut working_set) = setup();
    let context = Context::<S>::new(prover_address, Default::default(), sequencer, 1);
    let token_id = GAS_TOKEN_ID;

    // Get their *unlocked* balance before undbonding
    let initial_unlocked_balance = {
        module
            .bank
            .get_balance_of(&prover_address, token_id, &mut working_set)
            .unwrap_or_default()
    };

    // Unbond the prover
    module
        .unbond_prover(&context, &mut working_set)
        .expect("Unbonding should succeed");

    // Assert that the prover no longer has bonded tokens
    assert_eq!(module.get_bond_amount(prover_address, &mut working_set), 0);

    // Assert that the prover's unlocked balance has increased by the amount they unbonded
    let unlocked_balance = module
        .bank
        .get_balance_of(&prover_address, token_id, &mut working_set);
    assert_eq!(
        unlocked_balance,
        Some(BOND_AMOUNT + initial_unlocked_balance)
    );
}

#[test]
/// Tests that the prover cannot submit proofs if unbonded
fn test_prover_not_bonded() {
    let (module, prover_address, sequencer, mut working_set) = setup();
    let context = Context::<S>::new(prover_address, Default::default(), sequencer, 1);

    // Unbond the prover
    module
        .unbond_prover(&context, &mut working_set)
        .expect("Unbonding should succeed");

    // Assert that the prover no longer has bonded tokens
    assert_eq!(module.get_bond_amount(prover_address, &mut working_set), 0);

    // Process a valid proof
    {
        let proof = &MockZkvm::create_serialized_proof(true, ());
        // Assert that processing a valid proof fails
        assert_eq!(
            module
                .process_proof(proof, &context, &mut working_set)
                .expect_err("The proof should be rejected"),
            ProverIncentiveError::BondNotHighEnough
        );
    }
}
