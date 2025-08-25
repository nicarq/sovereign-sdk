use crate::runtime::{GenesisConfig, TestRuntime, RT, S};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_consensus::TxEip1559;
use alloy_consensus::TypedTransaction;
use alloy_eips::eip1559::MIN_PROTOCOL_BASE_FEE;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::Bytes;
use alloy_primitives::TxKind;
use alloy_primitives::{Address, U256};
use reth_primitives::TransactionSigned;
use secp256k1::rand::SeedableRng as _;
use secp256k1::{PublicKey, SecretKey};
use sov_eth_dev_signer::Signer;
use sov_evm::{AccountData, EvmConfig, RlpEvmTransaction, SpecId};
use sov_modules_api::macros::config_value;
use sov_modules_api::RawTx;
use sov_modules_api::{CredentialId, HexHash};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::SimpleStorageContract;
use sov_test_utils::TestUser;

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

    pub fn sign(&self, tx: TypedTransaction) -> (RlpEvmTransaction, TransactionSigned) {
        let signer = Signer::new(self.0);
        let signed_tx = signer.sign_transaction(tx).unwrap();
        let rlp = signed_tx.encoded_2718();
        (RlpEvmTransaction { rlp }, signed_tx)
    }
}

pub(crate) fn setup() -> (TestRunner<RT, S>, TestUser<S>, EvmAccount, EvmAccount) {
    let evm_account = EvmAccount::generate();
    let no_balance_account = EvmAccount::generate();
    let rollup_account = TestUser::generate_with_default_balance().add_credential_id(CredentialId(
        HexHash::new(evm_account.address().into_word().into()),
    ));
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts(vec![rollup_account.clone()]);
    let evm_config = EvmConfig {
        data: vec![
            AccountData {
                address: evm_account.address(),
                balance: U256::from(1000000000),
                code_hash: KECCAK_EMPTY,
                code: Default::default(),
                nonce: 0,
            },
            AccountData {
                address: no_balance_account.address(),
                balance: U256::from(0),
                code_hash: KECCAK_EMPTY,
                code: Default::default(),
                nonce: 0,
            },
        ],
        // SHANGHAI instead of LATEST
        // https://github.com/Sovereign-Labs/sovereign-sdk/issues/912
        spec: vec![(0, SpecId::SHANGHAI)].into_iter().collect(),
        ..Default::default()
    };
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), evm_config);
    let runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), TestRuntime::default());

    (runner, rollup_account, evm_account, no_balance_account)
}

pub(crate) fn create_contract_tx(
    nonce: u64,
    contract: &SimpleStorageContract,
    account: &EvmAccount,
) -> RawTx {
    let create_contract_tx_request = TypedTransaction::Eip1559(TxEip1559 {
        chain_id: config_value!("CHAIN_ID"),
        nonce,
        max_priority_fee_per_gas: Default::default(),
        max_fee_per_gas: MIN_PROTOCOL_BASE_FEE as u128 * 2,
        gas_limit: 1_000_000,
        to: TxKind::Create,
        value: Default::default(),
        input: Bytes::from(contract.byte_code().to_vec()),
        access_list: Default::default(),
    });
    let (signed_eth_tx, _) = account.sign(create_contract_tx_request);
    RawTx {
        data: borsh::to_vec(&signed_eth_tx).unwrap(),
    }
}

pub(crate) fn create_set_arg_tx(
    set_arg: u32,
    nonce: u64,
    contract: &SimpleStorageContract,
    contract_addr: Address,
    account: &EvmAccount,
) -> RawTx {
    let set_arg_eth_tx = TypedTransaction::Eip1559(TxEip1559 {
        chain_id: config_value!("CHAIN_ID"),
        nonce,
        max_priority_fee_per_gas: Default::default(),
        max_fee_per_gas: MIN_PROTOCOL_BASE_FEE as u128 * 2,
        gas_limit: 1_000_000,
        to: TxKind::Call(contract_addr),
        value: Default::default(),
        input: Bytes::from(hex::decode(hex::encode(contract.set_call_data(set_arg))).unwrap()),
        access_list: Default::default(),
    });

    let (signed_eth_tx, _) = account.sign(set_arg_eth_tx);
    RawTx {
        data: borsh::to_vec(&signed_eth_tx).unwrap(),
    }
}
