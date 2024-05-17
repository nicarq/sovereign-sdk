use sov_modules_api::{
    CallResponse, Context, Error, Module, ModuleInfo, Spec, StateValue, TxState, WorkingSet,
};

pub mod first_test_module {
    use super::*;
    use sov_modules_api::ModuleId;

    #[derive(ModuleInfo)]
    pub struct FirstTestStruct<S: Spec> {
        #[id]
        pub address: ModuleId,

        #[state]
        pub state_in_first_struct: StateValue<u8>,

        #[phantom]
        phantom: std::marker::PhantomData<S>,
    }

    impl<S: Spec> FirstTestStruct<S> {
        pub fn get_state_value(&self, working_set: &mut WorkingSet<S>) -> u8 {
            self.state_in_first_struct.get(working_set).unwrap()
        }
    }

    #[derive(
        borsh::BorshDeserialize,
        borsh::BorshSerialize,
        serde::Serialize,
        serde::Deserialize,
        Debug,
        PartialEq,
        Clone,
    )]
    pub enum Event {
        FirstModuleEnum1(u64),
        FirstModuleEnum2,
        FirstModuleEnum3(Vec<u8>),
    }

    impl<S: Spec> Module for FirstTestStruct<S> {
        type Spec = S;
        type Config = ();
        type CallMessage = u8;
        type Event = Event;

        fn genesis(
            &self,
            _config: &Self::Config,
            working_set: &mut impl sov_modules_api::GenesisState<S>,
        ) -> Result<(), Error> {
            self.state_in_first_struct.set(&1, working_set);
            Ok(())
        }

        fn call(
            &self,
            msg: Self::CallMessage,
            _context: &Context<Self::Spec>,
            working_set: &mut impl TxState<S>,
        ) -> Result<CallResponse, Error> {
            self.state_in_first_struct.set(&msg, working_set);
            Ok(CallResponse::default())
        }
    }
}

pub mod second_test_module {
    use super::*;
    use sov_modules_api::ModuleId;

    #[derive(ModuleInfo)]
    pub struct SecondTestStruct<S: Spec> {
        #[id]
        pub address: ModuleId,

        #[state]
        pub state_in_second_struct: StateValue<u8>,

        #[phantom]
        phantom: std::marker::PhantomData<S>,
    }

    impl<S: Spec> SecondTestStruct<S> {
        pub fn get_state_value(&self, working_set: &mut WorkingSet<S>) -> u8 {
            self.state_in_second_struct.get(working_set).unwrap()
        }
    }

    #[derive(
        borsh::BorshDeserialize,
        borsh::BorshSerialize,
        serde::Serialize,
        serde::Deserialize,
        Debug,
        PartialEq,
        Clone,
    )]
    pub enum Event {
        SecondModuleEnum,
    }

    impl<S: Spec> Module for SecondTestStruct<S> {
        type Spec = S;
        type Config = ();
        type CallMessage = u8;
        type Event = Event;

        fn genesis(
            &self,
            _config: &Self::Config,
            working_set: &mut impl sov_modules_api::GenesisState<S>,
        ) -> Result<(), Error> {
            self.state_in_second_struct.set(&2, working_set);
            Ok(())
        }

        fn call(
            &self,
            msg: Self::CallMessage,
            _context: &Context<Self::Spec>,
            working_set: &mut impl TxState<S>,
        ) -> Result<CallResponse, Error> {
            self.state_in_second_struct.set(&msg, working_set);
            Ok(CallResponse::default())
        }
    }
}

pub mod third_test_module {
    use super::*;
    use sov_modules_api::ModuleId;

    pub trait ModuleThreeStorable:
        borsh::BorshSerialize + borsh::BorshDeserialize + core::fmt::Debug + Default + Send + Sync
    {
    }

    impl ModuleThreeStorable for u32 {}

    #[derive(ModuleInfo)]
    pub struct ThirdTestStruct<S: Spec, OtherGeneric: ModuleThreeStorable> {
        #[id]
        pub address: ModuleId,

        #[state]
        pub state_in_third_struct: StateValue<OtherGeneric>,

        #[phantom]
        phantom: std::marker::PhantomData<S>,
    }

    impl<S: Spec, OtherGeneric: ModuleThreeStorable> ThirdTestStruct<S, OtherGeneric> {
        pub fn get_state_value(&self, working_set: &mut WorkingSet<S>) -> Option<OtherGeneric> {
            self.state_in_third_struct.get(working_set)
        }
    }

    #[derive(
        borsh::BorshDeserialize,
        borsh::BorshSerialize,
        serde::Serialize,
        serde::Deserialize,
        Debug,
        PartialEq,
        Clone,
    )]
    pub enum Event {
        ThirdModuleEnum,
    }

    impl<S: Spec, OtherGeneric: ModuleThreeStorable> Module for ThirdTestStruct<S, OtherGeneric> {
        type Spec = S;
        type Config = ();
        type CallMessage = OtherGeneric;
        type Event = Event;

        fn genesis(
            &self,
            _config: &Self::Config,
            working_set: &mut impl sov_modules_api::GenesisState<S>,
        ) -> Result<(), Error> {
            self.state_in_third_struct
                .set(&Default::default(), working_set);
            Ok(())
        }

        fn call(
            &self,
            msg: Self::CallMessage,
            _context: &Context<Self::Spec>,
            working_set: &mut impl TxState<S>,
        ) -> Result<CallResponse, Error> {
            self.state_in_third_struct.set(&msg, working_set);
            Ok(CallResponse::default())
        }
    }
}
