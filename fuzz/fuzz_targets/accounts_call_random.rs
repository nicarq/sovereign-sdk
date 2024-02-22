#![no_main]

use libfuzzer_sys::arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use sov_accounts::{Accounts, CallMessage};
use sov_modules_api::{Context, Module, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;

type S = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;

// Check arbitrary, random calls
fuzz_target!(|input: (&[u8], Vec<(Context<S>, CallMessage<S>)>)| {
    let tmpdir = tempfile::tempdir().unwrap();
    let storage = new_orphan_storage(tmpdir.path()).unwrap();
    let working_set = &mut WorkingSet::new(storage);

    let (seed, msgs) = input;
    let u = &mut Unstructured::new(seed);
    let accounts: Accounts<S> = Accounts::arbitrary_workset(u, working_set).unwrap();

    for (ctx, msg) in msgs {
        // assert malformed calls won't panic
        accounts.call(msg, &ctx, working_set).ok();
    }
});
