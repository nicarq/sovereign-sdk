use sov_modules_api::{Module, Spec};

use crate::constants::DEFAULT_MAX_FEE;
use crate::PreparedCallMessage;

pub(crate) fn get_prepared_call_message<S: Spec, M: Module<Spec = S>>(
    call_message: M::CallMessage,
    account_pool_index: u64,
    max_fee: Option<u64>,
) -> PreparedCallMessage<S, M> {
    PreparedCallMessage::new(
        call_message,
        account_pool_index,
        max_fee.unwrap_or(DEFAULT_MAX_FEE),
    )
}
