use std::convert::Infallible;

use alloy_primitives::{Address, U256};
use itertools::Itertools;
use revm::primitives::HashMap;
use revm::state::{Account, EvmStorageSlot};
use revm::DatabaseCommit;
use sov_address::{EthereumAddress, FromVmAddress};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{InfallibleStateAccessor, Spec};

use super::EvmDb;
use crate::{to_rollup_address, to_rollup_balance};

impl<Ws: InfallibleStateAccessor, S: Spec> DatabaseCommit for EvmDb<Ws, S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        changes
            .into_iter()
            .sorted_by_key(|(address, _)| *address) // Sort addresses to avoid non-determinism in ZK
            .for_each(|(address, account)| {
                self.commit_account(address, account).unwrap_infallible();
            });
    }
}

impl<Ws: InfallibleStateAccessor, S: Spec> EvmDb<Ws, S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn commit_account(&mut self, address: Address, account: Account) -> Result<(), Infallible> {
        // TODO figure out what to do when account is destroyed.
        // https://github.com/Sovereign-Labs/sovereign-sdk/issues/425
        if account.is_selfdestructed() {
            todo!("Account destruction not supported")
        }

        let mut db_account = self
            .accounts
            .get(&address, &mut self.state)?
            .unwrap_or_default();

        let mut account_info = account.info;
        let rollup_address: <S as Spec>::Address = to_rollup_address::<S>(address);
        let balance = to_rollup_balance(account_info.balance);

        self.bank_module
            .override_gas_balance(balance, &rollup_address, &mut self.state)?;

        // Set the EVM account balance to 0 - as balances are stored in the bank module.
        account_info.balance = U256::ZERO;

        if let Some(ref code) = account_info.code {
            if !code.is_empty() {
                // TODO: would be good to have a contains_key method on the StateMap that would be optimized, so we can check the hash before storing the code
                self.code
                    .set(&account_info.code_hash, code.bytecode(), &mut self.state)?;
            }
        }

        db_account.0 = account_info;
        self.accounts.set(&address, &db_account, &mut self.state)?;
        self.commit_storage(address, account.storage);

        Ok(())
    }

    fn commit_storage(&mut self, address: Address, storage: HashMap<U256, EvmStorageSlot>) {
        storage
            .into_iter()
            .sorted_by_key(|(key, _)| *key) // Sort keys explicitly to avoid non-determinism.
            .for_each(|(key, value)| {
                let value = value.present_value();
                self.account_storage
                    .set(&(&address, &key), &value, &mut self.state)
                    .unwrap_infallible();
            });
    }
}
