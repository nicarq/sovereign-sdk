//! Defines the [`BatchBuilder`] trait and related types. Implementations of the trait
//! are nested under this module.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use borsh::BorshSerialize;
use sov_db::sequencer_db::SeqDbTx;
use sov_modules_api::capabilities::{
    AuthenticationOutput, AuthorizationData, AuthorizeSequencerError, GasEnforcer, HasCapabilities,
    SequencerAuthorization, TransactionAuthenticator,
};
use sov_modules_api::rest::ApiState;
use sov_modules_api::{
    BasicGasMeter, DaSpec, DispatchCall, EventModuleName, FullyBakedTx, Gas, NestedEnumUtils,
    RawTx, RuntimeEventProcessor, RuntimeEventResponse, Spec, StateProvider, StateUpdateInfo,
    TxScratchpad,
};
use sov_modules_stf_blueprint::{PreExecError, Runtime};
use sov_rollup_interface::node::DaSyncState;
use tokio::task::JoinHandle;
use tracing::error;

use crate::sequencer::SequencerNotReadyDetails;
use crate::{SequencerConfig, TxHash, TxStatusManager};

pub mod preferred;
pub mod standard;

/// An aggregator of types for [`Runtime`]-aware
/// [`BatchBuilder`]s
///
/// This trait serves no purpose other than to reduce generics clutter in `impl`
/// blocks.
pub trait RtAwareBatchBuilderSpec: Send + Sync + 'static {
    /// The `Spec` defines the rollup's types.
    type Spec: Spec;
    /// The runtime of the rollup.
    type Rt: Runtime<Self::Spec>
        + HasCapabilities<Self::Spec, AuthorizationData = AuthorizationData<Self::Spec>>
        + TransactionAuthenticator<Self::Spec, AuthorizationData = AuthorizationData<Self::Spec>>;
}

impl<S, Rt> RtAwareBatchBuilderSpec for (S, Rt)
where
    S: Spec,
    Rt: Runtime<S>
        + HasCapabilities<S, AuthorizationData = AuthorizationData<S>>
        + TransactionAuthenticator<S, AuthorizationData = AuthorizationData<S>>
        + 'static,
{
    type Spec = S;
    type Rt = Rt;
}

/// [`BatchBuilder`] trait is responsible for accepting transactions and
/// assembling them into batches.
#[async_trait]
pub trait BatchBuilder: Sized + Send + Sync + 'static {
    /// See [`sov_modules_api::capabilities::TransactionAuthenticator::Input`].
    type TxInput: borsh::BorshSerialize + borsh::BorshDeserialize + Clone + Send + Sync + 'static;
    /// What data is returned to clients when a transaction is accepted.
    type Confirmation: SequencerConfirmation + serde::Serialize + Send + Sync + 'static;
    /// The batch type that will be serialized and sent to the DA layer.
    type Batch: BorshSerialize + Debug + Send + Sync + 'static;
    /// Arbitrary configuration value(s) fed to [`BatchBuilder::create`].
    type Config: Clone + Debug + Send + Sync + 'static;
    /// The rollup spec.
    type Spec: Spec;

    /// Encodes the transaction into the format accepted by [`BatchBuilder::accept_tx`].
    //
    // TODO(@neysofu): in the future, different sequencer endpoints will encode
    // transactions differently to support multiple transaction types.
    fn encode_tx(raw: RawTx) -> Self::TxInput;

    /// Returns an [`ApiState`] subscribed to updates of the batch builder's state.
    fn api_state(&self) -> ApiState<Self::Spec>;

    /// Checks whether the batch builder is ready to accept transactions.
    fn is_ready(&self) -> Result<(), SequencerNotReadyDetails>;

    /// Creates a new [`BatchBuilder`].
    async fn create(
        latest_state_info: StateUpdateInfo<<Self::Spec as Spec>::Storage>,
        da_sync_state: Arc<DaSyncState>,
        seq_db_txs: Vec<SeqDbTx>,
        config: &SequencerConfig<
            <Self::Spec as Spec>::Da,
            <Self::Spec as Spec>::Address,
            Self::Config,
        >,
    ) -> anyhow::Result<(Self, Option<JoinHandle<()>>)>;

    /// Returns a copy of the [`TxStatusManager`] that the [`BatchBuilder`] uses
    /// to notify about dropped transactions.
    fn tx_status_manager(&self) -> TxStatusManager<<Self::Spec as Spec>::Da>;

    /// Updates the sequencer's view of the state of the rollup.
    async fn update_state(&mut self, update_info: StateUpdateInfo<<Self::Spec as Spec>::Storage>);

    /// Adds a **not-encoded** transaction to the mempool. The [`BatchBuilder`]
    /// implementation itself is responsible for "encoding" the transaction.
    ///
    /// Can return an error if transaction is invalid or mempool is full.
    async fn accept_tx(
        &mut self,
        tx: Self::TxInput,
    ) -> Result<AcceptedTx<Self::Confirmation>, AcceptTxError>;

    /// Builds a new batch out of transactions in mempool.
    /// The logic of which transactions and how many of them are included in
    /// batch is up to implementation.
    async fn build_next_batch(
        &mut self,
        sequence_number: u64,
    ) -> anyhow::Result<FreshlyBuiltBatch<Self>>;

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

