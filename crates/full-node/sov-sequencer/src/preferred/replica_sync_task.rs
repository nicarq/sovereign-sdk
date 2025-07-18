use std::num::NonZero;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::anyhow;
use futures::stream::FuturesOrdered;
use futures::{Future, StreamExt};
use serde::{Deserialize, Serialize};
use sov_modules_api::{FullyBakedTx, Runtime, Spec, StateUpdateInfo, TxHash, VisibleSlotNumber};
use sov_rollup_interface::node::da::DaService;
use sqlx::postgres::{PgListener, PgPoolOptions};
use sqlx::{PgPool, Row};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use super::block_executor::AcceptedTxWithBudgetInfo;
use super::db::StoredBlob;
use crate::preferred::{exit_rollup, DbEvent, ExecutorEvent, PreferredSequencer};
use crate::{ProofBlobSender, SequencerNotReadyDetails};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "event_type", rename_all = "snake_case")]
enum EventType {
    Transaction,
    BatchStart,
    BatchEnd,
    NewProof,
}

impl FromStr for EventType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "transaction" => Ok(EventType::Transaction),
            "batch_start" => Ok(EventType::BatchStart),
            "batch_end" => Ok(EventType::BatchEnd),
            "new_proof" => Ok(EventType::NewProof),
            _ => Err(anyhow!("Invalid event type: {}", s)),
        }
    }
}

// Structures representing the CSV payload from PostgreSQL NOTIFY

#[derive(Debug)]
struct EventsNotificationPayload {
    event_id: u64,
    sequence_number: u64,
    event_type: EventType,
    index_in_batch: Option<u64>, // Only present for transaction events
}
#[derive(Debug)]
struct LeaderNotificationPayload {
    node_id: Uuid,
    last_updated_timestamp: f64,
}

impl EventsNotificationPayload {
    fn parse_csv(payload: &str) -> anyhow::Result<Self> {
        let parts: Vec<&str> = payload.split(',').collect();
        if parts.len() != 4 {
            return Err(anyhow!(
                "Invalid CSV format: expected 4 fields, got {}",
                parts.len()
            ));
        }

        Ok(EventsNotificationPayload {
            event_id: parts[0]
                .parse()
                .map_err(|e| anyhow!("Invalid event_id '{}': {}", parts[0], e))?,
            sequence_number: parts[1]
                .parse()
                .map_err(|e| anyhow!("Invalid sequence_number '{}': {}", parts[1], e))?,
            event_type: parts[2]
                .parse()
                .map_err(|e| anyhow!("Invalid event_type '{}': {}", parts[2], e))?,
            index_in_batch: if parts[3].is_empty() {
                None
            } else {
                Some(
                    parts[3]
                        .parse()
                        .map_err(|e| anyhow!("Invalid index_in_batch '{}': {}", parts[3], e))?,
                )
            },
        })
    }
}

impl LeaderNotificationPayload {
    fn parse_csv(payload: &str) -> anyhow::Result<Self> {
        let parts: Vec<&str> = payload.split(',').collect();
        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid leader notification CSV format: expected 2 fields, got {}",
                parts.len()
            ));
        }

        Ok(LeaderNotificationPayload {
            node_id: parts[0]
                .parse()
                .map_err(|e| anyhow!("Invalid node_id '{}': {}", parts[0], e))?,
            last_updated_timestamp: parts[1]
                .parse()
                .map_err(|e| anyhow!("Invalid timestamp '{}': {}", parts[1], e))?,
        })
    }
}

/// Structure representing batch metadata stored by the master sequencer
#[derive(Debug)]
struct BatchMetadata {
    visible_slot_number_after_increase: VisibleSlotNumber,
    visible_slots_to_advance: NonZero<u8>,
}

/// Type alias for futures in the concurrent processing pipeline
type PendingEventFuture = Pin<Box<dyn Future<Output = anyhow::Result<CompletedEvent>> + Send>>;

/// Represents a completed event ready for processing
#[derive(Debug)]
enum CompletedEvent {
    Event(DbEvent),
    Backfill,
}

