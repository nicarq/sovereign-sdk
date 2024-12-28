use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use sov_rollup_interface::da::DaSpec;
#[cfg(feature = "native")]
use sov_rollup_interface::execution_mode::{Native, WitnessGeneration};
use sov_rollup_interface::zk::{CryptoSpec as CryptoSpecT, Zkvm};
use sov_rollup_interface::{execution_mode, BasicAddress};
use sov_state::{ArrayWitness, DefaultStorageSpec};

use crate::higher_kinded_types::{Generic, HigherKindedHelper};
use crate::{CryptoSpecExt, GasUnit, Spec};

/// A default implementation of the [`Spec`] trait. Used for testing but can also be a good
/// starting point for implementing a custom rollup.
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(
    serde::Serialize, serde::Deserialize, BorshDeserialize, BorshSerialize, schemars::JsonSchema,
)]
#[serde(bound = "")]
pub struct ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode>(
    PhantomData<(Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode)>,
);

impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode> Default
    for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode>
{
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<Da: DaSpec, InnerZkvm: Zkvm, OuterZkvm: Zkvm, CryptoSpec, Address, M> Generic
    for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, M>
{
    type With<K> = ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, K>;
}

impl<Da: DaSpec, InnerZkvm: Zkvm, OuterZkvm: Zkvm, CryptoSpec, Address, M> HigherKindedHelper
    for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, M>
{
    type Inner = M;
}

mod default_impls {

    use std::marker::PhantomData;

    use super::ConfigurableSpec;

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode> Clone
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode>
    {
        fn clone(&self) -> Self {
            Self(PhantomData)
        }
    }

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode> PartialEq<Self>
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode>
    {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode> Eq
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode>
    {
    }

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode> core::fmt::Debug
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode>
    {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(
                f,
                "ConfigurableSpec<{}>",
                std::any::type_name::<(Da, InnerZkvm, OuterZkvm, Mode)>()
            )
        }
    }
}

#[cfg(feature = "native")]
impl<
        Da: DaSpec,
        InnerZkvm: Zkvm,
        OuterZkvm: Zkvm,
        CryptoSpec: CryptoSpecExt,
        Address: BasicAddress,
    > Spec for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, WitnessGeneration>
where
    for<'a> Address: From<&'a <CryptoSpec as CryptoSpecT>::PublicKey>,
{
    type Da = Da;
    type Address = Address;
    type Gas = GasUnit<2>;

    type Storage =
        sov_state::ProverStorage<DefaultStorageSpec<<Self::CryptoSpec as CryptoSpecT>::Hasher>>;

    type InnerZkvm = InnerZkvm;
    type OuterZkvm = OuterZkvm;

    type CryptoSpec = CryptoSpec;

    type Witness = ArrayWitness;
}

#[cfg(feature = "native")]
impl<
        Da: DaSpec,
        InnerZkvm: Zkvm,
        OuterZkvm: Zkvm,
        CryptoSpec: CryptoSpecExt,
        Address: BasicAddress,
    > Spec for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Native>
where
    for<'a> Address: From<&'a <CryptoSpec as CryptoSpecT>::PublicKey>,
{
    type Da = Da;
    type Address = Address;
    type Gas = GasUnit<2>;

    // TODO: Replace ProverStorage with an optimized impl!
    type Storage =
        sov_state::ProverStorage<DefaultStorageSpec<<Self::CryptoSpec as CryptoSpecT>::Hasher>>;

    type InnerZkvm = InnerZkvm;
    type OuterZkvm = OuterZkvm;

    type CryptoSpec = CryptoSpec;

    // TODO: Replace Array witness with an empty struct
    type Witness = ArrayWitness;
}

impl<
        Da: DaSpec,
        InnerZkvm: Zkvm,
        OuterZkvm: Zkvm,
        CryptoSpec: CryptoSpecExt,
        Address: BasicAddress,
    > Spec for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, execution_mode::Zk>
where
    for<'a> Address: From<&'a <CryptoSpec as CryptoSpecT>::PublicKey>,
{
    type Da = Da;
    type Address = Address;
    type Gas = GasUnit<2>;

    type Storage =
        sov_state::ZkStorage<DefaultStorageSpec<<Self::CryptoSpec as CryptoSpecT>::Hasher>>;

    type InnerZkvm = InnerZkvm;
    type OuterZkvm = OuterZkvm;

    type CryptoSpec = CryptoSpec;

    type Witness = ArrayWitness;
}
