use sov_modules_api::utils::generate_address as gen_addr_generic;
use sov_modules_api::Spec;
type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;

use crate::NonFungibleTokenConfig;

#[test]
fn test_config_serialization() {
    let address: <DefaultSpec as Spec>::Address = gen_addr_generic::<DefaultSpec>("admin");
    let owner: <DefaultSpec as Spec>::Address = gen_addr_generic::<DefaultSpec>("owner");

    let config = NonFungibleTokenConfig::<DefaultSpec> {
        admin: address,
        owners: vec![(0, owner)],
    };

    let data = r#"
    {
        "admin":"sov1335hded4gyzpt00fpz75mms4m7ck02wgw07yhw9grahj4dzg4yvqk63pml",
        "owners":[
            [0,"sov1fsgzj6t7udv8zhf6zj32mkqhcjcpv52yph5qsdcl0qt94jgdckqsczjm2y"]
        ]
    }"#;

    let parsed_config: NonFungibleTokenConfig<DefaultSpec> = serde_json::from_str(data).unwrap();
    assert_eq!(config, parsed_config)
}