/// Background task for replica sequencers to sync state from the master sequencer's database.
/// This task monitors the `txs` table for new batches and updates the replica's state accordingly.
pub async fn spawn_replica_sync_task<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    shutdown_receiver: watch::Receiver<()>,
    latest_state_update: StateUpdateInfo<S::Storage>,
    connection_string: String,
    latest_loaded_event_id: Option<u64>,
) -> JoinHandle<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    if let Err(e) = sequencer
        .replay_soft_confirmations_on_top_of_node_state(
            latest_state_update,
            std::time::Instant::now(),
            true,
            std::time::Duration::from_secs(0),
        )
        .await
    {
        error!(
            "Replica failed to replay existing soft confirmation on startup: {e:?}. Shutting down."
        );
        exit_rollup(&sequencer.shutdown_sender).await;
        unreachable!()
    }

    tokio::spawn(async move {
        if let Err(e) = ReplicationTask::connect_and_run(
            sequencer.clone(),
            &connection_string,
            shutdown_receiver,
            latest_loaded_event_id,
        )
        .await
        {
            error!("Replication task failed: {e:?}");
            exit_rollup(&sequencer.shutdown_sender).await;
        }
    })
}

/// Replication state for handling master/replica transitions
/// We need a transitional state to finish processing any state events from the master that were
/// still buffered at the point where we succeeded in takeover
#[derive(Debug, Clone, Copy, PartialEq)]
enum ReplicaState {
    Replica,
    TransitioningToMaster,
    Master,
}

/// Replication task state and handlers
struct ReplicationTask<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    query_pool: PgPool,
    listener: PgListener,
    latest_received_event_id: Option<u64>,

    // Role tracking
    replica_state: ReplicaState,
    last_heartbeat_time: SystemTime,

    // Timing
    failover_threshold: Duration,
}

