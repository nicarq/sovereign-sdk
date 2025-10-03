# Midnight Privacy Documentation

Welcome to the Midnight Privacy module documentation. This directory contains comprehensive guides about the shielded pool implementation.

## Available Documentation

### [Shielded Pool Guide](./shielded_pool_guide.md)

The main guide covering:
- **Core Concepts**: Commitments, nullifiers, and the note lifecycle
- **Shielded UTXO Model**: How it compares to Bitcoin's UTXO model
- **Spend Transactions**: Detailed walkthrough of how spending works
- **Privacy Properties**: What information is hidden vs. visible
- **Multi-Recipient Transactions**: Sending to multiple recipients in one transaction
- **Technical Details**: State management, proof systems, and audit support

## Quick Links

### For Developers
- Start with the [Shielded Pool Guide](./shielded_pool_guide.md) to understand the architecture
- Review the test files in `tests/shielded_pool.rs` for usage examples
- Check `src/call.rs` for the implementation of deposit/spend logic

### For Integration
- See the `CallMessage` enum in `src/call.rs` for available operations
- Review `PoolConfig` in `src/lib.rs` for configuration options
- Check `src/verifier/` for proof verification setup

### For Testing
- Use the mock verifier (default) for quick testing without real proofs
- Enable `verify-risc0` feature for integration testing with real zero-knowledge proofs
- Run: `RISC0_PROVER=ipc cargo test -p midnight-privacy -- --nocapture`

## Key Files

```
midnight-privacy/
├── docs/
│   ├── README.md (you are here)
│   └── shielded_pool_guide.md (main documentation)
├── src/
│   ├── lib.rs (module definition)
│   ├── call.rs (deposit/spend implementation)
│   ├── state.rs (state types and constants)
│   ├── verifier/ (proof verification)
│   ├── merkle.rs (commitment tree)
│   └── audit.rs (audit capabilities)
├── tests/
│   └── shielded_pool.rs (integration tests)
└── risc0-methods/ (RISC0 guest programs)
    └── guest-spend/ (ZK circuit for spend transactions)
```

## Contributing

When making changes to the shielded pool:
1. Update relevant documentation in this folder
2. Add or update tests to cover new functionality
3. Ensure privacy properties are maintained
4. Document any changes to the public API

## Further Reading

- **Zcash Protocol**: The original inspiration for shielded transactions
- **RISC0 Documentation**: Understanding the zero-knowledge proof system
- **Sparse Merkle Trees**: The data structure for commitment storage

