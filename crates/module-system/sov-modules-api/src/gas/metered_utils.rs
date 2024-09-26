use std::io;

use borsh::BorshDeserialize;
use digest::consts::U32;
use digest::Digest;
use serde::de::DeserializeOwned;
use sov_modules_macros::config_value;
use sov_rollup_interface::crypto::{SigVerificationError, Signature};
use thiserror::Error;

use crate::gas::traits::{Gas, GasMeter, GasMeteringError};
use crate::GasSpec;

/// A metered hasher that charges gas for each operation.
/// This data structure should be used in the module system to charge gas when hashing data.
pub struct MeteredHasher<'a, GU: Gas, Meter: GasMeter<GU>, Hasher: Digest<OutputSize = U32>> {
    inner: Hasher,
    meter: &'a mut Meter,
    gas_to_charge_for_hash_update: GU,
    gas_to_charge_for_hash_finalize: GU,
}

impl<'a, GU: Gas, Meter: GasMeter<GU>, Hasher: Digest<OutputSize = U32>>
    MeteredHasher<'a, GU, Meter, Hasher>
{
    /// Create a new metered hasher from a given gas meter with default gas prices [`GasSpec::gas_to_charge_per_byte_hash_update`] and [`GasSpec::gas_to_charge_per_byte_hash_finalize`]
    pub fn new<Spec: GasSpec<Gas = GU>>(meter: &'a mut Meter) -> Self {
        Self::new_with_custom_price(
            meter,
            Spec::gas_to_charge_per_byte_hash_update(),
            Spec::gas_to_charge_per_byte_hash_finalize(),
        )
    }

    /// Create a new metered hasher from a given gas meter with custom gas prices.
    pub fn new_with_custom_price(
        meter: &'a mut Meter,
        gas_to_charge_for_hash_update: GU,
        gas_to_charge_for_hash_finalize: GU,
    ) -> Self {
        Self {
            inner: Hasher::new(),
            meter,
            gas_to_charge_for_hash_update,
            gas_to_charge_for_hash_finalize,
        }
    }

    /// Update the [`MeteredHasher`] with the given data. Performs the same operation as [`Digest::update`] but charges gas.
    pub fn update(&mut self, data: &[u8]) -> Result<(), GasMeteringError<GU>> {
        self.meter.charge_gas(
            self.gas_to_charge_for_hash_update
                .scalar_product(data.len() as u64),
        )?;
        self.inner.update(data);
        Ok(())
    }

    /// Finalize the [`MeteredHasher`] and return the hash. Performs the same operation as [`Digest::finalize`] but charges gas.
    pub fn finalize(self) -> Result<[u8; 32], (Self, GasMeteringError<GU>)> {
        if let Err(e) = self.meter.charge_gas(&self.gas_to_charge_for_hash_finalize) {
            return Err((self, e));
        };

        let hash = self.inner.finalize();
        Ok(hash.into())
    }

    /// Computes the hash of the given data. Performs the same operation as [`Digest::digest`] but charges gas.
    pub fn digest<Spec: GasSpec<Gas = GU>>(
        data: &[u8],
        meter: &'a mut Meter,
    ) -> Result<[u8; 32], GasMeteringError<GU>> {
        let mut hasher = Self::new::<Spec>(meter);
        hasher.update(data)?;
        Self::finalize(hasher).map_err(|(_, e)| e)
    }
}

/// Representation of a signature verification error.
#[derive(Debug, thiserror::Error)]
pub enum MeteredSigVerificationError<GU: Gas> {
    /// The signature is invalid for the provided public key.
    #[error("Signature verification error: {0}")]
    BadSignature(SigVerificationError),

    /// There is not enough gas to verify the signature.
    #[error("A gas error was raised when trying to verify the signature, {0}")]
    GasError(GasMeteringError<GU>),
}

/// A metered signature that charges gas for signature verification. This is a wrapper around a [`Signature`] struct.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(bound = "GU: serde::Serialize + DeserializeOwned")]
pub struct MeteredSignature<GU: Gas, Sign: Signature> {
    inner: Sign,
    gas_to_charge_per_byte_for_verification: GU,
    fixed_gas_to_charge_per_verification: GU,
}

