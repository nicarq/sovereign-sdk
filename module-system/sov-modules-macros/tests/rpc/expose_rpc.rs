use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::{expose_rpc, rpc_gen};
use sov_modules_api::{
    CallResponse, Context, Error, Module, ModuleId, ModuleInfo, Spec, StateValue, WorkingSet,
};

#[derive(ModuleInfo)]
pub struct QueryModule<S: Spec> {
    #[address]
    pub id: ModuleId,

    #[state]
    pub data: StateValue<u8>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec> Module for QueryModule<S> {
    type Spec = S;
    type Config = u8;
    type CallMessage = u8;
    type Event = ();

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
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

#[derive(Debug, Eq, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryResponse {
    pub value: Option<u8>,
}

#[rpc_gen(client, server, namespace = "queryModule")]
impl<S: Spec> QueryModule<S> {
    #[rpc_method(name = "queryValue")]
    pub fn query_value(&self, working_set: &mut WorkingSet<S>) -> RpcResult<QueryResponse> {
        Ok(QueryResponse {
            value: self.data.get(working_set),
        })
    }
}

#[expose_rpc]
struct Runtime<S: Spec> {
    pub first: QueryModule<S>,
}

fn main() {}
