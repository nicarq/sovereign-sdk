//! BatchBuilder without any validation or state.
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sov_modules_api::capabilities::AuthenticationError;
use sov_modules_api::rest::ApiState;
use sov_modules_api::{FullyBakedTx, RawTx, Runtime, Spec, StateCheckpoint};
use sov_rest_utils::ErrorObject;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::{StateUpdateInfo, TxHash};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::batch_builders::{AcceptedTx, BatchBuilder, EmptyConfirmation, WithCachedTxHashes};
use crate::sequencer::SequencerNotReadyDetails;
use crate::{SequencerConfig, TxStatus, TxStatusManager};

#[derive(Clone)]
struct Inner<S: Spec> {
    // Storage is not used but needed for building WorkingSet.
    storage: S::Storage,
    mempool: Vec<FullyBakedTx>,
}

/// BatchBuilder that accepts any transaction without verification.
/// Build a batch out of all accepted transactions in the order they were received.
/// Does not impose any restrictions on transaction validity or batch size.
#[derive(Clone)]
pub struct TestStatelessBatchBuilder<R, S: Spec> {
    inner: Arc<Mutex<Inner<S>>>,
    _r: PhantomData<R>,
    state_sender: watch::Sender<StateCheckpoint<S>>,
}

impl<R, S> TestStatelessBatchBuilder<R, S>
where
    R: Runtime<S>,
    S: Spec,
{
    /// Creates new empty [`TestStatelessBatchBuilder`].
    pub fn new(storage: S::Storage) -> Self {
        let runtime = R::default();
        let kernel = Arc::new(runtime.kernel());
        let inner = Mutex::new(Inner {
            storage: storage.clone(),
            mempool: vec![],
        });
        let (state_sender, _rec) = watch::channel(StateCheckpoint::new(storage, &*kernel));
        Self {
            inner: inner.into(),
            _r: Default::default(),
            state_sender,
        }
    }

    async fn accept_encoded_tx(&self, tx: FullyBakedTx) -> AcceptedTx<EmptyConfirmation<R>> {
        let mut inner = self.inner.lock().await;
        let tx_hash = self.get_tx_hash(&tx, inner.storage.clone());
        inner.mempool.push(tx.clone());
        AcceptedTx {
            tx,
            tx_hash,
            confirmation: EmptyConfirmation(PhantomData),
        }
    }

    async fn take_batch(&self) -> WithCachedTxHashes<Vec<FullyBakedTx>> {
        let (mempool_txs, storage) = {
            let mut inner = self.inner.lock().await;
            (std::mem::take(&mut inner.mempool), inner.storage.clone())
        };
        let tx_hashes: Vec<_> = mempool_txs
            .iter()
            .map(|fully_baked_tx| self.get_tx_hash(fully_baked_tx, storage.clone()))
            .collect();
        WithCachedTxHashes {
            inner: mempool_txs,
            tx_hashes,
        }
    }

    fn get_tx_hash(&self, tx: &FullyBakedTx, storage: S::Storage) -> TxHash {
        let runtime = R::default();

        let checkpoint = StateCheckpoint::new(storage, &runtime.kernel());
        let mut tx_scratchpad = checkpoint.to_working_set_unmetered();

        match runtime.authenticate(tx, &mut tx_scratchpad) {
            Ok((a, _, _)) => a.raw_tx_hash,
            Err(err) => match err {
                AuthenticationError::FatalError(err, tx_hash) => {
                    tracing::trace!(?err, "Error during auth");
                    tx_hash
                }
                AuthenticationError::OutOfGas(_) => {
                    panic!("unmetered working set went ouf of gas");
                }
            },
        }
    }
}

#[async_trait]
impl<R, S> BatchBuilder for TestStatelessBatchBuilder<R, S>
where
    R: Runtime<S>,
    S: Spec,
{
    type Confirmation = EmptyConfirmation<R>;
    type Batch = Vec<FullyBakedTx>;
    type Config = ();
    type Spec = S;
    const PARALLEL_DA_SUBMISSION: bool = false;

    fn encode_tx(raw: RawTx) -> FullyBakedTx {
        R::encode_with_standard_auth(raw)
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        let runtime = R::default();

        ApiState::build(
            Default::default(),
            self.state_sender.subscribe(),
            runtime.kernel_with_slot_mapping(),
            None,
        )
    }

    fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        Ok(())
    }

    async fn tx_status(
        &self,
        _tx_hash: &TxHash,
    ) -> anyhow::Result<TxStatus<<<Self::Spec as Spec>::Da as DaSpec>::TransactionId>> {
        // We could technically iterate over mempool and hash every tx there to find matches...
        Ok(TxStatus::Unknown)
    }

    async fn create(
        latest_state_info: StateUpdateInfo<<Self::Spec as Spec>::Storage>,
        _tx_status_manager: TxStatusManager<<Self::Spec as Spec>::Da>,
        _da_sync_state: Arc<DaSyncState>,
        _storage_path: &Path,
        _config: &SequencerConfig<
            <Self::Spec as Spec>::Da,
            <Self::Spec as Spec>::Address,
            Self::Config,
        >,
    ) -> anyhow::Result<(Self, Option<JoinHandle<()>>)> {
        Ok((Self::new(latest_state_info.storage), None))
    }

    async fn update_state(&self, update_info: StateUpdateInfo<<Self::Spec as Spec>::Storage>) {
        let mut inner = self.inner.lock().await;
        inner.storage = update_info.storage;
    }

    async fn accept_tx(
        &self,
        tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject> {
        Ok(self.accept_encoded_tx(tx).await)
    }

    async fn assemble_batch(&self) -> anyhow::Result<Option<()>> {
        // There's always a batch ready.
        Ok(Some(()))
    }

    async fn peek_batches(&self) -> anyhow::Result<Vec<WithCachedTxHashes<Self::Batch>>> {
        Ok(vec![self.take_batch().await])
    }

    async fn pop_batch(&self) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().await;
        inner.mempool.clear();
        Ok(())
    }
}
