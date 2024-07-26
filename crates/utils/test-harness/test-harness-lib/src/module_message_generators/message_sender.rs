use std::marker::PhantomData;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use sov_modules_api::{DaSpec, EncodeCall, Module, Spec};
use sov_modules_stf_blueprint::Runtime;
use tokio::sync::mpsc::Sender;
use tokio::time::{self, Duration};

use crate::{PreparedCallMessage, SerializedPreparedCallMessage};

/// A trait to be implemented by any structure wishing to send module messages for later broadcast
/// to the rollup.
#[async_trait]
pub trait MessageSenderT {
    /// This will start the implementor of this trait sending messages.
    async fn send_messages(self: Box<Self>, max_num_txs: Option<usize>, interval: Option<u64>);
}

/// The [`MessageSender`] structure holds all that is required to create [`crate::PreparedCallMessage`]s,
/// serialize them, and send those [`crate::SerializedPreparedCallMessage`]s on to whomever is in charge
/// of signging ready for broadcasting to the rollup via the sequencer or directly to the DA layer.
pub struct MessageSender<R: Runtime<S, Da>, Da: DaSpec, S: Spec, M: Module<Spec = S>> {
    /// The name of this message sender.
    name: String,

    /// A flag used to tell this message sender to stop sending messages.
    should_stop: Arc<AtomicBool>,

    /// The message generator itself, whence [`PreparedCallMessages`] are generated.
    message_generator: Box<dyn Iterator<Item = PreparedCallMessage<S, M>> + Send + Sync>,

    /// A channel down which [`SerializedPreparedCallMesssage`]s are sent to be later broadcast
    /// to the rollup.
    sender: Sender<SerializedPreparedCallMessage>,

    _phantom: PhantomData<(R, Da)>,
}

impl<R: Runtime<S, Da>, Da: DaSpec, S: Spec, M: Module<Spec = S>> MessageSender<R, Da, S, M> {
    /// Creates a new [`MessageSender`].
    pub fn new(
        name: &str,
        should_stop: Arc<AtomicBool>,
        message_generator: Box<dyn Iterator<Item = PreparedCallMessage<S, M>> + Send + Sync>,
        sender: Sender<SerializedPreparedCallMessage>,
    ) -> Self {
        Self {
            sender,
            should_stop,
            message_generator,
            _phantom: PhantomData,
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl<
        R: Runtime<S, Da> + EncodeCall<M>,
        Da: DaSpec,
        S: Spec,
        M: Module<Spec = S> + Send + Sync + 'static,
    > MessageSenderT for MessageSender<R, Da, S, M>
where
    M::CallMessage: Send + Sync,
{
    async fn send_messages(
        self: Box<Self>,
        max_num_txs: Option<usize>,
        maybe_interval: Option<u64>,
    ) {
        let log_prefix = format!("{} message sender:", self.name);
        let mut interval = if let Some(interval_milliseconds) = maybe_interval {
            tracing::info!("{log_prefix} sending messages roughly every {interval_milliseconds}ms");
            time::interval(Duration::from_millis(interval_milliseconds))
        } else {
            tracing::debug!("{log_prefix} is not using an interval when sending messages");
            // NOTE This panics if set to zero, so we use a tiny interval to mimic no interval instead.
            time::interval(Duration::from_millis(1))
        };

        interval.tick().await; // NOTE The first tick completes immediately.

        let sender = self.sender.clone();
        let should_stop = self.should_stop.clone();
        let generators = self.message_generator;

        tokio::spawn(async move {
            let generators = generators.into_iter();
            let mut num_messages = 0;

            for message in generators {
                if should_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    tracing::debug!("{log_prefix} should stop switch is flipped");
                    break;
                };

                if Some(num_messages) >= max_num_txs {
                    tracing::debug!("{log_prefix} hit maximum number of messages");
                    break;
                };

                let serialized_msg = SerializedPreparedCallMessage {
                    max_fee: *message.max_fee(),
                    account_pool_index: *message.account_pool_index(),
                    call_message: <R as EncodeCall<M>>::encode_call(message.call_message),
                };

                match sender.send(serialized_msg).await {
                    Ok(_) => {
                        num_messages += 1;
                        tracing::debug!("{log_prefix} message #{num_messages} sent");
                    }
                    Err(err) => {
                        tracing::error!("{log_prefix} error sending serialized message #{num_messages} to sender: {err}");
                    }
                };

                interval.tick().await;
            }
        });
    }
}
