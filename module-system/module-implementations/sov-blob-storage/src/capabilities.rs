use std::cmp::Ordering;

use borsh::BorshDeserialize;
use sov_modules_api::batch::{Batch, BatchWithId};
use sov_modules_api::prelude::*;
use sov_modules_api::runtime::capabilities::BatchSelector;
use sov_modules_api::{BlobReaderTrait, Context, DaSpec, KernelWorkingSet, WorkingSet};
use tracing::{debug, error, info, warn};

use crate::{
    BlobStorage, PreferredBatch, PreferredBatchWithId, SequenceNumber, DEFERRED_SLOTS_COUNT,
};

impl<C: Context, Da: DaSpec> BlobStorage<C, Da> {
    pub(crate) fn log_discarded_blob(&self, b: &Da::BlobTransaction) {
        info!(
            "Blob hash=0x{} from sender {} is going to be discarded",
            hex::encode(b.hash()),
            b.sender()
        );
    }

    /// Check if a blob is allowed to be processed. (Meaning that its sender is appropriately bonded/registered)
    pub(crate) fn blob_is_allowed(
        &self,
        b: &Da::BlobTransaction,
        working_set: &mut WorkingSet<C>,
    ) -> bool {
        // TODO(@vlopes11): Add gas check
        self.sequencer_registry
            .is_sender_allowed(&b.sender(), working_set)
    }

    /// Slash a particular sequencer.
    pub(crate) fn slash_sequencer(&self, sender: &Da::Address, working_set: &mut WorkingSet<C>) {
        self.sequencer_registry.slash_sequencer(sender, working_set)
    }

