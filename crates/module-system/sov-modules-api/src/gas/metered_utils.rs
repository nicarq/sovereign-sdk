use std::io;

use digest::consts::U32;
use digest::Digest;
use serde::de::DeserializeOwned;
use sov_rollup_interface::crypto::{SigVerificationError, Signature};
use thiserror::Error;

use crate::gas::traits::{Gas, GasArray, GasMeter, GasMeteringError};
use crate::{GasSpec, Spec};

/// A metered hasher that charges gas for each operation.
/// This data structure should be used in the module system to charge gas when hashing data.
pub struct MeteredHasher<'a, Meter: GasMeter, Hasher: Digest<OutputSize = U32>> {
    inner: Hasher,
    meter: &'a mut Meter,
    gas_to_charge_for_hash_update: <Meter::Spec as Spec>::Gas,
    gas_to_charge_for_hash_finalize: <Meter::Spec as Spec>::Gas,
}

type GasUnit<S> = <S as Spec>::Gas;
type MeteringError<M> = GasMeteringError<GasUnit<<M as GasMeter>::Spec>>;

impl<'a, Meter: GasMeter, Hasher: Digest<OutputSize = U32>> MeteredHasher<'a, Meter, Hasher> {
    /// Create a new metered hasher from a given gas meter with default gas prices [`GasSpec::gas_to_charge_per_byte_hash_update`] and [`GasSpec::gas_to_charge_per_byte_hash_finalize`]
    pub fn new(meter: &'a mut Meter) -> Self {
        Self::new_with_custom_price(
            meter,
            Meter::Spec::gas_to_charge_per_byte_hash_update(),
            Meter::Spec::gas_to_charge_per_byte_hash_finalize(),
        )
    }

    /// Create a new metered hasher from a given gas meter with custom gas prices.
    pub fn new_with_custom_price(
        meter: &'a mut Meter,
        gas_to_charge_for_hash_update: GasUnit<Meter::Spec>,
        gas_to_charge_for_hash_finalize: GasUnit<Meter::Spec>,
    ) -> Self {
        Self {
            inner: Hasher::new(),
            meter,
            gas_to_charge_for_hash_update,
            gas_to_charge_for_hash_finalize,
        }
    }

    /// Update the [`MeteredHasher`] with the given data. Performs the same operation as [`Digest::update`] but charges gas.
    pub fn update(&mut self, data: &[u8]) -> Result<(), MeteringError<Meter>> {
        let total_cost = self
            .gas_to_charge_for_hash_update
            .checked_scalar_product(data.len() as u64)
            .ok_or(GasMeteringError::InvalidLength(
                "Unable to hash data".to_string(),
            ))?;

        self.meter.charge_gas(&total_cost)?;
        self.inner.update(data);
        Ok(())
    }

    /// Finalize the [`MeteredHasher`] and return the hash. Performs the same operation as [`Digest::finalize`] but charges gas.
    pub fn finalize(self) -> Result<[u8; 32], (Self, MeteringError<Meter>)> {
        if let Err(e) = self.meter.charge_gas(&self.gas_to_charge_for_hash_finalize) {
            return Err((self, e));
        };

        let hash = self.inner.finalize();
        Ok(hash.into())
    }

    /// Computes the hash of the given data. Performs the same operation as [`Digest::digest`] but charges gas.
    pub fn digest(data: &[u8], meter: &'a mut Meter) -> Result<[u8; 32], MeteringError<Meter>> {
        let mut hasher = Self::new(meter);
        hasher.update(data)?;
        Self::finalize(hasher).map_err(|(_, e)| e)
    }
}

/// Representation of a signature verification error.
#[derive(Debug, thiserror::Error)]
pub enum MeteredSigVerificationError<GU: Gas> {
    /// The signature is invalid for the provided public key.
    #[error("Invalid signature: {0}")]
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
    pub fn verify<Meter: GasMeter<Spec: Spec<Gas = GU>>>(
        &self,
        pub_key: &Sign::PublicKey,
        msg: &[u8],
        meter: &mut Meter,
    ) -> Result<(), MeteredSigVerificationError<GU>> {
        let fixed_gas_cost = self.fixed_gas_to_charge_per_verification.clone();

        let dynamic_cost = self
            .gas_to_charge_per_byte_for_verification
            .checked_scalar_product(msg.len() as u64)
            .ok_or(MeteredSigVerificationError::GasError(
                GasMeteringError::InvalidLength(
                    "Unable to verify message, gas cost overflows `u64::MAX` value".to_string(),
                ),
            ))?;

        meter
            .charge_gas(&fixed_gas_cost)
            .map_err(MeteredSigVerificationError::GasError)?;

        meter
            .charge_gas(&dynamic_cost)
            .map_err(MeteredSigVerificationError::GasError)?;

        self.inner
            .verify(pub_key, msg)
            .map_err(MeteredSigVerificationError::BadSignature)
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

/// Charges gas for deserialization.
pub trait MeteredBorshDeserialize<S: Spec>: Sized {
    /// Computes the cost to deserialize the given buffer, in Gas.
    fn gas_cost_to_deserialize(
        buf: &[u8],
    ) -> Result<<S as GasSpec>::Gas, MeteredBorshDeserializeError<<S as GasSpec>::Gas>> {
        let deserialization_cost = S::gas_to_charge_per_byte_borsh_deserialization();

        // This is safe to cast here as we don't support platforms where usize > u64.
        let buf_len: u64 = buf.len() as u64;

        total_deserialization_cost::<S>(deserialization_cost, buf_len)
    }

    /// Deserializes a type from a byte slice with the provided gas meter. Charge the [`GasSpec::gas_to_charge_per_byte_borsh_deserialization`]
    /// amount of gas for each byte of the struct to deserialize.
    fn deserialize(
        buf: &mut &[u8],
        meter: &mut impl GasMeter<Spec = S>,
    ) -> Result<Self, MeteredBorshDeserializeError<<S as GasSpec>::Gas>>;

    #[cfg(feature = "native")]
    /// Deserialized a type without charging gas.
    fn unmetered_deserialize(
        buf: &mut &[u8],
    ) -> Result<Self, MeteredBorshDeserializeError<<S as GasSpec>::Gas>>;
}

pub(crate) fn total_deserialization_cost<S: Spec>(
    deserialization_cost: S::Gas,
    buf_len: u64,
) -> Result<S::Gas, MeteredBorshDeserializeError<S::Gas>> {
    deserialization_cost.checked_scalar_product(buf_len).ok_or(
        MeteredBorshDeserializeError::GasError(GasMeteringError::InvalidLength(
            "Deserialization cost overflows `u64::MAX` value".to_string(),
        )),
    )
}
