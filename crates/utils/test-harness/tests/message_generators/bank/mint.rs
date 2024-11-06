use arbitrary::{Arbitrary, Unstructured};
use sov_bank::{CallMessageDiscriminants, TokenId};
use sov_test_harness::bank::message_generator::{BankMessageGenerator, Tag};
use sov_test_harness::interface::{
    CallMessageGenerator, GeneratorState, MessageValidity, TagAction,
};
use sov_test_harness::module_message_generators::interface::{Distribution, Percent};
use sov_test_harness::transaction_generator::State;
use sov_test_utils::TestSpec;

use super::get_random_bytes;

// Run mint generation with the given params
fn do_test(
    address_creation_rate: Percent,
    message_validity: MessageValidity,
) -> super::GeneratorOutput {
    let generator = BankMessageGenerator::<TestSpec>::new(
        Distribution::with_equiprobable_values([CallMessageDiscriminants::Mint; 5]), // Hack: Always generate a min!,
        address_creation_rate,
    );
    let mut state: State<TestSpec, BankMessageGenerator<TestSpec>> = State::new();
    let random_bytes = get_random_bytes(1_000, None);
    let mut u = Unstructured::new(random_bytes.as_ref());

    let (address, mut account) = state.generate_account(&mut u).unwrap();
    account.can_mint.insert(TokenId::arbitrary(&mut u).unwrap());

    state.update_account(address, account, vec![TagAction::Add(Tag::CanMint)]);

    generator
        .generate_call_message(&mut u, &(), &mut state, message_validity)
        .expect("Transfer generation must succeed")
}

#[test]
#[should_panic]
fn test_mint_generation_without_account() {
    let generator = BankMessageGenerator::<TestSpec>::new(
        Distribution::with_equiprobable_values([CallMessageDiscriminants::Mint; 5]), // Hack: Always generate a mint!,
        Percent::fifty(),
    );
    let mut state: State<TestSpec, BankMessageGenerator<TestSpec>> = State::new();
    let random_bytes = get_random_bytes(1_000, None);
    let mut u = Unstructured::new(random_bytes.as_ref());

    generator
        .generate_call_message(&mut u, &(), &mut state, MessageValidity::Valid)
        .unwrap();
}

#[test]
fn test_mint_generation() {
    let result = do_test(Percent::one_hundred(), MessageValidity::Valid);

    assert_eq!(result.changes.len(), 1);
}

#[test]
fn test_invalid_mint_generation() {
    let result = do_test(Percent::one_hundred(), MessageValidity::Invalid);

    assert_eq!(result.changes.len(), 0);
}
