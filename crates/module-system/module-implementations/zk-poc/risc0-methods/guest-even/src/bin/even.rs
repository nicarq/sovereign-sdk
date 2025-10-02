#![no_main]
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let value: u64 = env::read();
    assert!(value % 2 == 0, "Value must be even");
    env::commit_slice(&value.to_le_bytes());
}

