# 🚀 Quick Start: Testing Ligetron Adapter with Your Binaries

## 📋 **TL;DR - Get Testing in 2 Minutes**

1. **Place your binaries** (choose one option):
   ```bash
   # Option A: Copy to test_binaries/ (recommended)
   cd crates/adapters/ligetron
   mkdir test_binaries
   cp /path/to/your/webgpu_prover test_binaries/
   cp /path/to/your/webgpu_verifier test_binaries/
   cp -r /path/to/your/shader test_binaries/
   
   # Option B: Set environment variables
   export LIGETRON_PROVER=/path/to/your/webgpu_prover
   export LIGETRON_VERIFIER=/path/to/your/webgpu_verifier
   export LIGETRON_SHADER_PATH=/path/to/your/shader
   ```

2. **Run the setup script**:
   ```bash
   ./setup_test_env.sh
   ```

3. **Run tests**:
   ```bash
   # Quick validation
   cargo test --features native test_ligetron_config_validation -- --nocapture
   
   # All tests
   cargo test --features native -- --nocapture
   ```

## 📁 **Directory Structure After Setup**

```
crates/adapters/ligetron/
├── test_binaries/           # ← Place your binaries here
│   ├── webgpu_prover       # Your Ligetron prover binary
│   ├── webgpu_verifier     # Your Ligetron verifier binary
│   └── shader/             # Your shader directory
├── setup_test_env.sh       # ← Run this first
├── TESTING.md              # Detailed testing guide
└── tests/
    ├── native.rs           # Basic tests (no binaries needed)
    ├── integration_with_real_binaries.rs  # Tests with your binaries
    └── mock_ligetron.rs    # Mock implementations
```

## ✅ **What Each Test Does**

### Basic Tests (Always Run)
- ✅ **Adapter functionality**: Host/guest simulation, hint passing
- ✅ **Serialization**: Method IDs, proof packages, type safety
- ✅ **Error handling**: Invalid inputs, edge cases

### Integration Tests (With Your Binaries)
- 🔧 **Binary validation**: Checks if your binaries work
- 🧪 **End-to-end flow**: Proof generation → verification
- 📊 **Performance**: Timing and throughput measurements
- 🔄 **Multiple proofs**: Stress testing with different inputs

## 🎯 **Expected Output**

### ✅ **Success (Binaries Found)**
```bash
$ ./setup_test_env.sh
✅ Found binaries in test_binaries directory
✅ Prover found and executable: ./test_binaries/webgpu_prover
✅ Verifier found and executable: ./test_binaries/webgpu_verifier
✅ Environment configured successfully!

$ cargo test --features native -- --nocapture
✅ Found Ligetron configuration
✅ Prover binary is executable
✅ Verifier binary is executable
✅ Method ID: a1b2c3d4e5f6...
```

### ℹ️ **Partial Success (No Binaries)**
```bash
$ cargo test --features native
ℹ️  Ligetron binaries not configured for testing
   This is fine - other tests will use mocks
Skipping test: Ligetron binaries not found
test result: ok. 15 passed; 0 failed; 3 ignored
```

## 🔧 **Troubleshooting**

### Binary Not Executable
```bash
chmod +x test_binaries/webgpu_prover
chmod +x test_binaries/webgpu_verifier
```

### Missing Shader Directory
```bash
mkdir -p test_binaries/shader
# Copy your shader files here
```

### Environment Variables Not Working
```bash
# Check current values
echo $LIGETRON_PROVER
echo $LIGETRON_VERIFIER

# Reset and try again
unset LIGETRON_PROVER LIGETRON_VERIFIER LIGETRON_SHADER_PATH
./setup_test_env.sh
```

## 🚀 **Next Steps**

1. **Basic validation**: Run `./setup_test_env.sh` to check your setup
2. **Create real WASM program**: Replace `test_data/simple_program.wasm` with actual compiled Ligetron program
3. **Run full test suite**: `cargo test --features native -- --nocapture`
4. **Integration**: Add the adapter to your rollup configuration

## 📞 **Need Help?**

- 📖 **Detailed guide**: See `TESTING.md`
- 🐛 **Issues**: Check test output for specific error messages
- 🔧 **Configuration**: Run `cargo test test_ligetron_config_validation -- --nocapture`

The adapter is designed to work with or without your binaries - basic functionality is always tested with mocks!
