use demo_stf::runtime::RuntimeCall;
use sov_modules_api::sov_universal_wallet::schema::Schema;

use crate::test_helpers::DemoRollupSpec;

type S = DemoRollupSpec;

#[test]
fn test_produce_alloy_definitions() -> anyhow::Result<()> {
    let schema = Schema::of_single_type::<RuntimeCall<S>>()?;
    dbg!(&schema);
    let alloy = schema.into_alloy()?;
    dbg!(&alloy);
    Ok(())
}