    /// Enforce the ordering constraints on preferred batches by discarding or deferring blobs that arrive
    /// out of sequence.
    fn enforce_preferred_batch_ordering(
        &self,
        preferred_batch: PreferredBatch,
        next_sequence_number: SequenceNumber,
        blob: &Da::BlobTransaction,
        working_set: &mut WorkingSet<C>,
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
                self.deferred_preferred_sequencer_blobs.set(
                    &sequence_number,
                    &PreferredBatchWithId {
                        inner: preferred_batch,
                        id: blob.hash(),
                    },
                    working_set,
                );
                None
            }
            Ordering::Less => {
                // If the sequence number is less than the expected one, we discard the blob
                self.log_discarded_blob(blob);
                None
            }
        }
    }

    /// Select blobs when transitioning from a preferred sequencer back to normal operation.
    /// This occurs when the preferred sequencer was slashed for malicious behavior. In recovery mode,
    /// the rollup processes two virtual slots at a time until it catches up to the current slot, after
    /// which it performs standard based sequencing.
    pub fn select_blobs_in_recovery_mode<'a, 'k, I>(
        &self,
        current_blobs: I,
        working_set: &mut KernelWorkingSet<'k, C>,
    ) -> Vec<(BatchWithId, Da::Address)>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        let mut batches_to_process = Vec::new();

        // First, decide how many slots worth of stored blobs we need. It could be 0, 1, or 2.
        let batches_needed_from_this_slot = match working_set
            .current_slot()
            .saturating_sub(working_set.virtual_slot())
        {
            // If the virtual slot has caught up to the current slot, we don't need any stored blobs.
            // In this case, we act like a normal "based" rollup
            0 => return self.select_blobs_as_based_sequencer(current_blobs, working_set),
            // If the virtual slot is only trailing by one, we process one stored slot (to catch up) and
            // then process the new blobs from this slot
            1 => Some(self.select_blobs_as_based_sequencer(current_blobs, working_set)),
            // Otherwise, we need to process two slots from storage  - which means that we need to save the new blobs
            _ => {
                let mut new_batches = Vec::new();
                for blob in current_blobs.into_iter() {
                    if !self.blob_is_allowed(blob, working_set.inner) {
                        self.log_discarded_blob(blob);
                        continue;
                    }
                    match self.deserialize_batch::<Batch>(blob) {
                        Ok(batch) => new_batches.push((
                            BatchWithId {
                                txs: batch.txs,
                                id: blob.hash(),
                            },
                            blob.sender(),
                        )),
                        Err(_) => {
                            warn!(
                                    "Unable to deserialize blob with hash 0x{} as a valid batch. Slashing sender {}",
                                    hex::encode(blob.hash()), blob.sender()
                                );
                            self.slash_sequencer(&blob.sender(), working_set.inner);
                        }
                    }
                }
                self.store_batches(working_set.current_slot(), &new_batches, working_set.inner);
                None
            }
        };
        let num_slots_to_load = if batches_needed_from_this_slot.is_some() {
            1
        } else {
            2
        };

        for slot in 0..num_slots_to_load {
            let slot_to_check = working_set.virtual_slot().saturating_add(slot);
            let batches_from_next_slot =
                self.take_blobs_for_slot_height(slot_to_check, working_set.inner);
            batches_to_process.extend(batches_from_next_slot.into_iter());
        }

        self.chain_state
            .set_next_visible_slot_height(&(working_set.virtual_slot() + 2), working_set);

        batches_to_process
    }

    /// Select the blobs to execute this slot using "based sequencing". In this mode,
    /// blobs are processed in the order that they appear on the DA layer.
    pub fn select_blobs_as_based_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        working_set: &mut KernelWorkingSet<'k, C>,
    ) -> Vec<(BatchWithId, Da::Address)>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        self.chain_state
            .set_next_visible_slot_height(&(working_set.current_slot() + 1), working_set);
        let mut batches = Vec::new();
        for blob in current_blobs.into_iter() {
            if !self.blob_is_allowed(blob, working_set.inner) {
                self.log_discarded_blob(blob);
                continue;
            }
            match self.deserialize_batch::<Batch>(blob) {
                Ok(batch) => batches.push((
                    BatchWithId {
                        txs: batch.txs,
                        id: blob.hash(),
                    },
                    blob.sender(),
                )),
                Err(_) => {
                    warn!(
                        "Unable to deserialize blob with hash 0x{} as a valid batch. Slashing sender {}",
                        hex::encode(blob.hash()), blob.sender()
                    );
                    self.slash_sequencer(&blob.sender(), working_set.inner);
                }
            }
        }
        batches
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
    pub fn select_blobs_for_preferred_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        working_set: &mut KernelWorkingSet<'k, C>,
        preferred_sender: &Da::Address,
    ) -> Vec<(BatchWithId, Da::Address)>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        // We only want to process one blob from the preferred sequencer per slot, so we need to keep track of whether we've already processed one
        let mut preferred_batch = None;
        let mut new_forced_blobs = Vec::new();
        let next_sequence_number = self.next_sequence_number.get(working_set).unwrap_or(0);

        // Step 0: Retrieve the next preferred batch from storage, if applicable
        if let Some(next_preferred_batch) = self
            .deferred_preferred_sequencer_blobs
            .remove(&next_sequence_number, working_set.inner)
        {
            preferred_batch = Some(next_preferred_batch);
        }

        for blob in current_blobs.into_iter() {
            // Step 1: Filter any ineligible blobs from current slot.
            if !self.blob_is_allowed(blob, working_set.inner) {
                self.log_discarded_blob(blob);
                continue;
            }

            // Check if the blob is from the preferred sequencer
            if Some(blob.sender())
                == self
                    .sequencer_registry
                    .get_preferred_sequencer(working_set.inner)
            {
                // If so, deserialize it as the appropriate type
                let batch =
                    self.deserialize_or_slash_sender::<PreferredBatch>(blob, working_set.inner);
                if let Some(next_preferred_batch) = batch.and_then(|batch| {
                    self.enforce_preferred_batch_ordering(
                        batch,
                        next_sequence_number,
                        blob,
                        working_set.inner,
                    )
                }) {
                    preferred_batch = Some(next_preferred_batch)
                }
            } else {
                // Otherwise, the batch is from a valid sender (checked in step 1) but not the preferred sender
                // Deserialize it as a normal batch and store it in memory
                let batch = self.deserialize_or_slash_sender::<Batch>(blob, working_set.inner);
                if let Some(batch) = batch {
                    new_forced_blobs.push((
                        BatchWithId {
                            txs: batch.txs,
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
        let max_slots_to_advance = working_set
            .current_slot()
            .saturating_sub(working_set.virtual_slot())
            .saturating_add(1);
        let mut batches_to_process = Vec::new();
        let num_slots_to_advance = if let Some(preferred_batch) = preferred_batch {
            self.next_sequence_number
                .set(&next_sequence_number.saturating_add(1), working_set);

            let first_batch = BatchWithId {
                txs: preferred_batch.inner.txs,
                id: preferred_batch.id,
            };
            batches_to_process.push((first_batch, preferred_sender.clone()));
            tracing::debug!(
                "Processing preferred batch with sequence number {}. Requestd slots to advance: {}",
                preferred_batch.inner.sequence_number,
                preferred_batch.inner.virtual_slots_to_advance
            );

            if preferred_batch.inner.virtual_slots_to_advance as u64 > max_slots_to_advance {
                tracing::warn!(
                    "Preferred sequencer requested {} slots, but we can only advance {} slots",
                    preferred_batch.inner.virtual_slots_to_advance,
                    max_slots_to_advance
                );
                max_slots_to_advance
            } else {
                std::cmp::max(preferred_batch.inner.virtual_slots_to_advance as u64, 1)
            }
        } else {
            // If there's no preferred blob, advance only if the we would otherwise exceed the maximum deferred slots count
            if working_set
                .virtual_slot()
                .saturating_add(DEFERRED_SLOTS_COUNT)
                <= working_set.current_slot()
            {
                1
            } else {
                0
            }
        };
        tracing::debug!(
            "Advancing virtual slot by {} slots. Current real slot: {}",
            num_slots_to_advance,
            working_set.current_slot()
        );

        // Load all the necessary batches from storage
        for slot in 0..num_slots_to_advance {
            let slot_to_check = working_set.virtual_slot().saturating_add(slot);
            let batches_from_next_slot =
                self.take_blobs_for_slot_height(slot_to_check, working_set.inner);
            batches_to_process.extend(batches_from_next_slot.into_iter());
        }

        // Check if we also need the blobs from the current slot. Add them to the set to be processed or store them as appropriate.
        let next_virtual_height = working_set
            .virtual_slot()
            .saturating_add(num_slots_to_advance);
        if next_virtual_height > working_set.current_slot() {
            batches_to_process.extend(new_forced_blobs);
        } else {
            self.store_batches(
                working_set.current_slot(),
                &new_forced_blobs,
                working_set.inner,
            );
        }
        self.chain_state
            .set_next_visible_slot_height(&next_virtual_height, working_set);
        batches_to_process
    }
}

impl<C: Context, Da: DaSpec> BatchSelector<Da> for BlobStorage<C, Da> {
    type Context = C;

    type Batch = BatchWithId;

    // This implementation returns three categories of blobs:
    // 1. Any blobs sent by the preferred sequencer ("prority blobs")
    // 2. Any non-priority blobs which were sent `DEFERRED_SLOTS_COUNT` slots ago ("expiring deferred blobs")
    // 3. Some additional deferred blobs needed to fill the total requested by the sequencer, if applicable. ("bonus blobs")
    fn get_batches_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        working_set: &mut KernelWorkingSet<'k, C>,
    ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        // If `DEFERRED_SLOTS_COUNT` is 0, we treat the rollup as having no preferred sequencer.
        // In this case, we just process blobs in the order that they appeared on the DA layer
        if DEFERRED_SLOTS_COUNT == 0 {
            return Ok(self.select_blobs_as_based_sequencer(current_blobs, working_set));
        }

        // If there's a preferred sequencer, sequence accordingly.
        if let Some(preferred_sender) = self.get_preferred_sequencer(working_set.inner) {
            return Ok(self.select_blobs_for_preferred_sequencer(
                current_blobs,
                working_set,
                &preferred_sender,
            ));
        }

        // Otherwise, we're configured for a preferred sequencer but one doesn't exist. This usually means that the preferred sequencer was slashed.
        // Entery recovery mode.
        Ok(self.select_blobs_in_recovery_mode(current_blobs, working_set))
    }
}

impl<C: Context, Da: DaSpec> BlobStorage<C, Da> {
    /// Attempt to deserialize a blob into a list of transactions.
    pub(crate) fn deserialize_batch<B: BorshDeserialize>(
        &self,
        blob_data: &mut impl BlobReaderTrait,
    ) -> Result<B, borsh::maybestd::io::Error> {
        match B::try_from_slice(data_for_deserialization(blob_data)) {
            Ok(batch) => Ok(batch),
            Err(e) => {
                assert_eq!(blob_data.verified_data().len(), blob_data.total_len(), "Batch deserialization failed and some data was not provided. The prover might be malicious");
                // If the deserialization fails, we need to make sure it's not because the prover was malicious and left
                // out some relevant data! Make that check here. If the data is missing, panic.
                error!(
                    "Unable to deserialize batch provided by the sequencer {}",
                    e
                );
                Err(e)
            }
        }
    }

    /// Deserialize a blob into a `Batch` or slash the sender if it's malformed.
    pub(crate) fn deserialize_or_slash_sender<B: BorshDeserialize>(
        &self,
        blob: &mut Da::BlobTransaction,
        working_set: &mut WorkingSet<C>,
    ) -> Option<B> {
        let batch = self.deserialize_batch::<B>(blob);
        match batch {
            Ok(batch) => Some(batch),
            // if the blob is malformed, slash the sequencer
            Err(e) => {
                warn!(
                    "Unable to deserialize blob with hash 0x{} as a valid batch. Slashing sender {}",
                    hex::encode(blob.hash()), blob.sender()
                );
                debug!(
                    "The error returned from deserialization was: `{}`. Blob 0x{} had the following contents: {:?}",
                    e,
                    hex::encode(blob.hash()),
                    blob.verified_data()
                );
                self.sequencer_registry
                    .slash_sequencer(&blob.sender(), working_set);
                None
            }
        }
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
