# Ligetron Adapter for Sovereign SDK

This crate provides an adapter that allows Ligetron to be used as a zkVM backend for Sovereign SDK rollups.

## Overview

Ligetron is a lightweight, scalable zero-knowledge proof system that uses WASM as an intermediate representation. This adapter integrates Ligetron's WebGPU-based prover and verifier with the Sovereign SDK's zkVM interface.

## Features

- **WebGPU-based proving**: Leverages GPU acceleration for efficient proof generation
- **WASM as IR**: Uses WebAssembly as the intermediate representation for programs
- **Post-quantum security**: Based on Ligero-variant SNARKs
- **Privacy support**: Supports private inputs through Ligetron's private-indices mechanism

## Requirements

- WebGPU-capable hardware (supports Dawn/Metal/Vulkan/DX12)
- Ligetron prover and verifier binaries (`webgpu_prover`, `webgpu_verifier`)
- Shader files (typically in `./shader` directory)

## Environment Variables

Set these environment variables to configure the adapter:

```bash
export LIGETRON_PROVER=/path/to/webgpu_prover
export LIGETRON_VERIFIER=/path/to/webgpu_verifier
export LIGETRON_SHADER_PATH=/path/to/ligero-prover/shader
```

## Usage

### In your Rust code:

```rust
use sov_ligetron_adapter::Ligetron;
use sov_rollup_interface::zk::Zkvm;

// Use Ligetron as your zkVM
type MyZkvm = Ligetron;
```

### In your Ligetron WASM program:

Include the provided C/C++ helper to emit public values:

```c
#include "sov_journal.h"

int main() {
    // Your computation here...
    
    // Emit public outputs
    std::vector<uint8_t> public_data = compute_public_outputs();
    sov_emit_public(public_data.data(), public_data.size());
    
    // Verify the digest matches args[0] (enforced in circuit)
    verify_digest_matches_arg0(public_data);
    
    return 0;
}
```

## Architecture

The adapter works by:

1. **Host side**: Packages hints into a private argument, runs the Ligetron prover
2. **Guest side**: Reads hints, computes results, emits public values via stdout
3. **Verification**: Binds public outputs cryptographically using SHA-256 digest verification

## Security Notes

- All hints are passed as private inputs (only their length is visible)
- Public outputs are cryptographically bound to the proof via SHA-256 digest
- Ensure your guest program only emits the intended data as `SOV_JOURNAL_HEX:`
- The digest verification must be enforced within your circuit logic

## Performance

- Adjust the `packing` parameter to trade off between proof size and generation time
- Default packing is 8192, but this can be configured per use case
- GPU performance depends on available WebGPU implementation and hardware
