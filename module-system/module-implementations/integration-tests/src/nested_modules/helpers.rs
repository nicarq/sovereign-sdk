use sov_modules_api::{Context, EventEmitter, ModuleInfo, Spec, StateMap, StateValue, WorkingSet};

#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
pub(crate) enum Event {
    Update,
    Execute,
}

pub mod module_a {
    use sov_modules_api::{Module, ModuleId};

    use super::*;

    #[derive(ModuleInfo)]
    pub(crate) struct ModuleA<S: Spec> {
        #[address]
        pub id_module_a: ModuleId,

        #[state]
        pub(crate) state_1_a: StateMap<String, String>,

        #[state]
        pub(crate) state_2_a: StateValue<String>,

        #[phantom]
        phantom: std::marker::PhantomData<S>,
    }

    impl<S: Spec> Module for ModuleA<S> {
        type Spec = S;

        type Config = ();

        type CallMessage = ();

        type Event = Event;

        fn call(
            &self,
            _message: Self::CallMessage,
            _context: &Context<Self::Spec>,
            _working_set: &mut WorkingSet<Self::Spec>,
        ) -> Result<sov_modules_api::CallResponse, sov_modules_api::Error> {
            todo!()
        }
    }

    impl<S: Spec> ModuleA<S> {
        pub fn update(&mut self, key: &str, value: &str, working_set: &mut WorkingSet<S>) {
            self.emit_event(working_set, "modulea_update", Event::Update);
            self.state_1_a
                .set(&key.to_owned(), &value.to_owned(), working_set);
            self.state_2_a.set(&value.to_owned(), working_set);
        }
    }
}

pub mod module_b {
    use sov_modules_api::{Module, ModuleId};

    use super::*;

    #[derive(ModuleInfo)]
    pub(crate) struct ModuleB<S: Spec> {
        #[address]
        pub id_module_b: ModuleId,

        #[state]
        state_1_b: StateMap<String, String>,

        #[module]
        pub(crate) mod_1_a: module_a::ModuleA<S>,
    }

    impl<S: Spec> Module for ModuleB<S> {
        type Spec = S;

        type Config = ();

        type CallMessage = ();

        type Event = Event;

        fn call(
            &self,
            _message: Self::CallMessage,
            _context: &Context<Self::Spec>,
            _working_set: &mut WorkingSet<Self::Spec>,
        ) -> Result<sov_modules_api::CallResponse, sov_modules_api::Error> {
            todo!()
        }
    }

    impl<S: Spec> ModuleB<S> {
        pub fn update(&mut self, key: &str, value: &str, working_set: &mut WorkingSet<S>) {
            self.emit_event(working_set, "moduleb_update", Event::Update);
            self.state_1_b
                .set(&key.to_owned(), &value.to_owned(), working_set);
            self.mod_1_a.update("key_from_b", value, working_set);
        }
    }
}

pub(crate) mod module_c {
    use sov_modules_api::{Module, ModuleId};

    use super::*;

    #[derive(ModuleInfo)]
    pub(crate) struct ModuleC<S: Spec> {
        #[address]
        pub id: ModuleId,

        #[module]
        pub(crate) mod_1_a: module_a::ModuleA<S>,

        #[module]
        mod_1_b: module_b::ModuleB<S>,
    }

    impl<S: Spec> Module for ModuleC<S> {
        type Spec = S;

        type Config = ();

        type CallMessage = ();

        type Event = Event;

        fn call(
            &self,
            _message: Self::CallMessage,
            _context: &Context<Self::Spec>,
            _working_set: &mut WorkingSet<Self::Spec>,
        ) -> Result<sov_modules_api::CallResponse, sov_modules_api::Error> {
            todo!()
        }
    }

    impl<S: Spec> ModuleC<S> {
        pub fn execute(&mut self, key: &str, value: &str, working_set: &mut WorkingSet<S>) {
            self.emit_event(working_set, "modulec_execute", Event::Execute);
            self.mod_1_a.update(key, value, working_set);
            self.mod_1_b.update(key, value, working_set);
            self.mod_1_a.update(key, value, working_set);
        }
    }
}
