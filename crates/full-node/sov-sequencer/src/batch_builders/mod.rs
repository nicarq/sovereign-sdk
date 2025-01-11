//! Defines the [`BatchBuilder`] trait and related types. Implementations of the trait
//! are nested under this module.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use borsh::{BorshDeserialize, BorshSerialize};
use sov_modules_api::capabilities::{
    AuthenticationOutput, AuthorizationData, AuthorizeSequencerError, HasCapabilities,
    SequencerAuthorization, TransactionAuthenticator,
};
use sov_modules_api::rest::utils::ErrorObject;
use sov_modules_api::rest::ApiState;
use sov_modules_api::{
    BasicGasMeter, DaSpec, DispatchCall, EventModuleName, FullyBakedTx, Gas, GasSpec,
    NestedEnumUtils, RawTx, RuntimeEventProcessor, RuntimeEventResponse, Spec, StateProvider,
    StateUpdateInfo, TxScratchpad,
};
use sov_modules_stf_blueprint::{PreExecError, Runtime};
use sov_rest_utils::json_obj;
use sov_rollup_interface::node::DaSyncState;
use tokio::task::JoinHandle;
use tracing::{error, trace};
use uuid::Uuid;

use crate::sequencer::SequencerNotReadyDetails;
use crate::{SequencerConfig, TxHash, TxStatus, TxStatusManager};

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
    /// What data is returned to clients when a transaction is accepted.
    type Confirmation: SequencerConfirmation;
    /// The batch type that will be serialized and sent to the DA layer.
    type Batch: BorshSerialize + Debug + Send + Sync + 'static;
    /// Arbitrary configuration value(s) fed to [`BatchBuilder::create`].
    type Config: Clone + Debug + Send + Sync + 'static;
    /// The rollup spec.
    type Spec: Spec;

    /// Encodes the transaction into the format accepted by [`BatchBuilder::accept_tx`].
    fn encode_tx(raw: RawTx) -> FullyBakedTx;

    /// Returns an [`ApiState`] subscribed to updates of the batch builder's state.
    fn api_state(&self) -> ApiState<Self::Spec>;

    /// Checks whether the batch builder is ready to accept transactions.
    fn is_ready(&self) -> Result<(), SequencerNotReadyDetails>;

    /// Queries a transaction's status.
    async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<TxStatus<<<Self::Spec as Spec>::Da as DaSpec>::TransactionId>>;

    /// Creates a new [`BatchBuilder`].
    async fn create(
        latest_state_info: StateUpdateInfo<<Self::Spec as Spec>::Storage>,
        tx_status_manager: TxStatusManager<<Self::Spec as Spec>::Da>,
        da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        config: &SequencerConfig<
            <Self::Spec as Spec>::Da,
            <Self::Spec as Spec>::Address,
            Self::Config,
        >,
    ) -> anyhow::Result<(Self, Option<JoinHandle<()>>)>;

    /// Updates the sequencer's view of the state of the rollup.
    async fn update_state(&mut self, update_info: StateUpdateInfo<<Self::Spec as Spec>::Storage>);

    /// Adds a **not-encoded** transaction to the mempool. The [`BatchBuilder`]
    /// implementation itself is responsible for "encoding" the transaction.
    ///
    /// Can return an error if transaction is invalid or mempool is full.
    async fn accept_tx(
        &mut self,
        tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject>;

    /// Builds a new batch out of transactions in mempool.
    ///
    /// The logic of which transactions and how many of them are included in
    /// batch is up to implementation.
    async fn assemble_batch(&mut self) -> anyhow::Result<()>;

    /// Peeks the earliest assembled batch that hasn't been popped yet.
    ///
    /// FIXME(@neysofu): the assemble/peek/pop pattern of [`BatchBuilder`]
    /// doesn't quite support reorgs as-is. We probably ought to offer an API
    /// that allows to "rewind" the batch builder to a given unfinalized height.
    async fn peek_batch(&mut self) -> anyhow::Result<Option<WithCachedTxHashes<Self::Batch>>>;

    /// Pops the earliest assembled batch that hasn't been popped yet.
    ///
    /// Popped batches are lost forever.
    async fn pop_batch(&mut self) -> anyhow::Result<()>;
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

/// The return type of [`BatchBuilder::peek_batch`].
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct WithCachedTxHashes<I> {
    /// Inner batch data.
    pub inner: I,
    /// Metadata about the transactions contained in the batch. This data is
    /// *not* part of the batch itself nor will it be posted onto the DA layer.
    pub tx_hashes: Vec<TxHash>,
}

/// Common interface for [`BatchBuilder::Confirmation`].
pub trait SequencerConfirmation: serde::Serialize + Send + Sync + 'static {
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
            BasicGasMeter<S>,
        ),
        PreExecError,
    >,
);

