use std::convert::Infallible;

use capabilities::mocks::MockKernel;
use sov_modules_api::*;
use sov_state::{ArrayWitness, Prefix, ProverStorage, Storage, ZkStorage};
use sov_test_utils::storage::new_finalized_storage;
use unwrap_infallible::UnwrapInfallible;

use crate::state_tests::*;

pub trait StateThing {
    type Value: core::fmt::Debug + Eq + PartialEq;

    /// Write itself to the underlying infallible state accessor
    fn create(state: &mut impl InfallibleStateAccessor) -> Self;

    /// Gets value from the underlying infallible state accessor
    fn value(&self, state: &mut impl InfallibleStateAccessor) -> Self::Value;

    /// Changes itself in the underlying infallible state accessor
    fn change(&self, state: &mut impl InfallibleStateAccessor);
}

pub enum Condition {
    Checkpoint,
    Revert,
}

pub struct StateValueSet(StateValue<u32>);

impl StateThing for StateValueSet {
    type Value = u32;

    fn create(state: &mut impl InfallibleStateAccessor) -> Self {
        let state_value = StateValue::new(Prefix::new(vec![0]));
        state_value.set(&10, state).unwrap_infallible();
        StateValueSet(state_value)
    }

    fn value(&self, state: &mut impl InfallibleStateAccessor) -> Self::Value {
        self.0
            .get(state)
            .unwrap_infallible()
            .expect("Value wasn't set")
    }

    fn change(&self, state: &mut impl InfallibleStateAccessor) {
        let mut value = self.value(state);
        value += 1;
        self.0.set(&value, state).unwrap_infallible();
    }
}

pub struct StateVecSet(StateVec<u32>);

impl StateThing for StateVecSet {
    type Value = Vec<u32>;

    fn create(state: &mut impl InfallibleStateAccessor) -> Self {
        let state_vec = StateVec::new(Prefix::new(vec![0]));
        state_vec
            .set_all(vec![10, 20, 30, 40, 50, 60], state)
            .unwrap_infallible();
        StateVecSet(state_vec)
    }

    fn value(&self, state: &mut impl InfallibleStateAccessor) -> Self::Value {
        self.0.collect_infallible(state)
    }

    fn change(&self, state: &mut impl InfallibleStateAccessor) {
        let mut value = self.value(state);
        for v in value.iter_mut() {
            // TODO: More sophisticated ways of updating it
            *v += 1;
        }
        self.0.set_all(value, state).unwrap_infallible();
    }
}

pub struct StateVecPush(StateVec<u32>);

impl StateThing for StateVecPush {
    type Value = Vec<u32>;

    fn create(state: &mut impl InfallibleStateAccessor) -> Self {
        let state_vec = StateVec::new(Prefix::new(vec![0]));
        state_vec.set_all(vec![10], state).unwrap_infallible();
        StateVecPush(state_vec)
    }

    fn value(&self, state: &mut impl InfallibleStateAccessor) -> Self::Value {
        self.0.collect_infallible(state)
    }

    fn change(&self, state: &mut impl InfallibleStateAccessor) {
        let value = self
            .0
            .get(0, state)
            .unwrap_infallible()
            .expect("Value wasn't set");
        self.0.push(&(value + 1), state).unwrap_infallible();
    }
}

pub struct StateVecRemove(StateVec<u32>);

impl StateThing for StateVecRemove {
    type Value = Vec<u32>;

    fn create(state: &mut impl InfallibleStateAccessor) -> Self {
        let state_vec = StateVec::new(Prefix::new(vec![0]));
        state_vec
            .set_all(vec![3u32; 100], state)
            .unwrap_infallible();
        StateVecRemove(state_vec)
    }

    fn value(&self, state: &mut impl InfallibleStateAccessor) -> Self::Value {
        self.0.collect_infallible(state)
    }

    fn change(&self, state: &mut impl InfallibleStateAccessor) {
        self.0.pop(state).unwrap_infallible();
    }
}