/// Common interface for [`BatchBuilder::Confirmation`].
pub trait SequencerConfirmation {
    /// The generic type of [`RuntimeEventResponse`].
    type EventInner: EventModuleName
        + Clone
        + borsh::BorshDeserialize
        + borsh::BorshSerialize
        + serde::Serialize
        + serde::de::DeserializeOwned
        + Send
        + Sync
        + 'static;

    /// Extracts all events from this transaction confirmation.
    fn events(&self) -> Vec<RuntimeEventResponse<Self::EventInner>>;
}

/// Empty transaction confirmation data. See [`standard::StdBatchBuilder`].
#[derive(Clone, serde::Serialize)]
pub struct EmptyConfirmation<Z>(PhantomData<Z>);

impl<Z: RtAwareBatchBuilderSpec> SequencerConfirmation for EmptyConfirmation<Z> {
    type EventInner = <Z::Rt as RuntimeEventProcessor>::RuntimeEvent;

    fn events(&self) -> Vec<RuntimeEventResponse<Self::EventInner>> {
        vec![]
    }
}

type AuthRes<S, Rt, I> = (
    TxScratchpad<S, I>,
    Result<
        (
            AuthenticationOutput<
                S,
                <Rt as DispatchCall>::Decodable,
                <Rt as TransactionAuthenticator<S>>::AuthorizationData,
            >,
            BasicGasMeter<<S as Spec>::Gas>,
        ),
        PreExecError,
    >,
);

fn pre_exec_err_to_accept_tx_err(err: PreExecError) -> AcceptTxError {
    match err{
        PreExecError::SequencerError(error) => {
            AcceptTxError {
                http_status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                title: "The sequencer is currently unavailable; contact the administrator if the problem persists".to_string(),
                details: error.to_string(),
            }

        },
        PreExecError::AuthError(error) => {
            // For certain kinds of authentication errors, 401
            // or 403 would be more appropriate. But we'd have
            // to inspect the error contents to determine the
            // most appropriate status code... so 400 will do.
            AcceptTxError {
                http_status: StatusCode::BAD_REQUEST.as_u16(),
                title: "The transaction is invalid".to_string(),
                details:error.to_string(),
            }
        },
    }
}

fn generic_accept_tx_error(details: impl std::fmt::Debug) -> AcceptTxError {
    AcceptTxError {
        http_status: StatusCode::BAD_REQUEST.as_u16(),
        title: "The transaction is invalid".to_string(),
        details: format!("{:?}", details),
    }
}

fn tx_auth<S, Rt, I>(
    runtime: &Rt,
    mut tx_scratchpad: TxScratchpad<S, I>,
    gas_price: <S::Gas as Gas>::Price,
    sequencer_address: &<S::Da as DaSpec>::Address,
    input: <Rt as TransactionAuthenticator<S>>::Input,
) -> AuthRes<S, Rt, I>
where
    S: Spec,
    Rt: Runtime<S>,
    I: StateProvider<S>,
{
    let max_auth_cost = runtime
        .gas_enforcer()
        .max_tx_check_costs()
        .value(&gas_price);
    let gas_meter: BasicGasMeter<S::Gas> = match runtime
        .sequencer_authorization()
        .authorize_sequencer(sequencer_address, max_auth_cost, &mut tx_scratchpad)
    {
        Ok(allowed_sequencer) => BasicGasMeter::new(allowed_sequencer.balance, gas_price),
        Err(AuthorizeSequencerError { reason }) => {
            error!(%reason, "Sequencer authorization failed");
            return (tx_scratchpad, Err(PreExecError::SequencerError(reason)));
        }
    };

    let mut pre_exec_ws = tx_scratchpad.to_pre_exec_working_set(gas_meter);

    let auth_res = match runtime.authenticate(&input, &mut pre_exec_ws) {
        Ok(ok) => ok,
        Err(err) => {
            let tx_scratchpad = pre_exec_ws.to_scratchpad_and_gas_meter().0;
            return (tx_scratchpad, Err(PreExecError::AuthError(err)));
        }
    };

    let (tx_scratchpad, gas_meter) = pre_exec_ws.to_scratchpad_and_gas_meter();
    (tx_scratchpad, Ok((auth_res, gas_meter)))
}

/// Checks if the sender of a call is allowed to submit the message via the sequencer.
///
/// Returns true if either...
/// 1. The message is `sequencer_safe`, meaning that submitting will never change the sequencer's config
/// 2. The sender is an admin for this sequencer
fn sender_is_allowed<RT: Runtime<S>, S: Spec>(
    runtime: &RT,
    call: &<RT as DispatchCall>::Decodable,
    sender: Option<&S::Address>,
    sequencer_address: &<S::Da as DaSpec>::Address,
    admins: &[S::Address],
) -> bool {
    let destination_module = <RT as DispatchCall>::module_info(runtime, call.discriminant());
    destination_module.is_safe_for_sequencer(call.contents(), sequencer_address)
        || sender.is_some_and(|addr| admins.contains(addr))
}