fn pre_exec_err_to_accept_tx_err(err: PreExecError) -> ErrorObject {
    match err{
        PreExecError::SequencerError(error) => {
            ErrorObject {
                status: StatusCode::SERVICE_UNAVAILABLE,
                title: "The sequencer is currently unavailable; contact the administrator if the problem persists".to_string(),
                details: json_obj!({
                    "message": error.to_string()
                })
            }

        },
        PreExecError::AuthError(error) => {
            // For certain kinds of authentication errors, 401
            // or 403 would be more appropriate. But we'd have
            // to inspect the error contents to determine the
            // most appropriate status code... so 400 will do.
            ErrorObject {
                status: StatusCode::BAD_REQUEST,
                title: "The transaction is invalid".to_string(),
                details: json_obj!({
                    "message": error.to_string()
                })
            }
        },
    }
}

fn generic_accept_tx_error(details: impl std::fmt::Debug) -> ErrorObject {
    ErrorObject {
        status: StatusCode::BAD_REQUEST,
        title: "The transaction is invalid".to_string(),
        details: json_obj!({
            "message": format!("{:?}", details)
        }),
    }
}

fn tx_auth<S, Rt, I>(
    runtime: &Rt,
    mut tx_scratchpad: TxScratchpad<S, I>,
    gas_price: <S::Gas as Gas>::Price,
    sequencer_address: &<S::Da as DaSpec>::Address,
    baked_tx: &FullyBakedTx,
) -> AuthRes<S, Rt, I>
where
    S: Spec,
    Rt: Runtime<S>,
    I: StateProvider<S>,
{
    let gas_meter: BasicGasMeter<S> = match runtime
        .sequencer_authorization()
        .authorize_sequencer(sequencer_address, &mut tx_scratchpad)
    {
        Ok(sequencer) => BasicGasMeter::new(
            sequencer.balance,
            <S as GasSpec>::max_tx_check_costs(),
            gas_price,
        ),
        Err(AuthorizeSequencerError { reason }) => {
            error!(%reason, "Sequencer authorization failed");
            return (tx_scratchpad, Err(PreExecError::SequencerError(reason)));
        }
    };

    let mut pre_exec_ws = tx_scratchpad.to_pre_exec_working_set(gas_meter);

    let auth_res = match runtime.authenticate(baked_tx, &mut pre_exec_ws) {
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

/// ID of a [`SeqDbTx`].
pub(crate) type SeqDbTxId = u128;

/// Wrapper around encoded transactions that is ideal for database storage.
///
/// Transaction hashes are cached together with the transaction itself, and each
/// transaction is assigned a monotonically increasing
/// [UUIDv7](https://en.wikipedia.org/wiki/Universally_unique_identifier#Version_7_(timestamp_and_random)),
/// which is then converted to a [`u128`].
///
/// Note, this is not part of the [`BatchBuilder`] interface and it's just a
/// utility that [`BatchBuilder`] implementations MAY use.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) struct SeqDbTx {
    /// The encoded transaction bytes.
    pub tx: FullyBakedTx,
    /// The hash of the transaction, as calculated by
    /// the batch builder.
    pub hash: TxHash,
    /// A monotonically increasing UUIDv7 counter used to order transactions by
    /// insertion time. Gaps are allowed.
    pub uuid_v7: u128,
}

impl SeqDbTx {
    /// Creates a new [`SeqDbTx`] from the given transaction bytes.
    pub fn new(hash: TxHash, tx: FullyBakedTx) -> Self {
        // UUIDv7 are monotonically increasing. See here:
        // <https://github.com/uuid-rs/uuid/releases/tag/1.9.0>.
        let uuid_v7 = Uuid::now_v7().as_u128();

        trace!(uuid_v7, "Generating a new `SeqDbTx`");

        Self { tx, hash, uuid_v7 }
    }
}
