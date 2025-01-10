use reth_primitives::{Address, TransactionSigned, TxKind, U256};
use reth_rpc_types::transaction::EIP1559TransactionRequest;
use reth_rpc_types::TypedTransactionRequest;
use secp256k1::rand::SeedableRng as _;
use secp256k1::{PublicKey, SecretKey};
use sov_eth_dev_signer::DevSigner;
use sov_evm::{AccountData, EthereumAuthenticator, EvmConfig, RlpEvmTransaction, SpecId};
use sov_modules_api::capabilities::{config_chain_id, TransactionAuthenticator, UniquenessData};
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{CredentialId, EncodeCall, HexHash, RawTx, TxEffect};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{Runtime, TestRunner, ValueSetter, ValueSetterConfig};
use sov_test_utils::{
    SimpleStorageContract, TestUser, TransactionTestCase, TransactionType, TxProcessingError,
    TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
};
use sov_uniqueness::Uniqueness;

use crate::runtime::{GenesisConfig, TestNonceRuntime, RT, S};

pub(crate) struct EvmAccount(SecretKey);

impl EvmAccount {
    pub fn generate() -> Self {
        let mut rng = secp256k1::rand::rngs::StdRng::from_entropy();
        let secret_key = SecretKey::new(&mut rng);
        Self(secret_key)
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_secret_key(secp256k1::SECP256K1, &self.0)
    }

    pub fn address(&self) -> Address {
        reth_primitives::public_key_to_address(self.public_key())
    }

    pub fn sign(&self, tx: TypedTransactionRequest) -> (RlpEvmTransaction, TransactionSigned) {
        let signer = DevSigner::new(vec![self.0]);
        let signed_tx = signer.sign_transaction(tx, self.address()).unwrap();
        let rlp = signed_tx.envelope_encoded().to_vec();
        (RlpEvmTransaction { rlp }, signed_tx)
    }
}

fn generate_default_tx(
    uniqueness: UniquenessData,
    admin: &TestUser<S>,
    evm_account: &EvmAccount,
) -> TransactionType<RT, S> {
    match uniqueness {
        UniquenessData::Nonce(nonce) => {
            let contract = SimpleStorageContract::default();
            let create_contract_tx_request =
                TypedTransactionRequest::EIP1559(EIP1559TransactionRequest {
                    chain_id: config_value!("CHAIN_ID"),
                    nonce,
                    max_priority_fee_per_gas: Default::default(),
                    max_fee_per_gas: U256::from(
                        reth_primitives::constants::MIN_PROTOCOL_BASE_FEE * 2,
                    ),
                    gas_limit: U256::from(1_000_000u64),
                    kind: TxKind::Create,
                    value: Default::default(),
                    input: reth_primitives::Bytes::from(contract.byte_code().to_vec()),
                    access_list: Default::default(),
                });
            let (signed_eth_tx, _) = evm_account.sign(create_contract_tx_request);
            let create_contract_tx = RawTx {
                data: borsh::to_vec(&signed_eth_tx).unwrap(),
            };
            TransactionType::<RT, S>::PreAuthenticated(RT::encode_with_ethereum_auth(
                create_contract_tx,
            ))
        }
        UniquenessData::Generation(generation) => {
            let runtime_msg = <RT as EncodeCall<ValueSetter<S>>>::to_decodable(
                sov_value_setter::CallMessage::SetValue(10),
            );

            let transaction = UnsignedTransaction::new(
                runtime_msg,
                config_chain_id(),
                TEST_DEFAULT_MAX_PRIORITY_FEE,
                TEST_DEFAULT_MAX_FEE,
                generation,
                None,
            );

            let transaction = Transaction::<RT, S>::new_signed_tx(
                admin.private_key(),
                &<TestNonceRuntime<S> as Runtime<S>>::CHAIN_HASH,
                transaction,
            );

            TransactionType::PreAuthenticated(RT::encode_with_standard_auth(RawTx {
                data: borsh::to_vec(&transaction).unwrap(),
            }))
        }
    }
}

fn setup() -> (TestUser<S>, TestRunner<TestNonceRuntime<S>, S>, EvmAccount) {
    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let admin = genesis_config.additional_accounts.first().unwrap().clone();

    let evm_account = EvmAccount::generate();

    let evm_config = EvmConfig {
        data: vec![AccountData {
            address: evm_account.address(),
            balance: U256::from(1000000000),
            code_hash: reth_primitives::KECCAK_EMPTY,
            code: Default::default(),
            nonce: 0,
        }],
        // SHANGHAI instead of LATEST
        // https://github.com/Sovereign-Labs/sovereign-sdk/issues/912
        spec: vec![(0, SpecId::SHANGHAI)].into_iter().collect(),
        ..Default::default()
    };

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        ValueSetterConfig {
            admin: admin.address(),
        },
        evm_config,
    );

    let runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), TestNonceRuntime::default());

    (admin, runner, evm_account)
}

