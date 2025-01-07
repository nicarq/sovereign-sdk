use std::cmp::Ordering;

use borsh::BorshDeserialize;
use sov_modules_api::capabilities::{BlobOrigin, BlobSelectorOutput, SequencerType};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    BatchWithId, BlobData, BlobDataWithId, BlobReaderTrait, DaSpec, FullyBakedTx,
    InfallibleKernelStateAccessor, InfallibleStateAccessor, IterableBatchWithId,
    KernelStateAccessor, RawTx, Spec, VersionReader, VisibleSlotNumber,
};
use sov_sequencer_registry::AllowedSequencerError;
use tracing::{debug, error, info, warn};

use crate::max_size_checker::{take_blobs_with_size_limit, BlobsWithTotalSizeLimit};
use crate::{
    config_deferred_slots_count, config_unregistered_blobs_per_slot, BlobStorage,
    PreferredBatchData, PreferredBlobData, PreferredBlobDataWithId, PreferredProofData,
    PreferredSequenced, SequenceNumber,
};

/// Why blob can be discarded
#[derive(Debug)]
enum BlobDiscardReason {
    /// More complicated case for preferred sequencer. Ping @prestonevans__ at Twitter for more info
    SequenceNumberTooLow,
    /// Sender doesn't have enough staked sequencer funds
    SenderInsufficientStake,
    /// The max amount of unregistered blobs allowed to be processed per slot
    MaxAllowedUnregisteredBlobs,
}

#[derive(Debug)]
enum SequencerStatus {
    Registered,
    Unregistered,
}

enum ValidateBlobOutcome {
    Discard(BlobDiscardReason),
    Accept(SequencerStatus),
}

impl<S: Spec> BlobStorage<S> {
    fn set_next_visible_rollup_height(
        &self,
        value: VisibleSlotNumber,
        state: &mut KernelStateAccessor<S::Storage>,
    ) {
        self.chain_state
            .set_next_visible_rollup_height(&value, state);
    }

