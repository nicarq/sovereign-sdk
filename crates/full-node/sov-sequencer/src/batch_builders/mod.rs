//! Defines the [`BatchBuilder`] trait and related types. Implementations of the trait
//! are nested under this module.

use std::fmt::Debug;

use async_trait::async_trait;
use axum::http::StatusCode;
use borsh::BorshSerialize;
use sov_modules_api::capabilities::{
    AuthenticationOutput, AuthorizeSequencerError, SequencerAuthorization, TransactionAuthenticator,
};
use sov_modules_api::rest::{ApiState, StorageReceiver};
use sov_modules_api::{
    BasicGasMeter, DaSpec, DispatchCall, FullyBakedTx, Gas, RawTx, Spec, TxScratchpad,
};
use sov_modules_stf_blueprint::Runtime;
use tracing::error;

use crate::{SeqDbTx, TxHash, TxStatusManager};

// It's a private module because the preferred sequencer is not intended for use yet.
mod preferred;
pub mod standard;

/// An aggregator of types for [`Runtime`]-aware
/// [`BatchBuilder`]s
///
/// This trait serves no purpose other than to reduce generics clutter in `impl`
/// blocks.
pub trait RtAwareBatchBuilderSpec: Send + Sync + 'static {
    /// The `Spec` defines the rollup's types.
    type Spec: Spec;
    /// The `DaSpec` for the rollup.
    type Da: DaSpec;
    /// The runtime of the rollup.
    type Rt: Runtime<Self::Spec, Self::Da>;
}

impl<S, Da, Rt> RtAwareBatchBuilderSpec for (S, Da, Rt)
where
    S: Spec,
    Da: DaSpec,
    Rt: Runtime<S, Da> + 'static,
{
    type Spec = S;
    type Da = Da;
    type Rt = Rt;
}

/// [`BatchBuilder`] trait is responsible for accepting transactions and
/// assembling them into batches.
#[async_trait]
pub trait BatchBuilder: Sized + Send + Sync + 'static {
    /// What data is returned to clients when a transaction is accepted.
    type Confirmation: serde::Serialize + Send + Sync + 'static;
    /// The batch type that will be serialized and sent to the DA layer.
    type Batch: BorshSerialize + Debug + Send + Sync + 'static;
    /// Arbitrary configuration value(s) fed to [`BatchBuilder::create`].
    type Config: Clone + Debug + Send + Sync + 'static;
    /// The rollup spec.
    type Spec: Spec;
    /// The DA spec.
    type Da: DaSpec;

    /// A [`StorageReceiver`] which is notified each time the rollup's head storage changes.
    /// This happens when DA layer reorgs or a new block is successfully processed on top of
    /// the previous head.
    fn storage_receiver(&self) -> StorageReceiver<Self::Spec>;

    /// Returns an [`ApiState`] subscribed to updates of the batch builder's state.
    fn api_state(&self) -> ApiState<Self::Spec>;

    /// Returns true if and only if the sequencer is ready to accept transactions.
    fn is_ready(&self) -> bool;

    /// Creates a new [`BatchBuilder`].
    async fn create(
        storage: StorageReceiver<Self::Spec>,
        sequencer_address: <Self::Da as DaSpec>::Address,
        seq_db_txs: Vec<SeqDbTx>,
        config: &Self::Config,
    ) -> anyhow::Result<Self>;

    /// Returns a copy of the [`TxStatusManager`] that the [`BatchBuilder`] uses
    /// to notify about dropped transactions.
    fn tx_status_manager(&self) -> TxStatusManager<Self::Da>;

    /// Informs the [`BatchBuilder`] that the DA layer has progressed to a new
    /// slot.
    async fn set_state(&mut self, da_height: u64, stf_state: <Self::Spec as Spec>::Storage);

    /// Adds a **not-encoded** transaction to the mempool. The [`BatchBuilder`]
    /// implementation itself is responsible for "encoding" the transaction.
    ///
    /// Can return an error if transaction is invalid or mempool is full.
    async fn accept_tx(
        &mut self,
        tx: RawTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, AcceptTxError>;

    /// Builds a new batch out of transactions in mempool.
    /// The logic of which transactions and how many of them are included in
    /// batch is up to implementation.
    async fn build_next_batch(&mut self, height: u64) -> anyhow::Result<FreshlyBuiltBatch<Self>>;

    /// Called after [`BatchBuilder::build_next_batch`] to reset the batch
    /// builder.
    async fn clear_batch(&mut self) -> anyhow::Result<()>;
}

