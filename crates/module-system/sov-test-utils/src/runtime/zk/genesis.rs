use sov_bank::Bank;
use sov_modules_api::{DaSpec, Genesis, Spec};
use sov_prover_incentives::ProverIncentives;
use sov_sequencer_registry::SequencerRegistry;

/// Minimal genesis configuration for the zk runtime.
pub struct MinimalZkGenesisConfig<S: Spec, Da: DaSpec> {
    /// The sequencer registry config.
    pub sequencer_registry: <SequencerRegistry<S, Da> as Genesis>::Config,
    /// The prover incentives config.
    pub prover_incentives: <ProverIncentives<S, Da> as Genesis>::Config,
    /// The bank config.
    pub bank: <Bank<S> as Genesis>::Config,
}
