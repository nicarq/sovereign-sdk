use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use sov_rollup_interface::crypto::CredentialId;
use sov_rollup_interface::da::DaSpec;
#[cfg(feature = "native")]
use sov_rollup_interface::execution_mode::{Native, WitnessGeneration};
use sov_rollup_interface::zk::Zkvm;
use sov_rollup_interface::BasicAddress;
use sov_state::ArrayWitness;

use crate::higher_kinded_types::{Generic, HigherKindedHelper};
use crate::{CryptoSpecExt, GasUnit, Spec};

/// A default implementation of the [`Spec`] trait. Used for testing but can also be a good
/// starting point for implementing a custom rollup.
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(
    serde::Serialize, serde::Deserialize, BorshDeserialize, BorshSerialize, schemars::JsonSchema,
)]
#[serde(bound = "")]
#[schemars(
    rename = "ConfigurableSpec",
    bound = "Da: ::schemars::JsonSchema, InnerZkvm: ::schemars::JsonSchema, OuterZkvm: ::schemars::JsonSchema, CryptoSpec: ::schemars::JsonSchema, Address: ::schemars::JsonSchema, Mode: ::schemars::JsonSchema"
)]
pub struct ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>(
    PhantomData<(Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage)>,
);

impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage> Default
    for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>
{
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<Da: DaSpec, InnerZkvm: Zkvm, OuterZkvm: Zkvm, CryptoSpec, Address, Mode, Storage> Generic
    for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>
{
    type With<K> = ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, K, Storage>;
}

impl<Da: DaSpec, InnerZkvm: Zkvm, OuterZkvm: Zkvm, CryptoSpec, Address, Mode, Storage>
    HigherKindedHelper
    for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>
{
    type Inner = Mode;
}

mod default_impls {

    use std::marker::PhantomData;

    use super::ConfigurableSpec;

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage> Clone
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>
    {
        fn clone(&self) -> Self {
            Self(PhantomData)
        }
    }

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage> PartialEq<Self>
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>
    {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage> Eq
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>
    {
    }

    impl<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage> core::fmt::Debug
        for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Mode, Storage>
    {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(
                f,
                "ConfigurableSpec<{}>",
                std::any::type_name::<(Da, InnerZkvm, OuterZkvm, Mode, Storage)>()
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
        Storage: sov_state::Storage + sov_state::NativeStorage + Send + Sync + 'static,
    > Spec
    for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, WitnessGeneration, Storage>
where
    Address: From<CredentialId>,
{
    type Da = Da;
    type Gas = GasUnit<2>;
    type Address = Address;

    type Storage = Storage;

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
        Storage: sov_state::Storage + sov_state::NativeStorage + Send + Sync + 'static,
    > Spec for ConfigurableSpec<Da, InnerZkvm, OuterZkvm, CryptoSpec, Address, Native, Storage>
where
    Address: From<CredentialId>,
{
    type Da = Da;
    type Gas = GasUnit<2>;
    type Address = Address;

    type Storage = Storage;

    type InnerZkvm = InnerZkvm;
    type OuterZkvm = OuterZkvm;

    type CryptoSpec = CryptoSpec;

    // This TODO is for performance enhancement, not a security concern.
    // TODO: Replace Array witness with an empty struct
    type Witness = ArrayWitness;
}

#[cfg(not(feature = "native"))]
impl<
        Da: DaSpec,
        InnerZkvm: Zkvm,
        OuterZkvm: Zkvm,
        CryptoSpec: CryptoSpecExt,
        Address: BasicAddress,
        Storage: sov_state::Storage + Send + Sync + 'static,
    > Spec
    for ConfigurableSpec<
        Da,
        InnerZkvm,
        OuterZkvm,
        CryptoSpec,
        Address,
        crate::execution_mode::Zk,
        Storage,
    >
where
    Address: From<CredentialId>,
{
    type Da = Da;
    type Address = Address;
    type Gas = GasUnit<2>;

    type Storage = Storage;

    type InnerZkvm = InnerZkvm;
    type OuterZkvm = OuterZkvm;

    type CryptoSpec = CryptoSpec;

    type Witness = ArrayWitness;
}
