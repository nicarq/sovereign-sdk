# Shielded Pool: Privacy-Preserving Transactions

This document explains how the Shielded Pool module provides privacy-preserving transactions using zero-knowledge proofs, similar to Zcash's shielded transactions.

## Table of Contents

- [Overview](#overview)
- [Core Concepts](#core-concepts)
  - [Commitments](#commitments)
  - [Nullifiers](#nullifiers)
  - [The Note Lifecycle](#the-note-lifecycle)
- [Shielded UTXO Model](#shielded-utxo-model)
- [How Spend Transactions Work](#how-spend-transactions-work)
- [Privacy Properties](#privacy-properties)
- [Multi-Recipient Transactions](#multi-recipient-transactions)
- [Technical Details](#technical-details)

## Overview

The Shielded Pool is a privacy layer that allows users to deposit tokens and perform shielded transactions where the sender, receiver, and amounts are hidden using zero-knowledge proofs. It operates similarly to the UTXO model but with strong privacy guarantees.

## Core Concepts

### Commitments

**Commitments** are cryptographic representations of notes (outputs) in the shielded pool. They serve as hidden records of value that can later be spent.

- **Purpose**: Represent unspent notes (like UTXOs) in a privacy-preserving way
- **Properties**: 
  - Appear as random 32-byte hashes on-chain
  - Hide the amount, owner, and other details
  - Cannot be linked to any specific user
- **Storage**: Added to a Merkle tree, allowing efficient proofs of existence

**Example**: When you deposit 1000 tokens, a commitment `[0xABC123...]` is created and added to the tree.

### Nullifiers

**Nullifiers** are unique identifiers that are revealed when spending a note. They serve to prevent double-spending.

- **Purpose**: Mark notes as spent without revealing which note was spent
- **Properties**:
  - Derived deterministically from the note and a secret key
  - Unlinkable to the original commitment (without the secret)
  - Once revealed, permanently recorded to prevent reuse
- **Storage**: Stored in a set to check for duplicates

**Example**: To spend the note with commitment `[0xABC123...]`, you reveal its nullifier `[0xXYZ789...]`.

### The Note Lifecycle

```
┌─────────────┐
│   Deposit   │
│  (Public)   │
└──────┬──────┘
       │
       ↓
┌──────────────────┐
│   Commitment     │ ← Note is "unspent"
│   Added to Tree  │
└──────┬───────────┘
       │
       │ (Time passes...)
       │
       ↓
┌──────────────────┐
│  Spend Transaction│
│  Reveals Nullifier│ ← Note is now "spent"
└──────┬───────────┘
       │
       ↓
┌──────────────────┐
│  New Commitments │ ← New notes created
│  Added to Tree   │   for recipients
└──────────────────┘
```

## Shielded UTXO Model

The shielded pool operates like Bitcoin's UTXO model, but with privacy:

| **Bitcoin UTXOs** | **Shielded Pool** |
|-------------------|-------------------|
| Create UTXO output | Create commitment |
| Reference UTXO as input | Reveal nullifier |
| **Linkage is PUBLIC** | **Linkage is HIDDEN** |
| Amounts visible | Amounts encrypted |
| Recipients visible | Recipients hidden |

**Key Insight**: A commitment is essentially a "future nullifier" - when you create a commitment (output), you can later spend it by revealing its nullifier (input).

## How Spend Transactions Work

A spend transaction consumes existing notes and creates new ones. Here's what happens:

### 1. **Preparation (Off-chain)**

The user:
- Selects which notes to spend (has their secret keys)
- Chooses an anchor root (recent Merkle tree root)
- Determines output notes (commitments) for recipients
- Computes nullifiers for input notes

### 2. **Proof Generation (Off-chain)**

A zero-knowledge proof is generated that proves:
- ✅ Each nullifier corresponds to a valid commitment in the tree at the anchor root
- ✅ The prover knows the secret keys for those notes
- ✅ Total input value = total output value + fee
- ✅ All computations are correct

**Without revealing**:
- ❌ Which commitments are being spent
- ❌ The amounts involved
- ❌ The secret keys

### 3. **On-chain Verification**

The contract:
1. **Validates the anchor**: Checks that the claimed tree root is recent
2. **Verifies the proof**: Uses RISC0 to verify the zero-knowledge proof
3. **Checks nullifiers**: Ensures none have been used before
4. **Records nullifiers**: Marks them as spent (prevents double-spending)
5. **Adds commitments**: Inserts new output commitments into the tree
6. **Updates tree root**: Computes and stores the new Merkle root
7. **Processes fees**: Transfers fee to the sequencer
8. **Optional withdrawal**: If requested, transfers tokens out of the pool

### Code Flow

```rust
pub(crate) fn spend(
    proof: Vec<u8>,
    anchor_root: Hash32,
    // ...
) -> Result<()> {
    // 1) Anchor validation
    ensure!(roots.contains(&anchor_root), "unknown anchor");
    
    // 2) Verify zero-knowledge proof
    let public = verifier.verify(&proof, vk_hash)?;
    
    // 3) Validate public inputs
    ensure!(public.anchor_root == anchor_root, "anchor mismatch");
    
    // 4) Check and record nullifiers (prevent double-spend)
    for nf in &public.nullifiers {
        ensure!(self.nullifiers.get(&key, state)?.is_none(), "nullifier re-use");
        self.nullifiers.set(&key, &(), state)?;
    }
    
    // 5) Insert new commitments and update tree
    for c in &public.commitments {
        tree.insert(*c)?;
    }
    
    // 6) Process fees and optional withdrawal
    // ...
}
```

## Privacy Properties

### What Observers CAN See (Public Information)

When a spend transaction is executed, the following is visible on-chain:

- ✅ **A transaction occurred**
- ✅ **Number of inputs** - How many nullifiers were revealed (e.g., 2 notes spent)
- ✅ **Number of outputs** - How many commitments were created (e.g., 3 new notes)
- ✅ **Fee amount** - The transaction fee paid
- ✅ **Nullifier values** - The specific nullifiers (but not what they correspond to)
- ✅ **Commitment values** - The specific commitments (but not who they belong to)

### What Observers CANNOT See (Hidden Information)

- ❌ **Sender identity** - Cannot determine who initiated the transaction
- ❌ **Recipient identities** - Cannot determine who receives the outputs
- ❌ **Input-output linkage** - Cannot tell which commitment each nullifier corresponds to
- ❌ **Individual amounts** - Cannot determine the value of each note
- ❌ **Transaction graph** - Cannot build a spending graph like Bitcoin
- ❌ **Total transaction value** - Only the fee is revealed, not the amounts transferred

### Example: Transaction on Blockchain

```
┌───────────────────────────────────────────────┐
│ Transaction #45891                            │
├───────────────────────────────────────────────┤
│ Nullifiers: [0x4F2A..., 0x8B91...]           │  ← 2 inputs consumed
│ Commitments: [0xC3E7..., 0x1D6F..., 0x9A2B...]│  ← 3 outputs created
│ Fee: 100                                      │
│ Anchor Root: 0x7D4C...                        │
│ Proof: [large byte array]                    │
└───────────────────────────────────────────────┘

What everyone sees:
"Someone spent 2 notes and created 3 new notes, paying 100 fee"

What remains hidden:
- Who is "someone"? → Unknown
- Which 2 notes from the tree? → Cannot determine
- Who gets the 3 new notes? → Hidden
- How much in each note? → Secret (could be 1000/500/300 or 5/10/15)
```

## Multi-Recipient Transactions

Like Bitcoin, you can send to multiple recipients in a **single transaction**, which is more efficient and provides better privacy.

### Example: Sending to 5 Recipients

```rust
SpendPublic {
    anchor_root: [tree_root],
    nullifiers: vec![
        [input_note_1],  // Your note #1
        [input_note_2],  // Your note #2
    ],
    commitments: vec![
        [alice_note],    // Recipient 1
        [bob_note],      // Recipient 2  
        [carol_note],    // Recipient 3
        [dave_note],     // Recipient 4
        [eve_note],      // Recipient 5
    ],
    fee: 100,
    // ... ZK proof proves sum(inputs) = sum(outputs) + fee
}
```

### Limits

The module defines limits to control computational costs and prevent DoS attacks:

```rust
/// Maximum nullifiers (inputs) per transaction
pub const MAX_NULLIFIERS_PER_TX: usize = 64;

/// Maximum commitments (outputs) per transaction  
pub const MAX_COMMITMENTS_PER_TX: usize = 8;
```

- **Up to 64 inputs**: Can consolidate many small notes into fewer larger ones
- **Up to 8 outputs**: Can send to 8 recipients in a single transaction

**Note**: These limits are for efficiency - generating zero-knowledge proofs with many inputs/outputs is computationally expensive.

## Technical Details

### State Management

The module maintains several pieces of state:

1. **Commitment Tree**: Sparse Merkle tree containing all commitments ever created
2. **Recent Roots**: Sliding window of the last 256 tree roots (for anchor validation)
3. **Nullifiers Set**: Set of all revealed nullifiers (for double-spend prevention)
4. **Configuration**: Domain tag, verification key hash, fee parameters

### Zero-Knowledge Proof System

The module supports two proof systems:

- **Mock Verifier** (default): For testing, accepts serialized `SpendPublic` as "proof"
- **RISC0 Verifier** (feature flag): Real zero-knowledge proofs using RISC0 zkVM

The RISC0 guest program (`spend.rs`) implements the circuit logic that:
- Validates merkle proofs for input notes
- Computes nullifiers from notes and secret keys
- Checks balance equations
- Outputs the public inputs

### Audit Support

The module includes optional audit capabilities:

- **Viewer Registration**: Auditors can register their public keys
- **Audit Ciphertexts**: Transactions can include encrypted metadata for registered viewers
- **Privacy-Preserving**: Only designated viewers can decrypt audit data

This allows for regulatory compliance while maintaining transaction privacy for regular users.

## Further Reading

- **Zcash Protocol**: The shielded pool design is inspired by Zcash's Sapling/Orchard protocols
- **RISC0**: The zero-knowledge proof system used for verification
- **Sparse Merkle Trees**: The data structure used for commitment storage
- **Nullifier Sets**: How double-spending is prevented in shielded systems

