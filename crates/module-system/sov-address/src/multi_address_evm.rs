use reth_primitives::revm_primitives::Address;
use sov_modules_api::{Address as SovAddress, BasicAddress, RollupAddress};

use crate::{EthereumAddress, MultiAddress};

pub type MultiAddressEvm = MultiAddress<EthereumAddress>;

impl BasicAddress for MultiAddressEvm {}
impl RollupAddress for MultiAddressEvm {}

// NOTE for people implementing MultiAddress for other VMs:
// Ethereum addresses are 20 bytes, so when we see a 32 byte array, we can safely
// assume that this is our Standard address in bytes. The same assumption might
// not hold for other VM addresses.
impl From<[u8; 28]> for MultiAddressEvm {
    fn from(value: [u8; 28]) -> Self {
        Self::Standard(SovAddress::from(value))
    }
}

// NOTE for people implementing MultiAddress for other VMs:
// Again, as Ethereum addresses are 20 bytes, we can safely implement this method to return byte arrays
// for both underlying address types as its easy to differentiate between them just by checking the length.
// This might not be true for other VMs.
impl AsRef<[u8]> for MultiAddressEvm {
    fn as_ref(&self) -> &[u8] {
        match self {
            MultiAddress::Standard(addr) => addr.as_ref(),
            MultiAddress::Vm(addr) => addr.as_ref(),
        }
    }
}

impl TryFrom<&[u8]> for MultiAddressEvm {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() == 28 {
            return Ok(Self::Standard(sov_modules_api::Address::try_from(value)?));
        } else if value.len() == 20 {
            return Ok(Self::Vm(EthereumAddress::try_from(value)?));
        }

        Err(anyhow::anyhow!("MultiAddressEvm: Invalid address length"))
    }
}

impl std::str::FromStr for MultiAddressEvm {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("0x") {
            Ok(Self::Vm(EthereumAddress::from_str(s)?))
        } else {
            Ok(Self::Standard(sov_modules_api::Address::from_str(s)?))
        }
    }
}

impl From<EthereumAddress> for MultiAddressEvm {
    fn from(value: EthereumAddress) -> Self {
        Self::Vm(value)
    }
}

impl From<Address> for MultiAddressEvm {
    fn from(value: Address) -> Self {
        Self::Vm(EthereumAddress::from(value))
    }
}

#[cfg(test)]
#[cfg(feature = "native")]
mod evm_spec_address_tests {
    use std::str::FromStr;

    use borsh::{BorshDeserialize, BorshSerialize};
    use sha2::Sha256;
    use sov_modules_api::configurable_spec::ConfigurableSpec;
    use sov_modules_api::execution_mode::Native;
    use sov_modules_api::Spec;
    use sov_test_utils::{MockDaSpec, MockZkvm, MockZkvmCryptoSpec};

    use super::*;
    type S = ConfigurableSpec<
        MockDaSpec,
        MockZkvm,
        MockZkvm,
        MockZkvmCryptoSpec,
        MultiAddressEvm,
        Native,
    >;

    #[test]
    fn test_serde_json_multi_address_evm_vm() {
        let address = MultiAddressEvm::Vm(
            EthereumAddress::from_str("0x71334bf1710D12c9f689cC819476fA589F08C64C").unwrap(),
        );
        let serialized = serde_json::to_string(&address).unwrap();
        let deserialized: MultiAddressEvm = serde_json::from_str(&serialized).unwrap();
        assert_eq!(address, deserialized);
    }

    #[test]
    fn test_serde_json_multi_address_evm_standard() {
        let address = MultiAddressEvm::Standard(
            sov_modules_api::Address::<Sha256>::from_str(
                "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv",
            )
            .unwrap(),
        );
        let serialized = serde_json::to_string(&address).unwrap();
        let deserialized: MultiAddressEvm = serde_json::from_str(&serialized).unwrap();
        assert_eq!(address, deserialized);
    }

    #[test]
    fn test_bincode_multi_address_evm_vm() {
        let address = MultiAddressEvm::Vm(
            EthereumAddress::from_str("0x71334bf1710D12c9f689cC819476fA589F08C64C").unwrap(),
        );
        let serialized = bincode::serialize(&address).unwrap();
        let deserialized: MultiAddressEvm = bincode::deserialize(&serialized).unwrap();
        assert_eq!(address, deserialized);
    }

    #[test]
    fn test_bincode_multi_address_evm_standard() {
        let address = MultiAddressEvm::Standard(
            sov_modules_api::Address::<Sha256>::from_str(
                "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv",
            )
            .unwrap(),
        );
        let serialized = bincode::serialize(&address).unwrap();
        let deserialized: MultiAddressEvm = bincode::deserialize(&serialized).unwrap();
        assert_eq!(address, deserialized);
    }

    #[test]
    fn test_borsh_allowed_sequencer() {
        #[derive(BorshSerialize, BorshDeserialize, Debug, PartialEq, Eq)]
        pub struct TypeWithFieldAfterAddress<S: Spec> {
            address: S::Address,
            variable: u64,
        }

        let allowed_sequencer = TypeWithFieldAfterAddress {
            address: MultiAddressEvm::Vm(
                EthereumAddress::from_str("0x71334bf1710D12c9f689cC819476fA589F08C64C").unwrap(),
            ),
            variable: 90000000000,
        };
        let mut serialized: Vec<u8> = Vec::new();
        BorshSerialize::serialize(&allowed_sequencer, &mut serialized).unwrap();
        let deserialized: TypeWithFieldAfterAddress<S> =
            BorshDeserialize::try_from_slice(&serialized).unwrap();
        assert_eq!(allowed_sequencer, deserialized);
    }

    #[test]
    fn test_borsh_multi_address_evm_vm() {
        let spec_address = MultiAddressEvm::Vm(
            EthereumAddress::from_str("0x71334bf1710D12c9f689cC819476fA589F08C64C").unwrap(),
        );
        let mut spec_address_bytes: Vec<u8> = Vec::new();
        BorshSerialize::serialize(&spec_address, &mut spec_address_bytes).unwrap();
        let deserialized: MultiAddressEvm =
            BorshDeserialize::try_from_slice(spec_address_bytes.as_slice()).unwrap();
        assert_eq!(spec_address, deserialized);
    }

    #[test]
    fn test_borsh_multi_address_evm_standard() {
        let standard_address = MultiAddressEvm::Standard(
            sov_modules_api::Address::<Sha256>::from_str(
                "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv",
            )
            .unwrap(),
        );
        let mut spec_address_bytes: Vec<u8> = Vec::new();
        BorshSerialize::serialize(&standard_address, &mut spec_address_bytes).unwrap();
        let deserialized: MultiAddressEvm =
            BorshDeserialize::try_from_slice(spec_address_bytes.as_slice()).unwrap();
        assert_eq!(standard_address, deserialized);
    }
}
