use sov_rollup_interface::zk::{CryptoSpec, Zkvm};
use sov_state::{ArrayWitness, DefaultStorageSpec};

use crate::{Address, GasUnit, Spec};

#[cfg(feature = "native")]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(bound = "")]
pub struct DefaultSpec<InnerZkvm, OuterZkvm>(std::marker::PhantomData<(InnerZkvm, OuterZkvm)>);

#[cfg(feature = "native")]
mod default_impls {
    use super::DefaultSpec;

    impl<InnerZkvm, OuterZkvm> Clone for DefaultSpec<InnerZkvm, OuterZkvm> {
        fn clone(&self) -> Self {
            Self(std::marker::PhantomData)
        }
    }

    impl<InnerZkvm, OuterZkvm> PartialEq<Self> for DefaultSpec<InnerZkvm, OuterZkvm> {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl<InnerZkvm, OuterZkvm> core::fmt::Debug for DefaultSpec<InnerZkvm, OuterZkvm> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(
                f,
                "DefaultSpec<{}>",
                std::any::type_name::<(InnerZkvm, OuterZkvm)>()
            )
        }
    }
}

#[cfg(feature = "native")]
impl<InnerZkvm: Zkvm, OuterZkvm: Zkvm> Spec for DefaultSpec<InnerZkvm, OuterZkvm>
where
    InnerZkvm::CryptoSpec: crate::CryptoSpecExt,
{
    type Address = Address<<Self::CryptoSpec as CryptoSpec>::Hasher>;
    type Gas = GasUnit<2>;

    type Storage =
        sov_state::ProverStorage<DefaultStorageSpec<<Self::CryptoSpec as CryptoSpec>::Hasher>>;

    type VisibleHash = sov_state::VisibleHash;

    type InnerZkvm = InnerZkvm;
    type OuterZkvm = OuterZkvm;

    type CryptoSpec = InnerZkvm::CryptoSpec;

    type Witness = ArrayWitness;
}

#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(bound = "")]
pub struct ZkDefaultSpec<InnerZkvm, OuterZkvm>(std::marker::PhantomData<(InnerZkvm, OuterZkvm)>);

impl<InnerZkvm: Zkvm, OuterZkvm: Zkvm> Spec for ZkDefaultSpec<InnerZkvm, OuterZkvm>
where
    InnerZkvm::CryptoSpec: crate::CryptoSpecExt,
{
    type Address = Address<<Self::CryptoSpec as CryptoSpec>::Hasher>;
    type Gas = GasUnit<2>;

    type Storage =
        sov_state::ZkStorage<DefaultStorageSpec<<Self::CryptoSpec as CryptoSpec>::Hasher>>;

    type VisibleHash = sov_state::VisibleHash;

    type InnerZkvm = InnerZkvm;
    type OuterZkvm = OuterZkvm;

    type CryptoSpec = InnerZkvm::CryptoSpec;

    type Witness = ArrayWitness;
}

mod default_zk_impls {
    use super::ZkDefaultSpec;

    impl<InnerZkvm, OuterZkvm> Clone for ZkDefaultSpec<InnerZkvm, OuterZkvm> {
        fn clone(&self) -> Self {
            Self(std::marker::PhantomData)
        }
    }

    impl<InnerZkvm, OuterZkvm> PartialEq<Self> for ZkDefaultSpec<InnerZkvm, OuterZkvm> {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl<InnerZkvm, OuterZkvm> core::fmt::Debug for ZkDefaultSpec<InnerZkvm, OuterZkvm> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(
                f,
                "DefaultSpec<{}>",
                std::any::type_name::<(InnerZkvm, OuterZkvm)>()
            )
        }
    }
}
