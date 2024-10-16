//! Defines the query methods for the attester incentives module

use serde::{Deserialize, Serialize};
use sov_modules_api::capabilities::HasKernel;
use sov_modules_api::optimistic::{BondingProofService, ProofOfBond};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{ApiStateAccessor, Gas, GasMeter, Spec, StateCheckpoint, StateReader};
use sov_state::storage::{SlotKey, Storage, StorageProof};
use sov_state::User;

use super::AttesterIncentives;
use crate::UnbondingInfo;

/// The response type to the `getBondAmount` query.
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct BondAmountResponse {
    /// The value of the bond
    pub value: u64,
}

impl<S> AttesterIncentives<S>
where
    S: Spec,
{
    /// Queries the state of the module.
    pub fn get_attester_bond_amount<Reader: StateReader<User>>(
        &self,
        address: &S::Address,
        state: &mut Reader,
    ) -> Result<BondAmountResponse, Reader::Error> {
        Ok(BondAmountResponse {
            value: self
                .bonded_attesters
                .get(address, state)?
                .unwrap_or_default(),
        })
    }

    /// Queries the state of the module.
    pub fn get_challenger_bond_amount<Reader: StateReader<User>>(
        &self,
        address: S::Address,

        state: &mut Reader,
    ) -> Result<BondAmountResponse, Reader::Error> {
        Ok(BondAmountResponse {
            value: self
                .bonded_challengers
                .get(&address, state)?
                .unwrap_or_default(),
        })
    }

    /// Gives storage key for given address
    pub fn get_attester_storage_key(&self, address: S::Address) -> SlotKey {
        let prefix = self.bonded_attesters.prefix();
        let codec = self.bonded_attesters.codec();
        // Maybe we will need to store the namespace somewhere in the rollup
        SlotKey::new(prefix, &address, codec)
    }

    /// Used by attesters to get a proof that they were bonded before starting to produce attestations.
    /// A bonding proof is valid for `max_finality_period` blocks, the attester can only produce transition
    /// attestations for this specific amount of time.
    pub fn get_bond_proof(
        &self,
        address: S::Address,
        state: &mut ApiStateAccessor<S>,
    ) -> Option<StorageProof<<S::Storage as Storage>::Proof>> {
        self.bonded_attesters.get_with_proof(&address, state)
    }

    /// Returns the value of the `minimum_attester_bond` at the current gas price.
    pub fn get_minimal_attester_bond_value(&self, state: &mut ApiStateAccessor<S>) -> u64 {
        self.minimum_attester_bond
            .get(state)
            .unwrap_infallible()
            .expect("The minimum attester bond should be set at genesis")
            .value(&state.gas_info().gas_price)
    }

    /// Returns the value of the `minimum_challenger_bond` at the current gas price.
    pub fn get_minimal_challenger_bond_value(&self, state: &mut ApiStateAccessor<S>) -> u64 {
        self.minimum_challenger_bond
            .get(state)
            .unwrap_infallible()
            .expect("The minimum challenger bond should be set at genesis")
            .value(&state.gas_info().gas_price)
    }

    /// Returns the unbonding amount of the given address.
    pub fn get_unbonding_amount(
        &self,
        address: S::Address,
        state: &mut ApiStateAccessor<S>,
    ) -> Option<UnbondingInfo> {
        self.unbonding_attesters
            .get(&address, state)
            .unwrap_infallible()
    }
}

/// Implementation of the [`BondingProofServiceImpl`] for the [`AttesterIncentives`] module.
pub struct BondingProofServiceImpl<S, K>
where
    S: Spec,
{
    attester_address: S::Address,
    attester_incentives: AttesterIncentives<S>,
    storage: tokio::sync::watch::Receiver<S::Storage>,
    has_kernel: K,
}

impl<S, K> BondingProofServiceImpl<S, K>
where
    S: Spec,
    K: HasKernel<S>,
{
    /// Creates a new `BondingProofServiceImpl` service.
    pub fn new(
        attester_address: S::Address,
        attester_incentives: AttesterIncentives<S>,
        storage: tokio::sync::watch::Receiver<S::Storage>,
        has_kernel: K,
    ) -> Self {
        Self {
            attester_address,
            attester_incentives,
            storage,
            has_kernel,
        }
    }
}

impl<S, K> BondingProofService for BondingProofServiceImpl<S, K>
where
    S: Spec,
    K: HasKernel<S> + Send + Sync + 'static,
{
    type StateProof = StorageProof<<S::Storage as Storage>::Proof>;

    fn get_bonding_proof(
        &self,
        height: u64,
    ) -> Option<ProofOfBond<<Self as BondingProofService>::StateProof>> {
        let storage = self.storage.borrow().clone();
        let checkpoint = StateCheckpoint::new(storage, &self.has_kernel.kernel());
        let state = ApiStateAccessor::<S>::new(
            &checkpoint,
            self.has_kernel.kernel_with_slot_mapping(),
            Some(height),
        );
        let mut state = state.get_archival_at(height);
        let proof = self
            .attester_incentives
            .get_bond_proof(self.attester_address.clone(), &mut state)?;

        Some(ProofOfBond {
            claimed_rollup_height: height,
            proof,
        })
    }
}
