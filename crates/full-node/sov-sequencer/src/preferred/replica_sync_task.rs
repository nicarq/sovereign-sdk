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

use super::db::StoredBlob;
use crate::preferred::{exit_rollup, DbEvent, PreferredSequencer};
use crate::ProofBlobSender;

/// Event type enum for type-safe parsing
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
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

/// Structure representing the CSV payload from PostgreSQL NOTIFY
#[derive(Debug)]
struct EventsNotificationPayload {
    event_id: u64,
    sequence_number: u64,
    event_type: EventType,
    index_in_batch: Option<u64>, // Only present for transaction events
}

/// Structure representing leader election notification payload from PostgreSQL NOTIFY
#[derive(Debug)]
struct LeaderNotificationPayload {
    node_id: Uuid,
    last_updated_timestamp: f64, // Unix timestamp from EXTRACT(EPOCH FROM timestamp)
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
    tokio::spawn(replica_sync_task_inner(
        sequencer,
        shutdown_receiver,
        latest_loaded_event_id,
    ))
}

async fn replica_sync_task_inner<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    mut shutdown_receiver: watch::Receiver<()>,
    latest_loaded_event_id: Option<u64>,
) where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    let is_replica = sequencer.config.sequencer_kind_config.is_replica;
    
    if is_replica {
        info!("Starting replica sync task for a replica sequencer");
    } else {
        info!("Starting replica sync task for a master sequencer (heartbeat mode)");
    }

    // Ensure we're running postgres, else leader election is not supported
    let has_postgres = sequencer
        .config
        .sequencer_kind_config
        .postgres_connection_string
        .is_some();

    if !has_postgres {
        if is_replica {
            warn!("Replicas are not supported on the rocksdb backend. Configure a postgres database that will be shared between all sequencers, and provide the `sequencer.preferred.postgres_connection_string` config value.");
        } else {
            warn!("Leader election requires PostgreSQL. Configure postgres_connection_string to enable leader election heartbeat.");
        }
        return;
    }
    let Some(connection_string) = sequencer
        .config
        .sequencer_kind_config
        .postgres_connection_string
        .as_ref()
    else {
        warn!("Replicas are not supported on the rocksdb backend. Configure a postgres database that will be shared between all sequencers, and provide the `sequencer.preferred.postgres_connection_string` config value.");
        return;
    };

    // Create a separate persistent connection pool for querying transaction data
    let query_pool = match PgPoolOptions::default()
        .max_connections(5) // Small pool since we're just doing simple queries
        .connect(connection_string)
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            error!("Failed to connect to PostgreSQL: {e:?}. Replica shutting down.");
            exit_rollup(&sequencer.shutdown_sender).await;
            return;
        }
    };

    // Create dedicated listeners for PostgreSQL LISTEN/NOTIFY
    let mut listener = match PgListener::connect(connection_string).await {
        Ok(listener) => listener,
        Err(e) => {
            error!("Failed to create PostgreSQL listener: {e:?}. Sequencer shutting down.");
            exit_rollup(&sequencer.shutdown_sender).await;
            return;
        }
    };

    // Always listen to leader election changes
    if let Err(e) = listener.listen("leader_changes").await {
        error!("Failed to listen on leader_changes channel: {e:?}. Sequencer shutting down.");
        exit_rollup(&sequencer.shutdown_sender).await;
        return;
    }

    // Replicas also need to listen to events_changes for syncing
    if is_replica {
        if let Err(e) = listener.listen("events_changes").await {
            error!("Failed to listen on events_changes channel: {e:?}. Replica shutting down.");
            exit_rollup(&sequencer.shutdown_sender).await;
            return;
        }
    }

    debug!("Successfully connected and listening for PostgreSQL notifications");

    // Masters should claim initial leadership on startup
    if !is_replica {
        if let Err(e) = send_heartbeat(&query_pool, sequencer.node_id).await {
            warn!("Failed to claim initial leadership: {e:?}");
        } else {
            info!("Claimed initial leadership with node ID: {}", sequencer.node_id);
        }
    }

    // Use unified event processing loop for both master and replica modes
    let mut latest_received_event_id = latest_loaded_event_id;
    if let Err(e) = unified_event_processing_loop(
        sequencer.clone(),
        &mut listener,
        &query_pool,
        &mut latest_received_event_id,
        &mut shutdown_receiver,
        is_replica,
    )
    .await
    {
        error!("Event processing failed: {e:?}. Sequencer shutting down.");
        exit_rollup(&sequencer.shutdown_sender).await;
    }
}

