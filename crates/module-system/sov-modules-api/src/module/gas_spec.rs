use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use sov_modules_macros::config_value_private;

use super::Spec;
use crate::{new_constant, Gas};

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

    /// Maximum amount of gas the sequencer can pay for the tx execution. Typically this will be the sum
    /// of authentication (sig check) gas and process_tx_pre_exec_checks_gas.
    fn max_tx_check_costs() -> Self::Gas;

    /// Maximum amount of gas that can be charged for sequencer registration.
    fn max_unregistered_tx_check_costs() -> Self::Gas;

    /// The gas used for the transaction pre-execution checks.
    /// For example nonce checks, context resolution etc..
    fn process_tx_pre_exec_checks_gas() -> Self::Gas;
}

impl<S: Spec> GasSpec for S {
    type Gas = S::Gas;

    fn gas_forced_sequencer_registration_cost() -> Self::Gas {
        new_constant!("GAS_FORCED_SEQUENCER_REGISTRATION_COST", Self::Gas)
    }

    fn gas_to_charge_for_access() -> Self::Gas {
        new_constant!("GAS_TO_CHARGE_FOR_ACCESS", Self::Gas)
    }

    fn gas_to_charge_for_decoding() -> Self::Gas {
        new_constant!("GAS_TO_CHARGE_FOR_DECODING", Self::Gas)
    }

    fn fixed_gas_to_charge_per_signature_verification() -> Self::Gas {
        new_constant!(
            "DEFAULT_FIXED_GAS_TO_CHARGE_PER_SIGNATURE_VERIFICATION",
            Self::Gas
        )
    }

    fn gas_to_charge_for_delete() -> Self::Gas {
        Self::gas_to_charge_for_write()
    }

    fn gas_to_charge_for_write() -> Self::Gas {
        new_constant!("GAS_TO_CHARGE_FOR_WRITE", Self::Gas)
    }

    fn gas_to_charge_per_byte_borsh_deserialization() -> Self::Gas {
        new_constant!(
            "DEFAULT_GAS_TO_CHARGE_PER_BYTE_BORSH_DESERIALIZATION",
            Self::Gas
        )
    }

    fn gas_to_charge_per_byte_hash_finalize() -> Self::Gas {
        new_constant!("GAS_TO_CHARGE_PER_BYTE_HASH_FINALIZE", Self::Gas)
    }

    fn gas_to_charge_per_byte_hash_update() -> Self::Gas {
        new_constant!("GAS_TO_CHARGE_PER_BYTE_HASH_UPDATE", Self::Gas)
    }

    fn gas_to_charge_per_byte_signature_verification() -> Self::Gas {
        new_constant!(
            "DEFAULT_GAS_TO_CHARGE_PER_BYTE_SIGNATURE_VERIFICATION",
            Self::Gas
        )
    }

    fn gas_to_refund_for_hot_access() -> Self::Gas {
        new_constant!("GAS_TO_REFUND_FOR_HOT_ACCESS", Self::Gas)
    }

    fn gas_to_refund_for_hot_delete() -> Self::Gas {
        Self::gas_to_refund_for_hot_write()
    }

    fn gas_to_refund_for_hot_write() -> Self::Gas {
        new_constant!("GAS_TO_REFUND_FOR_HOT_WRITE", Self::Gas)
    }

    fn initial_base_fee_per_gas() -> <Self::Gas as Gas>::Price {
        <Self::Gas as Gas>::Price::from(config_value_private!("INITIAL_BASE_FEE_PER_GAS"))
    }

    fn initial_gas_limit() -> Self::Gas {
        Self::Gas::from(config_value_private!("INITIAL_GAS_LIMIT"))
    }

    fn max_tx_check_costs() -> Self::Gas {
        new_constant!("MAX_SEQUENCER_EXEC_GAS_PER_TX", Self::Gas)
    }

    fn max_unregistered_tx_check_costs() -> Self::Gas {
        new_constant!("MAX_UNREGISTERED_SEQUENCER_EXEC_GAS_PER_TX", Self::Gas)
    }

    fn process_tx_pre_exec_checks_gas() -> Self::Gas {
        new_constant!("PROCESS_TX_PRE_EXEC_GAS", Self::Gas)
    }
}
