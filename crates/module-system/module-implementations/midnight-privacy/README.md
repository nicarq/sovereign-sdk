# `midnight-privacy` — Shielded Pool with Selective Privacy

This module implements a minimal shielded pool with nullifier-based double-spend protection and optional selective disclosure via viewing keys.

- Calls:
  - `Deposit { token_id, amount, commitment }`
  - `Spend { proof, anchor_root, audit_payloads, withdraw_to?, withdraw? }`
  - `Withdraw { proof, anchor_root, to, token_id, amount, audit_payloads }`
  - `RegisterViewer { id, pubkey }`
  - `GrantViewAccess { tx_ref, payload }`

- Events:
  - `CommitmentInserted { commitment, new_root }`
  - `NullifierUsed { nf }`
  - `AuditPayloadPublished { viewer_id, tx_ref, epk, size }`

Notes

- Verification is pluggable. By default, the crate includes a mock verifier that expects the `proof` field to be a bincode-serialized `SpendPublic`. Feature-gated host/guest verifiers can be added behind `verify-groth16` / `verify-risc0`.
- Audit payloads are emitted as events for off-chain indexing and selective disclosure.

Build & test

```
cargo build --bins
cargo test -p midnight-privacy
```
