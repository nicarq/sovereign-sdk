use sov_hyperlane_integration::test_recipient::{
    CallMessage as RecipientCallMessage, TestRecipient,
};
use sov_hyperlane_integration::{HyperlaneAddress, Ism, Mailbox as RawMailbox, MerkleTreeHooks};
use sov_modules_api::capabilities::{
    authenticate, decode_sov_tx, AuthenticationError, AuthenticationOutput,
    BatchFromUnregisteredSequencer, FatalError, TransactionAuthenticator,
    UnregisteredAuthenticationError,
};
use sov_modules_api::sov_universal_wallet::schema::SchemaGenerator;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{DispatchCall, HexHash, ProvableStateReader, Runtime, Spec};
use sov_state::User;
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_bare_runtime, AsUser, TestSpec, TestUser, TransactionTestCase};

pub type Mailbox<S> = RawMailbox<S, TestRecipient<S>>;
pub type S = TestSpec;
pub type RT = TestRuntime<S>;

generate_bare_runtime! {
    name: TestRuntime,
    modules: [mailbox: Mailbox<S>, test_recipient: TestRecipient<S>, merkle_tree_hooks: MerkleTreeHooks<S>],
    operating_mode: sov_modules_api::runtime::OperatingMode::Zk,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::zk::config::MinimalZkGenesisConfig<S>,
    runtime_trait_impl_bounds: [S::Address: HyperlaneAddress],
    kernel_type: sov_test_utils::runtime::BasicKernel<'a, S>
}

/// The input for the runtime's authenticator functionality.
#[derive(std::fmt::Debug, Clone, borsh::BorshDeserialize, borsh::BorshSerialize)]
pub struct AuthenticatorInput(sov_modules_api::RawTx);

impl<S: Spec> TransactionAuthenticator<S> for TestRuntime<S>
where
    S::Address: HyperlaneAddress,
    Transaction<Self, S>: SchemaGenerator,
{
    type Decodable = <Self as DispatchCall>::Decodable;
    type Input = AuthenticatorInput;

    type Signature = sov_modules_api::transaction::TransactionWithoutCall<S>;

    fn decode_serialized_tx(
        &self,
        tx: &sov_modules_api::FullyBakedTx,
    ) -> Result<(Self::Decodable, Self::Signature), FatalError> {
        let tx: AuthenticatorInput = borsh::from_slice(&tx.data)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;
        decode_sov_tx::<_, Self>(&tx.0.data)
    }

    fn authenticate<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        tx: &sov_modules_api::FullyBakedTx,
        pre_exec_ws: &mut Accessor,
    ) -> Result<AuthenticationOutput<S, Self::Decodable>, AuthenticationError> {
        let input: AuthenticatorInput = borsh::from_slice(&tx.data).map_err(|e| {
            sov_modules_api::capabilities::fatal_deserialization_error::<_, S, _>(
                &tx.data,
                e,
                pre_exec_ws,
            )
        })?;

        ::sov_modules_api::capabilities::authenticate::<_, S, Self>(
            &input.0.data,
            &<Self as Runtime<S>>::CHAIN_HASH,
            pre_exec_ws,
        )
    }

    fn compute_tx_hash(
        &self,
        tx: &sov_modules_api::FullyBakedTx,
    ) -> anyhow::Result<sov_modules_api::TxHash> {
        let input: AuthenticatorInput = borsh::from_slice(&tx.data)?;

        Ok(sov_modules_api::runtime::capabilities::calculate_hash(
            &input.0.data,
            &mut sov_modules_api::gas::UnlimitedGasMeter::<S>::default(),
        )?)
    }

    fn authenticate_unregistered<Accessor: ProvableStateReader<User, Spec = S>>(
        &self,
        batch: &BatchFromUnregisteredSequencer,
        pre_exec_ws: &mut Accessor,
    ) -> Result<AuthenticationOutput<S, Self::Decodable>, UnregisteredAuthenticationError> {
        authenticate::<_, S, Self>(
            &batch.tx.data,
            &<Self as Runtime<S>>::CHAIN_HASH,
            pre_exec_ws,
        )
        .map_err(|e| match e {
            AuthenticationError::FatalError(err, hash) => {
                UnregisteredAuthenticationError::FatalError(err, hash)
            }
            AuthenticationError::OutOfGas(err) => UnregisteredAuthenticationError::OutOfGas(err),
        })
    }

    fn add_standard_auth(tx: ::sov_modules_api::RawTx) -> Self::Input {
        AuthenticatorInput(tx)
    }
}

#[allow(clippy::type_complexity)]
pub fn setup() -> (TestRunner<TestRuntime<S>, S>, TestUser<S>, TestUser<S>) {
    let genesis_config = HighLevelZkGenesisConfig::generate_with_additional_accounts(2);

    let admin_account = genesis_config.additional_accounts[0].clone();
    let extra_account = genesis_config.additional_accounts[1].clone();

    let genesis = GenesisConfig::from_minimal_config(genesis_config.clone().into(), (), (), ());

    (
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default()),
        admin_account,
        extra_account,
    )
}

pub fn register_recipient(
    runner: &mut TestRunner<RT, S>,
    user: &TestUser<S>,
    recipient_address: HexHash,
) {
    register_recipient_with_ism(runner, user, recipient_address, Ism::AlwaysTrust);
}

pub fn register_recipient_with_ism(
    runner: &mut TestRunner<RT, S>,
    user: &TestUser<S>,
    recipient_address: HexHash,
    ism: Ism,
) {
    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, TestRecipient<S>>(RecipientCallMessage::Register {
            address: recipient_address,
            ism,
        }),
        assert: Box::new(|result, _| {
            assert!(
                result.tx_receipt.is_successful(),
                "Recipient was not registered successfully"
            );
        }),
    });
}
