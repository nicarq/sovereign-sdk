use borsh::{BorshDeserialize, BorshSerialize};
use reth_primitives::TransactionSigned;
use sov_address::{EthereumAddress, MultiAddressEvm};
use sov_evm::Evm;
use sov_modules_api::capabilities::{AuthorizationData, TransactionAuthenticator};
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::runtime::Runtime;
use sov_modules_api::transaction::TransactionWithoutCall;
use sov_modules_api::{DispatchCall, FullyBakedTx, ProvableStateReader, RawTx, Spec};
use sov_rollup_interface::execution_mode::Native;
use sov_state::User;
use sov_test_utils::{generate_bare_runtime, MockDaSpec, MockZkvm, MockZkvmCryptoSpec};

type EvmTestSpec =
    ConfigurableSpec<MockDaSpec, MockZkvm, MockZkvm, MockZkvmCryptoSpec, MultiAddressEvm, Native>;

generate_bare_runtime! {
    name: TestRuntime,
    modules: [evm: Evm<S>],
    operating_mode:OperatingMode::Zk,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S>,
    gas_enforcer: bank: sov_test_utils::runtime::Bank<S>,
    runtime_trait_impl_bounds: [S::Address: From<EthereumAddress>],
    kernel_type: sov_kernels::basic::BasicKernel<'a, S>
}

#[derive(std::fmt::Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum Auth<T = sov_modules_api::RawTx, U = sov_modules_api::RawTx> {
    Evm(T),
    Standard(U),
}

impl<S: Spec> TransactionAuthenticator<S> for TestRuntime<S>
where
    S::Address: From<EthereumAddress>,
{
    type Decodable = <Self as DispatchCall>::Decodable;

    type AuthorizationData = AuthorizationData<S>;

    type Input = Auth;

    type Signature = Auth<TransactionSigned, TransactionWithoutCall<S>>;

    fn parse_input(
        &self,
        tx: &FullyBakedTx,
    ) -> Result<(Self::Decodable, Self::Signature), sov_modules_api::capabilities::FatalError> {
        let input: Auth = borsh::from_slice(&tx.data).map_err(|e| {
            sov_modules_api::capabilities::FatalError::DeserializationFailed(e.to_string())
        })?;

        match input {
            Auth::Evm(raw_tx) => {
                let (call, tx) = sov_evm::parse_input(&raw_tx.data)?;
                Ok((
                    TestRuntimeCall::Evm(sov_evm::CallMessage { rlp: call }),
                    Auth::Evm(tx),
                ))
            }
            Auth::Standard(raw_tx) => {
                let (call, tx) =
                    sov_modules_api::capabilities::parse_input::<S, Self>(&raw_tx.data)?;
                Ok((call, Auth::Standard(tx)))
            }
        }
    }

    fn authenticate<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        tx: &FullyBakedTx,
        state: &mut Accessor,
    ) -> Result<
        sov_modules_api::capabilities::AuthenticationOutput<
            S,
            Self::Decodable,
            Self::AuthorizationData,
        >,
        sov_modules_api::capabilities::AuthenticationError,
    > {
        let input: Auth = borsh::from_slice(&tx.data).map_err(|e| {
            sov_modules_api::capabilities::fatal_deserialization_error::<_, S, _>(
                &tx.data, e, state,
            )
        })?;

        match input {
            Auth::Evm(tx) => {
                let (tx_and_raw_hash, auth_data, runtime_call) =
                    sov_evm::authenticate::<_, _>(&tx.data, state)?;
                let call = TestRuntimeCall::Evm(runtime_call);

                Ok((tx_and_raw_hash, auth_data, call))
            }
            Auth::Standard(tx) => {
                let (tx_and_raw_hash, auth_data, runtime_call) =
                    sov_modules_api::capabilities::authenticate::<_, S, Self>(
                        &tx.data,
                        &Self::CHAIN_HASH,
                        state,
                    )
                    .unwrap();

                Ok((tx_and_raw_hash, auth_data, runtime_call))
            }
        }
    }

    fn authenticate_unregistered<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        _tx: &Self::Input,
        _state: &mut Accessor,
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
        Auth::Standard(tx)
    }
}

impl<S: Spec> sov_evm::EthereumAuthenticator<S> for TestRuntime<S>
where
    S::Address: From<EthereumAddress>,
{
    fn add_ethereum_auth(tx: RawTx) -> <Self as TransactionAuthenticator<S>>::Input {
        Auth::Evm(tx)
    }
}

pub(crate) type S = EvmTestSpec;
pub(crate) type RT = TestRuntime<EvmTestSpec>;
