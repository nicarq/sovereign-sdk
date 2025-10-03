#![no_main]

use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct SpendPublicGuest {
    anchor_root: [u8; 32],
    nullifiers: Vec<[u8; 32]>,
    commitments: Vec<[u8; 32]>,
    fee: u128,
    chain_id: [u8; 32],
    module_id: [u8; 32],
    vk_hash: [u8; 32],
    audit_commitment: [u8; 32],
}

pub fn main() {
    // Read the public inputs from the host
    let public: SpendPublicGuest = env::read();

    // For this demo guest, we enforce no additional constraints here;
    // the module enforces anchor/vk/audit constraints.

    // Commit the bincode-encoded public inputs to the journal so the host can
    // decode them as the canonical SpendPublic type.
    let bytes = bincode::serialize(&public).expect("bincode serialize public");
    env::commit_slice(&bytes);
}

