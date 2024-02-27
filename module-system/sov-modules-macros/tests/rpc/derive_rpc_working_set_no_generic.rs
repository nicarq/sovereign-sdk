use sov_modules_api::macros::rpc_gen;

#[derive(sov_modules_api::ModuleInfo)]
pub struct TestStruct<S: sov_modules_api::Spec> {
    #[address]
    pub(crate) address: S::Address,
}

#[rpc_gen(client, server, namespace = "test")]
impl<S: sov_modules_api::Spec> TestStruct<S> {
    #[rpc_method(name = "foo")]
    pub fn foo(
        &self,
        _working_set: &mut sov_modules_api::WorkingSet,
    ) -> jsonrpsee::core::RpcResult<u32> {
        Ok(42)
    }
}

fn main() {}