    /// Select the blobs to execute this slot using "based sequencing". In this mode,
    /// blobs are processed in the order that they appear on the DA layer.
    fn select_blobs_as_based_sequencer_inner<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S::Storage>,
    ) -> BlobSelectorOutput<S, BlobDataWithId<BatchWithId>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        tracing::trace!("On based sequencer path");

        self.set_next_visible_rollup_height(state.rollup_height_to_access().as_visible(), state);

        state.update_visible_rollup_height(state.rollup_height_to_access().as_visible());

        BlobSelectorOutput {
            selected_blobs: self.select_blobs_da_ordering(current_blobs, state),
            should_execute_slot_hooks: true,
        }
    }

    fn select_blobs_da_ordering<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S::Storage>,
    ) -> Vec<(BlobDataWithId<BatchWithId>, SequencerType<S>)>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        let mut blobs_with_total_size_limit = BlobsWithTotalSizeLimit::<S>::new();

        let mut unregistered_blobs = 0;
        for (idx, item) in current_blobs.into_iter().enumerate() {
            tracing::trace!(idx, "Processing blob");
            match item {
                BlobOrigin::Proof(blob) => {
                    tracing::trace!(idx, "Processing as proof");
                    if self
                        .sequencer_registry
                        .is_sender_allowed(&blob.sender(), state)
                        .is_ok()
                    {
                        if let Some(proof) =
                            self.deserialize_or_try_slash_sender::<Vec<u8>>(blob, true, state)
                        {
                            let data = BlobDataWithId::Proof {
                                proof,
                                id: blob.hash().into(),
                            };
                            blobs_with_total_size_limit.push_or_ignore((
                                data,
                                SequencerType::Standard(blob.sender().clone()),
                            ));
                        }
                    } else {
                        self.log_discarded_blob(blob, BlobDiscardReason::SenderInsufficientStake);
                    }
                }
                BlobOrigin::Batch(blob) => {
                    tracing::trace!("processing as batch");
                    match self.validate_blob_and_sender(blob, unregistered_blobs, state) {
                        ValidateBlobOutcome::Discard(reason) => {
                            self.log_discarded_blob(blob, reason);
                        }
                        ValidateBlobOutcome::Accept(sequencer_status) => {
                            let from_registered_sequencer =
                                matches!(sequencer_status, SequencerStatus::Registered);

                            tracing::trace!(%from_registered_sequencer);
                            let res = if from_registered_sequencer {
                                self.deserialize_or_try_slash_sender::<Vec<FullyBakedTx>>(
                                    blob,
                                    from_registered_sequencer,
                                    state,
                                )
                                .map(BlobData::new_batch)
                            } else {
                                unregistered_blobs += 1;
                                self.deserialize_or_try_slash_sender::<RawTx>(
                                    blob,
                                    from_registered_sequencer,
                                    state,
                                )
                                .map(BlobData::EmergencyRegistration)
                            };

                            if let Some(data) = res {
                                tracing::trace!(
                                    "Successfully deserialized blob {} ({}). Adding to batch.",
                                    idx,
                                    hex::encode(blob.hash()),
                                );

                                blobs_with_total_size_limit.push_or_ignore((
                                    data.with_id(blob.hash().into()),
                                    SequencerType::Standard(blob.sender().clone()),
                                ));
                            }
                        }
                    };
                }
            }
        }

        blobs_with_total_size_limit.inner()
    }

    fn validate_blob_and_sender(
        &self,
        blob: &<S::Da as DaSpec>::BlobTransaction,
        unregistered_blobs_processed: u64,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> ValidateBlobOutcome {
        match self
            .sequencer_registry
            .is_sender_allowed(&blob.sender(), state)
        {
            Ok(_) => ValidateBlobOutcome::Accept(SequencerStatus::Registered),
            Err(AllowedSequencerError::NotRegistered) => {
                if unregistered_blobs_processed >= config_unregistered_blobs_per_slot() {
                    ValidateBlobOutcome::Discard(BlobDiscardReason::MaxAllowedUnregisteredBlobs)
                } else {
                    ValidateBlobOutcome::Accept(SequencerStatus::Unregistered)
                }
            }
        }
    }

    fn log_discarded_blob(
        &self,
        blob: &<S::Da as DaSpec>::BlobTransaction,
        reason: BlobDiscardReason,
    ) {
        info!(
            blob_hash = hex::encode(blob.hash()),
            sender = hex::encode(blob.sender()),
            ?reason,
            "Discarding blob"
        );
    }

    /// Enforce the ordering constraints on preferred blobs by discarding or deferring blobs that arrive
    /// out of sequence.
    fn enforce_preferred_blob_ordering<T: PreferredSequenced>(
        &self,
        preferred_blob: T,
        next_sequence_number: SequenceNumber,
        blob: &<S::Da as DaSpec>::BlobTransaction,
        state: &mut impl InfallibleKernelStateAccessor,
        needs_blob: bool,
    ) -> Option<T> {
        match (
            preferred_blob.sequence_number().cmp(&next_sequence_number),
            needs_blob,
        ) {
            (Ordering::Equal, true) => {
                // If the blob has the next sequence number, we'll process it.
                Some(preferred_blob)
            }
            (Ordering::Greater, _) | (Ordering::Equal, false) => {
                // If the sequence number is greater than the expected one, we defer the blob
                let sequence_number = preferred_blob.sequence_number();
                self.deferred_preferred_sequencer_blobs
                    .set(
                        &sequence_number,
                        &PreferredBlobDataWithId {
                            inner: preferred_blob.into(),
                            id: blob.hash().into(),
                        },
                        state,
                    )
                    .unwrap_infallible();
                None
            }
            (Ordering::Less, _) => {
                // If the sequence number is less than the expected one, we discard the blob
                self.log_discarded_blob(blob, BlobDiscardReason::SequenceNumberTooLow);
                None
            }
        }
    }

    /// Select blobs when transitioning from a preferred sequencer back to normal operation.
    /// This occurs when the preferred sequencer was slashed for malicious behavior. In recovery mode,
    /// the rollup processes two visible slots at a time until it catches up to the current slot, after
    /// which it performs standard based sequencing.
    fn select_blobs_in_recovery_mode<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S::Storage>,
    ) -> BlobSelectorOutput<S, BlobDataWithId<BatchWithId>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        tracing::trace!("On recovery mode path");

        // First, decide how many slots worth of stored blobs we need. It could be 0, 1, or 2.
        let batches_needed_from_this_slot = match state
            .rollup_height_to_access()
            .get()
            .saturating_sub(state.visible_rollup_height().get())
        {
            // If the visible slot has caught up to the current slot, we don't need any stored blobs.
            // In this case, we act like a normal "based" rollup
            0 => return self.select_blobs_as_based_sequencer_inner(current_blobs, state),
            // If the visible slot is only trailing by one, we process one stored slot (to catch up) and
            // then process the new blobs from this slot
            1 => {
                self.select_blobs_as_based_sequencer_inner(current_blobs, state);
                1
            }
            // Otherwise, we need to process two slots from storage  - which means that we need to save the new blobs
            _ => {
                let new_batches: Vec<_> = self
                    .select_blobs_da_ordering(current_blobs, state)
                    .into_iter()
                    .map(|(batch, seq)| (batch, seq.address().clone()))
                    .collect();

                self.store_batches(state.rollup_height_to_access(), &new_batches, state);
                self.set_next_visible_rollup_height(
                    state.visible_rollup_height().saturating_add(2).as_visible(),
                    state,
                );
                2
            }
        };

        let mut blobs_with_total_size_limit = BlobsWithTotalSizeLimit::<S>::new();

        for slot in 0..=batches_needed_from_this_slot {
            let slot_to_check = state.visible_rollup_height().saturating_add(slot);
            let batches_from_next_slot = self.take_blobs_for_rollup_height(slot_to_check, state);

            for (batch, sender) in batches_from_next_slot {
                // Only push the blobs that are within the total size limit.
                blobs_with_total_size_limit
                    .push_or_ignore((batch, SequencerType::Standard(sender)));
            }
        }

        BlobSelectorOutput {
            selected_blobs: blobs_with_total_size_limit.inner(),
            should_execute_slot_hooks: true,
        }
    }

    /// Select the blobs to execute this slot based on the preferred sequencer and set the next visible_height.
    ///
    /// During each `slot`, we process up to one `Batch` of transactions from the preferred sequencer, plus all of the proofs
    /// and batches that the preferred sequencer had seen by the time they submitted their batch. This is accomplished using
    /// a sequencer number (to prevent re-ordering of data submitted by the preferred sequencer) and a visible rollup height.
    /// In each of their batches, the sequencer gets to choose how far to advance the visible rollup height. For example, if
    /// the preferred sequencer had seen all the data up to slot 10 but hadn't posted a batch since slot 5, they would choose
    /// to increment the visible rollup height by 5 in their next on-chain message.
    ///
    /// During each `slot`, we process all proofs...
    /// - (If sent by the preferred sequencer) whose sequence number is less than that of the next preferred batch
    /// - (If sent by anyone else) which appeared on chain before or during the current *visible* rollup height
    /// For batches, we select
    /// - The next one sent by the preferred sequencer (if available)
    /// - Any batches which appeared on chain before or during the current *visible* slot number
    #[tracing::instrument(skip_all)]
    fn select_blobs_for_preferred_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S::Storage>,
        preferred_sender: &<S::Da as DaSpec>::Address,
    ) -> BlobSelectorOutput<S, BlobDataWithId<BatchWithId>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        tracing::trace!("On preferred sequencer path");
        let mut unregistered_blobs = 0;
        let mut new_forced_blobs = Vec::new();
        let mut next_sequence_number = self
            .next_sequence_number
            .get(state)
            .unwrap_infallible()
            .unwrap_or(0);

        let mut blobs_with_total_size_limit = BlobsWithTotalSizeLimit::<S>::new();
        // We store the preferred batch with the lowest sequence number in this variable.
        let mut next_preferred_batch = None;
        // First, we loop through the blobs and categorize them based on sender and namespace.
        // We'll add first batch sent by the preferred sequencer and all proofs sent by the preferred sequencer
        // to the list of blobs to execute in this slot.
        for (idx, origin) in current_blobs.into_iter().enumerate() {
            tracing::trace!("Checking blob {}", idx);

            match origin {
                BlobOrigin::Proof(blob) => {
                    if &blob.sender() == preferred_sender {
                        if let Some(proof) = self
                            .deserialize_or_try_slash_sender::<PreferredProofData>(
                                blob, true, state,
                            )
                            .and_then(|batch| {
                                self.enforce_preferred_blob_ordering(
                                    batch,
                                    next_sequence_number,
                                    blob,
                                    state,
                                    true,
                                )
                            })
                        {
                            next_sequence_number += 1;
                            let data = BlobData::Proof(proof.data).with_id(blob.hash().into());

                            // Only push the blobs that are within the total size limit.
                            blobs_with_total_size_limit.push_or_ignore((
                                data,
                                SequencerType::Preferred(preferred_sender.clone()),
                            ));
                        }
                    } else if self
                        .sequencer_registry
                        .is_sender_allowed(&blob.sender(), state)
                        .is_ok()
                    {
                        if let Some(proof) =
                            self.deserialize_or_try_slash_sender::<Vec<u8>>(blob, true, state)
                        {
                            let data = BlobData::Proof(proof).with_id(blob.hash().into());
                            new_forced_blobs.push((data, blob.sender()));
                        }
                    } else {
                        self.log_discarded_blob(blob, BlobDiscardReason::SenderInsufficientStake);
                    }
                }
                BlobOrigin::Batch(blob) => {
                    match self.validate_blob_and_sender(blob, unregistered_blobs, state) {
                        ValidateBlobOutcome::Discard(reason) => {
                            self.log_discarded_blob(blob, reason);
                        }
                        ValidateBlobOutcome::Accept(sequencer_status) => {
                            let from_registered_sequencer =
                                matches!(sequencer_status, SequencerStatus::Registered);

                            if !from_registered_sequencer {
                                unregistered_blobs += 1;
                            }

                            // Check if the blob is from the preferred sequencer
                            if &blob.sender() == preferred_sender {
                                let batch = if let Some(batch) = self
                                    .deserialize_or_try_slash_sender::<PreferredBatchData>(
                                        blob,
                                        from_registered_sequencer,
                                        state,
                                    ) {
                                    batch
                                } else {
                                    continue;
                                };

                                let maybe_next_batch = self.enforce_preferred_blob_ordering(
                                    batch,
                                    next_sequence_number,
                                    blob,
                                    state,
                                    next_preferred_batch.is_none(),
                                );
                                if let Some(next_batch) = maybe_next_batch {
                                    next_preferred_batch = Some((next_batch, blob.hash().into()));
                                    next_sequence_number += 1;
                                }
                            } else {
                                // Otherwise, the batch is from a valid sender (checked in step 1) but not the preferred sender
                                // Deserialize it as a normal batch and store it in memory
                                let data = if from_registered_sequencer {
                                    self.deserialize_or_try_slash_sender::<Vec<FullyBakedTx>>(
                                        blob,
                                        from_registered_sequencer,
                                        state,
                                    )
                                    .map(BlobData::new_batch)
                                } else {
                                    self.deserialize_or_try_slash_sender::<RawTx>(
                                        blob,
                                        from_registered_sequencer,
                                        state,
                                    )
                                    .map(BlobData::EmergencyRegistration)
                                };
                                if let Some(data) = data {
                                    new_forced_blobs
                                        .push((data.with_id(blob.hash().into()), blob.sender()));
                                }
                            }
                        }
                    }
                }
            }
        }

        // If we haven't found a preferred batch yet, iterate through our saved ("deferred") blobs looking for a batch. Assuming we find at least
        // one preferred batch, we'll also process any proofs we encounter along the way during this slot.
        while next_preferred_batch.is_none() {
            if let Some(next_blob) = self
                .deferred_preferred_sequencer_blobs
                .remove(&next_sequence_number, state)
                .unwrap_infallible()
            {
                next_sequence_number += 1;
                match next_blob.inner {
                    PreferredBlobData::Batch(next_batch) => {
                        next_preferred_batch = Some((next_batch, next_blob.id));
                    }
                    PreferredBlobData::Proof(p) => {
                        let data = BlobData::Proof(p.data).with_id(next_blob.id);

                        // Only push the blobs that are within the total size limit.
                        blobs_with_total_size_limit.push_or_ignore((
                            data,
                            SequencerType::Preferred(preferred_sender.clone()),
                        ));
                    }
                }
            } else {
                break;
            }
        }

        // Next, find number of visible slots to advance.
        // - If the preferred sequencer requested a number, advance up to that many (stopping early if the next visible slot would be in the future)
        // - Otherwise, advance only if we would otherwise exceed the maximum deferred slots count
        let max_slots_to_advance = state
            .rollup_height_to_access()
            .get()
            .saturating_sub(state.visible_rollup_height().get())
            .saturating_add(1);
        self.next_sequence_number
            .set(&next_sequence_number, state)
            .unwrap_infallible();
        let num_slots_to_advance = if let Some((preferred_batch, id)) = next_preferred_batch {
            let next_batch = BlobDataWithId::Batch(BatchWithId::new(preferred_batch.data, id));

            // Only push the blobs that are within the total size limit.
            blobs_with_total_size_limit.push_or_ignore((
                next_batch,
                SequencerType::Preferred(preferred_sender.clone()),
            ));

            tracing::debug!(
                seq_number = preferred_batch.sequence_number,
                slots_to_advance = preferred_batch.visible_slots_to_advance,
                "Requested to advance slots"
            );

            if preferred_batch.visible_slots_to_advance as u64 > max_slots_to_advance {
                warn!(
                    "Preferred sequencer requested {} slots, but we can only advance {} slots",
                    preferred_batch.visible_slots_to_advance, max_slots_to_advance
                );
                max_slots_to_advance
            } else {
                std::cmp::max(preferred_batch.visible_slots_to_advance as u64, 1)
            }
        } else {
            // If there's no preferred blob, advance only if the we would otherwise exceed the maximum deferred slots count
            if state
                .visible_rollup_height()
                .saturating_add(config_deferred_slots_count())
                <= state.rollup_height_to_access()
            {
                1
            } else {
                0
            }
        };

        tracing::debug!(
            num_slots_to_advance,
            current_real_slot = %state.rollup_height_to_access(),
            "Advancing visible rollup height"
        );

        // Load all the necessary batches from storage
        for slot in 0..=num_slots_to_advance {
            let slot_to_check = state.visible_rollup_height().saturating_add(slot);
            let batches_from_next_slot = self.take_blobs_for_rollup_height(slot_to_check, state);
            tracing::trace!(
                "Found {} additional blobs in slot {} ",
                batches_from_next_slot.len(),
                slot_to_check
            );
            for (batch, sender) in batches_from_next_slot {
                // Only push the blobs that are within the total size limit.
                blobs_with_total_size_limit
                    .push_or_ignore((batch, SequencerType::Standard(sender)));
            }
        }

        // Check if we also need the blobs from the current slot. Add them to the set to be processed or store them as appropriate.
        let next_visible_height = state
            .visible_rollup_height()
            .saturating_add(num_slots_to_advance);
        if next_visible_height >= state.rollup_height_to_access() {
            for (batch, sender) in new_forced_blobs.into_iter().map(|b| (b.0, b.1)) {
                blobs_with_total_size_limit
                    .push_or_ignore((batch, SequencerType::Standard(sender)));
            }
        } else {
            self.store_batches(state.rollup_height_to_access(), &new_forced_blobs, state);
        }

        self.set_next_visible_rollup_height(next_visible_height.as_visible(), state);

        BlobSelectorOutput {
            selected_blobs: blobs_with_total_size_limit.inner(),
            should_execute_slot_hooks: num_slots_to_advance > 0,
        }
    }

    /// Deserialize a blob into a `Batch` or slash the sender if it's malformed.
    /// The sequencer might not exist if we're processing a blob submitted by an unregistered
    /// sequencer - in the case of direct sequencer registration via DA.
    fn deserialize_or_try_slash_sender<B: BorshDeserialize>(
        &self,
        blob: &mut <S::Da as DaSpec>::BlobTransaction,
        registered_sender: bool,
        state: &mut impl InfallibleStateAccessor,
    ) -> Option<B> {
        match B::try_from_slice(data_for_deserialization(blob)) {
            Ok(batch) => Some(batch),
            // if the blob is malformed, slash the sequencer
            Err(e) => {
                assert_eq!(blob.verified_data().len(), blob.total_len(), "Batch deserialization failed and some data was not provided. The prover might be malicious");
                let leading_bytes =
                    &blob.verified_data()[..std::cmp::min(100, blob.verified_data().len())];
                debug!(
                    deserializing_as = std::any::type_name::<B>(),
                    ?leading_bytes,
                    "Deserializing blob"
                );
                error!(
                    blob_hash = hex::encode(blob.hash()),
                    slashed_sender = %blob.sender(),
                    error = ?e,
                    "Unable to deserialize blob. slashing sender if they are registered"
                );

                if registered_sender {
                    self.sequencer_registry
                        .slash_sequencer(&blob.sender(), state);
                } else {
                    info!("Unable to slash sequencer, they were not registered");
                }

                None
            }
        }
    }
}

