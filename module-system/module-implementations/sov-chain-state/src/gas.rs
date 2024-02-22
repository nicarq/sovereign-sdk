use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_modules_api::{DaSpec, Gas, GasArray, Spec, StateAccessor, StateMap, StateMapAccessor};
use sov_state::codec::BcsCodec;

use crate::{StateTransitionId, TransitionHeight};

/// The parameters for the state based gas price computation.
#[derive(
    BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash,
)]
pub struct GasPriceState<S: Spec> {
    /// The depth at which the elastic gas price will extract its average target price from the
    /// blocks.
    pub blocks_depth: u64,

    /// The elasticity reflects the degree to which the rate of change in price is responsive to
    /// variations in used gas distances from the average target price.
    pub maximum_elasticity: i64,

    /// The current gas price.
    pub price: <S::Gas as Gas>::Price,

    /// The minimum price computed for a block execution.
    pub minimum_price: <S::Gas as Gas>::Price,
}

impl<S: Spec> GasPriceState<S> {
    /// Sets the gas price of the underlying network to the provided historical value, and adjusts
    /// the gas price for the working set accordingly.
    ///
    /// Will return `None` if the `historical_transitions` doesn't contain one of the queried
    /// heights.
    ///
    /// For additional information, check [Gas::elastic_price].
    pub fn update<Da: DaSpec>(
        mut self,
        height: TransitionHeight,
        historical_transitions: &StateMap<TransitionHeight, StateTransitionId<S, Da>, BcsCodec>,
        state_checkpoint: &mut impl StateAccessor,
    ) -> Option<Self> {
        let genesis_height = 0;
        let parent_height = height.saturating_sub(1);

        // on genesis, fetch the initial gas price
        if parent_height == genesis_height {
            return Some(self);
        }

        let height_from = height
            .saturating_sub(self.blocks_depth)
            .max(genesis_height + 1);
        let height_count = height.saturating_sub(height_from);

        // TODO(@vlopes11): Update this calculation to be based on proof latency
        let mut gas_target = S::Gas::zero();
        let mut transition = None;
        for h in height_from..height {
            let history = historical_transitions.get(&h, state_checkpoint)?;
            gas_target.combine(&history.gas_used);
            transition.replace(history);
        }
        gas_target.scalar_division(height_count);

        // there was no gas consumed on the past blocks; preserve the price
        // TODO(@vlopes11): Shouldn't we drop the price by the maximum amount in this case?
        if gas_target == S::Gas::zero() {
            return Some(self);
        }

        self.price = S::Gas::elastic_price(
            self.maximum_elasticity,
            &gas_target,
            &transition?.gas_used,
            &self.price,
            &self.minimum_price,
        );

        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use sov_mock_da::{MockDaSpec, MockValidityCond};
    use sov_modules_api::{GasPrice, StateMap, WorkingSet};
    use sov_modules_core::{Prefix, StateCheckpoint};
    use sov_prover_storage_manager::new_orphan_storage;
    use sov_state::{DefaultStorageSpec, ProverStorage};

    use super::*;

    type DefaultSpec = sov_modules_api::default_spec::DefaultSpec<sov_mock_zkvm::MockZkVerifier>;
    type W = WorkingSet<DefaultSpec>;
    type M = StateMap<TransitionHeight, StateTransitionId<DefaultSpec, MockDaSpec>, BcsCodec>;
    type DefaultGasPriceState = GasPriceState<DefaultSpec>;

    #[test]
    fn price_is_unchanged_with_genesis_blocks() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let ws = &mut W::new(storage);
        let prefix = Prefix::new(b"test".to_vec());
        let ht = &M::with_codec(prefix, BcsCodec);

        let height = 0;

        let expected = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price: [5, 7].into(),
            minimum_price: [2, 3].into(),
        };
        let state = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price: [5, 7].into(),
            minimum_price: [2, 3].into(),
        }
        .update(height, ht, ws)
        .unwrap();

        assert_eq!(
            state,
            expected,
            "The genesis block does not include historical gas data and should be disregarded during price calculations."
        );
    }

    #[test]
    fn price_is_unchanged_with_singe_block() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let ws = &mut W::new(storage);
        let prefix = Prefix::new(b"test".to_vec());
        let ht = &M::with_codec(prefix, BcsCodec);

        let height = 1;

        let expected = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price: [5, 7].into(),
            minimum_price: [2, 3].into(),
        };
        let state = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price: [5, 7].into(),
            minimum_price: [2, 3].into(),
        }
        .update(height, ht, ws)
        .unwrap();

        assert_eq!(
            state,
            expected,
            "A standalone blockchain will not have any predecessor but the genesis block, and the genesis block does not carry any utilized gas data. Consequently, the price should remain constant."
        );
    }

    #[test]
    fn price_is_unchanged_with_two_blocks() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let ws = &mut W::new(storage);
        let prefix = Prefix::new(b"test".to_vec());
        let ht = &M::with_codec(prefix, BcsCodec);

        let height = 2;
        let price: GasPrice<2> = [5, 7].into();
        let original_price = [0, 0].into();
        let used = [1000, 2000].into();

        ht.set(
            &1,
            &StateTransitionId::new(
                [1; 32].into(),
                [2; 32].into(),
                MockValidityCond { is_valid: true },
                original_price,
                used,
            ),
            ws,
        );

        let expected = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price: price.clone(),
            minimum_price: [2, 3].into(),
        };
        let state = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price,
            minimum_price: [2, 3].into(),
        }
        .update(height, ht, ws)
        .unwrap();

        assert_eq!(
            state,
            expected,
            "One analysis block does not affect the price update as it consumes the average itself, resulting in an empty target."
        );
    }

    #[test]
    fn price_is_changed_with_three_blocks() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let ws = &mut W::new(storage);
        let prefix = Prefix::new(b"test".to_vec());
        let ht = &M::with_codec(prefix, BcsCodec);

        let height = 3;

        ht.set(
            &1,
            &StateTransitionId::new(
                [1; 32].into(),
                [2; 32].into(),
                MockValidityCond { is_valid: true },
                [5, 7].into(),
                [1000, 1000].into(),
            ),
            ws,
        );

        ht.set(
            &2,
            &StateTransitionId::new(
                [1; 32].into(),
                [2; 32].into(),
                MockValidityCond { is_valid: true },
                [7, 11].into(),
                [2000, 2000].into(),
            ),
            ws,
        );

        let expected = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price: [17, 22].into(),
            minimum_price: [2, 3].into(),
        };
        let state = DefaultGasPriceState {
            blocks_depth: 10,
            maximum_elasticity: 1,
            price: [13, 17].into(),
            minimum_price: [2, 3].into(),
        }
        .update(height, ht, ws)
        .unwrap();

        assert_eq!(
            state,
            expected,
            "If a moving average is applied to the historical data of an asset's price, then it is expected that the price will exhibit fluctuations."
        );
    }

    #[test]
    fn analysis_will_consider_only_blocks_depth() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage: ProverStorage<DefaultStorageSpec> = new_orphan_storage(tmpdir.path()).unwrap();
        let ws = &mut StateCheckpoint::<DefaultSpec>::new(storage);
        let prefix = Prefix::new(b"test".to_vec());
        let ht = &M::with_codec(prefix, BcsCodec);

        let height = 4;

        ht.set(
            &1,
            &StateTransitionId::new(
                [1; 32].into(),
                [2; 32].into(),
                MockValidityCond { is_valid: true },
                [5, 7].into(),
                [1000, 1000].into(),
            ),
            ws,
        );

        ht.set(
            &2,
            &StateTransitionId::new(
                [1; 32].into(),
                [2; 32].into(),
                MockValidityCond { is_valid: true },
                [7, 11].into(),
                [2000, 2000].into(),
            ),
            ws,
        );

        ht.set(
            &3,
            &StateTransitionId::new(
                [1; 32].into(),
                [2; 32].into(),
                MockValidityCond { is_valid: true },
                [7, 11].into(),
                [1500, 1500].into(),
            ),
            ws,
        );

        let expected = DefaultGasPriceState {
            blocks_depth: 2,
            maximum_elasticity: 1,
            price: [11, 14].into(),
            minimum_price: [2, 3].into(),
        };
        let state = DefaultGasPriceState {
            blocks_depth: 2,
            maximum_elasticity: 1,
            price: [13, 17].into(),
            minimum_price: [2, 3].into(),
        }
        .update(height, ht, ws)
        .unwrap();

        assert_eq!(
            state, expected,
            "The price analysis should have considered only the provided blocks depth."
        );
    }
}
