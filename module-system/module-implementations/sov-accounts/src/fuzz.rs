use arbitrary::{Arbitrary, Unstructured};
use sov_modules_api::{CryptoSpec, Hash, Module, Spec, WorkingSet};

use crate::{Account, AccountConfig, Accounts, CallMessage};

impl<'a> Arbitrary<'a> for CallMessage {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let hash = <[u8; 32]>::arbitrary(u)?;
        Ok(Self::UpdatePublicKey(Hash(hash)))
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

impl<'a, S> Arbitrary<'a> for AccountConfig<S>
where
    S: Spec,
    <S::CryptoSpec as CryptoSpec>::PublicKey: Arbitrary<'a>,
{
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        // TODO we might want a dedicated struct that will generate the private key counterpart so
        // payloads can be signed and verified
        Ok(Self {
            pub_keys: u.arbitrary_iter()?.collect::<Result<_, _>>()?,
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
