use sov_modules_api::{Spec, StateMap, StateValue};
use sov_state::BorshCodec;

/// Configurable constants
pub const MAX_ROOTS: usize = 256;
/// Upper bound on nullifiers a single spend can consume. Keep small for gas/DOS control.
pub const MAX_NULLIFIERS_PER_TX: usize = 64;
/// Upper bound on output commitments a single spend can append.
pub const MAX_COMMITMENTS_PER_TX: usize = 8;

pub type Hash32 = [u8; 32];
pub type Nullifier = [u8; 32];
pub type Commitment = [u8; 32];
pub type DomainTag = [u8; 32];

/// Viewer key id (hash of the public key), and raw X25519 pubkey bytes
pub type ViewerId = [u8; 32];
pub type ViewerPubKey = [u8; 32];

// State struct intentionally omitted; fields are in module struct.
