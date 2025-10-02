use anyhow::{anyhow, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sov_modules_api::macros::{serialize, UniversalWallet};
use sov_modules_api::{Context, EventEmitter, Spec, TxState};
#[cfg(feature = "verify-risc0")]
use sov_risc0_adapter::Risc0Verifier;
#[cfg(feature = "verify-risc0")]
use sov_rollup_interface::zk::CodeCommitment;
#[cfg(feature = "verify-risc0")]
use sov_rollup_interface::zk::ZkVerifier;

use crate::event::Event;
use crate::ZkPoc;

/// Public journal committed by the RISC0 guest: the value that was checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvenPublic {
    pub value: u64,
}

/// Available call messages for the `ZkPoc` module.
#[cfg_attr(
    feature = "arbitrary",
    derive(arbitrary::Arbitrary, proptest_derive::Arbitrary)
)]
#[derive(Debug, PartialEq, Eq, Clone, JsonSchema, UniversalWallet)]
#[serialize(Borsh, Serde)]
#[serde(rename_all = "snake_case")]
pub enum CallMessage {
    /// Sets a new value if the provided proof verifies divisibility by two.
    /// `proof` is a serialized RISC0 receipt (bincode(Proof::Full(Receipt))).
    SetValue { value: u64, proof: sov_modules_api::SafeVec<u8, 1_000_000> },
}

impl<S: Spec> ZkPoc<S> {
    /// Set `value` to `value` if `proof` verifies that `value` is even.
    pub(crate) fn set_value(
        &mut self,
        value: u64,
        proof: sov_modules_api::SafeVec<u8, 1_000_000>,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        // Verify the RISC0 proof against the configured method ID.
        let method_id_bytes = self
            .method_id
            .get(state)?
            .ok_or_else(|| anyhow!("method_id not set"))?;
        #[cfg(feature = "verify-risc0")]
        let method_id = sov_risc0_adapter::Risc0MethodId::decode(&method_id_bytes)
            .map_err(|e| anyhow!("invalid method_id bytes: {e}"))?;

        #[cfg(feature = "verify-risc0")]
        let public: EvenPublic = Risc0Verifier::verify(&proof, &method_id)?;

        #[cfg(not(feature = "verify-risc0"))]
        let public: EvenPublic = bincode::deserialize(&proof)?;

        // Ensure the verified journal matches the requested value.
        let public_value: u64 = public.value;
        if public_value != value {
            return Err(anyhow!("Invalid proof: journal value mismatch"));
        }

        // Optional local check: value even. Real enforcement should be performed in the guest.
        if value % 2 != 0 {
            return Err(anyhow!("Proof program must enforce evenness; value is odd"));
        }

        self.value.set(&value, state)?;
        self.emit_event(state, Event::Set { value });
        Ok(())
    }
}
