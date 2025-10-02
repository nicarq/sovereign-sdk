# zk-poc Module

Stores a `u64` only when accompanied by a valid proof that the value is divisible by 2.

- Call: `SetValue { value: u64, proof: SafeVec<u8, 1_000_000> }`
- Proof: a serialized RISC0 receipt (bincode of `Proof::Full(Receipt)`) when `verify-risc0` is enabled; otherwise tests use a mock bincode of `EvenPublic { value }`.
- Config: set `method_id: [u8; 32]` (the RISC0 code commitment) at genesis.
- Event: `Event::Set { value }` on success.
- Query (native): `query_value()` to fetch the stored value.

Verification modes
- Feature `verify-risc0` (real): Uses `sov-risc0-adapter::Risc0Verifier` to verify the receipt against the configured `method_id`, and parses the journal as `EvenPublic { value }`.
- Default (tests): Deserializes `EvenPublic { value }` from `proof` and enforces that `value % 2 == 0`. This keeps unit tests simple without requiring the RISC0 toolchain.

RISC0 guest contract (expected behavior)
- Input: the candidate `value: u64`.
- Logic: assert `value % 2 == 0`, then write `EvenPublic { value }` to the journal with bincode.
- The host produces a receipt and passes it as `proof`. The module verifies the receipt using the `method_id` set at genesis and checks the journal value matches the call’s `value`.

Supplying the method ID
- Compute the method ID from the ELF of your guest program (via `sov_risc0_adapter::ZkvmHost::code_commitment()` or `risc0_zkvm::compute_image_id`).
- Provide its 32-byte encoding in `ZkPocConfig { method_id: [u8; 32] }` during genesis.
