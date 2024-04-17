use sov_modules_api::da::{NanoSeconds, Time};
use sov_test_utils::TestSpec;

use crate::ChainStateConfig;

#[test]
fn test_config_serialization() {
    let time = Time::new(2, NanoSeconds::new(3).unwrap());
    let config = ChainStateConfig {
        current_time: time,
        initial_base_fee_per_gas: [2, 2].into(),
    };

    let data = r#"
    {
        "current_time":{
            "secs":2,
            "nanos":3
        },
        "initial_base_fee_per_gas": [2, 2]
    }"#;

    let parsed_config: ChainStateConfig<TestSpec> = serde_json::from_str(data).unwrap();
    assert_eq!(config, parsed_config);
}
