use sov_evm::Evm;
use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::{AuthorizationData, ProofProcessor, RuntimeAuthenticator};
use sov_modules_api::hooks::{FinalizeHook, SlotHooks};
use sov_modules_api::{DaSpec, DispatchCall, Spec};
use sov_test_utils::runtime::capabilities::SequencerStakeMeter;
use sov_test_utils::{generate_runtime, TestSpec};

generate_runtime! {
    name: TestRuntime,
    modules: [evm: Evm<S>],
    base_fee_recipient: attester_incentives: sov_test_utils::runtime::AttesterIncentives<S, Da>,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S, Da>,
    impl_capabilities: [GasEnforcer, SequencerAuthorization, SequencerRemuneration, RuntimeAuthorization],
    impl_hooks: [ApplyBatchHooks, TxHooks]
}

impl<S: Spec, Da: DaSpec> ProofProcessor<S, Da> for TestRuntime<S, Da> {
    fn process_aggregated_proof(
        &self,
        _proof: sov_modules_api::SerializedAggregatedProof,
        _prover_address: &<S as Spec>::Address,
        _state: &mut sov_modules_api::WorkingSet<S>,
    ) -> sov_modules_api::SovProofOutcome<S, Da> {
        unimplemented!()
    }

    fn process_attestation(
        &self,
        _proof: sov_modules_api::SerializedAttestation,
        _prover_address: &<S as Spec>::Address,
        _state: &mut sov_modules_api::WorkingSet<S>,
    ) -> sov_modules_api::SovProofOutcome<S, Da> {
        unimplemented!()
    }

    fn process_challenge(
        &self,
        _proof: sov_modules_api::SerializedChallenge,
        _transition_num: u64,
        _prover_address: &<S as Spec>::Address,
        _state: &mut sov_modules_api::WorkingSet<S>,
    ) -> sov_modules_api::SovProofOutcome<S, Da> {
        unimplemented!()
    }
}

impl<S: Spec, Da: DaSpec> RuntimeAuthenticator<S> for TestRuntime<S, Da> {
    type Decodable = <Self as DispatchCall>::Decodable;

    type SequencerStakeMeter = SequencerStakeMeter<S::Gas>;

    type AuthorizationData = AuthorizationData<S>;

    fn authenticate(
        &self,
        tx: &sov_modules_api::RawTx,
        pre_exec_ws: &mut sov_modules_api::PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> sov_modules_api::capabilities::AuthenticationResult<
        S,
        Self::Decodable,
        Self::AuthorizationData,
    > {
        let (tx_and_raw_hash, auth_data, runtime_call) =
            sov_evm::authenticate(&tx.data, pre_exec_ws)?;
        let call = TestRuntimeCall::evm(runtime_call);

        Ok((tx_and_raw_hash, auth_data, call))
    }

    fn authenticate_unregistered(
        &self,
        _tx: &sov_modules_api::RawTx,
        _state: &mut sov_modules_api::PreExecWorkingSet<
            S,
            sov_modules_api::UnlimitedGasMeter<<S as Spec>::Gas>,
        >,
    ) -> sov_modules_api::capabilities::AuthenticationResult<
        S,
        Self::Decodable,
        Self::AuthorizationData,
        sov_modules_api::capabilities::UnregisteredAuthenticationError,
    > {
        unimplemented!()
    }
}

impl<S: Spec, Da: DaSpec> FinalizeHook for TestRuntime<S, Da> {
    type Spec = S;

    fn finalize_hook(
        &self,
        root_hash: <Self::Spec as Spec>::VisibleHash,
        state: &mut impl sov_modules_api::AccessoryStateReaderAndWriter,
    ) {
        self.evm.finalize_hook(root_hash, state);
    }
}

impl<S: Spec, Da: DaSpec> SlotHooks for TestRuntime<S, Da> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        pre_state_root: <Self::Spec as Spec>::VisibleHash,
        state: &mut sov_modules_api::VersionedStateReadWriter<
            sov_modules_api::StateCheckpoint<Self::Spec>,
        >,
    ) {
        self.evm.begin_slot_hook(pre_state_root, state);
    }

    fn end_slot_hook(&self, state: &mut sov_modules_api::StateCheckpoint<Self::Spec>) {
        self.evm.end_slot_hook(state);
    }
}

pub(crate) type RT = TestRuntime<TestSpec, MockDaSpec>;