impl<GU: Gas, Sign: Signature> MeteredSignature<GU, Sign> {
    /// Creates a new [`MeteredSignature`] from a given [`Signature`] with a default gas price.
    pub fn new<Spec: GasSpec<Gas = GU>>(inner: Sign) -> Self {
        Self {
            inner,
            gas_to_charge_per_byte_for_verification:
                Spec::gas_to_charge_per_byte_signature_verification(),
            fixed_gas_to_charge_per_verification:
                Spec::fixed_gas_to_charge_per_signature_verification(),
        }
    }

    /// Creates a new [`MeteredSignature`] from a given [`Signature`] and a gas price.
    pub fn new_with_price(
        inner: Sign,
        fixed_gas_to_charge_per_signature: GU,
        gas_to_charge_per_byte_for_signature: GU,
    ) -> Self {
        Self {
            inner,
            fixed_gas_to_charge_per_verification: fixed_gas_to_charge_per_signature,
            gas_to_charge_per_byte_for_verification: gas_to_charge_per_byte_for_signature,
        }
    }

    /// Verifies a signature with the provided gas meter. This method is a wrapper around [`Signature::verify`].
    pub fn verify(
        &self,
        pub_key: &Sign::PublicKey,
        msg: &[u8],
        meter: &mut impl GasMeter<GU>,
    ) -> Result<(), MeteredSigVerificationError<GU>> {
        let mut fixed_gas_cost = self.fixed_gas_to_charge_per_verification.clone();
        let total_gas_cost = fixed_gas_cost.combine(
            self.gas_to_charge_per_byte_for_verification
                .clone()
                .scalar_product(msg.len() as u64),
        );

        meter
            .charge_gas(total_gas_cost)
            .map_err(|e| MeteredSigVerificationError::GasError(e))?;

        self.inner
            .verify(pub_key, msg)
            .map_err(|e| MeteredSigVerificationError::BadSignature(e))
    }
}

/// Representation of a metered borsh deserialization error.
#[derive(Debug, Error)]
pub enum MeteredBorshDeserializeError<GU: Gas> {
    /// A gas error was raised when trying to deserialize the data.
    #[error("A gas error was raised when trying to deserialize the data, {0}")]
    GasError(GasMeteringError<GU>),
    /// An io error occurred while deserializing the data.
    #[error("IO error: {0}")]
    IOError(io::Error),
}

/// A wrapper around [`BorshDeserialize`] that charges gas for deserialization.
pub trait MeteredBorshDeserialize<GU: Gas>: BorshDeserialize {
    /// The default amount of gas to charge for each byte of struct to deserialize with borsh.
    const DEFAULT_GAS_TO_CHARGE_PER_BYTE_BORSH_DESERIALIZATION: [u64; 2] =
        config_value!("DEFAULT_GAS_TO_CHARGE_PER_BYTE_BORSH_DESERIALIZATION");

    /// Deserializes a value from a byte slice with the provided gas meter. Charge the [`GasSpec::gas_to_charge_per_byte_borsh_deserialization`]
    /// amount of gas for each byte of the struct to deserialize.
    fn deserialize<Spec: GasSpec<Gas = GU>>(
        buf: &mut &[u8],
        meter: &mut impl GasMeter<GU>,
    ) -> Result<Self, MeteredBorshDeserializeError<GU>> {
        let deserialization_cost = Spec::gas_to_charge_per_byte_borsh_deserialization();

        Self::deserialize_with_custom_cost(buf, meter, deserialization_cost)
    }

    /// Deserializes a value from a byte slice with the provided gas meter and custom deserialization cost.
    fn deserialize_with_custom_cost(
        buf: &mut &[u8],
        meter: &mut impl GasMeter<GU>,
        deserialization_cost: GU,
    ) -> Result<Self, MeteredBorshDeserializeError<GU>> {
        meter
            .charge_gas(
                deserialization_cost
                    .clone()
                    .scalar_product(buf.len() as u64),
            )
            .map_err(|e| MeteredBorshDeserializeError::GasError(e))?;

        <Self as BorshDeserialize>::deserialize(buf)
            .map_err(|e| MeteredBorshDeserializeError::IOError(e))
    }
}

impl<T: BorshDeserialize, GU: Gas> MeteredBorshDeserialize<GU> for T {}
