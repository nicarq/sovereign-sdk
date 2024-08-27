use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::de::DeserializeOwned;
use sov_modules_api::Spec;

pub(crate) fn from_toml_path<P: AsRef<Path>, R: DeserializeOwned>(path: P) -> anyhow::Result<R> {
    let mut contents = String::new();
    {
        let mut file = File::open(path)?;
        file.read_to_string(&mut contents)?;
    }

    let result: R = toml::from_str(&contents)?;

    Ok(result)
}

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
