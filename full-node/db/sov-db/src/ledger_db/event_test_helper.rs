use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::Value;
use sov_modules_api::default_context::DefaultContext;
use sov_modules_api::utils::generate_address;
use sov_modules_api::AddressBech32;
use sov_rollup_interface::rpc::Event;
use sov_rollup_interface::stf::StoredEvent;

use crate::ledger_db::{LedgerDB, SchemaBatch};
use crate::schema::types::{EventNumber, TxNumber};

pub(crate) const NUM_MODULES: usize = 3;
pub(crate) const NUM_TXNS_PER_MODULE: usize = 10;
pub(crate) const NUM_EVENTS_PER_TXN: usize = 100;
pub(crate) const MAX_NUM_EVENTS_FIXED_KEY: usize = 1357;
pub(crate) const FIXED_EVENT_KEY: &str = "fixed_key";

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct TestEvent {
    value: u64,
    module_name: String,
}

impl From<TestEvent> for Event {
    fn from(test_event: TestEvent) -> Self {
        Event {
            event_value: Value::from(test_event.value),
            module_name: test_event.module_name,
        }
    }
}

pub(crate) fn generate_events(
    ledger_db: &LedgerDB,
    schema_batch: &mut SchemaBatch,
    num_modules: usize,
    num_txns_per_module: usize,
    num_events_per_txn: usize,
    num_events_fixed_key: usize,
) -> usize {
    let mut event_num = 0;
    let mut txn_num = 0;
    let mut module_num = 0;
    let mut events = vec![];
    for _ in 0..num_modules {
        module_num += 1;
        let module_name = format!("module_{}", module_num);
        let module_address = AddressBech32::from(generate_address::<DefaultContext>(&module_name))
            .try_to_vec()
            .unwrap();
        for _ in 0..num_txns_per_module {
            txn_num += 1;
            for _ in 0..num_events_per_txn {
                event_num += 1;
                let event_value = TestEvent {
                    value: event_num + 1,
                    module_name: module_name.clone(),
                }
                .try_to_vec()
                .unwrap();

                let event_key = match event_num {
                    n if n <= (num_events_fixed_key as u64) => FIXED_EVENT_KEY.to_string(),
                    n => format!("key_{}", n),
                };

                events.push((
                    StoredEvent::new(event_key.as_bytes(), &module_address, &event_value),
                    event_num,
                    txn_num,
                ));
            }
        }
    }
    for (serialized_event, event_num, txn_num) in &events {
        ledger_db
            .put_event(
                serialized_event,
                &EventNumber(*event_num),
                TxNumber(*txn_num),
                schema_batch,
            )
            .unwrap();
    }
    events.len()
}

pub(crate) fn find_event_details(
    event_number: u64,
    _num_modules: usize,
    num_txns_per_module: usize,
    num_events_per_txn: usize,
) -> (usize, usize) {
    let tnum = (event_number as f64 / num_events_per_txn as f64).ceil() as usize;
    let mnum = (tnum as f64 / num_txns_per_module as f64).ceil() as usize;
    (tnum, mnum)
}
