use std::marker::PhantomData;

use arbitrary::Arbitrary;
use indexmap::IndexMap;
use sov_modules_api::prelude::arbitrary;

use super::bank::message_generator::BankMessageGenerator;
use super::interface::{CallMessageGenerator, GeneratorState, RandomUniform};
pub trait TransactionGenerator {
    /// Generate a transaction
    fn generate_transaction(&mut self, u: arbitrary::Unstructured<'_>);
}

use sov_bank::Coins;
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};

use super::bank::message_generator::BankAccount;

// TODO: make this extensible by users by adding a generic field
#[derive(Clone, Debug)]
pub struct AccountState<S: Spec> {
    pub balances: Vec<Coins>,
    pub sequencing_bond: Option<u64>,
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
}

impl<S: Spec> From<&AccountState<S>> for BankAccount<S> {
    fn from(value: &AccountState<S>) -> BankAccount<S> {
        BankAccount {
            private_key: value.private_key.clone(),
            balances: value.balances.clone(),
        }
    }
}

/// Allows a state view to update the global state.
pub trait ApplyToAccount<S: Spec> {
    /// Applies any changes to a view onto the global account state
    fn apply_to(self, state: &mut AccountState<S>);
}

pub struct State<S: Spec, M> {
    accounts: IndexMap<S::Address, AccountState<S>>,
    phantom_module: PhantomData<M>,
}
pub struct BankTransactionGenerator<S: Spec> {
    bank_generator: BankMessageGenerator<S>,
}

impl<S: Spec, M: CallMessageGenerator<S>> GeneratorState<S> for State<S, M>
where
    for<'a> M::AccountView: From<&'a AccountState<S>>,
    M::AccountView: ApplyToAccount<S>,
{
    type AccountView = M::AccountView;

    fn get_account(&self, address: S::Address) -> Option<Self::AccountView> {
        self.accounts.get(&address).map(Into::into)
    }

    fn get_random_existing_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        if self.accounts.is_empty() {
            self.generate_account(u)
        } else {
            let idx = usize::less_than(&self.accounts.len(), u)?;
            let (address, account) = self.accounts.get_index(idx).expect("Accounts is nonempty");
            Ok((address.clone(), account.into()))
        }
    }

    fn update_account(&mut self, address: S::Address, account_view: Self::AccountView) {
        assert!(
            self.accounts
                .get_mut(&address)
                .map(|acct| account_view.apply_to(acct))
                .is_some(),
            "Tried to update account that doesn't exist"
        );
    }

    fn generate_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        let private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey =
            Arbitrary::arbitrary(u)?;
        let address: S::Address = (&private_key.pub_key()).into();
        let account = AccountState {
            balances: Vec::new(),
            sequencing_bond: None,
            private_key,
        };
        let view = (&account).into();
        self.accounts.insert(address.clone(), account);
        Ok((address, view))
    }
}
