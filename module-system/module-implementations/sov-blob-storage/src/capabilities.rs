use std::cmp::Ordering;

use borsh::BorshDeserialize;
use sov_modules_api::batch::{Batch, BatchWithId};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::runtime::capabilities::BatchSelector;
use sov_modules_api::{BlobReaderTrait, DaSpec, KernelWorkingSet, Spec, StateCheckpoint};
use tracing::{error, info, warn};

use crate::{
    BlobStorage, PreferredBatch, PreferredBatchWithId, SequenceNumber, DEFERRED_SLOTS_COUNT,
};

/// Why blob can be discarded
#[derive(Debug)]
enum BlobDiscardReason {
    /// Sender simply not registered in the registry
    SenderNotAllowed,
    /// More complicated case for preferred sequencer. Ping @prestonevans__ at Twitter for more info
    SequenceNumberTooLow,
}

impl<S: Spec, Da: DaSpec> BlobStorage<S, Da> {
    /// Select the blobs to execute this slot using "based sequencing". In this mode,
    /// blobs are processed in the order that they appear on the DA layer.
    pub fn select_blobs_as_based_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelWorkingSet<'k, S>,
    ) -> Vec<(BatchWithId, Da::Address)>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        self.chain_state
            .set_next_visible_slot_number(&(state.current_slot().saturating_add(1)), state)
            .unwrap_infallible();
        self.select_blobs_da_ordering(current_blobs, state)
    }

    fn select_blobs_da_ordering<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelWorkingSet<'k, S>,
    ) -> Vec<(BatchWithId, Da::Address)>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        let mut batches = Vec::new();
        for blob in current_blobs.into_iter() {
            if !self.blob_is_allowed(blob, state.inner) {
                self.log_discarded_blob(blob, BlobDiscardReason::SenderNotAllowed);
                continue;
            }

            if let Some(batch) = self.deserialize_or_slash_sender::<Batch>(blob, state.inner) {
                batches.push((
                    BatchWithId {
                        batch,
                        id: blob.hash(),
                    },
                    blob.sender(),
                ));
            }
        }
        batches
    }

    fn log_discarded_blob(&self, b: &Da::BlobTransaction, reason: BlobDiscardReason) {
        info!(
            blob_hash = hex::encode(b.hash()),
            sender = hex::encode(b.sender()),
            ?reason,
            "Discarding blob"
        );
    }

    /// Check if a blob is allowed to be processed. (Meaning that its sender is appropriately bonded/registered)
    fn blob_is_allowed(&self, b: &Da::BlobTransaction, state: &mut StateCheckpoint<S>) -> bool {
        // TODO(@vlopes11): Add gas check
        self.sequencer_registry
            .is_sender_allowed(&b.sender(), state)
            .is_ok()
    }

    /// Enforce the ordering constraints on preferred batches by discarding or deferring blobs that arrive
    /// out of sequence.
    fn enforce_preferred_batch_ordering(
        &self,
        preferred_batch: PreferredBatch,
        next_sequence_number: SequenceNumber,
        blob: &Da::BlobTransaction,
        state: &mut StateCheckpoint<S>,
    ) -> Option<PreferredBatchWithId> {
        match preferred_batch.sequence_number.cmp(&next_sequence_number) {
            Ordering::Equal => {
                // If the blob has the next sequence number, we'll process it.
                Some(PreferredBatchWithId {
                    inner: preferred_batch,
                    id: blob.hash(),
                })
            }
            Ordering::Greater => {
                // If the sequence number is greater than the expected one, we defer the blob
                let sequence_number = preferred_batch.sequence_number;
                self.deferred_preferred_sequencer_blobs
                    .set(
                        &sequence_number,
                        &PreferredBatchWithId {
                            inner: preferred_batch,
                            id: blob.hash(),
                        },
                        state,
                    )
                    .unwrap_infallible();
                None
            }
            Ordering::Less => {
                // If the sequence number is less than the expected one, we discard the blob
                self.log_discarded_blob(blob, BlobDiscardReason::SequenceNumberTooLow);
                None
            }
        }
    }

    /// Select blobs when transitioning from a preferred sequencer back to normal operation.
    /// This occurs when the preferred sequencer was slashed for malicious behavior. In recovery mode,
    /// the rollup processes two virtual slots at a time until it catches up to the current slot, after
    /// which it performs standard based sequencing.
    fn select_blobs_in_recovery_mode<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelWorkingSet<'k, S>,
    ) -> Vec<(BatchWithId, Da::Address)>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        let mut batches_to_process = Vec::new();

        // First, decide how many slots worth of stored blobs we need. It could be 0, 1, or 2.
        let batches_needed_from_this_slot =
            match state.current_slot().saturating_sub(state.virtual_slot()) {
                // If the virtual slot has caught up to the current slot, we don't need any stored blobs.
                // In this case, we act like a normal "based" rollup
                0 => return self.select_blobs_as_based_sequencer(current_blobs, state),
                // If the virtual slot is only trailing by one, we process one stored slot (to catch up) and
                // then process the new blobs from this slot
                1 => {
                    self.select_blobs_as_based_sequencer(current_blobs, state);
                    1
                }
                // Otherwise, we need to process two slots from storage  - which means that we need to save the new blobs
                _ => {
                    let new_batches = self.select_blobs_da_ordering(current_blobs, state);
                    self.store_batches(state.current_slot(), &new_batches, state.inner);
                    2
                }
            };

        for slot in 0..batches_needed_from_this_slot {
            let slot_to_check = state.virtual_slot().saturating_add(slot);
            let batches_from_next_slot =
                self.take_blobs_for_slot_number(slot_to_check, state.inner);
            batches_to_process.extend(batches_from_next_slot.into_iter());
        }

        self.chain_state
            .set_next_visible_slot_number(&(state.virtual_slot().saturating_add(2)), state)
            .unwrap_infallible();

        batches_to_process
    }

    // We have two cases to handle:
    //   Step 0: Retrieve the next preferred sequencer blob in sequence (if any). Add to list of blobs to execute. Such a blob exists only in the presence of DA shenanigans
    //   Step 1: Filter any ineligible blobs from current slot. Sort by sender (preferred sequencer first)
    //   Step 2: Deserialize all preferred sequencer blobs. Stash any future sequence numbers for later retrieval.
    //   On deserialization failure, slash preferred sequencer. If preferred sequencer is slashed, enter recovery mode.
    //   Recovery mode: Start processing forced transactions two slots at a time until we’re back to real-time processing.
    //   Step 3: Find number of virtual slots to advance. Retrieve all stored (non-preferred) blobs from those slots. These become our blobs
    //   Step 4: Deserialize all blobs into batches. Return (Batch, Sender)
    /// Select the blobs to execute this slot based on the preferred sequencer and set the next virtual_height
    #[tracing::instrument(skip_all)]
    fn select_blobs_for_preferred_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelWorkingSet<'k, S>,
        preferred_sender: &Da::Address,
    ) -> Vec<(BatchWithId, Da::Address)>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        let mut new_forced_blobs = Vec::new();
        let next_sequence_number = self
            .next_sequence_number
            .get(state)
            .unwrap_infallible()
            .unwrap_or(0);

        // Step 0: Retrieve the next preferred batch from storage, if applicable
        let mut preferred_batch = self
            .deferred_preferred_sequencer_blobs
            .remove(&next_sequence_number, state.inner)
            .unwrap_infallible();

        for blob in current_blobs.into_iter() {
            // Step 1: Filter any ineligible blobs from current slot.
            if !self.blob_is_allowed(blob, state.inner) {
                self.log_discarded_blob(blob, BlobDiscardReason::SenderNotAllowed);
                continue;
            }

            // Check if the blob is from the preferred sequencer
            if &blob.sender() == preferred_sender {
                let maybe_batch = self
                    .deserialize_or_slash_sender::<PreferredBatch>(blob, state.inner)
                    .and_then(|batch| {
                        self.enforce_preferred_batch_ordering(
                            batch,
                            next_sequence_number,
                            blob,
                            state.inner,
                        )
                    });

                // Even if we retrieved `preferred_batch`` in `step0``, we override it because it has the same `sequence_number`.
                if let Some(next_preferred_batch) = maybe_batch {
                    preferred_batch = Some(next_preferred_batch);
                }
            } else {
                // Otherwise, the batch is from a valid sender (checked in step 1) but not the preferred sender
                // Deserialize it as a normal batch and store it in memory
                let batch = self.deserialize_or_slash_sender::<Batch>(blob, state.inner);
                if let Some(batch) = batch {
                    new_forced_blobs.push((
                        BatchWithId {
                            batch,
                            id: blob.hash(),
                        },
                        blob.sender(),
                    ));
                }
            }
        }

        // Step 3: Find number of virtual slots to advance.
        // - If the preferred sequencer requested a number, advance up to that many (stopping early if the next virtual slot would be in the future)
        // - Otherwise, advance only if we would otherwise exceed the maximum deferred slots count
        let max_slots_to_advance = state
            .current_slot()
            .saturating_sub(state.virtual_slot())
            .saturating_add(1);
        let mut batches_to_process = Vec::new();

        let num_slots_to_advance = if let Some(preferred_batch) = preferred_batch {
            self.next_sequence_number
                .set(&next_sequence_number.saturating_add(1), state)
                .unwrap_infallible();

            let first_batch = BatchWithId {
                batch: preferred_batch.inner.batch,
                id: preferred_batch.id,
            };

            batches_to_process.push((first_batch, preferred_sender.clone()));
            tracing::debug!(
                seq_number = preferred_batch.inner.sequence_number,
                slots_to_advance = preferred_batch.inner.virtual_slots_to_advance,
                "Requested to advance slots"
            );

            if preferred_batch.inner.virtual_slots_to_advance as u64 > max_slots_to_advance {
                warn!(
                    "Preferred sequencer requested {} slots, but we can only advance {} slots",
                    preferred_batch.inner.virtual_slots_to_advance, max_slots_to_advance
                );
                max_slots_to_advance
            } else {
                std::cmp::max(preferred_batch.inner.virtual_slots_to_advance as u64, 1)
            }
        } else {
            // If there's no preferred blob, advance only if the we would otherwise exceed the maximum deferred slots count
            if state.virtual_slot().saturating_add(DEFERRED_SLOTS_COUNT) <= state.current_slot() {
                1
            } else {
                0
            }
        };
        tracing::debug!(
            num_slots_to_advance,
            current_real_slot = state.current_slot(),
            "Advancing virtual slot number"
        );

        // Load all the necessary batches from storage
        for slot in 0..num_slots_to_advance {
            let slot_to_check = state.virtual_slot().saturating_add(slot);
            let batches_from_next_slot =
                self.take_blobs_for_slot_number(slot_to_check, state.inner);
            batches_to_process.extend(batches_from_next_slot.into_iter());
        }

        // Check if we also need the blobs from the current slot. Add them to the set to be processed or store them as appropriate.
        let next_virtual_height = state.virtual_slot().saturating_add(num_slots_to_advance);
        if next_virtual_height > state.current_slot() {
            batches_to_process.extend(new_forced_blobs);
        } else {
            self.store_batches(state.current_slot(), &new_forced_blobs, state.inner);
        }
        self.chain_state
            .set_next_visible_slot_number(&next_virtual_height, state)
            .unwrap_infallible();
        batches_to_process
    }

    /// Deserialize a blob into a `Batch` or slash the sender if it's malformed.
    fn deserialize_or_slash_sender<B: BorshDeserialize>(
        &self,
        blob: &mut Da::BlobTransaction,
        state: &mut StateCheckpoint<S>,
    ) -> Option<B> {
        match B::try_from_slice(data_for_deserialization(blob)) {
            Ok(batch) => Some(batch),
            // if the blob is malformed, slash the sequencer
            Err(e) => {
                assert_eq!(blob.verified_data().len(), blob.total_len(), "Batch deserialization failed and some data was not provided. The prover might be malicious");
                error!(
                    blob_hash = hex::encode(blob.hash()),
                    slashed_sender = %blob.sender(),
                    error = ?e,
                    "Unable to deserialize blob. slashing sender"
                );

                self.sequencer_registry
                    .slash_sequencer(&blob.sender(), state);

                None
            }
        }
    }
}

