use borsh::{BorshDeserialize, BorshSerialize};
use sov_evm::Evm;
use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::{AuthorizationData, TransactionAuthenticator};
use sov_modules_api::hooks::{FinalizeHook, SlotHooks};
use sov_modules_api::{DaSpec, DispatchCall, RawTx, Spec};
use sov_state::Storage;
use sov_test_utils::{generate_bare_runtime, TestSpec};

generate_bare_runtime! {
    name: TestRuntime,
    modules: [evm: Evm<S>],
    operating_mode:OperatingMode::Zk,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S, Da>,
    impl_hooks: [ApplyBatchHooks, TxHooks],
    runtime_trait_impl_bounds: [EthereumToRollupAddressConverter: TryInto<S::Address>]
}

/// A converter from an Ethereum address to a rollup address.
pub struct EthereumToRollupAddressConverter([u8; 20]);

impl From<sov_evm::EvmAddress> for EthereumToRollupAddressConverter {
    fn from(address: sov_evm::EvmAddress) -> Self {
        Self(address.into())
    }
}

impl TryInto<reth_primitives::Address> for EthereumToRollupAddressConverter {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<reth_primitives::Address, Self::Error> {
        Ok(reth_primitives::Address::from(self.0))
    }
}

impl<H> TryInto<sov_modules_api::Address<H>> for EthereumToRollupAddressConverter {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<sov_modules_api::Address<H>, Self::Error> {
        anyhow::bail!("Not implemented")
    }
}

#[derive(std::fmt::Debug, Clone, BorshDeserialize, BorshSerialize)]
pub struct AuthenticatorInput(sov_modules_api::RawTx);

impl<S: Spec, Da: DaSpec> TransactionAuthenticator<S> for TestRuntime<S, Da>
where
    EthereumToRollupAddressConverter: TryInto<S::Address>,
{
    type Decodable = <Self as DispatchCall>::Decodable;

    type AuthorizationData = AuthorizationData<S>;

    type Input = AuthenticatorInput;

    fn authenticate(
        &self,
        tx: &AuthenticatorInput,
        pre_exec_ws: &mut sov_modules_api::PreExecWorkingSet<S>,
    ) -> Result<
        sov_modules_api::capabilities::AuthenticationOutput<
            S,
            Self::Decodable,
            Self::AuthorizationData,
        >,
        sov_modules_api::capabilities::AuthenticationError,
    > {
        let (tx_and_raw_hash, auth_data, runtime_call) =
            sov_evm::authenticate::<_, EthereumToRollupAddressConverter>(&tx.0.data, pre_exec_ws)?;
        let call = TestRuntimeCall::Evm(runtime_call);

        Ok((tx_and_raw_hash, auth_data, call))
    }

    fn authenticate_unregistered(
        &self,
        _tx: &AuthenticatorInput,
        _state: &mut sov_modules_api::PreExecWorkingSet<S>,
    ) -> Result<
        sov_modules_api::capabilities::AuthenticationOutput<
            S,
            Self::Decodable,
            Self::AuthorizationData,
        >,
        sov_modules_api::capabilities::UnregisteredAuthenticationError,
    > {
        unimplemented!()
    }

    fn add_standard_auth(tx: RawTx) -> Self::Input {
        AuthenticatorInput(tx)
    }
}

impl<S: Spec, Da: DaSpec> FinalizeHook for TestRuntime<S, Da> {
    type Spec = S;

    fn finalize_hook(
        &self,
        root_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut impl sov_modules_api::AccessoryStateReaderAndWriter,
    ) {
        self.evm.finalize_hook(root_hash, state);
    }
}

impl<S: Spec, Da: DaSpec> SlotHooks for TestRuntime<S, Da> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut sov_modules_api::StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) {
        self.evm.begin_slot_hook(pre_state_root, state);
        assert!(
            self.evm.block_env(state).unwrap().is_some(),
            "Block env should be set by the begin slot hook"
        );
        assert!(
            self.evm.head(state).unwrap().is_some(),
            "Head should be set by the begin slot hook"
        );
    }

    fn end_slot_hook(
        &self,
        state: &mut sov_modules_api::StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) {
        self.evm.end_slot_hook(state);
    }
}

pub(crate) type RT = TestRuntime<TestSpec, MockDaSpec>;
