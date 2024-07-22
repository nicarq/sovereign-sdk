#[cfg(feature = "test-utils")]
use crate::GasArray;
use crate::{Gas, GasMeter, GasMeteringError};

/// A gas meter for transaction execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TxGasMeter<GU>
where
    GU: Gas,
{
    pub(super) remaining_funds: u64,
    pub(super) gas_price: GU::Price,
    pub(super) gas_used: GU,
}

impl<GU> GasMeter<GU> for TxGasMeter<GU>
where
    GU: Gas,
{
    /// Returns the total gas incurred.
    fn gas_used(&self) -> &GU {
        &self.gas_used
    }

    fn refund_gas(&mut self, gas: &GU) -> Result<(), GasMeteringError<GU>> {
        self.gas_used = self.gas_used.checked_sub(gas).ok_or_else(|| {
            GasMeteringError::ImpossibleToRefundGas {
                gas_to_refund: gas.clone(),
                gas_used: self.gas_used.clone(),
            }
        })?;

        self.remaining_funds = self
            .remaining_funds
            .saturating_add(gas.value(&self.gas_price));

        Ok(())
    }

    /// Deducts the provided gas unit from the remaining funds, computing the scalar value of the
    /// funds from the price of the instance.
    fn charge_gas(&mut self, gas: &GU) -> Result<(), GasMeteringError<GU>> {
        // Check that there's enough gas to cover the cost before mutating the gas_used counter.
        // This ensures that in the corner case where...
        //  - User wants to do expensive operation
        //  - User does not have enough gas left
        // ... the check fails and the user does not lose any gas - which is what we want
        // since the operation won't be performed.
        //
        // This also ensures that the `gas_used` stays in sync with the `remaining_funds` counter.
        let funds_to_charge = gas.value(&self.gas_price);
        let remaining_funds = self.remaining_funds;
        self.remaining_funds = remaining_funds
            .checked_sub(funds_to_charge)
            .ok_or_else(|| GasMeteringError::OutOfGas {
                gas_to_charge: gas.clone(),
                gas_price: self.gas_price.clone(),
                remaining_funds: self.remaining_funds,
                total_gas_consumed: self.gas_used.clone(),
            })?;

        self.gas_used.combine(gas);

        Ok(())
    }

    /// Returns the gas price.
    fn gas_price(&self) -> &GU::Price {
        &self.gas_price
    }

    fn remaining_funds(&self) -> u64 {
        self.remaining_funds
    }
}

#[cfg(feature = "test-utils")]
impl<GU> TxGasMeter<GU>
where
    GU: Gas,
{
    /// Returns a gas meter which does not charge for gas.
    pub(crate) fn unmetered() -> Self {
        Self {
            remaining_funds: u64::MAX,
            gas_price: GU::Price::ZEROED,
            gas_used: GU::ZEROED,
        }
    }
}

#[cfg(test)]
impl<GU: Gas> TxGasMeter<GU> {
    pub fn new(remaining_funds: u64, gas_price: GU::Price) -> Self {
        Self {
            remaining_funds,
            gas_price,
            gas_used: GU::ZEROED,
        }
    }
}
