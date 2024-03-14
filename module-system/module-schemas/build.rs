use std::fs::File;
use std::io::{self, Write};

use sov_mock_da::verifier::MockDaSpec;
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::ModuleCallJsonSchema;

type S = DefaultSpec<sov_mock_zkvm::MockZkVerifier>;

fn main() -> io::Result<()> {
    store_json_schema::<sov_bank::Bank<S>>("sov-bank.json")?;
    store_json_schema::<sov_accounts::Accounts<S>>("sov-accounts.json")?;
    store_json_schema::<sov_value_setter::ValueSetter<S>>("sov-value-setter.json")?;
    store_json_schema::<sov_prover_incentives::ProverIncentives<S, MockDaSpec>>(
        "sov-prover-incentives.json",
    )?;
    store_json_schema::<sov_sequencer_registry::SequencerRegistry<S, MockDaSpec>>(
        "sov-sequencer-registry.json",
    )?;
    Ok(())
}

fn store_json_schema<M: ModuleCallJsonSchema>(filename: &str) -> io::Result<()> {
    let mut file = File::create(format!("schemas/{}", filename))?;
    file.write_all(M::json_schema().as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}
