use sov_modules_api::da::{NanoSeconds, Time};

type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;
use crate::ChainStateConfig;

#[test]
fn test_config_serialization() {
    let time = Time::new(2, NanoSeconds::new(3).unwrap());
    let config = ChainStateConfig {
        current_time: time,
        gas_price_blocks_depth: 10,
        gas_price_maximum_elasticity: 5,
        initial_gas_price: [2, 2].into(),
        minimum_gas_price: [1, 1].into(),
    };

    let data = r#"
    {
        "current_time":{
            "secs":2,
            "nanos":3
        },
        "gas_price_blocks_depth": 10,
        "gas_price_maximum_elasticity": 5,
        "initial_gas_price": [2, 2],
        "minimum_gas_price": [1, 1]
    }"#;

    let parsed_config: ChainStateConfig<DefaultSpec> = serde_json::from_str(data).unwrap();
    assert_eq!(config, parsed_config)
}
