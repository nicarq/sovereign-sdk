use sov_modules_api::hooks::TxHooks;
use sov_modules_api::{
    CallResponse, Context, Module, ModuleError, ModuleId, ModuleInfo, Spec, StateValue, TxState,
    WorkingSet,
};

#[derive(Debug, Clone, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct Foo {
    i: u64,
    j: u64,
}

#[derive(Clone, ModuleInfo, sov_modules_api::macros::ModuleRestApi)]
pub struct MyModule<S: Spec, D>
where
    D: std::hash::Hash
        + Clone
        + borsh::BorshSerialize
        + borsh::BorshDeserialize
        + serde::Serialize
        + serde::de::DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    #[id]
    pub id: ModuleId,

    // Normal values
    #[state]
    pub value: ::sov_modules_api::StateValue<D>,
    #[state]
    pub another_value: StateValue<String>,
    #[state]
    pub mapping: sov_modules_api::StateMap<D, D>,
    #[state]
    pub list: sov_modules_api::StateVec<D>,

    // Skipped values, because missing serde serialization
    #[state]
    pub skipped_value: StateValue<Foo>,
    #[state]
    pub skipped_mapping: sov_modules_api::StateMap<Foo, Foo>,
    #[state]
    pub skipped_list: sov_modules_api::StateVec<Foo>,
    // Explicitly skipped value
    #[state]
    #[rest_api(skip)]
    pub explicitly_skipped_value: ::sov_modules_api::StateValue<D>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec, D> Module for MyModule<S, D>
where
    D: std::hash::Hash
        + Clone
        + borsh::BorshSerialize
        + borsh::BorshDeserialize
        + serde::Serialize
        + serde::de::DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    type Spec = S;
    type Config = ();
    type CallMessage = ();
    type Event = ();

    fn call(
        &self,
        _message: Self::CallMessage,
        _context: &Context<Self::Spec>,
        _state: &mut impl TxState<Self::Spec>,
    ) -> Result<CallResponse, ModuleError> {
        Ok(CallResponse::default())
    }
}

#[derive(Default, sov_modules_api::macros::RuntimeRestApi)]
pub struct TestRuntime<S: Spec> {
    module: MyModule<S, u32>,
}

impl<S: Spec> TxHooks for TestRuntime<S> {
    type Spec = S;
    type TxState = WorkingSet<S>;
}

fn main() {}
