#![no_main]

use libfuzzer_sys::fuzz_target;
use sov_bank::{Bank, CallMessage};
use sov_modules_api::capabilities::mocks::MockKernel;
use sov_modules_api::{Context, ExecutionContext, Module, WorkingSet};
use sov_test_utils::storage::new_finalized_storage;

type S = sov_test_utils::TestSpec;

fuzz_target!(|input: (&[u8], [u8; 32], [u8; 32])| {
    let (data, sender, sequencer) = input;
    if let Ok(msgs) = serde_json::from_slice::<Vec<CallMessage<S>>>(data) {
        let tmpdir = tempfile::tempdir().unwrap();
        let mut state = WorkingSet::<S>::new_deprecated(
            new_finalized_storage(tmpdir.path()),
            &MockKernel::<S>::default(),
        );
        let ctx = Context::<S>::new(
            sender.into(),
            Default::default(),
            sequencer.into(),
            1,
            ExecutionContext::Node,
        );
        let bank = Bank::default();
        for msg in msgs {
            bank.call(msg, &ctx, &mut state).ok();
        }
    }
});
