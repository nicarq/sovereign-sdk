use sov_bank::{config_gas_token_id, Coins};

pub(crate) fn gas_coins(amount: u64) -> Coins {
    Coins {
        amount,
        token_id: config_gas_token_id(),
    }
}
