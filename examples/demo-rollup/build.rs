use std::fs::File;
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus};

use demo_stf::runtime::RuntimeCall;
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::macros::config_value;
use sov_modules_api::schemars::schema_for;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_universal_wallet::schema::{Schema, SchemaGenerator};

type S = sov_modules_api::default_spec::DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-env-changed=SKIP_GUEST_BUILD");
    println!("cargo::rerun-if-env-changed=SOV_PROVER_MODE");
    println!("cargo::rustc-check-cfg=cfg(skip_guest_build)");

    let is_risczero_installed = Command::new("cargo")
        .args(["risczero", "help"])
        .status()
        .unwrap_or(ExitStatus::from_raw(1)); // If we can't execute the command, assume risczero isn't installed since duplicate install attempts are no-ops.

    if !is_risczero_installed.success() {
        // If installation fails, just exit silently. The user can try again.
        let _ = Command::new("cargo")
            .args(["install", "cargo-risczero"])
            .status();
    }

    let skip_guest_build = std::env::var("SKIP_GUEST_BUILD").unwrap_or_else(|_| "0".to_string());
    if skip_guest_build == "1" {
        println!("cargo::rustc-cfg=skip_guest_build");
    }

    store_schema_as_json::<Transaction<S>, UnsignedTransaction<S>, RuntimeCall<S>>(
        "demo-rollup-schema.json",
    )?;

    // usage with quicktype (after removing invalid empty `NotInstantiable` enum)
    // quicktype -s schema runtime_call.json -o runtime_call.ts
    // Resulting TypeScript file will contain strong types for the runtimes call messages
    let mut runtime_call = File::create("runtime_call.json").unwrap();
    let schema = schema_for!(RuntimeCall<S>);
    let schema_str = serde_json::to_string_pretty(&schema).unwrap();
    runtime_call.write_all(schema_str.as_bytes()).unwrap();
    runtime_call.write_all(b"\n")?;
    Ok(())
}

fn store_schema_as_json<T: SchemaGenerator, U: SchemaGenerator, R: SchemaGenerator>(
    filename: &str,
) -> io::Result<()> {
    let schema = Schema::of_rollup_types_with_metadata::<T, U, R>(config_value!("CHAIN_ID"));
    let schema_string = serde_json::to_string_pretty(&schema)?;
    let mut file = File::create(filename)?;
    file.write_all(schema_string.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}
