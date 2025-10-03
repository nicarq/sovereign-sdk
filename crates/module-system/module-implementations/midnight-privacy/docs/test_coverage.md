# Test Coverage - Shielded Pool Module

This document tracks the test coverage for the Shielded Pool module, organized by proof type (mock vs. real RISC0 proofs).

## Test Summary

### Mock Proof Tests (Fast, No ZK Overhead)

These tests use serialized `SpendPublic` as "proof" for rapid development and CI.

| Test | Purpose | Status |
|------|---------|--------|
| `deposit_and_spend_with_mock_proof` | Basic deposit → spend flow | ✅ |
| `withdraw_path_emits_bank_transfer` | Withdrawal to transparent address | ✅ |
| `reject_unknown_anchor` | Invalid anchor rejection | ✅ |
| `reject_double_nullifier` | Double-spend prevention | ✅ |
| `register_viewer_and_audit_payloads_and_grant` | Audit functionality | ✅ |

### Real RISC0 Proof Tests (Integration)

These tests use actual zero-knowledge proofs and verify end-to-end functionality.

#### Basic Functionality

| Test | Purpose | Importance | Status |
|------|---------|------------|--------|
| `spend_with_valid_risc0_proof` | Basic spend (1 in → 1 out) | ⭐⭐⭐ | ✅ |
| `spend_reject_vk_mismatch_with_valid_proof` | VK mismatch detection | ⭐⭐⭐ | ✅ |

#### NEW - Critical Real-World Scenarios

| Test | Purpose | Importance | Status |
|------|---------|------------|--------|
| `spend_with_multiple_outputs_real_proof` | **Multi-recipient (1 in → 3 out)** | ⭐⭐⭐ Critical | ✅ NEW |
| `spend_with_multiple_inputs_real_proof` | **Note consolidation (3 in → 1 out)** | ⭐⭐⭐ Critical | ✅ NEW |
| `sequential_spend_note_lifecycle_real_proof` | **Full lifecycle: create → spend** | ⭐⭐⭐ Critical | ✅ NEW |
| `reject_double_spend_with_real_proof` | **Security: nullifier reuse** | ⭐⭐⭐ Critical | ✅ NEW |
| `withdraw_to_transparent_with_real_proof` | **Exit to transparent layer** | ⭐⭐⭐ Critical | ✅ NEW |
| `reject_stale_anchor_with_real_proof` | **Anchor validation** | ⭐⭐ Important | ✅ NEW |
| `spend_with_max_outputs_real_proof` | **Limits: 8 recipients max** | ⭐⭐ Important | ✅ NEW |
| `spend_with_old_anchor_in_window_real_proof` | **Old anchor usage (within window)** | ⭐⭐⭐ Critical | ✅ NEW |

## Coverage Matrix

### Transaction Types

| Type | Mock | Real | Notes |
|------|------|------|-------|
| Deposit | ✅ | ✅ | Via setup in all tests |
| Spend (1→1) | ✅ | ✅ | Basic flow |
| Spend (1→N) | ❌ | ✅ | Multi-recipient |
| Spend (N→1) | ❌ | ✅ | Consolidation |
| Spend (N→M) | ❌ | ❌ | Complex multi-input/output |
| Withdraw | ✅ | ✅ | Exit to transparent |
| Sequential spends | ❌ | ✅ | Note lifecycle |

### Security Tests

| Scenario | Mock | Real | Notes |
|----------|------|------|-------|
| Double-spend | ✅ | ✅ | Nullifier reuse |
| Invalid anchor | ✅ | ✅ | Old/fake root |
| VK mismatch | ❌ | ✅ | Wrong verification key |
| Duplicate nullifiers in tx | ❌ | ❌ | TODO: Add test |
| Exceeds MAX_COMMITMENTS | ❌ | ❌ | TODO: Add test (>8) |
| Exceeds MAX_NULLIFIERS | ❌ | ❌ | TODO: Add test (>64) |

### Edge Cases

| Scenario | Mock | Real | Notes |
|----------|------|------|-------|
| Max outputs (8) | ❌ | ✅ | Boundary test |
| Empty commitments | ✅ | ✅ | Via withdraw test |
| Zero fee | ✅ | ✅ | All current tests |
| Non-zero fee | ❌ | ❌ | TODO: Add test |
| Old anchor (in window) | ❌ | ✅ | Multiple anchors test |

### Audit Features

| Feature | Mock | Real | Notes |
|---------|------|------|-------|
| Viewer registration | ✅ | ❌ | TODO: Add with real proof |
| Audit payload | ✅ | ❌ | TODO: Add with real proof |
| Grant access | ✅ | ❌ | TODO: Add with real proof |

## Running Tests

### Run All Tests (Mock Proofs)
```bash
cargo test -p midnight-privacy
```

### Run Real RISC0 Tests
```bash
# Requires RISC0 toolchain installed
RISC0_PROVER=ipc cargo test -p midnight-privacy -- --nocapture

# Run specific test
RISC0_PROVER=ipc cargo test -p midnight-privacy spend_with_multiple_outputs_real_proof -- --nocapture
```

### Run Only New Tests
```bash
RISC0_PROVER=ipc cargo test -p midnight-privacy -- real_proof --nocapture
```

## Future Test Additions

### High Priority

1. **Fee handling with real proof**
   - Test non-zero fees are properly extracted
   - Verify sequencer receives fee payment

2. **Complex multi-input/multi-output (N→M)**
   - Realistic: Spend 3 notes to send to 5 recipients + change

3. **Exceeds limit tests**
   - Test rejection when >8 commitments
   - Test rejection when >64 nullifiers

### Medium Priority

4. **Audit with real proofs**
   - Viewer registration + audit payloads with real proof
   - Grant access functionality with real proof

5. **Anchor window tests**
   - Test with multiple roots in the window
   - Test anchor expiration (>256 roots)

6. **Duplicate nullifiers within transaction**
   - Ensure duplicate check works with real proofs

### Nice to Have

7. **Performance benchmarks**
   - Measure proof generation time vs. number of inputs/outputs
   - Measure on-chain verification time

8. **Stress tests**
   - Many sequential transactions
   - Large tree with many commitments

9. **Integration scenarios**
   - Multiple users interacting
   - Complex spending patterns

## Test Maintenance

- All tests use `#[serial]` to avoid conflicts with RISC0 proof generation
- Each real proof test checks for `SPEND_ELF.is_empty()` and skips if guest not built
- Tests use unique `domain` values to avoid cross-test contamination
- All tests verify both transaction success and emitted events

## Notes

- **Real proof tests are slow**: Each proof generation can take 10-60 seconds depending on hardware
- **Mock tests are fast**: Use for rapid iteration during development
- **CI considerations**: Real proof tests may need to run on dedicated hardware or be optional
- **Coverage gaps**: See "Future Test Additions" section for areas needing more tests

