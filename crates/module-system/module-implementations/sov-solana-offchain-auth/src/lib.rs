mod capabilities;

/// Blob storage contains only address and vector of blobs
#[derive(Clone, ModuleInfo)]
pub struct SolanaOffchainAuth<S: Spec> {
    /// The ID of blob storage module
    #[id]
    pub(crate) id: ModuleId,
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
