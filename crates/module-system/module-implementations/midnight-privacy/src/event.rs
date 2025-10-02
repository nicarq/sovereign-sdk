use sov_modules_api::macros::serialize;

use crate::state::{Commitment, Hash32, Nullifier, ViewerId};

/// Events streamed by the module
#[derive(Debug, PartialEq, Clone, schemars::JsonSchema)]
#[serialize(Borsh, Serde)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    CommitmentInserted { commitment: Commitment, new_root: Hash32 },
    NullifierUsed { nf: Nullifier },
    AuditPayloadPublished { viewer_id: ViewerId, tx_ref: Hash32, epk: [u8; 32], size: u32 },
}
