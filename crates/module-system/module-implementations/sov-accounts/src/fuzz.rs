use arbitrary::{Arbitrary, Unstructured};
use sov_modules_api::{CryptoSpec, Module, Spec, StateCheckpoint};

use crate::{Account, AccountConfig, AccountData, Accounts, CallMessage};

impl<'a> Arbitrary<'a> for CallMessage {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self::InsertCredentialId(u.arbitrary()?))
    }
}

impl<'a, S> Arbitrary<'a> for Account<S>
where
    S: Spec,
    S::Address: Arbitrary<'a>,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self {
            addr: u.arbitrary()?,
        })
    }
}

impl<'a, Addr: arbitrary::Arbitrary<'a>> Arbitrary<'a> for AccountData<Addr> {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self {
            credential_id: u.arbitrary()?,
            address: u.arbitrary()?,
        })
    }
}

impl<'a, S> Arbitrary<'a> for AccountConfig<S>
where
    S: Spec,
    S::Address: Arbitrary<'a>,
    <S::CryptoSpec as CryptoSpec>::PublicKey: Arbitrary<'a>,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self {
            accounts: u.arbitrary_iter()?.collect::<Result<_, _>>()?,
        })
    }
}

impl<'a, S> Accounts<S>
where
    S: Spec,
    S::Address: Arbitrary<'a>,
    <S::CryptoSpec as CryptoSpec>::PublicKey: Arbitrary<'a>,
{
    /// Creates an arbitrary set of accounts and stores it under `state`.
    pub fn arbitrary_workset(
        u: &mut Unstructured<'a>,
        state: &mut StateCheckpoint<S::Storage>,
    ) -> arbitrary::Result<Self> {
        let config: AccountConfig<S> = match u.arbitrary() {
            Ok(config) => config,
            Err(e) => return Err(e),
        };
        let accounts = Accounts::default();
        let mut genesis_state = state.to_genesis_state_accessor::<Accounts<S>, S>(&config);

        if accounts.genesis(&config, &mut genesis_state).is_err() {
            return Err(arbitrary::Error::IncorrectFormat);
        };

        Ok(accounts)
    }
}
