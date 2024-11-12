use sov_bank::CallMessage;
use sov_test_utils::TestSpec;
use sov_transaction_generator::generators::bank::BankChangeLogEntry;
use sov_transaction_generator::interface::GeneratedMessage;

mod mint;
mod transfer;

pub type GeneratorOutput =
    GeneratedMessage<TestSpec, CallMessage<TestSpec>, BankChangeLogEntry<TestSpec>>;
