#![allow(dead_code)]

use sov_rollup_interface::reexports::anyhow;
use sov_universal_wallet::{schema::Schema, UniversalWallet};

#[derive(UniversalWallet)]
struct Inner {
    field: u8,
}

#[derive(UniversalWallet)]
struct Outer {
    field: Inner,
}

fn main() -> anyhow::Result<()> {
    let schema = Schema::of_single_type::<Outer>()?;
    let alloy_schema = schema.into_alloy()?;

    let out_dir = std::env::var("OUT_DIR")?;
    let dest_path = std::path::Path::new(&out_dir).join("alloy_schema.rs");
    std::fs::write(dest_path, format!("{alloy_schema}"))?;

    Ok(())
}
