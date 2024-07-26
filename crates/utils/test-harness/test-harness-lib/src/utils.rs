use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use sov_modules_api::Spec;

/// Create [`sov_bank::BankConfig`] from a genesis directory.
pub fn get_bank_config<S: Spec>(
    genesis_dir: impl AsRef<Path>,
) -> anyhow::Result<sov_bank::BankConfig<S>> {
    let path = genesis_dir.as_ref().join("bank.json");

    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let bank_config = serde_json::from_reader(reader)?;

    Ok(bank_config)
}