// The public API of the BlobStorage module.
impl<S: Spec> BlobStorage<S> {
    /// This implementation returns three categories of blobs:
    /// 1. Any blobs sent by the preferred sequencer ("prority blobs")
    /// 2. Any non-priority blobs which were sent `DEFERRED_SLOTS_COUNT` slots ago ("expiring deferred blobs")
    /// 3. Some additional deferred blobs needed to fill the total requested by the sequencer, if applicable. ("bonus blobs")
    pub fn get_blobs_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S::Storage>,
    ) -> anyhow::Result<BlobSelectorOutput<S, BlobDataWithId<IterableBatchWithId>>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        let current_blobs = take_blobs_with_size_limit::<_, S>(current_blobs);
        // If `DEFERRED_SLOTS_COUNT` is 0, we treat the rollup as having no preferred sequencer.
        // In this case, we just process blobs in the order that they appeared on the DA layer
        if config_deferred_slots_count() == 0 {
            return Ok(self
                .select_blobs_as_based_sequencer_inner(current_blobs, state)
                .map_blobs(|b| b.map_batch(IterableBatchWithId::new)));
        }

        // If there's a preferred sequencer, sequence accordingly.
        if let Some(preferred_sender) = self.get_preferred_sequencer(state) {
            return Ok(self
                .select_blobs_for_preferred_sequencer(current_blobs, state, &preferred_sender)
                .map_blobs(|b| b.map_batch(IterableBatchWithId::new)));
        }

        // Otherwise, we're configured for a preferred sequencer but one doesn't exist. This usually means that the preferred sequencer was slashed.
        // Entery recovery mode.
        Ok(self
            .select_blobs_in_recovery_mode(current_blobs, state)
            .map_blobs(|b| b.map_batch(IterableBatchWithId::new)))
    }

    /// Select the blobs to execute this slot using "based sequencing". In this mode,
    /// blobs are processed in the order that they appear on the DA layer.
    pub fn select_blobs_as_based_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S::Storage>,
    ) -> BlobSelectorOutput<S, BlobDataWithId<BatchWithId>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        let current_blobs = take_blobs_with_size_limit::<_, S>(current_blobs);

        self.select_blobs_as_based_sequencer_inner(current_blobs, state)
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
