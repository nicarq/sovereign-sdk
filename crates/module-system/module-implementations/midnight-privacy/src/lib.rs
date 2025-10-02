mod call;
mod event;
pub mod merkle;
pub mod audit;
pub mod verifier;
pub mod state;

pub use call::CallMessage;
pub use event::Event;
pub use state::*;
pub use verifier::*;

use serde::{Deserialize, Serialize};
use sov_bank::Bank;
use sov_modules_api::{
    Context, DaSpec, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi, Spec, StateMap,
    StateValue, TxState,
};
use sov_state::BorshCodec;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PoolConfig {
    pub domain: state::DomainTag,
    pub vk_hash: state::Hash32,
    #[serde(default)]
    pub fee_bips: u16,
    #[serde(default)]
    pub initial_viewers: Vec<(state::ViewerId, state::ViewerPubKey)>,
}

/// A shielded pool with selective privacy via viewing keys.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct ShieldedPool<S: Spec> {
    #[id]
    pub id: ModuleId,

    #[state]
    pub commitment_tree: StateValue<merkle::OpaqueTree>,
    #[state]
    pub recent_roots: StateValue<Vec<Hash32>>,
    #[state]
    pub nullifiers: StateMap<sov_modules_api::HexHash, (), BorshCodec>,
    #[state]
    pub vk_hash: StateValue<Hash32>,
    #[state]
    pub domain: StateValue<DomainTag>,
    #[state]
    pub viewers: StateMap<sov_modules_api::HexHash, ViewerPubKey, BorshCodec>,
    #[state]
    pub audit_index: StateMap<sov_modules_api::HexHash, audit::AuditIndexEntry, BorshCodec>,

    #[module]
    pub bank: Bank<S>,
}

// Default is derived by macros for tests/queries.

impl<S: Spec> Module for ShieldedPool<S> {
    type Spec = S;
    type Config = PoolConfig;
    type CallMessage = CallMessage<S>;
    type Event = Event;

    fn genesis(
        &mut self,
        _h: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        cfg: &Self::Config,
        st: &mut impl GenesisState<S>,
    ) -> anyhow::Result<()> {
        self.domain.set(&cfg.domain, st)?;
        self.vk_hash.set(&cfg.vk_hash, st)?;
        let empty: Vec<Hash32> = Vec::new();
        self.recent_roots.set::<Vec<Hash32>, _>(&empty, st)?;
        for (id, pk) in &cfg.initial_viewers {
            self.viewers.set(&sov_modules_api::HexHash::new(*id), pk, st)?;
        }
        Ok(())
    }

    fn call(
        &mut self,
        msg: Self::CallMessage,
        ctx: &Context<Self::Spec>,
        st: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        match msg {
            CallMessage::Deposit { token_id, amount, commitment } => {
                self.deposit(token_id, amount, commitment, ctx, st)
            }
            CallMessage::Spend { proof, anchor_root, audit_payloads, withdraw_to, withdraw } => self
                .spend(
                    proof.into(),
                    anchor_root,
                    audit_payloads.into(),
                    withdraw_to,
                    withdraw,
                    ctx,
                    st,
                ),
            CallMessage::Withdraw { proof, anchor_root, to, token_id, amount, audit_payloads } => self
                .spend(
                    proof.into(),
                    anchor_root,
                    audit_payloads.into(),
                    Some(to),
                    Some((token_id, amount)),
                    ctx,
                    st,
                ),
            CallMessage::RegisterViewer { id, pubkey } => {
                self.viewers.set(&sov_modules_api::HexHash::new(id), &pubkey, st)?;
                Ok(())
            }
            CallMessage::GrantViewAccess { tx_ref, payload } => self.grant_access(tx_ref, payload, st),
        }
    }
}
