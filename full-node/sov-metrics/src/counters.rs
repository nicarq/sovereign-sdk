use once_cell::sync::Lazy;
use prometheus::{register_int_counter, register_int_gauge, IntCounter, IntGauge};

pub(crate) static DA_BLOCKS_PROCESSED: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!("da_blocks_processed", "Number of DA blocks processed").unwrap()
});

pub(crate) static ROLLUP_BATCHES_PROCESSED: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!("rollup_batches_processed", "Rollup batches processed").unwrap()
});

pub(crate) static ROLLUP_TRANSACTIONS_PROCESSED: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!("rollup_txns_processed", "Rollup transactions processed").unwrap()
});

pub(crate) static ROLLUP_TRANSACTIONS_PER_DA_BLOCK: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "rollup_txns_per_da_block",
        "Total number of transactions in the DA block"
    )
    .unwrap()
});

pub(crate) static CURRENT_DA_HEIGHT: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!("current_da_height", "Current DA height for the rollup").unwrap()
});