impl<S, Rt, Da> ReplicationTask<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    async fn connect_and_run(
        sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
        connection_string: &str,
        mut shutdown_receiver: watch::Receiver<()>,
        latest_loaded_event_id: Option<u64>,
    ) -> anyhow::Result<()> {
        debug!("Starting replica sync task for a replica sequencer");

        let query_pool = PgPoolOptions::default()
            .max_connections(20) // Large pool for extra parallel queries when processing txs
            .connect(connection_string)
            .await?;

        let mut listener = PgListener::connect(connection_string).await?;

        listener.listen("leader_changes").await?;

        // All nodes start as replicas and need to listen to events_changes for syncing.
        // If this is the only node it will immediately promote to master and stop listening (but
        // since it's the only node there it won't receive any events in the meantime).
        listener.listen("events_changes").await?;

        debug!("Successfully connected and listening for PostgreSQL notifications");

        let failover_threshold = Duration::from_millis(
            sequencer
                .config
                .sequencer_kind_config
                .failover_threshold_millis,
        );

        let mut task = Self {
            sequencer,
            query_pool,
            listener,
            latest_received_event_id: latest_loaded_event_id,
            replica_state: ReplicaState::Replica,
            // Initialize to a time in the past to force an immediate takeover attempt on startup
            last_heartbeat_time: SystemTime::now() - failover_threshold - Duration::from_secs(1),
            failover_threshold,
        };

        task.run(&mut shutdown_receiver).await
    }

    async fn run(&mut self, shutdown_receiver: &mut watch::Receiver<()>) -> anyhow::Result<()> {
        // This determines the maximum number of futures. The futures are used to fetch extra info from
        // the DB when a notification comes in, so that notification processing doesn't block on this.
        // In theory, this will only grow above the pool's max_connections() if either the replica is
        // processing transactions slower than the master, or when the replica hits a backfill - in
        // both cases, accumulating completed futures in the queue.
        // The PgListener has its own internal queue, so hitting this limit will just shift where the
        // notifications are queued. Having a large number of completed futures just ensures we'll be
        // able to start processing events immediately with all relevant DB lookups already completed,
        // in the case of a backfill. (In case the replica is too slow, we're in trouble anyway.)
        const MAX_CONCURRENT: usize = 1000;
        let mut pending_events: FuturesOrdered<PendingEventFuture> = FuturesOrdered::new();

        let mut heartbeat_interval = tokio::time::interval(Duration::from_millis(100));
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Create a takeover interval that ticks immediately on startup to check for leadership
        let mut takeover_interval =
            tokio::time::interval_at(tokio::time::Instant::now(), Duration::from_millis(200));
        takeover_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        if let Ok(true) = attempt_leadership_takeover(
            &self.query_pool,
            self.sequencer.node_id,
            self.failover_threshold,
        )
        .await
        {
            info!("Successfully claimed leadership on sequencer startup - transitioning to master (most likely, we're the only running node)");
            self.start_transition_to_master(&mut pending_events).await?;
        }

        loop {
            tokio::select! {
                // Master heartbeat timer
                _ = heartbeat_interval.tick(), if matches!(self.replica_state, ReplicaState::Master | ReplicaState::TransitioningToMaster) => {
                    self.tick_heartbeat().await;
                },

                // PostgreSQL notifications (both leader changes and events)
                notification_result = self.listener.recv(), if pending_events.len() < MAX_CONCURRENT => {
                    match notification_result {
                        Ok(notification) => {
                            match notification.channel() {
                                "leader_changes" => {
                                    self.handle_leader_change_notification(notification.payload(), &mut pending_events).await?;
                                    // Check if we need to complete transition to master after the
                                    // notification, and if we can do it immediately
                                    if self.replica_state == ReplicaState::TransitioningToMaster && pending_events.is_empty() {
                                        self.complete_transition_to_master().await?;
                                    }
                                }
                                "events_changes" => {
                                    self.handle_event_notification(notification.payload(), &mut pending_events).await?;
                                }
                                _ => {
                                    debug!("Received notification on unknown channel: {}", notification.channel());
                                }
                            }
                        }
                        Err(e) => {
                            error!("Error receiving PostgreSQL notification: {e:?}. Will sleep then retry.");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                },

                // Process events in FIFO order, once we've fetched what we need from the DB
                Some(completed_result) = pending_events.next() => {
                    process_completed_event(self.sequencer.clone(), completed_result?, &self.query_pool).await?;

                    // Check if we can complete transition to master, if we need to
                    if self.replica_state == ReplicaState::TransitioningToMaster && pending_events.is_empty() {
                        self.complete_transition_to_master().await?;
                    }
                },

                _ = takeover_interval.tick(), if self.replica_state == ReplicaState::Replica => {
                    if let Err(e) = self.tick_takeover(&mut pending_events).await {
                        warn!("Failed takeover attempt: {e:?}");
                    } else {
                        // Check if we can immediately complete transition to master
                        if self.replica_state == ReplicaState::TransitioningToMaster && pending_events.is_empty() {
                            self.complete_transition_to_master().await?;
                            }
                        }
                },

                _ = shutdown_receiver.changed() => {
                    info!("Shutdown signal received, stopping event processing");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn tick_heartbeat(&mut self) {
        match send_heartbeat(&self.query_pool, self.sequencer.node_id).await {
            Ok((rows_affected, current_node_id)) => {
                match (rows_affected, current_node_id) {
                    (1, Some(our_node_id)) if our_node_id == self.sequencer.node_id => {
                        debug!(
                            "Heartbeat successfully sent for node {}",
                            self.sequencer.node_id
                        );
                    }
                    (0, Some(other_node_id)) if other_node_id != self.sequencer.node_id => {
                        warn!(
                            "Another node {} has taken over as master. Transitioning to replica.",
                            other_node_id
                        );
                        if let Err(e) = self.transition_to_replica().await {
                            error!("Failed to transition to replica: {e:?}. Shutting down.");
                            exit_rollup(&self.sequencer.shutdown_sender).await;
                        }
                    }
                    (rows, node_id) => {
                        // Sanity check: more than one row was affected by the insert
                        error!(
                            "Unexpected heartbeat result: rows_affected={}, current_node_id={:?}, our_node_id={}. This should not be possible. Shutting down.",
                            rows, node_id, self.sequencer.node_id
                        );
                        exit_rollup(&self.sequencer.shutdown_sender).await;
                        unreachable!();
                    }
                }
            }
            Err(e) => {
                // Database error - log and continue. If this is transient, the next heartbeat
                // should succeed.
                // If it's a persistent connection failure, the sequencer cannot function and will
                // shut down anyway.
                warn!("Failed to send heartbeat: {e:?}. If this persists the sequencer may be demoted to a replica.");
            }
        }
    }

    async fn handle_leader_change_notification(
        &mut self,
        payload: &str,
        pending_events: &mut FuturesOrdered<PendingEventFuture>,
    ) -> anyhow::Result<()> {
        match LeaderNotificationPayload::parse_csv(payload) {
            Ok(leader_info) => {
                if leader_info.node_id == self.sequencer.node_id {
                    // This is our own heartbeat - we're the master sequencer
                    if self.replica_state == ReplicaState::Replica {
                        // We weren't master before but somehow now we are.
                        // The only way this should be possible would be through manual database
                        // editing. (Which is fine if it happens.)
                        info!("Replica promoted to master via DB notification.");
                        self.start_transition_to_master(pending_events).await?;
                    }
                } else {
                    // Another node is master
                    if matches!(
                        self.replica_state,
                        ReplicaState::Master | ReplicaState::TransitioningToMaster
                    ) {
                        warn!("Another node took over as master: {}. Downgrading to operate as a replica.", leader_info.node_id);
                        self.transition_to_replica().await?;
                    }
                    self.last_heartbeat_time = SystemTime::UNIX_EPOCH
                        + Duration::from_secs_f64(leader_info.last_updated_timestamp);
                }
            }
            Err(e) => {
                // Because we support some manual editing of the leader table, we ignore unknown
                // notifications; this way user error cannot bring down all sequencer instances at
                // once.
                // This is safe because all write operations are guarded by a node_id check anyway,
                // so split brain should not be possible. In the worst case a replica will not
                // promote to master when it should, but that's still better than all replicas
                // crashing.
                // The invalid row will be ignored, and the next valid change will trigger the
                // desired behaviour without issue.
                warn!("Failed to parse leader notification: {e:?}");
            }
        }
        Ok(())
    }

    /// Helper method to create event futures from notification payload
    fn create_event_futures_from_notification(
        &mut self,
        payload: &str,
    ) -> anyhow::Result<Vec<PendingEventFuture>> {
        let parsed_notification = EventsNotificationPayload::parse_csv(payload)?;
        let mut futures = Vec::new();

        // Check for gaps and add backfill if needed
        if detect_gap(self.latest_received_event_id, parsed_notification.event_id).is_some() {
            let backfill_future = create_backfill_future(
                self.sequencer.clone(),
                &self.query_pool,
                self.latest_received_event_id,
                parsed_notification.event_id,
            );
            futures.push(backfill_future);
        }

        self.latest_received_event_id = Some(parsed_notification.event_id);

        // Create future for current notification
        let event_future = create_event_future(parsed_notification, &self.query_pool);
        futures.push(event_future);

        Ok(futures)
    }

    async fn handle_event_notification(
        &mut self,
        payload: &str,
        pending_events: &mut FuturesOrdered<PendingEventFuture>,
    ) -> anyhow::Result<()> {
        // Handle database sync events (for replicas and transitioning nodes)
        if matches!(self.replica_state, ReplicaState::TransitioningToMaster) {
            return Err(anyhow!("Received new pg event while transitioning to master - no other sequencer should be writing to the DB anymore!"));
        }

        let futures = self.create_event_futures_from_notification(payload)?;
        for future in futures {
            pending_events.push_back(future);
        }

        Ok(())
    }

    /// Start transitioning to master
    async fn start_transition_to_master(
        &mut self,
        pending_events: &mut FuturesOrdered<PendingEventFuture>,
    ) -> anyhow::Result<()> {
        info!("Starting transition to master");
        self.replica_state = ReplicaState::TransitioningToMaster;

        // Drain all buffered notifications to avoid missing events written before we became master
        while let Some(notification) = self.listener.next_buffered() {
            match notification.channel() {
                "events_changes" => {
                    self.handle_event_notification(notification.payload(), pending_events)
                        .await?;
                }
                "leader_changes" => {
                    // Handle inline to avoid recursive call to start_transition_to_master.
                    // We don't need full handling, just a single check if we need to abort.
                    if let Ok(leader_info) =
                        LeaderNotificationPayload::parse_csv(notification.payload())
                    {
                        if leader_info.node_id != self.sequencer.node_id {
                            // Another node is suddenly master, abort our transition
                            info!("Another node became master before we finished transitioning, aborting");
                            self.transition_to_replica().await?;
                            return Ok(());
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Complete the transition from TransitioningToMaster to Master
    async fn complete_transition_to_master(&mut self) -> anyhow::Result<()> {
        // Unsubscribe from events_changes since we're now the master
        self.listener.unlisten("events_changes").await?;

        self.replica_state = ReplicaState::Master;
        self.sequencer.set_is_master(true).await;

        Ok(())
    }

    /// Transition from Master or TransitioningToMaster to Replica
    async fn transition_to_replica(&mut self) -> anyhow::Result<()> {
        self.listener.listen("events_changes").await?;

        self.replica_state = ReplicaState::Replica;
        self.sequencer.set_is_master(false).await;
        Ok(())
    }

    async fn tick_takeover(
        &mut self,
        pending_events: &mut FuturesOrdered<PendingEventFuture>,
    ) -> anyhow::Result<()> {
        let time_since_last_heartbeat = SystemTime::now()
            .duration_since(self.last_heartbeat_time)
            .unwrap_or(Duration::MAX);

        // No point in trying to take over if we aren't ready to operate just yet.
        // Worst case we'll take over once we're ready (i.e. finish syncing), best case another
        // replica is better positioned to take over and we shouldn't get in the way.
        if let Err(e) = {
            let inner = self.sequencer.lock_inner().await;
            inner.is_ready.clone()
        } {
            if !matches!(e, SequencerNotReadyDetails::ReplicaMode) {
                info!("Master heartbeat timeout detected; however, this replica is currently not ready to take over: {e:?}.");
                return Ok(());
            } else {
                // We don't set it to ReplicaMode anywhere. But guard against it in case we ever
                // do: it's obviously natural and we should proceed with takeover.
                // (If it ever becomes an expected value, this print can be removed.)
                debug!("Replica takeover: `inner.is_ready` was set to `SequencerNotReadyDetails::ReplicaMode`. This is not harmful, but is not expected to be possible.");
            }
        }

        if time_since_last_heartbeat > self.failover_threshold {
            info!(
                "Master heartbeat timeout detected ({:?} > {:?}). Attempting takeover...",
                time_since_last_heartbeat, self.failover_threshold
            );

            match attempt_leadership_takeover(
                &self.query_pool,
                self.sequencer.node_id,
                self.failover_threshold,
            )
            .await
            {
                Ok(true) => {
                    info!("Successfully claimed leadership - transitioning to master");
                    self.start_transition_to_master(pending_events).await?;
                }
                Ok(false) => {
                    info!("Another replica beat us to takeover or master recovered. Continuing to operate as replica.");
                }
                Err(e) => {
                    warn!("Takeover attempt failed with error; continuing to operate as replica. Error: {e:?}");
                }
            }
        }
        Ok(())
    }
}

/// Send a heartbeat to maintain leadership
/// Returns (rows_affected, current_node_id) for sanity checking
async fn send_heartbeat(query_pool: &PgPool, node_id: Uuid) -> anyhow::Result<(u64, Option<Uuid>)> {
    // Use a CTE to perform the update and return the current node_id in one query
    let row = sqlx::query(
        "WITH heartbeat_update AS (
            INSERT INTO sequencer_leader (node_id, last_updated) 
            VALUES ($1, NOW()) 
            ON CONFLICT (singleton) DO UPDATE SET 
                last_updated = EXCLUDED.last_updated 
            WHERE sequencer_leader.node_id = EXCLUDED.node_id
            RETURNING 1 as updated
        )
        SELECT 
            (SELECT COUNT(*) FROM heartbeat_update) as rows_affected,
            (SELECT node_id FROM sequencer_leader WHERE singleton = 1) as current_node_id",
    )
    .bind(node_id)
    .fetch_one(query_pool)
    .await?;

    let rows_affected = row.get::<i64, _>("rows_affected") as u64;
    let current_node_id: Option<Uuid> = row.get("current_node_id");

    trace!(
        "Heartbeat result: rows_affected={}, current_node_id={:?}",
        rows_affected,
        current_node_id
    );
    Ok((rows_affected, current_node_id))
}

/// Attempt to take over leadership atomically
async fn attempt_leadership_takeover(
    query_pool: &PgPool,
    node_id: Uuid,
    failover_threshold: Duration,
) -> anyhow::Result<bool> {
    let threshold_millis = failover_threshold.as_millis() as i64;

    // Atomic takeover: only succeed if the current leader's heartbeat is old enough
    let result = sqlx::query(
        "INSERT INTO sequencer_leader (node_id, last_updated) 
         VALUES ($1, NOW()) 
         ON CONFLICT (singleton) DO UPDATE SET 
             node_id = EXCLUDED.node_id, 
             last_updated = EXCLUDED.last_updated 
         WHERE sequencer_leader.last_updated < NOW() - INTERVAL '1 millisecond' * $2",
    )
    .bind(node_id)
    .bind(threshold_millis)
    .execute(query_pool)
    .await?;

    // Check if our update actually succeeded by verifying we're the current leader
    if result.rows_affected() > 0 {
        let current_leader: Option<Uuid> =
            sqlx::query_scalar("SELECT node_id FROM sequencer_leader WHERE singleton = 1")
                .fetch_optional(query_pool)
                .await?;

        Ok(current_leader == Some(node_id))
    } else {
        Ok(false)
    }
}

/// Detects if there's a gap between the last processed event and the current event
fn detect_gap(latest_received_event_id: Option<u64>, current_event_id: u64) -> Option<u64> {
    match latest_received_event_id {
        Some(last_id) if current_event_id > last_id + 1 => Some(last_id + 1),
        None if current_event_id > 1 => Some(1),
        _ => None,
    }
}

/// Creates a future for processing a single event notification
fn create_event_future(
    notification: EventsNotificationPayload,
    query_pool: &PgPool,
) -> PendingEventFuture {
    let pool = query_pool.clone();

    match notification.event_type {
        EventType::Transaction => {
            let sequence_number = notification.sequence_number;
            let index_in_batch = notification.index_in_batch.unwrap() as i64;

            Box::pin(async move {
                let (tx_hash, baked_tx) =
                    query_transaction_body_from_db(&pool, sequence_number, index_in_batch).await?;

                Ok(CompletedEvent::Event(DbEvent::TxAccepted(
                    baked_tx, tx_hash,
                )))
            })
        }
        EventType::BatchStart => {
            let sequence_number = notification.sequence_number;

            Box::pin(async move {
                let metadata = query_batch_metadata_from_db(&pool, sequence_number).await?;

                Ok(CompletedEvent::Event(DbEvent::BatchStarted {
                    sequence_number,
                    visible_slot_number_after_increase: metadata.visible_slot_number_after_increase,
                    visible_slots_to_advance: metadata.visible_slots_to_advance,
                }))
            })
        }
        EventType::BatchEnd => {
            let sequence_number = notification.sequence_number;

            Box::pin(
                async move { Ok(CompletedEvent::Event(DbEvent::BatchClosed(sequence_number))) },
            )
        }
        EventType::NewProof => {
            let sequence_number = notification.sequence_number;

            Box::pin(async move {
                Ok(CompletedEvent::Event(DbEvent::ProofBlobAccepted(
                    sequence_number,
                )))
            })
        }
    }
}

/// Creates a future for backfilling a gap in events using existing backfill logic
fn create_backfill_future<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    query_pool: &PgPool,
    current_latest: Option<u64>,
    target_event_id: u64,
) -> PendingEventFuture
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    let sequencer = sequencer.clone();
    let pool = query_pool.clone();

    Box::pin(async move {
        backfill_to_event_id(sequencer.clone(), &pool, current_latest, target_event_id).await?;
        Ok(CompletedEvent::Backfill)
    })
}

/// Processes a completed DB event
async fn process_completed_event<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    event: CompletedEvent,
    query_pool: &PgPool,
) -> anyhow::Result<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    match event {
        CompletedEvent::Event(db_event) => match db_event {
            DbEvent::TxAccepted(baked_tx, tx_hash) => do_new_tx(sequencer, tx_hash, baked_tx).await,
            DbEvent::BatchClosed(_) => do_batch_end(sequencer).await,
            DbEvent::BatchStarted {
                sequence_number: _,
                visible_slot_number_after_increase,
                visible_slots_to_advance,
            } => {
                do_batch_start(
                    sequencer,
                    visible_slot_number_after_increase,
                    visible_slots_to_advance,
                )
                .await
            }
            DbEvent::ProofBlobAccepted(sequence_number) => {
                do_proof_blob(sequencer, sequence_number, query_pool).await
            }
            DbEvent::Flushed(_) => Ok(()),
        },
        CompletedEvent::Backfill => {
            debug!("Backfill completed up to event_id");
            Ok(())
        }
    }
}

async fn do_batch_start<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    visible_slot_number_after_increase: VisibleSlotNumber,
    visible_slots_to_advance: NonZero<u8>,
) -> anyhow::Result<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    while {
        sequencer.lock_inner().await.latest_info.slot_number
            < visible_slot_number_after_increase.as_true()
    } {
        // TODO: once read APIs get an is_ready state, set it to not ready here and reset
        // afterwards
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let mut inner = sequencer.lock_inner().await;
    if inner.executor.has_in_progress_batch() {
        return Err(anyhow!(
            "Received open batch notification, but replica already has an open batch"
        ));
    }

    inner
        .try_start_batch_with_parameters_from_master(
            visible_slot_number_after_increase,
            visible_slots_to_advance,
        )
        .await?;

    // Ensure the batch was successfully started
    if !inner.executor.has_in_progress_batch() {
        panic!(
            "Replica: no batch in progress, and no batch could be started. This should not be possible under any circumstances as the master was able to create a batch at this point. Please report this bug. {:?} {:?}",
            &inner.executor.checkpoint, inner.latest_info
        );
    }
    Ok(())
}

async fn do_batch_end<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
) -> anyhow::Result<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    let mut inner = sequencer.lock_inner().await;
    inner.close_current_batch().await;

    Ok(())
}

/// Inner transaction processing logic extracted from handle_transaction_notification
/// This version takes the already-constructed transaction data to avoid re-fetching from DB
async fn do_new_tx<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    tx_hash: TxHash,
    baked_tx: FullyBakedTx,
) -> anyhow::Result<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    let mut inner = sequencer.lock_inner().await;

    let AcceptedTxWithBudgetInfo {
        accepted_tx,
        execution_time_micros,
        ..
    } = inner.executor.replay_tx(tx_hash, &baked_tx).await;
    inner
        .batch_size_tracker
        .add_tx(baked_tx.data.len(), execution_time_micros);
    inner
        .executor_events_sender
        .send(ExecutorEvent::InsertTxWithoutConfirmation(accepted_tx))
        .await;
    inner
        .executor_events_sender
        .send(ExecutorEvent::ForceUpdateApiState(
            inner
                .executor
                .checkpoint
                .clone_with_empty_witness_dropping_temp_cache(),
        ))
        .await;
    Ok(())
}

async fn do_proof_blob<S: Spec, Rt: Runtime<S>, Da: DaService<Spec = S::Da>>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    sequence_number: u64,
    query_pool: &PgPool,
) -> anyhow::Result<()> {
    let proof_blob = query_proof_blob_from_db(query_pool, sequence_number).await?;
    sequencer.produce_and_publish_proof_blob(proof_blob).await
}

