use std::rc::Rc;

use sov_attester_incentives::CallMessage;
use sov_modules_api::{CryptoSpec, DaSpec, Gas, GasArray, Spec};

use crate::{Message, MessageGenerator};

/// Generates messages for the attester incentives module.
pub struct AttesterIncentivesMessageGenerator<S: Spec, Da: DaSpec>(
    #[allow(clippy::type_complexity)]
    Vec<(
        <S::CryptoSpec as CryptoSpec>::PrivateKey,
        CallMessage<S, Da>,
    )>,
);

impl<S: Spec, Da: DaSpec>
    From<
        Vec<(
            <S::CryptoSpec as CryptoSpec>::PrivateKey,
            CallMessage<S, Da>,
        )>,
    > for AttesterIncentivesMessageGenerator<S, Da>
{
    fn from(
        messages: Vec<(
            <S::CryptoSpec as CryptoSpec>::PrivateKey,
            CallMessage<S, Da>,
        )>,
    ) -> Self {
        Self(messages)
    }
}

impl<S: Spec, Da: DaSpec> MessageGenerator for AttesterIncentivesMessageGenerator<S, Da> {
    type Module = sov_attester_incentives::AttesterIncentives<S, Da>;
    type Spec = S;

    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        let mut nonce = 0;

        self.0
            .iter()
            .map(|(addr, call_message)| {
                nonce += 1;

                Message::new(
                    Rc::new(addr.clone()),
                    call_message.clone(),
                    Self::DEFAULT_CHAIN_ID,
                    Self::DEFAULT_GAS_TIP,
                    Self::DEFAULT_GAS_LIMIT,
                    Some(<<Self::Spec as Spec>::Gas as Gas>::Price::from_slice(
                        &Self::DEFAULT_MAX_GAS_PRICE,
                    )),
                    nonce,
                )
            })
            .collect::<Vec<Message<Self::Spec, Self::Module>>>()
    }
}
