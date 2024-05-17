use arbitrary::{Arbitrary, Unstructured};
use sov_modules_api::{CredentialId, CryptoSpec, Module, Spec, WorkingSet};

use crate::{Account, AccountConfig, AccountData, Accounts, CallMessage};

impl<'a> Arbitrary<'a> for CallMessage {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let credential_id = <[u8; 32]>::arbitrary(u)?;
        Ok(Self::UpdatePublicKey(CredentialId(credential_id)))
    }
}

impl<'a, S> Arbitrary<'a> for Account<S>
where
    S: Spec,
    S::Address: Arbitrary<'a>,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let addr = u.arbitrary()?;
        let nonce = u.arbitrary()?;
        Ok(Self { addr, nonce })
    }
}

impl<'a, Addr: arbitrary::Arbitrary<'a>> Arbitrary<'a> for AccountData<Addr> {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self {
            credential_id: CredentialId(u.arbitrary()?),
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
    /// Creates an arbitrary set of accounts and stores it under `working_set`.
    pub fn arbitrary_workset(
        u: &mut Unstructured<'a>,
        working_set: &mut WorkingSet<S>,
    ) -> arbitrary::Result<Self> {
        let config: AccountConfig<S> = u.arbitrary()?;
        let accounts = Accounts::default();

        accounts
            .genesis(&config, working_set)
            .map_err(|_| arbitrary::Error::IncorrectFormat)?;

        Ok(accounts)
    }
}
