use digest::consts::U32;
use digest::Digest;
use sov_modules_macros::config_value;

use crate::{Gas, GasMeter, GasMeteringError};

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
    /// Default gas price to charge for each hash update operation. This is a per-byte price and it has to be multiplied by the length of the data.
    pub const DEFAULT_GAS_TO_CHARGE_FOR_HASH_UPDATE: [u64; 2] =
        config_value!("GAS_TO_CHARGE_FOR_HASH_UPDATE");

    /// Default gas price to charge for each hash finalize operation.
    pub const DEFAULT_GAS_TO_CHARGE_FOR_HASH_FINALIZE: [u64; 2] =
        config_value!("GAS_TO_CHARGE_FOR_HASH_FINALIZE");

    /// Create a new metered hasher from a given gas meter with default gas prices [`Self::DEFAULT_GAS_TO_CHARGE_FOR_HASH_UPDATE`] and [`Self::DEFAULT_GAS_TO_CHARGE_FOR_HASH_FINALIZE`]
    pub fn new(meter: &'a mut Meter) -> Self {
        Self::new_with_custom_price(
            meter,
            GU::from_slice(&Self::DEFAULT_GAS_TO_CHARGE_FOR_HASH_UPDATE),
            GU::from_slice(&Self::DEFAULT_GAS_TO_CHARGE_FOR_HASH_FINALIZE),
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
    pub fn digest(data: &[u8], meter: &'a mut Meter) -> Result<[u8; 32], GasMeteringError<GU>> {
        let mut hasher = Self::new(meter);
        hasher.update(data)?;
        Self::finalize(hasher).map_err(|(_, e)| e)
    }
}

#[cfg(test)]
mod test {
    use sha2::Sha256;
    use sov_mock_zkvm::MockZkVerifier;
    use sov_prover_storage_manager::new_orphan_storage;
    use sov_rollup_interface::execution_mode::Native;

    use crate::common::gas::GasArray;
    use crate::default_spec::DefaultSpec;
    use crate::{Gas, GasPrice, GasUnit, MeteredHasher, Spec, WorkingSet};
    type S = DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;

    fn create_working_set(
        remaining_funds: u64,
        gas_price: &<<S as Spec>::Gas as Gas>::Price,
    ) -> WorkingSet<S> {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        WorkingSet::new_with_gas_meter(storage, remaining_funds, gas_price)
    }

    #[test]
    fn test_metered_hasher_happy_path() {
        let gas_to_charge_for_hash_update = GasUnit::<2>::from_slice(&[5, 5]);
        let gas_to_charge_for_hash_finalize = GasUnit::<2>::from_slice(&[2, 2]);

        let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

        let data = [1_u8; 32];

        let remaining_funds = gas_to_charge_for_hash_update
            .clone()
            .scalar_product(data.len() as u64)
            .value(&gas_price)
            + gas_to_charge_for_hash_finalize.value(&gas_price);

        let mut ws = create_working_set(remaining_funds, &gas_price);

        let mut hasher = MeteredHasher::<_, _, Sha256>::new_with_custom_price(
            &mut ws,
            gas_to_charge_for_hash_update,
            gas_to_charge_for_hash_finalize,
        );

        assert!(
            hasher.update(&data).is_ok(),
            "Hasher should be able to update"
        );
        assert!(
            hasher.finalize().is_ok(),
            "Hasher should be able to finalize"
        );
    }

    #[test]
    fn test_metered_hasher_not_enough_gas_to_finalize() {
        let gas_to_charge_for_hash_update = GasUnit::<2>::from_slice(&[5, 5]);
        let gas_to_charge_for_hash_finalize = GasUnit::<2>::from_slice(&[2, 2]);

        let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

        let data = [1_u8; 32];

        let remaining_funds = gas_to_charge_for_hash_update
            .clone()
            .scalar_product(data.len() as u64)
            .value(&gas_price);

        let mut ws = create_working_set(remaining_funds, &gas_price);

        let mut hasher = MeteredHasher::<_, _, Sha256>::new_with_custom_price(
            &mut ws,
            gas_to_charge_for_hash_update,
            gas_to_charge_for_hash_finalize,
        );

        assert!(
            hasher.update(&data).is_ok(),
            "Hasher should be able to update"
        );
        assert!(
            hasher.finalize().is_err(),
            "Hasher should not be able to finalize because it should not have enough gas"
        );
    }

    #[test]
    fn test_metered_hasher_not_enough_gas_to_update() {
        let gas_to_charge_for_hash_update = GasUnit::<2>::from_slice(&[5, 5]);
        let gas_to_charge_for_hash_finalize = GasUnit::<2>::from_slice(&[2, 2]);

        let gas_price = GasPrice::<2>::from_slice(&[1, 1]);

        let data = [1_u8; 32];

        let remaining_funds = gas_to_charge_for_hash_update
            .clone()
            .scalar_product(data.len() as u64 - 1)
            .value(&gas_price);

        let mut ws = create_working_set(remaining_funds, &gas_price);

        let mut hasher = MeteredHasher::<_, _, Sha256>::new_with_custom_price(
            &mut ws,
            gas_to_charge_for_hash_update,
            gas_to_charge_for_hash_finalize,
        );

        assert!(
            hasher.update(&data).is_err(),
            "Hasher should be not able to update because it should not have enough gas"
        );
    }
}
