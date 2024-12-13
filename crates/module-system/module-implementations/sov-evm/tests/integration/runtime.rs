use borsh::{BorshDeserialize, BorshSerialize};
use reth_primitives::TransactionSigned;
use sov_evm::Evm;
use sov_modules_api::capabilities::{AuthorizationData, TransactionAuthenticator};
use sov_modules_api::runtime::Runtime;
use sov_modules_api::transaction::TransactionWithoutCall;
use sov_modules_api::{DispatchCall, ProvableStateReader, RawTx, Spec};
use sov_state::User;
use sov_test_utils::{generate_bare_runtime, TestSpec};

generate_bare_runtime! {
    name: TestRuntime,
    modules: [evm: Evm<S>],
    operating_mode:OperatingMode::Zk,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S>,
    gas_enforcer: bank: sov_test_utils::runtime::Bank<S>,
    runtime_trait_impl_bounds: [EthereumToRollupAddressConverter: TryInto<S::Address>],
    kernel_type: sov_kernels::basic::BasicKernel<'a, S>
}

#[derive(std::fmt::Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum Auth<T = sov_modules_api::RawTx, U = sov_modules_api::RawTx> {
    Evm(T),
    Standard(U),
}

impl<S: Spec> TransactionAuthenticator<S> for TestRuntime<S>
where
    EthereumToRollupAddressConverter: TryInto<S::Address>,
{
    type Decodable = <Self as DispatchCall>::Decodable;

    type AuthorizationData = AuthorizationData<S>;

    type Input = Auth;

    type Signature = Auth<TransactionSigned, TransactionWithoutCall<S>>;

    fn parse_input(
        &self,
        tx: &Self::Input,
    ) -> Result<(Self::Decodable, Self::Signature), sov_modules_api::capabilities::FatalError> {
        match tx {
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
        tx: &Self::Input,
        state: &mut Accessor,
    ) -> Result<
        sov_modules_api::capabilities::AuthenticationOutput<
            S,
            Self::Decodable,
            Self::AuthorizationData,
        >,
        sov_modules_api::capabilities::AuthenticationError,
    > {
        match tx {
            Auth::Evm(tx) => {
                let (tx_and_raw_hash, auth_data, runtime_call) =
                    sov_evm::authenticate::<_, _, EthereumToRollupAddressConverter>(
                        &tx.data, state,
                    )?;
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
    EthereumToRollupAddressConverter: TryInto<S::Address>,
{
    fn add_ethereum_auth(tx: RawTx) -> <Self as TransactionAuthenticator<S>>::Input {
        Auth::Evm(tx)
    }
}

/// A converter from an Ethereum address to a rollup address.
pub struct EthereumToRollupAddressConverter(
    /// The raw bytes of the ethereum address.
    pub [u8; 20],
);

impl From<sov_evm::RethAddress> for EthereumToRollupAddressConverter {
    fn from(address: sov_evm::RethAddress) -> Self {
        Self(address.into())
    }
}

impl<H> TryInto<sov_modules_api::Address<H>> for EthereumToRollupAddressConverter {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<sov_modules_api::Address<H>, Self::Error> {
        anyhow::bail!("Not implemented")
    }
}

pub(crate) type RT = TestRuntime<TestSpec>;