impl<S: Spec, Da: DaSpec> BatchSelector<Da> for BlobStorage<S, Da> {
    type Spec = S;

    type Batch = BatchWithId;

    // This implementation returns three categories of blobs:
    // 1. Any blobs sent by the preferred sequencer ("prority blobs")
    // 2. Any non-priority blobs which were sent `DEFERRED_SLOTS_COUNT` slots ago ("expiring deferred blobs")
    // 3. Some additional deferred blobs needed to fill the total requested by the sequencer, if applicable. ("bonus blobs")
    fn get_batches_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelWorkingSet<'k, S>,
    ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        // If `DEFERRED_SLOTS_COUNT` is 0, we treat the rollup as having no preferred sequencer.
        // In this case, we just process blobs in the order that they appeared on the DA layer
        if DEFERRED_SLOTS_COUNT == 0 {
            return Ok(self.select_blobs_as_based_sequencer(current_blobs, state));
        }

        // If there's a preferred sequencer, sequence accordingly.
        if let Some(preferred_sender) = self.get_preferred_sequencer(state.inner) {
            return Ok(self.select_blobs_for_preferred_sequencer(
                current_blobs,
                state,
                &preferred_sender,
            ));
        }

        // Otherwise, we're configured for a preferred sequencer but one doesn't exist. This usually means that the preferred sequencer was slashed.
        // Entery recovery mode.
        Ok(self.select_blobs_in_recovery_mode(current_blobs, state))
    }
}

#[cfg(feature = "native")]
fn data_for_deserialization(blob: &mut impl BlobReaderTrait) -> &[u8] {
    blob.full_data()
}

#[cfg(not(feature = "native"))]
fn data_for_deserialization(blob: &mut impl BlobReaderTrait) -> &[u8] {
    blob.verified_data()
}