#[test]
fn send_tx_works_nonce() {
    let (admin, mut runner, evm_account) = setup();
    let evm_credential_id = CredentialId(HexHash::new(evm_account.address().into_word().into()));

    runner.query_visible_state(|state| {
        assert_eq!(
            Uniqueness::<S>::default()
                .nonce(&evm_credential_id, state)
                .unwrap_infallible(),
            None,
            "The nonce should not be set"
        );
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Nonce(0), &admin, &evm_account),
        assert: Box::new(move |ctx, state| {
            assert!(ctx.tx_receipt.is_successful());

            assert_eq!(
                Uniqueness::<S>::default()
                    .nonce(&evm_credential_id, state)
                    .unwrap_infallible(),
                Some(1),
                "The nonce should be 1"
            );
        }),
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Nonce(1), &admin, &evm_account),
        assert: Box::new(move |ctx, state| {
            assert!(ctx.tx_receipt.is_successful());
            assert_eq!(
                Uniqueness::<S>::default()
                    .nonce(&evm_credential_id, state)
                    .unwrap_infallible(),
                Some(2),
                "The nonce should be 2"
            );
        }),
    });
}

#[test]
fn send_tx_works_generation() {
    let (admin, mut runner, evm_account) = setup();
    let admin_credential_id: CredentialId = admin.credential_id();

    runner.query_visible_state(|state| {
        assert_eq!(
            Uniqueness::<S>::default()
                .latest_generation(&admin_credential_id, state)
                .unwrap_infallible(),
            0,
            "The generation for a new account should start at 0"
        );
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Generation(0), &admin, &evm_account),
        assert: Box::new(move |ctx, state| {
            assert!(ctx.tx_receipt.is_successful());

            assert_eq!(
                Uniqueness::<S>::default()
                    .latest_generation(&admin_credential_id, state)
                    .unwrap_infallible(),
                0,
                "The latest generation should not change when a transaction of the same generation is sent"
            );
        }),
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Generation(5), &admin, &evm_account),
        assert: Box::new(move |ctx, state| {
            assert!(ctx.tx_receipt.is_successful());
            assert_eq!(
                Uniqueness::<S>::default()
                    .latest_generation(&admin_credential_id, state)
                    .unwrap_infallible(),
                5,
                "The latest generation should update when a transaction with a higher generation is sent"
            );
        }),
    });
}

#[test]
fn send_tx_bad_nonce() {
    let (admin, mut runner, evm_account) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Nonce(5), &admin, &evm_account),
        assert: Box::new(move |ctx, _state| {
            if let TxEffect::Skipped(skipped) = &ctx.tx_receipt {
                assert!(matches!(
                    skipped.error,
                    TxProcessingError::IncorrectNonce(_)
                ));
            } else {
                panic!(
                    "Expected Skipped error, but got a different TxEffect: {:?}",
                    ctx.tx_receipt
                );
            }
        }),
    });
}

#[test]
fn send_tx_bad_generation_duplicate() {
    let (admin, mut runner, evm_account) = setup();

    // initialise generation
    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Generation(5), &admin, &evm_account),
        assert: Box::new(move |ctx, _state| {
            assert!(ctx.tx_receipt.is_successful());
        }),
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Generation(5), &admin, &evm_account),
        assert: Box::new(move |ctx, _state| {
            if let TxEffect::Skipped(skipped) = &ctx.tx_receipt {
                assert!(matches!(
                    skipped.error,
                    TxProcessingError::IncorrectNonce(_)
                ));
            } else {
                panic!(
                    "Expected Skipped error, but got a different TxEffect: {:?}",
                    ctx.tx_receipt
                );
            }
        }),
    });
}

#[test]
fn send_tx_bad_generation_too_old() {
    let (admin, mut runner, evm_account) = setup();

    // initialise generation
    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Generation(10), &admin, &evm_account),
        assert: Box::new(move |ctx, _state| {
            assert!(ctx.tx_receipt.is_successful());
        }),
    });

    runner.execute_transaction(TransactionTestCase {
        input: generate_default_tx(UniquenessData::Generation(0), &admin, &evm_account),
        assert: Box::new(move |ctx, _state| {
            if let TxEffect::Skipped(skipped) = &ctx.tx_receipt {
                assert!(matches!(
                    skipped.error,
                    TxProcessingError::IncorrectNonce(_)
                ));
            } else {
                panic!(
                    "Expected Skipped error, but got a different TxEffect: {:?}",
                    ctx.tx_receipt
                );
            }
        }),
    });
}