impl Condition {
    fn replace_working_set<S: Spec>(&self, working_set: WorkingSet<S>) -> WorkingSet<S> {
        match self {
            Condition::Checkpoint => {
                let (checkpoint, _tx_consumption, _events) = working_set.checkpoint();
                checkpoint.to_working_set_unmetered()
            }
            Condition::Revert => {
                let (tx_scratchpad, _tx_consumption) = working_set.revert();
                let checkpoint = tx_scratchpad.revert();
                checkpoint.to_working_set_unmetered()
            }
        }
    }

    fn process_thing<S: Spec, St: StateThing>(
        &self,
        thing: &St,
        mut working_set: WorkingSet<S>,
    ) -> WorkingSet<S> {
        let value_before = thing.value(&mut working_set.to_unmetered());
        thing.change(&mut working_set.to_unmetered());
        working_set = self.replace_working_set(working_set);
        let value_after = thing.value(&mut working_set.to_unmetered());
        match self {
            Condition::Checkpoint => {
                assert_ne!(
                    value_before, value_after,
                    "Value hasn't been changed after `.checkpoint()`"
                );
            }
            Condition::Revert => {
                assert_eq!(
                    value_before, value_after,
                    "Value has changed after `.revert()`"
                );
            }
        }
        working_set
    }
}

const CONDITIONS: [Condition; 8] = [
    Condition::Checkpoint,
    Condition::Revert,
    Condition::Checkpoint,
    Condition::Revert,
    Condition::Checkpoint,
    Condition::Revert,
    Condition::Revert,
    Condition::Checkpoint,
];

/// Creates thing and checks it with all condition combinations
pub fn test_state_thing<S: Spec<Storage = ProverStorage<StorageSpec>>, St: StateThing>(
    conditions: &[Condition],
) {
    let tmpdir = tempfile::tempdir().unwrap();
    let storage: ProverStorage<StorageSpec> = new_finalized_storage(tmpdir.path());
    let mut working_set = WorkingSet::<S>::new_deprecated(storage, &MockKernel::<S>::default());
    let thing = St::create(&mut working_set.to_unmetered());

    for condition in conditions {
        working_set = condition.process_thing(&thing, working_set);
    }
}

#[test]
fn test_state_value_set() {
    test_state_thing::<S, StateValueSet>(&CONDITIONS[..]);
}

#[test]
fn test_state_vec_set() {
    test_state_thing::<S, StateVecSet>(&CONDITIONS[..]);
}

#[test]
fn test_state_vec_push() {
    test_state_thing::<S, StateVecPush>(&CONDITIONS[..]);
}

#[test]
fn test_state_vec_remove() {
    test_state_thing::<S, StateVecRemove>(&CONDITIONS[..]);
}

#[test]
fn test_witness_round_trip() -> Result<(), Infallible> {
    let tempdir = tempfile::tempdir().unwrap();
    let state_value = StateValue::new(Prefix::new(vec![0]));

    // Native execution
    let witness: ArrayWitness = {
        let storage = new_finalized_storage::<StorageSpec>(tempdir.path());
        let mut state: StateCheckpoint<<S as Spec>::Storage> =
            StateCheckpoint::new(storage.clone(), &MockKernel::<S>::default());
        state_value.set(&11, &mut state)?;
        let _ = state_value.get(&mut state);
        state_value.set(&22, &mut state)?;
        let (cache_log, _, witness) = state.freeze();

        let _ = storage
            .validate_and_materialize(cache_log, &witness)
            .expect("Native jmt validation should succeed");
        witness
    };

    {
        let storage = ZkStorage::<StorageSpec>::new();
        let mut state_checkpoint: StateCheckpoint<<Zk as Spec>::Storage> =
            StateCheckpoint::with_witness(storage.clone(), witness, &MockKernel::<Zk>::default());
        state_value.set(&11, &mut state_checkpoint)?;
        let _ = state_value.get(&mut state_checkpoint);
        state_value.set(&22, &mut state_checkpoint)?;
        let (cache_log, _, witness) = state_checkpoint.freeze();

        let _ = storage
            .validate_and_materialize(cache_log, &witness)
            .expect("ZK validation should succeed");
    };

    Ok(())
}
