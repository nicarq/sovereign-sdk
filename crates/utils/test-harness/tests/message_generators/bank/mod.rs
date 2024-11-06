use rand::{RngCore, SeedableRng};
use sov_bank::CallMessage;
use sov_test_harness::bank::message_generator::BankChangeLogEntry;
use sov_test_harness::interface::GeneratedMessage;
use sov_test_utils::TestSpec;

mod mint;
mod transfer;

pub type GeneratorOutput =
    GeneratedMessage<TestSpec, CallMessage<TestSpec>, BankChangeLogEntry<TestSpec>>;

pub fn get_random_bytes(num: usize, salt: Option<u64>) -> Vec<u8> {
    let mut rng = rand_chacha::ChaChaRng::seed_from_u64((num as u64) ^ salt.unwrap_or_default());
    let mut output = vec![0; num];
    rng.fill_bytes(&mut output[..]);
    output
}
