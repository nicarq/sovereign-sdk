use sov_modules_api::prelude::*;
use sov_modules_api::{ModuleInfo, Spec, StateValue, WorkingSet};
use sov_modules_core::{StateCodec, StateKeyCodec, StateValueCodec};
use sov_state::{DefaultStorageSpec, ZkStorage};
use std::panic::catch_unwind;

type ZkDefaultSpec = sov_modules_api::default_spec::ZkDefaultSpec<sov_mock_zkvm::MockZkVerifier>;

#[derive(ModuleInfo)]
struct TestModule<S>
where
    S: Spec,
{
    #[address]
    address: S::Address,

    #[state(codec_builder = "crate::CustomCodec::new")]
    state_value: StateValue<u32, CustomCodec>,
}

#[derive(Default)]
struct CustomCodec;

impl CustomCodec {
    fn new() -> Self {
        Self
    }
}

impl StateCodec for CustomCodec {
    type KeyCodec = Self;
    type ValueCodec = Self;
    fn key_codec(&self) -> &Self::KeyCodec {
        self
    }
    fn value_codec(&self) -> &Self::ValueCodec {
        self
    }
}

impl<K> StateKeyCodec<K> for CustomCodec {
    fn encode_key(&self, _key: &K) -> Vec<u8> {
        unimplemented!()
    }
}

impl<V> StateValueCodec<V> for CustomCodec {
    type Error = String;

    fn encode_value(&self, _value: &V) -> Vec<u8> {
        unimplemented!()
    }

    fn try_decode_value(&self, _bytes: &[u8]) -> Result<V, Self::Error> {
        unimplemented!()
    }
}

fn main() {
    let storage: ZkStorage<DefaultStorageSpec> = ZkStorage::new();
    let module: TestModule<ZkDefaultSpec> = TestModule::default();

    catch_unwind(|| {
        let mut working_set: WorkingSet<ZkDefaultSpec> = WorkingSet::new(storage);
        module.state_value.set(&0u32, &mut working_set);
    })
    .unwrap_err();
}
