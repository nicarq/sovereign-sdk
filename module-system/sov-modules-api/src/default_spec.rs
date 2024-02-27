use sov_modules_core::{Address, GasUnit, Spec};
use sov_state::{ArrayWitness, DefaultStorageSpec};

#[cfg(feature = "native")]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(bound = "")]
pub struct DefaultSpec<Zkvm>(std::marker::PhantomData<Zkvm>);

#[cfg(feature = "native")]
mod default_impls {
    use super::DefaultSpec;

    impl<Zkvm> Clone for DefaultSpec<Zkvm> {
        fn clone(&self) -> Self {
            Self(std::marker::PhantomData)
        }
    }

    impl<Zkvm> PartialEq<Self> for DefaultSpec<Zkvm> {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl<Zkvm> core::fmt::Debug for DefaultSpec<Zkvm> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "DefaultSpec<{}>", std::any::type_name::<Zkvm>())
        }
    }
}

#[cfg(feature = "native")]
impl<Zkvm: sov_rollup_interface::zk::Zkvm> Spec for DefaultSpec<Zkvm>
where
    Zkvm::CryptoSpec: sov_modules_core::CryptoSpecExt,
{
    type Address = Address;
    type Gas = GasUnit<2>;

    type Storage = sov_state::ProverStorage<DefaultStorageSpec>;

    type VisibleHash = sov_state::VisibleHash;

    type Zkvm = Zkvm;

    type CryptoSpec = Zkvm::CryptoSpec;

    type Witness = ArrayWitness;
}

#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(bound = "")]
pub struct ZkDefaultSpec<Zkvm>(std::marker::PhantomData<Zkvm>);

impl<Zkvm: sov_rollup_interface::zk::Zkvm> Spec for ZkDefaultSpec<Zkvm>
where
    Zkvm::CryptoSpec: sov_modules_core::CryptoSpecExt,
{
    type Address = Address;
    type Gas = GasUnit<2>;

    type Storage = sov_state::ZkStorage<DefaultStorageSpec>;

    type VisibleHash = sov_state::VisibleHash;

    type Zkvm = Zkvm;

    type CryptoSpec = Zkvm::CryptoSpec;

    type Witness = ArrayWitness;
}

mod default_zk_impls {
    use super::ZkDefaultSpec;

    impl<Zkvm> Clone for ZkDefaultSpec<Zkvm> {
        fn clone(&self) -> Self {
            Self(std::marker::PhantomData)
        }
    }

    impl<Zkvm> PartialEq<Self> for ZkDefaultSpec<Zkvm> {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl<Zkvm> core::fmt::Debug for ZkDefaultSpec<Zkvm> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "DefaultSpec<{}>", std::any::type_name::<Zkvm>())
        }
    }
}
