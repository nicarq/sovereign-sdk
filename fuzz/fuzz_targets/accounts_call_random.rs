#![no_main]

use libfuzzer_sys::arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use sov_accounts::{Accounts, CallMessage};
use sov_modules_api::{Context, Module, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;

type S = sov_test_utils::TestSpec;

// Check arbitrary, random calls
fuzz_target!(|input: (&[u8], Vec<(Context<S>, CallMessage)>)| {
    let tmpdir = tempfile::tempdir().unwrap();
    let storage = new_orphan_storage(tmpdir.path()).unwrap();
    let state = &mut WorkingSet::new(storage);

    let (seed, msgs) = input;
    let u = &mut Unstructured::new(seed);
    let accounts: Accounts<S> = Accounts::arbitrary_workset(u, state).unwrap();

    for (ctx, msg) in msgs {
        // assert malformed calls won't panic
        accounts.call(msg, &ctx, state).ok();
    }
});
