use std::convert::Infallible;

use reth_primitives::Bytes;
use revm::primitives::{Address, Bytecode, B256, U256};
use revm::Database;
use sov_modules_api::StateAccessor;
use sov_state::codec::BcsCodec;

use super::DbAccount;

pub(crate) struct EvmDb<'a, Ws> {
    pub(crate) accounts: sov_modules_api::StateMap<Address, DbAccount, BcsCodec>,
    pub(crate) code: sov_modules_api::StateMap<B256, Bytes, BcsCodec>,
    pub(crate) state: &'a mut Ws,
}

impl<'a, Ws> EvmDb<'a, Ws> {
    pub(crate) fn new(
        accounts: sov_modules_api::StateMap<Address, DbAccount, BcsCodec>,
        code: sov_modules_api::StateMap<B256, Bytes, BcsCodec>,
        state: &'a mut Ws,
    ) -> Self {
        Self {
            accounts,
            code,
            state,
        }
    }
}

impl<'a, Ws: StateAccessor> Database for EvmDb<'a, Ws> {
    type Error = Infallible;

    fn basic(
        &mut self,
        address: Address,
    ) -> Result<Option<revm::primitives::AccountInfo>, Self::Error> {
        let db_account = self.accounts.get(&address, self.state);
        Ok(db_account.map(|acc| acc.info))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        // TODO move to new_raw_with_hash for better performance
        let bytecode = Bytecode::new_raw(self.code.get(&code_hash, self.state).unwrap_or_default());

        Ok(bytecode)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let storage_value: U256 = if let Some(acc) = self.accounts.get(&address, self.state) {
            acc.storage.get(&index, self.state).unwrap_or_default()
        } else {
            U256::default()
        };

        Ok(storage_value)
    }

    fn block_hash(&mut self, _number: U256) -> Result<B256, Self::Error> {
        todo!("block_hash not yet implemented")
    }
}
