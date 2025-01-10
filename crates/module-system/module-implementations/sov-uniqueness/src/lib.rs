#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod capabilities;
mod generations;
mod nonces;
use std::collections::{BTreeMap, HashSet};

use sov_modules_api::{
    Context, CredentialId, DaSpec, Error, GenesisState, Module, ModuleId, ModuleInfo,
    ModuleRestApi, NotInstantiable, Spec, StateMap, StateReader, TxHash, TxState,
};
use sov_state::User;

/// A module responsible for managing nonces on the rollup.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct Uniqueness<S: Spec> {
    /// The ID of the sov-nonces module.
    #[id]
    pub id: ModuleId,

    /// Mapping from a credential id to several generations of buckets.
    #[state]
    pub(crate) generations: StateMap<CredentialId, BTreeMap<u64, HashSet<TxHash>>>,

    /// Mapping from a credential id to a nonce.
    #[state]
    pub(crate) nonces: StateMap<CredentialId, u64>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec> Uniqueness<S> {
    /// Retrieves the nonce for a given credential id.
    pub fn nonce<Reader: StateReader<User>>(
        &self,
        credential_id: &CredentialId,
        state: &mut Reader,
    ) -> Result<Option<u64>, Reader::Error> {
        self.nonces.get(credential_id, state)
    }

    /// Retrieves the latest known generation number for a given credential id.
    pub fn latest_generation<Reader: StateReader<User>>(
        &self,
        credential_id: &CredentialId,
        state: &mut Reader,
    ) -> Result<u64, Reader::Error> {
        self.generations
            .get(credential_id, state)
            .map(|maybe_generations| {
                maybe_generations
                    .unwrap_or_default()
                    .last_key_value()
                    .map(|(k, _)| k.to_owned())
                    .unwrap_or_default()
            })
    }
}

impl<S: Spec> Module for Uniqueness<S> {
    type Spec = S;

    type Config = ();

    type CallMessage = NotInstantiable;

    type Event = ();

    fn genesis(
        &self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        _config: &Self::Config,
        _state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn call(
        &self,
        _msg: Self::CallMessage,
        _context: &Context<S>,
        _state: &mut impl TxState<S>,
    ) -> Result<(), Error> {
        unreachable!()
    }
}
