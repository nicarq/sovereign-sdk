use sov_bank::{CallMessageDiscriminants, Coins, TokenId};
use sov_modules_api::prelude::arbitrary::{Arbitrary, Unstructured};
use sov_test_utils::TestSpec;
use sov_transaction_generator::generators::bank::{BankMessageGenerator, Tag};
use sov_transaction_generator::interface::{
    CallMessageGenerator, Distribution, GeneratorState, MessageValidity, Percent, TagAction,
};
use sov_transaction_generator::state::State;

use crate::get_random_bytes;

#[test]
#[should_panic]
fn test_transfer_generation_without_account() {
    let generator = BankMessageGenerator::<TestSpec>::new(
        Distribution::with_equiprobable_values([CallMessageDiscriminants::Transfer; 5]), // Hack: Always generate a transfer!,
        Percent::fifty(),
    );
    let mut state: State<TestSpec, BankMessageGenerator<TestSpec>> = State::new();
    let random_bytes = get_random_bytes(1_000, 0);
    let mut u = Unstructured::new(random_bytes.as_ref());

    generator
        .generate_call_message(&mut u, &(), &mut state, MessageValidity::Valid)
        .unwrap();
}

// Run transfer generation with the given params
fn do_test(
    address_creation_rate: Percent,
    message_validity: MessageValidity,
) -> super::GeneratorOutput {
    let generator = BankMessageGenerator::<TestSpec>::new(
        Distribution::with_equiprobable_values([CallMessageDiscriminants::Transfer; 5]), // Hack: Always generate a transfer!,
        address_creation_rate,
    );
    let mut state: State<TestSpec, BankMessageGenerator<TestSpec>> = State::new();
    let random_bytes = get_random_bytes(1_000, 0);
    let mut u = Unstructured::new(random_bytes.as_ref());

    let (address, mut account) = state.generate_account(&mut u).unwrap();
    account.balances.push(Coins {
        token_id: TokenId::arbitrary(&mut u).unwrap(),
        amount: 1_000_000_000,
    });

    state.update_account(address, account, vec![TagAction::Add(Tag::HasBalance)]);

    generator
        .generate_call_message(&mut u, &(), &mut state, message_validity)
        .expect("Transfer generation must succeed")
}

#[test]
fn test_outside_transfer_generation() {
    let result = do_test(Percent::one_hundred(), MessageValidity::Valid);

    assert_eq!(result.changes.len(), 2);
}

#[test]
fn test_self_transfer_generation() {
    let result = do_test(Percent::zero(), MessageValidity::Valid);

    assert_eq!(result.changes.len(), 0);
}

#[test]
fn test_invalid_transfer_generation() {
    let result = do_test(Percent::zero(), MessageValidity::Invalid);

    assert_eq!(result.changes.len(), 0);
}
