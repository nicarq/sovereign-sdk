use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::{expose_rpc, rpc_gen, DefaultRuntime};
use sov_modules_api::{
    Address, CallResponse, Context, DispatchCall, EncodeCall, Error, Genesis, MessageCodec, Module,
    ModuleId, ModuleInfo, Spec, StateValue, WorkingSet,
};
use sov_state::ZkStorage;
use sov_test_utils::ZkTestSpec;

pub trait TestSpec {
    type Data: Data;
}

pub trait Data:
    Clone
    + Eq
    + PartialEq
    + std::fmt::Debug
    + serde::Serialize
    + Send
    + Sync
    + serde::de::DeserializeOwned
    + borsh::BorshSerialize
    + borsh::BorshDeserialize
    + 'static
{
}

impl Data for u32 {}

pub mod my_module {
    use super::*;

    #[derive(ModuleInfo)]
    pub struct QueryModule<S: Spec, D: Data> {
        #[address]
        pub id: ModuleId,

        #[state]
        pub data: StateValue<D>,

        #[phantom]
        phantom: std::marker::PhantomData<S>,
    }

    impl<S: Spec, D> Module for QueryModule<S, D>
    where
        D: Data,
    {
        type Spec = S;
        type Config = D;
        type CallMessage = D;
        type Event = ();

        fn genesis(
            &self,
            config: &Self::Config,
            working_set: &mut WorkingSet<S>,
        ) -> Result<(), Error> {
            self.data.set(config, working_set);
            Ok(())
        }

        fn call(
            &self,
            msg: Self::CallMessage,
            _context: &Context<Self::Spec>,
            working_set: &mut WorkingSet<S>,
        ) -> Result<CallResponse, Error> {
            self.data.set(&msg, working_set);
            Ok(CallResponse::default())
        }
    }

    pub mod rpc {
        use super::*;
        use crate::my_module::QueryModule;

        #[derive(Debug, Eq, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
        pub struct QueryResponse {
            pub value: Option<String>,
        }

        #[rpc_gen(client, server, namespace = "queryModule")]
        impl<S, D: Data> QueryModule<S, D>
        where
            S: Spec,
        {
            #[rpc_method(name = "queryValue")]
            pub fn query_value(&self, working_set: &mut WorkingSet<S>) -> RpcResult<QueryResponse> {
                let value = self.data.get(working_set).map(|d| format!("{:?}", d));
                Ok(QueryResponse { value })
            }
        }
    }
}

use my_module::rpc::{QueryModuleRpcImpl, QueryModuleRpcServer};

#[expose_rpc]
#[derive(Genesis, DispatchCall, MessageCodec, DefaultRuntime)]
#[serialization(borsh::BorshDeserialize, borsh::BorshSerialize)]
struct Runtime<S: Spec, T: TestSpec> {
    pub first: my_module::QueryModule<S, T::Data>,
}

struct ActualSpec;

impl TestSpec for ActualSpec {
    type Data = u32;
}

fn main() {
    type S = ZkTestSpec;
    type RT = Runtime<S, ActualSpec>;
    let storage = ZkStorage::new();
    let working_set = &mut WorkingSet::new(storage);
    let runtime = &mut Runtime::<S, ActualSpec>::default();
    let config = GenesisConfig::new(22);
    runtime.genesis(&config, working_set).unwrap();

    let message: u32 = 33;
    let serialized_message =
        <RT as EncodeCall<my_module::QueryModule<S, u32>>>::encode_call(message);
    let module = RT::decode_call(&serialized_message).unwrap();
    let sender = Address::try_from([11; 32].as_ref()).unwrap();
    let sequencer = Address::try_from([11; 32].as_ref()).unwrap();
    let context = Context::<S>::new(sender, sequencer, 1);

    let _ = runtime
        .dispatch_call(module, working_set, &context)
        .unwrap();
}