/// Backfill missing batches and transactions to catch up to the current notification.
/// Fetches events in the open interval between latest_received_event_id and target_event_id: the
/// former is treated as already having been received, and the latter is presumably the ID of an
/// event that just came in that we noticed isn't consecutive with the current
/// latest_received_event_id (so we don't fetch it either).
async fn backfill_to_event_id<S: Spec, Rt: Runtime<S>, Da: DaService<Spec = S::Da>>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    query_pool: &PgPool,
    latest_received_event_id: Option<u64>,
    target_event_id: u64,
) -> anyhow::Result<()> {
    let mut current_event_id = latest_received_event_id.map(|id| id + 1).unwrap_or(0);
    // If we're already caught up, nothing to do
    if current_event_id > target_event_id {
        return Ok(());
    }

    debug!(
        "Backfilling events from {} to {}",
        current_event_id, target_event_id
    );

    // Process events in pages (batches, not to be confused with sequencer batches) to avoid
    // excessive memory consumption
    const PAGE_SIZE: i64 = 2000;
    while current_event_id < target_event_id {
        let page_end = std::cmp::min(current_event_id + PAGE_SIZE as u64, target_event_id);

        trace!(
            "Processing backfill page: events {} to {}",
            current_event_id,
            page_end
        );

        // Query and process events for this page
        let events = sqlx::query(
            "SELECT sequence_number, index_in_batch, event_type, hash, data FROM events 
             WHERE event_id >= $1 AND event_id < $2
             ORDER BY event_id ASC",
        )
        .bind(current_event_id as i64)
        .bind(page_end as i64)
        .fetch_all(query_pool)
        .await?;

        for row in events {
            let event_type: EventType = row.get("event_type");
            let sequence_number = row.get::<i64, _>("sequence_number") as u64;

            match event_type {
                EventType::Transaction => {
                    let hash_bytes: Vec<u8> = row.get("hash");
                    let tx_data: Vec<u8> = row.get("data");

                    let tx_hash = TxHash::new(hash_bytes.try_into().map_err(|_| {
                        anyhow::anyhow!("Invalid transaction hash length from database")
                    })?);
                    let baked_tx = FullyBakedTx::new(tx_data);

                    do_new_tx(sequencer.clone(), tx_hash, baked_tx).await?;
                }
                EventType::BatchStart => {
                    let batch_data: Vec<u8> = row.get("data");
                    let batch_metadata = parse_serialized_batch(batch_data, sequence_number)?;

                    do_batch_start(
                        sequencer.clone(),
                        batch_metadata.visible_slot_number_after_increase,
                        batch_metadata.visible_slots_to_advance,
                    )
                    .await?;
                }
                EventType::BatchEnd => {
                    do_batch_end(sequencer.clone()).await?;
                }
                EventType::NewProof => {
                    do_proof_blob(sequencer.clone(), sequence_number, query_pool).await?;
                }
            }
        }

        current_event_id = page_end;
    }

    debug!("Backfill completed successfully");
    Ok(())
}