/// Unified event processing loop that handles both master heartbeat and replica sync
async fn unified_event_processing_loop<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    listener: &mut PgListener,
    query_pool: &PgPool,
    latest_received_event_id: &mut Option<u64>,
    shutdown_receiver: &mut watch::Receiver<()>,
    initial_is_replica: bool,
) -> anyhow::Result<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
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
    
    // Track current role state
    let mut are_we_master = !initial_is_replica;
    let mut last_heartbeat_time = SystemTime::now();
    
    // Heartbeat interval for masters (every 0.5s)
    let mut heartbeat_interval = tokio::time::interval(Duration::from_millis(500));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    
    // Failover threshold from config
    let failover_threshold = Duration::from_secs(sequencer.config.sequencer_kind_config.failover_threshold_secs);
    
    debug!("Starting database watching loop - role: {}", if are_we_master { "master" } else { "replica" });
    println!("Starting database watching loop - role: {}", if are_we_master { "master" } else { "replica" });

    loop {
        tokio::select! {
            // Master heartbeat timer (only active if we're currently the master)
            _ = heartbeat_interval.tick(), if are_we_master => {
                if let Err(e) = send_heartbeat(query_pool, sequencer.node_id).await {
                    error!("Failed to send heartbeat: {e:?}");
                    // Continue trying, but this could indicate we're no longer master
                }
            },
            
            // PostgreSQL notifications (both leader changes and events)
            notification_result = listener.recv(), if pending_events.len() < MAX_CONCURRENT => {
                match notification_result {
                    Ok(notification) => {
                        trace!("Node {} received PostgreSQL notification: {:?}", sequencer.node_id, notification);
                        println!("Node {} received PostgreSQL notification: {:?}", sequencer.node_id, notification);
                        
                        match notification.channel() {
                            "leader_changes" => {
                                // Handle leader election notifications
                                let payload = notification.payload();
                                match LeaderNotificationPayload::parse_csv(payload) {
                                    Ok(leader_info) => {
                                        if leader_info.node_id == sequencer.node_id {
                                            // This is our own heartbeat
                                            if !are_we_master {
                                                info!("Replica promoted to master!");
                                                are_we_master = true;
                                            }
                                        } else {
                                            // Another node is master
                                            if are_we_master {
                                                warn!("Another node took over as master: {}", leader_info.node_id);
                                                are_we_master = false;
                                            }
                                            last_heartbeat_time = SystemTime::UNIX_EPOCH + Duration::from_secs_f64(leader_info.last_updated_timestamp);
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse leader notification: {e:?}");
                                    }
                                }
                            },
                            "events_changes" => {
                                // Handle database sync events (only for replicas)
                                if !are_we_master {
                                    let payload = notification.payload();
                                    match EventsNotificationPayload::parse_csv(payload) {
                                        Ok(parsed_notification) => {
                                            println!("Replica node {} received PostgreSQL notification: {:?}", sequencer.node_id, parsed_notification);
                                            // Check for gaps and add backfill if needed
                                            if detect_gap(*latest_received_event_id, parsed_notification.event_id).is_some() {
                                                let backfill_future = create_backfill_future(
                                                    sequencer.clone(),
                                                    query_pool,
                                                    *latest_received_event_id,
                                                    parsed_notification.event_id,
                                                );
                                                pending_events.push_back(backfill_future);
                                            }

                                            *latest_received_event_id = Some(parsed_notification.event_id);

                                            // Create future for current notification
                                            let event_future = create_event_future(parsed_notification, query_pool);
                                            pending_events.push_back(event_future);
                                        }
                                        Err(e) => {
                                            warn!("Failed to parse events notification: {e:?}");
                                        }
                                    }
                                }
                            },
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

            // Always process completions in FIFO order
            Some(completed_result) = pending_events.next() => {
                // println!("Processing DbEvent from completed_result: {completed_result:?}");
                match completed_result? {
                    CompletedEvent::Event(db_event) => {
                        process_db_event(sequencer.clone(), db_event, query_pool).await?;
                    }
                    CompletedEvent::Backfill => {
                        trace!("Backfill completed up to event_id");
                    }
                }
            },

            // Failover timeout check (only for replicas)
            _ = tokio::time::sleep(Duration::from_secs(1)), if !are_we_master => {
                let time_since_last_heartbeat = SystemTime::now()
                    .duration_since(last_heartbeat_time)
                    .unwrap_or(Duration::MAX);
                
                if time_since_last_heartbeat > failover_threshold {
                    info!("Master heartbeat timeout detected ({:?} > {:?}). Attempting takeover...", 
                          time_since_last_heartbeat, failover_threshold);
                    
                    match attempt_leadership_takeover(query_pool, sequencer.node_id, failover_threshold).await {
                        Ok(true) => {
                            info!("Successfully took over as master!");
                            are_we_master = true;
                        }
                        Ok(false) => {
                            debug!("Another replica beat us to takeover or master recovered");
                        }
                        Err(e) => {
                            warn!("Failed to attempt takeover: {e:?}");
                        }
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

/// Send a heartbeat to maintain leadership
async fn send_heartbeat(query_pool: &PgPool, node_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO sequencer_leader (node_id, last_updated) 
         VALUES ($1, NOW()) 
         ON CONFLICT (singleton) DO UPDATE SET 
             node_id = EXCLUDED.node_id, 
             last_updated = EXCLUDED.last_updated"
    )
    .bind(node_id)
    .execute(query_pool)
    .await?;
    
    debug!("Heartbeat sent for node {}", node_id);
    Ok(())
}

/// Attempt to take over leadership atomically
async fn attempt_leadership_takeover(
    query_pool: &PgPool, 
    node_id: Uuid, 
    failover_threshold: Duration
) -> anyhow::Result<bool> {
    let threshold_seconds = failover_threshold.as_secs() as i64;
    
    // Atomic takeover: only succeed if the current leader's heartbeat is old enough
    let result = sqlx::query(
        "INSERT INTO sequencer_leader (node_id, last_updated) 
         VALUES ($1, NOW()) 
         ON CONFLICT (singleton) DO UPDATE SET 
             node_id = EXCLUDED.node_id, 
             last_updated = EXCLUDED.last_updated 
         WHERE sequencer_leader.last_updated < NOW() - INTERVAL '1 second' * $2"
    )
    .bind(node_id)
    .bind(threshold_seconds)
    .execute(query_pool)
    .await?;
    
    // Check if our update actually succeeded by verifying we're the current leader
    if result.rows_affected() > 0 {
        let current_leader: Option<Uuid> = sqlx::query_scalar(
            "SELECT node_id FROM sequencer_leader WHERE singleton = 1"
        )
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
async fn process_db_event<S, Rt, Da>(
    sequencer: Arc<PreferredSequencer<S, Rt, Da>>,
    db_event: DbEvent,
    query_pool: &PgPool,
) -> anyhow::Result<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    match db_event {
        DbEvent::TxAccepted(baked_tx, tx_hash) => {
            sequencer
                .synchronized_state_updator
                .do_new_tx_msg(tx_hash, baked_tx, "replica_sync_task:do_new_tx")
                .await;
            Ok(())
        }
        DbEvent::BatchClosed(_) => {
            sequencer
                .synchronized_state_updator
                .close_current_batch_msg("replica_sync_task:close_current_batch")
                .await;
            Ok(())
        }
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
        let process_latest_slot_number = sequencer
            .synchronized_state_updator
            .latest_slot_number_msg(
                "replica_sync_task:do_batch_start::wait_for_visible_slot_catchup",
            )
            .await;

        process_latest_slot_number < visible_slot_number_after_increase.as_true()
    } {
        // TODO: once read APIs get an is_ready state, set it to not ready here and reset
        // afterwards
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    sequencer
        .synchronized_state_updator
        .do_batch_start_msg(
            visible_slot_number_after_increase,
            visible_slots_to_advance,
            "replica_sync_task:do_batch_start::start_batch",
        )
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
            let event_type_str: String = row.get("event_type");
            let sequence_number = row.get::<i64, _>("sequence_number") as u64;

            let event_type: EventType = event_type_str
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid event type '{}': {}", event_type_str, e))?;

            match event_type {
                EventType::Transaction => {
                    let hash_bytes: Vec<u8> = row.get("hash");
                    let tx_data: Vec<u8> = row.get("data");

                    let tx_hash = TxHash::new(hash_bytes.try_into().map_err(|_| {
                        anyhow::anyhow!("Invalid transaction hash length from database")
                    })?);
                    let baked_tx = FullyBakedTx::new(tx_data);

                    sequencer
                        .synchronized_state_updator
                        .do_new_tx_msg(tx_hash, baked_tx, "replica_sync_task:do_new_tx")
                        .await;
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
                    sequencer
                        .synchronized_state_updator
                        .close_current_batch_msg("replica_sync_task:close_current_batch")
                        .await;
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
