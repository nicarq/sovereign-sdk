use sov_bank::CallMessage;
use sov_rollup_interface::reexports::anyhow;
use sov_test_utils::TestSpec;
use sov_universal_wallet::schema::Schema;

fn main() -> anyhow::Result<()> {
    let schema = Schema::of_single_type::<CallMessage<TestSpec>>()?;
    let alloy_schema = schema.into_alloy()?;

    let out_dir = std::env::var("OUT_DIR")?;
    let dest_path = std::path::Path::new(&out_dir).join("alloy_schema.rs");
    std::fs::write(dest_path, format!("{alloy_schema}"))?;

    Ok(())
}
