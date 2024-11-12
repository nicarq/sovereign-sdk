use sov_bank::CallMessage;
use sov_test_harness::bank::message_generator::BankChangeLogEntry;
use sov_test_harness::interface::GeneratedMessage;
use sov_test_utils::TestSpec;

mod mint;
mod transfer;

pub type GeneratorOutput =
    GeneratedMessage<TestSpec, CallMessage<TestSpec>, BankChangeLogEntry<TestSpec>>;
