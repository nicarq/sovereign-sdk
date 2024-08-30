// Much of this code was copy-pasted from reth-evm, and we'd rather keep it as
// similar as possible to upstream than clean it up.
#![allow(clippy::match_same_arms)]

use alloy_eips::eip1559::BaseFeeParams;
use reth_primitives::revm_primitives::{AccountInfo, Address, SpecId, U256};
use serde::{Deserialize, Serialize};
use sov_modules_api::macros::config_value;
use sov_modules_api::{StateMap, StateReader};
use sov_state::{Prefix, User};

pub(crate) mod conversions;
pub(crate) mod db;
mod db_commit;
pub(crate) mod db_init;
/// EVM execution utilities
pub mod executor;
pub(crate) mod primitive_types;

pub use primitive_types::RlpEvmTransaction;
use sov_state::codec::BcsCodec;

/// Stores information about an EVM account and a corresponding account state.
#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
pub struct DbAccount {
    pub(crate) info: AccountInfo,
    pub(crate) storage: StateMap<U256, U256, BcsCodec>,
}

impl DbAccount {
    fn new(parent_prefix: &Prefix, address: Address) -> Self {
        let prefix = Self::create_storage_prefix(parent_prefix, address);
        Self {
            info: Default::default(),
            storage: StateMap::with_codec(prefix, BcsCodec {}),
        }
    }

    pub(crate) fn new_with_info(
        parent_prefix: &Prefix,
        address: Address,
        info: AccountInfo,
    ) -> Self {
        let prefix = Self::create_storage_prefix(parent_prefix, address);
        Self {
            info,
            storage: StateMap::with_codec(prefix, BcsCodec {}),
        }
    }

    /// The account info associated with this db account.
    pub fn account_info(&self) -> &AccountInfo {
        &self.info
    }

    /// Lookup the storage at the specified index for the account.
    pub fn get_storage<Accessor: StateReader<User>>(
        &self,
        index: &U256,
        state: &mut Accessor,
    ) -> Result<Option<U256>, Accessor::Error> {
        self.storage.get(index, state)
    }

    fn create_storage_prefix(parent_prefix: &Prefix, address: Address) -> Prefix {
        let mut prefix = parent_prefix.as_ref().to_vec();
        prefix.extend_from_slice(address.as_slice());
        Prefix::new(prefix)
    }
}

/// EVM Chain configuration
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EvmChainConfig {
    /// Unique chain id
    /// Chains can be registered at <https://github.com/ethereum-lists/chains>.
    pub chain_id: u64,

    /// Limits size of contract code size
    /// By default it is 0x6000 (~25kb).
    pub limit_contract_code_size: Option<usize>,

    /// List of EVM hard forks by block number
    pub spec: Vec<(u64, SpecId)>,

    /// Coinbase where all the fees go
    pub coinbase: Address,

    /// Gas limit for single block
    pub block_gas_limit: u64,

    /// Delta to add to parent block timestamp
    pub block_timestamp_delta: u64,

    /// Base fee params.
    pub base_fee_params: BaseFeeParams,
}

impl Default for EvmChainConfig {
    fn default() -> EvmChainConfig {
        EvmChainConfig {
            chain_id: config_value!("CHAIN_ID"),
            limit_contract_code_size: None,
            spec: vec![(0, SpecId::SHANGHAI)],
            coinbase: Address::ZERO,
            block_gas_limit: reth_primitives::constants::ETHEREUM_BLOCK_GAS_LIMIT,
            block_timestamp_delta: 1,
            base_fee_params: BaseFeeParams::ethereum(),
        }
    }
}
