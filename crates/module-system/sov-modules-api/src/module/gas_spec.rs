use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use sov_modules_macros::config_value;

use super::Spec;
use crate::Gas;

/// The trait that defines the gas specification for the rollup.
pub trait GasSpec:
    BorshDeserialize + BorshSerialize + Default + Debug + Clone + Send + Sync + PartialEq + 'static
{
    /// The type of gas used in the rollup.
    type Gas: Gas;

    /// The fixed gas price of checking forced sequencer registration transactions.
    /// This price is added to regular transaction checks & execution costs.
    /// This should be set in such a way that forced sequencer registration is more expensive
    /// than regular registration to prevent this mechanism being gamed instead of
    /// used only when users feel they are being censored.
    fn gas_forced_sequencer_registration_cost() -> Self::Gas;

    // --- Gas parameters to charge for state accesses ---

    /// Returns the gas to charge for a read operation. This value is the maximum amount of gas that can be charged
    /// for a read operation. Some of this amount may be refunded to the gas meter if the read operation access a warm value.
    fn gas_to_charge_for_access() -> Self::Gas;
    /// Gas to refund for a read operation. Now this is the value to refund for a read operation that accesses a warm value.
    /// In the future we may want to support more access patterns and improve the granularity of the refund.
    fn gas_to_refund_for_hot_access() -> Self::Gas;

    /// Returns the gas to charge for a write operation. This value is the maximum amount of gas that can be charged
    /// for a write operation. Some of this amount may be refunded to the gas meter if the write operation access a warm value.
    fn gas_to_charge_for_write() -> Self::Gas;
    /// Gas to refund for a write operation. Now this is the value to refund for a write operation that accesses a warm value.
    /// In the future we may want to support more access patterns and improve the granularity of the refund.
    fn gas_to_refund_for_hot_write() -> Self::Gas;

    /// Gas to charge for a delete a cold storage slot
    fn gas_to_charge_for_delete() -> Self::Gas;
    /// Gas to refund for a delete a hot storage slot
    fn gas_to_refund_for_hot_delete() -> Self::Gas;

    /// Gas to charge for decoding a state access
    fn gas_to_charge_for_decoding() -> Self::Gas;

    // --- End Gas parameters to charge for state accesses ---

    // --- Gas parameters to specify how to charge gas for hashing ---
    /// The cost of updating a hash.
    fn gas_to_charge_per_byte_hash_update() -> Self::Gas;
    /// The cost of finalizing a hash.
    fn gas_to_charge_per_byte_hash_finalize() -> Self::Gas;
    // --- End Gas parameters to specify how to charge gas for hashing ---

    // --- Gas parameters to specify how to charge gas for signature verification ---
    /// The cost of verifying a signature per byte of the signature
    fn gas_to_charge_per_byte_signature_verification() -> Self::Gas;
    /// The fixed cost of verifying a signature
    fn fixed_gas_to_charge_per_signature_verification() -> Self::Gas;
    // --- End Gas parameters to specify how to charge gas for signature verification ---

    /// The cost of deserializing a message using Borsh
    fn gas_to_charge_per_byte_borsh_deserialization() -> Self::Gas;

    // --- Gas fee adjustment parameters: See https://eips.ethereum.org/EIPS/eip-1559 for a detailed description ---
    /// The initial gas limit of the rollup.
    fn initial_gas_limit() -> Self::Gas;
    /// The initial "base fee" that every transaction emits when executed.
    fn initial_base_fee_per_gas() -> <Self::Gas as Gas>::Price;
}

impl<S: Spec> GasSpec for S {
    type Gas = S::Gas;

    fn gas_forced_sequencer_registration_cost() -> Self::Gas {
        const GAS_FORCED_SEQUENCER_REGISTRATION_COST: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_FORCED_SEQUENCER_REGISTRATION_COST");

        Self::Gas::from(GAS_FORCED_SEQUENCER_REGISTRATION_COST)
    }

    fn gas_to_charge_for_access() -> Self::Gas {
        const GAS_TO_CHARGE_FOR_ACCESS: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_CHARGE_FOR_ACCESS");

        Self::Gas::from(GAS_TO_CHARGE_FOR_ACCESS)
    }

