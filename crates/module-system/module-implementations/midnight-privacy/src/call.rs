use anyhow::{anyhow, ensure, Result};
use schemars::JsonSchema;
use sov_bank::{config_gas_token_id, Coins, IntoPayable};
use sov_modules_api::macros::{serialize, UniversalWallet};
use sov_modules_api::{Context, EventEmitter, Spec, TxState};

use crate::audit::{AuditCiphertext, AuditIndexEntry};
use crate::event::Event;
use crate::merkle;
use crate::state::{Commitment, Hash32, ViewerId};
use crate::verifier::{mock::AcceptAll, SpendVerifier};
use crate::ShieldedPool;

/// Call messages for interacting with the shielded pool.
#[derive(Debug, PartialEq, Eq, Clone, JsonSchema, UniversalWallet)]
#[serialize(Borsh, Serde)]
#[serde(bound = "S: Spec", rename_all = "snake_case")]
#[schemars(bound = "S: Spec", rename = "CallMessage")]
pub enum CallMessage<S: Spec> {
    Deposit {
        token_id: sov_bank::TokenId,
        amount: sov_bank::Amount,
        commitment: Commitment,
    },
    Spend {
        proof: sov_modules_api::SafeVec<u8, 1_000_000>,
        anchor_root: Hash32,
        audit_payloads: sov_modules_api::SafeVec<AuditCiphertext, 32>,
        withdraw_to: Option<S::Address>,
        withdraw: Option<(sov_bank::TokenId, sov_bank::Amount)>,
    },
    Withdraw {
        proof: sov_modules_api::SafeVec<u8, 1_000_000>,
        anchor_root: Hash32,
        to: S::Address,
        token_id: sov_bank::TokenId,
        amount: sov_bank::Amount,
        audit_payloads: sov_modules_api::SafeVec<AuditCiphertext, 32>,
    },
    RegisterViewer { id: ViewerId, pubkey: [u8; 32] },
    GrantViewAccess { tx_ref: Hash32, payload: AuditCiphertext },
}

impl<S: Spec> ShieldedPool<S> {
    pub(crate) fn deposit(
        &mut self,
        token_id: sov_bank::TokenId,
        amount: sov_bank::Amount,
        c: Commitment,
        ctx: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        // Move funds from sender to module escrow
        self.bank
            .transfer(self.id.to_payable(), Coins { token_id, amount }, ctx, state)?;

        // Append commitment and rotate roots window
        let mut tree = self.commitment_tree.get(state)?.unwrap_or_default();
        tree.insert(c)?;
        let new_root = tree.root()?;
        self.commitment_tree.set(&tree, state)?;
        let mut window = self.recent_roots.get(state)?.unwrap_or_default();
        if window.len() >= crate::state::MAX_ROOTS {
            window.remove(0);
        }
        window.push(new_root);
        self.recent_roots.set::<Vec<Hash32>, _>(&window, state)?;
        self.emit_event(state, Event::CommitmentInserted { commitment: c, new_root });
        Ok(())
    }

