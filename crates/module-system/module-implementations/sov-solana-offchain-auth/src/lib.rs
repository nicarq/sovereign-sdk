use sov_modules_api::{DaSpec, GenesisState, Module, ModuleId, ModuleInfo, NotInstantiable, Spec};

pub mod capabilities;
pub mod utils;

#[derive(Clone, ModuleInfo)]
pub struct SolanaOffchainAuth<S: Spec> {
    #[id]
    pub(crate) id: ModuleId,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

/// Empty module implementation
impl<S: Spec> Module for SolanaOffchainAuth<S> {
    type Spec = S;
    type Config = ();
    type CallMessage = NotInstantiable;
    type Event = ();

    fn genesis(
        &mut self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _config: &Self::Config,
        _state: &mut impl GenesisState<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn call(
        &mut self,
        _message: Self::CallMessage,
        _context: &sov_modules_api::Context<Self::Spec>,
        _state: &mut impl sov_modules_api::TxState<Self::Spec>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
