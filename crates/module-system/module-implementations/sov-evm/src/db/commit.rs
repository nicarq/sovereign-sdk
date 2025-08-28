use std::convert::Infallible;

use alloy_primitives::{Address, U256};
use itertools::Itertools;
use revm::primitives::HashMap;
use revm::state::{Account, EvmStorageSlot};
use revm::DatabaseCommit;
use sov_address::{EthereumAddress, FromVmAddress};
use sov_modules_api::{Spec, StateAccessor};

use super::EvmDb;
use crate::db::DbAccount;
use crate::{to_rollup_address, to_rollup_balance};

impl<'a, Ws: StateAccessor, S: Spec> DatabaseCommit for EvmDb<'a, Ws, S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        changes
            .into_iter()
            .sorted_by_key(|(address, _)| *address) // Sort addresses to avoid non-determinism in ZK
            .for_each(|(address, account)| {
                self.commit_account(address, account).unwrap();
            });
    }
}

impl<'a, Ws: StateAccessor, S: Spec> EvmDb<'a, Ws, S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn commit_account(&mut self, address: Address, account: Account) -> Result<(), Infallible> {
        // TODO figure out what to do when account is destroyed.
        // https://github.com/Sovereign-Labs/sovereign-sdk/issues/425
        if account.is_selfdestructed() {
            todo!("Account destruction not supported")
        }

        self.commit_storage(address, account.storage);

        let mut account = account.info;

        self.bank_module
            .override_gas_balance(
                to_rollup_balance(account.balance),
                &to_rollup_address::<S>(address),
                self.state,
            )
            .expect("Failed to override gas balance");
        // Set the EVM account balance to 0 - as balances are stored in the bank module.
        account.balance = U256::ZERO;

        if let Some(ref code) = account.code {
            if !code.is_empty() {
                // TODO: would be good to have a contains_key method on the StateMap that would be optimized, so we can check the hash before storing the code
                self.code
                    .set(&account.code_hash, code.bytecode(), self.state)
                    .expect("Failed to set code");
            }
        }

        self.accounts
            .set(&address, &DbAccount(account), self.state)
            .expect("Failed to set account");

        Ok(())
    }

    fn commit_storage(&mut self, address: Address, storage: HashMap<U256, EvmStorageSlot>) {
        storage
            .into_iter()
            .sorted_by_key(|(key, _)| *key) // Sort keys explicitly to avoid non-determinism.
            .for_each(|(key, value)| {
                let value = value.present_value();
                self.account_storage
                    .set(&(&address, &key), &value, self.state)
                    .expect("Failed to set storage");
            });
    }
}