    fn gas_to_charge_for_decoding() -> Self::Gas {
        const GAS_TO_CHARGE_FOR_DECODING: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_CHARGE_FOR_DECODING");

        Self::Gas::from(GAS_TO_CHARGE_FOR_DECODING)
    }

    fn fixed_gas_to_charge_per_signature_verification() -> Self::Gas {
        const FIXED_GAS_TO_CHARGE_PER_SIGNATURE_VERIFICATION: [u64; config_value!(
            "GAS_DIMENSIONS"
        )] = config_value!("DEFAULT_FIXED_GAS_TO_CHARGE_PER_SIGNATURE_VERIFICATION");

        Self::Gas::from(FIXED_GAS_TO_CHARGE_PER_SIGNATURE_VERIFICATION)
    }

    fn gas_to_charge_for_delete() -> Self::Gas {
        const GAS_TO_CHARGE_FOR_DELETE: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_CHARGE_FOR_WRITE");

        Self::Gas::from(GAS_TO_CHARGE_FOR_DELETE)
    }

    fn gas_to_charge_for_write() -> Self::Gas {
        const GAS_TO_CHARGE_FOR_WRITE: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_CHARGE_FOR_WRITE");

        Self::Gas::from(GAS_TO_CHARGE_FOR_WRITE)
    }

    fn gas_to_charge_per_byte_borsh_deserialization() -> Self::Gas {
        const GAS_TO_CHARGE_PER_BYTE_BORSH_DESERIALIZATION: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("DEFAULT_GAS_TO_CHARGE_PER_BYTE_BORSH_DESERIALIZATION");

        Self::Gas::from(GAS_TO_CHARGE_PER_BYTE_BORSH_DESERIALIZATION)
    }

    fn gas_to_charge_per_byte_hash_finalize() -> Self::Gas {
        const GAS_TO_CHARGE_PER_BYTE_HASH_FINALIZE: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_CHARGE_PER_BYTE_HASH_FINALIZE");

        Self::Gas::from(GAS_TO_CHARGE_PER_BYTE_HASH_FINALIZE)
    }

    fn gas_to_charge_per_byte_hash_update() -> Self::Gas {
        const GAS_TO_CHARGE_PER_BYTE_HASH_UPDATE: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_CHARGE_PER_BYTE_HASH_UPDATE");

        Self::Gas::from(GAS_TO_CHARGE_PER_BYTE_HASH_UPDATE)
    }

    fn gas_to_charge_per_byte_signature_verification() -> Self::Gas {
        const GAS_TO_CHARGE_PER_BYTE_SIGNATURE_VERIFICATION: [u64; config_value!(
            "GAS_DIMENSIONS"
        )] = config_value!("DEFAULT_GAS_TO_CHARGE_PER_BYTE_SIGNATURE_VERIFICATION");

        Self::Gas::from(GAS_TO_CHARGE_PER_BYTE_SIGNATURE_VERIFICATION)
    }

    fn gas_to_refund_for_hot_access() -> Self::Gas {
        const GAS_TO_REFUND_FOR_HOT_ACCESS: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_REFUND_FOR_HOT_ACCESS");

        Self::Gas::from(GAS_TO_REFUND_FOR_HOT_ACCESS)
    }

    fn gas_to_refund_for_hot_delete() -> Self::Gas {
        const GAS_TO_REFUND_FOR_HOT_DELETE: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_REFUND_FOR_HOT_WRITE");

        Self::Gas::from(GAS_TO_REFUND_FOR_HOT_DELETE)
    }

    fn gas_to_refund_for_hot_write() -> Self::Gas {
        const GAS_TO_REFUND_FOR_HOT_WRITE: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("GAS_TO_REFUND_FOR_HOT_WRITE");

        Self::Gas::from(GAS_TO_REFUND_FOR_HOT_WRITE)
    }

    fn initial_base_fee_per_gas() -> <Self::Gas as Gas>::Price {
        const INITIAL_BASE_FEE_PER_GAS: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("INITIAL_BASE_FEE_PER_GAS");

        <Self::Gas as Gas>::Price::from(INITIAL_BASE_FEE_PER_GAS)
    }

    fn initial_gas_limit() -> Self::Gas {
        const INITIAL_GAS_LIMIT: [u64; config_value!("GAS_DIMENSIONS")] =
            config_value!("INITIAL_GAS_LIMIT");

        Self::Gas::from(INITIAL_GAS_LIMIT)
    }
}
