use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    AccessoryStateValue, CallResponse, Context, DaSpec, GenesisState, Module, ModuleError,
    ModuleId, ModuleInfo, Spec, StateAccessor, TxState,
};
use sov_state::{ProvableNamespace, StateRoot};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, get_gas_used, AsUser};

type S = sov_test_utils::TestSpec;

#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CallMessage {
    SetAccessoryValue(u32),
    Nop(u32),
}

#[derive(ModuleInfo, Clone)]
pub struct TestAccessoryModule<S: Spec> {
    #[id]
    id: ModuleId,

    #[state]
    accessory_state: AccessoryStateValue<u32>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec> Module for TestAccessoryModule<S> {
    type Spec = S;
    type Config = ();
    type CallMessage = CallMessage;
    type Event = ();

    fn genesis(
        &self,
        _genesis_rollup_header: &<S::Da as DaSpec>::BlockHeader,
        _validity_condition: &<S::Da as DaSpec>::ValidityCondition,
        _config: &Self::Config,
        _state: &mut impl GenesisState<S>,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        _context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ModuleError> {
        match msg {
            CallMessage::SetAccessoryValue(value) => {
                let unmetered_state = &mut state.to_unmetered();

                self.accessory_state
                    .set(&value, unmetered_state)
                    .unwrap_infallible();

                Ok(CallResponse::default())
            }
            CallMessage::Nop(_) => Ok(CallResponse::default()),
        }
    }
}

generate_optimistic_runtime!(TestAccessoryRuntime <= accessory_module: TestAccessoryModule<S>);

/// Check that:
/// 1. Accessory state does not change normal state root hash.
/// 2. Accessory state is reverted together with normal state.
/// Changes are returned explicitly by storage trait.
#[test]
fn test_accessory_value_setter() {
    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let user = genesis_config.additional_accounts.first().unwrap().clone();

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), ());

    let mut runner = TestRunner::new_with_genesis(
        genesis.into_genesis_params(),
        TestAccessoryRuntime::default(),
    );

    let (result_with_update, _) = runner.simulate(
        user.create_plain_message::<TestAccessoryModule<S>>(CallMessage::SetAccessoryValue(42)),
    );

    let root_hash_with_update = result_with_update
        .state_root
        .namespace_root(ProvableNamespace::User);
    let gas_consumed_with_update =
        get_gas_used(&result_with_update.batch_receipts[0].tx_receipts[0]);

    let (result_without_update, _) =
        runner.simulate(user.create_plain_message::<TestAccessoryModule<S>>(CallMessage::Nop(42)));

    let root_hash_without_update = result_without_update
        .state_root
        .namespace_root(ProvableNamespace::User);
    let gas_consumed_without_update =
        get_gas_used(&result_without_update.batch_receipts[0].tx_receipts[0]);

    assert_eq!(
        gas_consumed_with_update, gas_consumed_without_update,
        "Gas consumption has been changed by accessory writes"
    );

    assert_eq!(
        root_hash_with_update, root_hash_without_update,
        "State root has been changed by accessory writes"
    );
}
