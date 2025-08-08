use alloy_primitives::{address, U256};

use alloy_sol_types::{eip712_domain, Eip712Domain, SolStruct};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/alloy_schema.rs"));
}

const DOMAIN: Eip712Domain = eip712_domain! {
    name: "Transaction",
    version: "1",
    chain_id: 4321,
    verifying_contract: address!("0000000000000000000000000000000000000000"),
};

fn main() {
    // Print the contents of the generated alloy_schema.rs file
    let alloy_schema_content = include_str!(concat!(env!("OUT_DIR"), "/alloy_schema.rs"));
    println!("{alloy_schema_content}");

    let value = generated::CallMessage_SetValue {
        value: U256::ZERO,
        gas: [U256::ZERO, U256::ZERO],
    };
    let hash = value.eip712_signing_hash(&DOMAIN);
    dbg!(&hash);
}
