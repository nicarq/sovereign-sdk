#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod capabilities;
use sov_modules_api::{
    CallResponse, Context, CredentialId, DaSpec, Error, GenesisState, Module, ModuleId, ModuleInfo,
    ModuleRestApi, NotInstantiable, Spec, StateMap, StateReader, TxState,
};
use sov_state::User;

/// A module responsible for managing nonces on the rollup.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct Nonces<S: Spec> {
    /// The ID of the sov-nonces module.
    #[id]
    pub id: ModuleId,

    /// Mapping from a credential id to a nonce.
    #[state]
    pub(crate) nonces: StateMap<CredentialId, u64>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec> Nonces<S> {
    /// Retrieves the nonce for a given credential id.
    pub fn nonce<Reader: StateReader<User>>(
        &self,
        credential_id: &CredentialId,
        state: &mut Reader,
    ) -> Result<Option<u64>, Reader::Error> {
        self.nonces.get(credential_id, state)
    }
}

impl<S: Spec> Module for Nonces<S> {
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
    ) -> Result<CallResponse, Error> {
        unreachable!()
    }
}