    pub(crate) fn spend(
        &mut self,
        proof: Vec<u8>,
        anchor_root: Hash32,
        audit_payloads: Vec<AuditCiphertext>,
        withdraw_to: Option<S::Address>,
        withdraw: Option<(sov_bank::TokenId, sov_bank::Amount)>,
        ctx: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        // 1) Anchor validation
        let roots = self.recent_roots.get(state)?.unwrap_or_default();
        ensure!(roots.contains(&anchor_root), "unknown anchor");

        // 2) Verify proof (mock by default, deserialize SpendPublic)
        let vk_hash = self
            .vk_hash
            .get(state)?
            .ok_or_else(|| anyhow!("vk not set"))?;
        let verifier = AcceptAll::default();
        let public = verifier.verify(&proof, vk_hash)?;

        // 3) Public inputs checks
        let domain = self
            .domain
            .get(state)?
            .ok_or_else(|| anyhow!("domain not set"))?;
        ensure!(public.anchor_root == anchor_root, "anchor mismatch");
        ensure!(public.vk_hash == vk_hash, "vk mismatch");
        ensure!(
            merkle::hash_bytes(&bincode::serialize(&audit_payloads).unwrap())
                == public.audit_commitment,
            "audit commitment mismatch"
        );

        // Defensive bounds to mitigate DOS from unusually large public inputs
        ensure!(
            public.nullifiers.len() <= crate::state::MAX_NULLIFIERS_PER_TX,
            "too many nullifiers"
        );
        ensure!(
            public.commitments.len() <= crate::state::MAX_COMMITMENTS_PER_TX,
            "too many commitments"
        );

        // Defensive: ensure no duplicate nullifiers are included within the same spend
        {
            use std::collections::HashSet;
            let mut seen = HashSet::with_capacity(public.nullifiers.len());
            for nf in &public.nullifiers {
                ensure!(seen.insert(*nf), "duplicate nullifier in spend");
            }
        }

        // 4) Nullifiers: no duplicates
        for nf in &public.nullifiers {
            let key = sov_modules_api::HexHash::new(merkle::hash_bytes(&[domain.as_slice(), nf.as_slice()].concat()));
            ensure!(self.nullifiers.get(&key, state)?.is_none(), "nullifier re-use");
        }
        for nf in &public.nullifiers {
            let key = sov_modules_api::HexHash::new(merkle::hash_bytes(&[domain.as_slice(), nf.as_slice()].concat()));
            self.nullifiers.set(&key, &(), state)?;
            self.emit_event(state, Event::NullifierUsed { nf: *nf });
        }

        // 5) Insert new commitments, update root
        let mut tree = self.commitment_tree.get(state)?.unwrap_or_default();
        for c in &public.commitments {
            tree.insert(*c)?;
            let new_root = tree.root()?;
            self.emit_event(state, Event::CommitmentInserted { commitment: *c, new_root });
        }
        let new_root = tree.root()?;
        self.commitment_tree.set(&tree, state)?;
        let mut window = self.recent_roots.get(state)?.unwrap_or_default();
        if window.len() >= crate::state::MAX_ROOTS {
            window.remove(0);
        }
        window.push(new_root);
        self.recent_roots.set::<Vec<Hash32>, _>(&window, state)?;

        // 6) Fee routing (gas token)
        if public.fee > 0 {
            self.bank.transfer_from(
                self.id.to_payable(),
                ctx.sequencer(),
                Coins { token_id: config_gas_token_id(), amount: public.fee.into() },
                state,
            )?;
        }

        // 7) Optional withdraw
        if let (Some(to), Some((token_id, amount))) = (withdraw_to, withdraw) {
            self.bank
                .transfer_from(self.id.to_payable(), &to, Coins { token_id, amount }, state)?;
        }

        // 8) Emit audit payloads and store index entry
        let tx_ref_bytes = merkle::hash_bytes(&proof);
        let tx_ref = tx_ref_bytes;
        let mut count: u16 = 0;
        for a in &audit_payloads {
            let viewer_key = sov_modules_api::HexHash::new(a.viewer_id);
            ensure!(self.viewers.get(&viewer_key, state)?.is_some(), "unknown viewer_id");
            count = count.saturating_add(1);
            self.emit_event(
                state,
                Event::AuditPayloadPublished {
                    viewer_id: a.viewer_id,
                    tx_ref: tx_ref_bytes,
                    epk: a.epk,
                    size: a.ct.len() as u32,
                },
            );
        }
        self.audit_index.set(
            &sov_modules_api::HexHash::new(tx_ref_bytes),
            &AuditIndexEntry {
                count,
                audit_commitment: public.audit_commitment,
            },
            state,
        )?;
        Ok(())
    }

    pub(crate) fn grant_access(
        &mut self,
        tx_ref: Hash32,
        payload: AuditCiphertext,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let viewer_key = sov_modules_api::HexHash::new(payload.viewer_id);
        ensure!(self.viewers.get(&viewer_key, state)?.is_some(), "unknown viewer_id");
        let mut idx = self
            .audit_index
            .get(&sov_modules_api::HexHash::new(tx_ref), state)?
            .ok_or_else(|| anyhow!("unknown tx_ref"))?;
        idx.count = idx.count.saturating_add(1);
        self.audit_index
            .set(&sov_modules_api::HexHash::new(tx_ref), &idx, state)?;
        self.emit_event(
            state,
            Event::AuditPayloadPublished { viewer_id: payload.viewer_id, tx_ref, epk: payload.epk, size: payload.ct.len() as u32 },
        );
        Ok(())
    }
}