async fn query_batch_metadata_from_db(
    query_pool: &PgPool,
    sequence_number: u64,
) -> anyhow::Result<BatchMetadata> {
    let blob_data: Vec<u8> = sqlx::query(
        "SELECT data FROM events WHERE sequence_number = $1 AND event_type = 'batch_start'",
    )
    .bind(i64::try_from(sequence_number)?)
    .fetch_one(query_pool)
    .await?
    .get("data");

    parse_serialized_batch(blob_data, sequence_number)
}

fn parse_serialized_batch(data: Vec<u8>, sequence_number: u64) -> anyhow::Result<BatchMetadata> {
    let stored_blob: StoredBlob = borsh::from_slice(&data)?;

    match stored_blob {
        StoredBlob::Batch {
            visible_slot_number_after_increase,
            visible_slots_to_advance,
            ..
        } => Ok(BatchMetadata {
            visible_slot_number_after_increase,
            visible_slots_to_advance,
        }),
        StoredBlob::Proof { .. } => Err(anyhow::anyhow!(
            "Expected batch blob but found proof blob for sequence_number {}",
            sequence_number
        )),
    }
}

async fn query_transaction_body_from_db(
    query_pool: &PgPool,
    sequence_number: u64,
    index_in_batch: i64,
) -> anyhow::Result<(TxHash, FullyBakedTx)> {
    let row =
        sqlx::query("SELECT hash, data FROM events WHERE sequence_number = $1 AND index_in_batch = $2 AND event_type = 'transaction'")
            .bind(i64::try_from(sequence_number)?)
            .bind(index_in_batch)
            .fetch_one(query_pool)
            .await?;

    let hash_bytes: Vec<u8> = row.get("hash");
    let tx_data: Vec<u8> = row.get("data");

    let tx_hash = TxHash::new(
        hash_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid transaction hash length from database"))?,
    );

    let baked_tx = FullyBakedTx::new(tx_data);

    Ok((tx_hash, baked_tx))
}

async fn query_proof_blob_from_db(
    query_pool: &PgPool,
    sequence_number: u64,
) -> anyhow::Result<Arc<[u8]>> {
    let row = sqlx::query("SELECT borsh_value FROM proof_blobs WHERE sequence_number = $1")
        .bind(i64::try_from(sequence_number)?)
        .fetch_one(query_pool)
        .await?;

    let blob_data: Vec<u8> = row.get("borsh_value");
    let stored_blob: StoredBlob = borsh::from_slice(&blob_data)?;

    match stored_blob {
        StoredBlob::Proof { data, .. } => Ok(data),
        StoredBlob::Batch { .. } => Err(anyhow::anyhow!(
            "Expected proof blob but found batch blob for sequence_number {}",
            sequence_number
        )),
    }
}