/// A transaction that has been accepted by the batch builder.
#[serde_with::serde_as]
#[derive(Debug, Clone, serde::Serialize)]
pub struct AcceptedTx<C> {
    /// Encoded transaction, as will appear on-chain.
    #[serde_as(as = "serde_with::base64::Base64")]
    pub tx: FullyBakedTx,
    /// Hash of the transaction.
    pub tx_hash: TxHash,
    /// Confirmation data. Could be empty, a receipt, or other data.
    pub confirmation: C,
}

impl<C> AcceptedTx<C> {
    /// Maps the inner confirmation data.
    pub fn map_confirmation<D>(self, f: impl FnOnce(C) -> D) -> AcceptedTx<D> {
        AcceptedTx {
            tx: self.tx,
            tx_hash: self.tx_hash,
            confirmation: f(self.confirmation),
        }
    }
}

/// Error type that can possibly arise during [`BatchBuilder::accept_tx`].
#[derive(Debug)]
pub struct AcceptTxError {
    /// The HTTP status code to return to the client.
    pub http_status: u16,
    /// Short, human-readable error message in English.
    pub title: String,
    /// Any additional information that might be useful for debugging. Will be sent to the client.
    pub details: String,
}

/// An encoded transaction with its hash as returned by
/// [`BatchBuilder::build_next_batch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxWithHash {
    /// Encoded transaction.
    pub fully_baked_tx: FullyBakedTx,
    /// Transaction hash.
    pub hash: TxHash,
}

/// The return type of [`BatchBuilder::build_next_batch`].
#[derive(Default, derivative::Derivative)]
#[derivative(Debug(bound = "B::Batch: Debug"))]
pub struct FreshlyBuiltBatch<B: BatchBuilder> {
    /// Actual batch data, which will then be serialized using
    /// and published to the DA layer.
    pub inner: B::Batch,
    /// Metadata about the transactions contained in the batch. This data is
    /// *not* part of the batch itself nor will it be posted onto the DA layer.
    pub hashes: Vec<TxHash>,
}

type AuthRes<S, Rt> = (
    TxScratchpad<<S as Spec>::Storage>,
    Result<
        (
            AuthenticationOutput<
                S,
                <Rt as DispatchCall>::Decodable,
                <Rt as TransactionAuthenticator<S>>::AuthorizationData,
            >,
            BasicGasMeter<<S as Spec>::Gas>,
        ),
        AcceptTxError,
    >,
);

fn tx_auth<S, Da, Rt>(
    runtime: &Rt,
    mut tx_scratchpad: TxScratchpad<S::Storage>,
    gas_price: <S::Gas as Gas>::Price,
    sequencer_address: &Da::Address,
    input: <Rt as TransactionAuthenticator<S>>::Input,
) -> AuthRes<S, Rt>
where
    S: Spec,
    Da: DaSpec,
    Rt: Runtime<S, Da>,
{
    let gas_meter = match runtime.sequencer_authorization().authorize_sequencer(
        sequencer_address,
        &gas_price,
        &mut tx_scratchpad,
    ) {
        Ok(allowed_sequencer) => BasicGasMeter::new(allowed_sequencer.balance, gas_price),
        Err(AuthorizeSequencerError { reason }) => {
            error!(%reason, "Sequencer authorization failed");
            return (
                        tx_scratchpad,
                        Err(AcceptTxError {
                            http_status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                            title: "The sequencer is currently unavailable; contact the administrator if the problem persists".to_string(),
                            details: reason.to_string(),
                        }),
                    );
        }
    };

    let mut pre_exec_ws = tx_scratchpad.to_pre_exec_working_set(gas_meter);

    let auth_res = match runtime.authenticate(&input, &mut pre_exec_ws) {
        Ok(ok) => ok,
        Err(err) => {
            let details = err.to_string();
            let tx_scratchpad = pre_exec_ws.to_scratchpad_and_gas_meter().0;
            return (
                tx_scratchpad,
                Err(AcceptTxError {
                    // For certain kinds of authentication errors, 401
                    // or 403 would be more appropriate. But we'd have
                    // to inspect the error contents to determine the
                    // most appropriate status code... so 400 will do.
                    http_status: StatusCode::BAD_REQUEST.as_u16(),
                    title: "The transaction is invalid".to_string(),
                    details,
                }),
            );
        }
    };

    let (tx_scratchpad, gas_meter) = pre_exec_ws.to_scratchpad_and_gas_meter();

    (tx_scratchpad, Ok((auth_res, gas_meter)))
}
