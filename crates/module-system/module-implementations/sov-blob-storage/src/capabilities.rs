use std::cmp::Ordering;

use borsh::BorshDeserialize;
use sov_modules_api::capabilities::BlobOrigin;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::runtime::capabilities::BlobSelector;
use sov_modules_api::{
    Batch, BlobData, BlobDataWithId, BlobReaderTrait, DaSpec, InfallibleStateAccessor,
    KernelStateAccessor, RawTx, Spec, VersionReader,
};
use sov_sequencer_registry::AllowedSequencerError;
use tracing::{debug, error, info, warn};

use crate::{
    BlobStorage, PreferredBatchData, PreferredBlobData, PreferredBlobDataWithId,
    PreferredProofData, PreferredSequenced, SequenceNumber, DEFERRED_SLOTS_COUNT,
    UNREGISTERED_BLOBS_PER_SLOT,
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

enum SequencerStatus {
    Registered,
    Unregistered,
}

enum ValidateBlobOutcome {
    Discard(BlobDiscardReason),
    Accept(SequencerStatus),
}

impl<S: Spec, Da: DaSpec> BlobStorage<S, Da> {
    fn set_next_visible_slot_number(&self, value: u64, state: &mut KernelStateAccessor<S>) {
        self.chain_state.set_next_visible_slot_number(&value, state);
    }

    /// Select the blobs to execute this slot using "based sequencing". In this mode,
    /// blobs are processed in the order that they appear on the DA layer.
    pub fn select_blobs_as_based_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S>,
    ) -> Vec<(BlobDataWithId, Da::Address)>
    where
        I: IntoIterator<Item = BlobOrigin<'a, Da::BlobTransaction>>,
    {
        tracing::trace!("On based sequencer path");

        self.set_next_visible_slot_number(state.rollup_height_to_access().saturating_add(1), state);

        self.select_blobs_da_ordering(current_blobs, state)
    }

    fn select_blobs_da_ordering<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S>,
    ) -> Vec<(BlobDataWithId, Da::Address)>
    where
        I: IntoIterator<Item = BlobOrigin<'a, Da::BlobTransaction>>,
    {
        let mut batches = Vec::new();
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
                            let data = BlobDataWithId {
                                data: BlobData::Proof(proof),
                                id: blob.hash().into(),
                            };
                            batches.push((data, blob.sender()));
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
                                self.deserialize_or_try_slash_sender::<Batch>(
                                    blob,
                                    from_registered_sequencer,
                                    state,
                                )
                                .map(BlobData::Batch)
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

                                batches.push((
                                    BlobDataWithId {
                                        data,
                                        id: blob.hash().into(),
                                    },
                                    blob.sender(),
                                ));
                            }
                        }
                    };
                }
            }
        }

        batches
    }

    fn validate_blob_and_sender(
        &self,
        blob: &Da::BlobTransaction,
        unregistered_blobs_processed: u64,
        state: &mut KernelStateAccessor<S>,
    ) -> ValidateBlobOutcome {
        match self
            .sequencer_registry
            .is_sender_allowed(&blob.sender(), state)
        {
            Ok(_) => ValidateBlobOutcome::Accept(SequencerStatus::Registered),
            Err(e) => match e {
                AllowedSequencerError::InsufficientStakeAmount { .. } => {
                    ValidateBlobOutcome::Discard(BlobDiscardReason::SenderInsufficientStake)
                }
                AllowedSequencerError::NotRegistered => {
                    if unregistered_blobs_processed >= UNREGISTERED_BLOBS_PER_SLOT {
                        ValidateBlobOutcome::Discard(BlobDiscardReason::MaxAllowedUnregisteredBlobs)
                    } else {
                        ValidateBlobOutcome::Accept(SequencerStatus::Unregistered)
                    }
                }
            },
        }
    }

    fn log_discarded_blob(&self, blob: &Da::BlobTransaction, reason: BlobDiscardReason) {
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
        blob: &Da::BlobTransaction,
        state: &mut impl InfallibleStateAccessor,
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
    /// the rollup processes two virtual slots at a time until it catches up to the current slot, after
    /// which it performs standard based sequencing.
    fn select_blobs_in_recovery_mode<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S>,
    ) -> Vec<(BlobDataWithId, Da::Address)>
    where
        I: IntoIterator<Item = BlobOrigin<'a, Da::BlobTransaction>>,
    {
        tracing::trace!("On recovery mode path");
        let mut batches_to_process = Vec::new();

        // First, decide how many slots worth of stored blobs we need. It could be 0, 1, or 2.
        let batches_needed_from_this_slot = match state
            .rollup_height_to_access()
            .saturating_sub(state.virtual_slot_number())
        {
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
                self.store_batches(state.rollup_height_to_access(), &new_batches, state);
                2
            }
        };

        for slot in 0..=batches_needed_from_this_slot {
            let slot_to_check = state.virtual_slot_number().saturating_add(slot);
            let batches_from_next_slot = self.take_blobs_for_slot_number(slot_to_check, state);
            batches_to_process.extend(batches_from_next_slot.into_iter());
        }

        self.set_next_visible_slot_number(
            state
                .virtual_slot_number()
                .saturating_add(batches_needed_from_this_slot),
            state,
        );

        batches_to_process
    }

    /// Select the blobs to execute this slot based on the preferred sequencer and set the next virtual_height.
    ///
    /// During each `slot`, we process up to one `Batch` of transactions from the preferred sequencer, plus all of the proofs
    /// and batches that the preferred sequencer had seen by the time they submitted their batch. This is accomplished using
    /// a sequencer number (to prevent re-ordering of data submitted by the preferred sequencer) and a virtual slot number.
    /// In each of their batches, the sequencer gets to choose how far to advance the virtual slot number. For example, if
    /// the preferred sequencer had seen all the data up to slot 10 but hadn't posted a batch since slot 5, they would choose
    /// to increment the virtual slot number by 5 in their next on-chain message.
    ///
    /// During each `slot`, we process all proofs...
    /// - (If sent by the prferred sequencer) whose sequence number is less than that of the next preferred batch
    /// - (If sent by anyone else) which appeared on chain before or during the current *virtual* slot number
    /// For batches, we select
    /// - The next one sent by the preferred sequencer (if available)
    /// - Any batches  which appeared on chain before or during the current *virtual* slot number
    #[tracing::instrument(skip_all)]
    fn select_blobs_for_preferred_sequencer<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S>,
        preferred_sender: &Da::Address,
    ) -> Vec<(BlobDataWithId, Da::Address)>
    where
        I: IntoIterator<Item = BlobOrigin<'a, Da::BlobTransaction>>,
    {
        tracing::trace!("On preferred sequencer path");
        let mut unregistered_blobs = 0;
        let mut new_forced_blobs = Vec::new();
        let mut next_sequence_number = self
            .next_sequence_number
            .get(state)
            .unwrap_infallible()
            .unwrap_or(0);

        let mut blobs_to_process = Vec::new();
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
                            let data = BlobDataWithId {
                                data: BlobData::Proof(proof.data),
                                id: blob.hash().into(),
                            };
                            blobs_to_process.push((data, preferred_sender.clone()));
                        }
                    } else if self
                        .sequencer_registry
                        .is_sender_allowed(&blob.sender(), state)
                        .is_ok()
                    {
                        if let Some(proof) =
                            self.deserialize_or_try_slash_sender::<Vec<u8>>(blob, true, state)
                        {
                            let data = BlobDataWithId {
                                data: BlobData::Proof(proof),
                                id: blob.hash().into(),
                            };
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
                                    self.deserialize_or_try_slash_sender::<Batch>(
                                        blob,
                                        from_registered_sequencer,
                                        state,
                                    )
                                    .map(BlobData::Batch)
                                } else {
                                    self.deserialize_or_try_slash_sender::<RawTx>(
                                        blob,
                                        from_registered_sequencer,
                                        state,
                                    )
                                    .map(BlobData::EmergencyRegistration)
                                };
                                if let Some(data) = data {
                                    new_forced_blobs.push((
                                        BlobDataWithId {
                                            data,
                                            id: blob.hash().into(),
                                        },
                                        blob.sender(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        // If we haven't found a preferred batch yet, through our saved ("deferred") blobs looking for a batch. We'll process any proofs we encounter
        // along the way in this slot.
        while next_preferred_batch.is_none() {
            if let Some(next_blob) = self
                .deferred_preferred_sequencer_blobs
                .get(&next_sequence_number, state)
                .unwrap_infallible()
            {
                next_sequence_number += 1;
                match next_blob.inner {
                    PreferredBlobData::Batch(next_batch) => {
                        next_preferred_batch = Some((next_batch, next_blob.id));
                    }
                    PreferredBlobData::Proof(p) => {
                        let data = BlobDataWithId {
                            data: BlobData::Proof(p.data),
                            id: next_blob.id,
                        };
                        blobs_to_process.push((data, preferred_sender.clone()));
                    }
                }
            } else {
                break;
            }
        }

        // Next, find number of virtual slots to advance.
        // - If the preferred sequencer requested a number, advance up to that many (stopping early if the next virtual slot would be in the future)
        // - Otherwise, advance only if we would otherwise exceed the maximum deferred slots count
        let max_slots_to_advance = state
            .rollup_height_to_access()
            .saturating_sub(state.virtual_slot_number())
            .saturating_add(1);
        self.next_sequence_number
            .set(&next_sequence_number, state)
            .unwrap_infallible();
        let num_slots_to_advance = if let Some((preferred_batch, id)) = next_preferred_batch {
            let next_batch = BlobDataWithId {
                data: BlobData::Batch(preferred_batch.data),
                id,
            };

            blobs_to_process.push((next_batch, preferred_sender.clone()));
            tracing::debug!(
                seq_number = preferred_batch.sequence_number,
                slots_to_advance = preferred_batch.virtual_slots_to_advance,
                "Requested to advance slots"
            );

            if preferred_batch.virtual_slots_to_advance as u64 > max_slots_to_advance {
                warn!(
                    "Preferred sequencer requested {} slots, but we can only advance {} slots",
                    preferred_batch.virtual_slots_to_advance, max_slots_to_advance
                );
                max_slots_to_advance
            } else {
                std::cmp::max(preferred_batch.virtual_slots_to_advance as u64, 1)
            }
        } else {
            // If there's no preferred blob, advance only if the we would otherwise exceed the maximum deferred slots count
            if state
                .virtual_slot_number()
                .saturating_add(DEFERRED_SLOTS_COUNT)
                <= state.rollup_height_to_access()
            {
                1
            } else {
                0
            }
        };
        tracing::debug!(
            num_slots_to_advance,
            current_real_slot = state.rollup_height_to_access(),
            "Advancing virtual slot number"
        );

        // Load all the necessary batches from storage
        for slot in 0..=num_slots_to_advance {
            let slot_to_check = state.virtual_slot_number().saturating_add(slot);
            let batches_from_next_slot = self.take_blobs_for_slot_number(slot_to_check, state);
            tracing::trace!(
                "Found {} additional blobs in slot {} ",
                batches_from_next_slot.len(),
                slot_to_check
            );
            blobs_to_process.extend(batches_from_next_slot.into_iter());
        }

        // Check if we also need the blobs from the current slot. Add them to the set to be processed or store them as appropriate.
        let next_virtual_height = state
            .virtual_slot_number()
            .saturating_add(num_slots_to_advance);
        if next_virtual_height >= state.rollup_height_to_access() {
            blobs_to_process.extend(new_forced_blobs);
        } else {
            self.store_batches(state.rollup_height_to_access(), &new_forced_blobs, state);
        }

        self.set_next_visible_slot_number(next_virtual_height, state);

        blobs_to_process
    }

    /// Deserialize a blob into a `Batch` or slash the sender if it's malformed.
    /// The sequencer might not exist if we're processing a blob submitted by an unregistered
    /// sequencer - in the case of direct sequencer registration via DA.
    fn deserialize_or_try_slash_sender<B: BorshDeserialize>(
        &self,
        blob: &mut Da::BlobTransaction,
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

impl<S: Spec, Da: DaSpec> BlobSelector<Da> for BlobStorage<S, Da> {
    type Spec = S;

    type BlobType = BlobDataWithId;

    // This implementation returns three categories of blobs:
    // 1. Any blobs sent by the preferred sequencer ("prority blobs")
    // 2. Any non-priority blobs which were sent `DEFERRED_SLOTS_COUNT` slots ago ("expiring deferred blobs")
    // 3. Some additional deferred blobs needed to fill the total requested by the sequencer, if applicable. ("bonus blobs")
    fn get_blobs_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, S>,
    ) -> anyhow::Result<Vec<(Self::BlobType, Da::Address)>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, Da::BlobTransaction>>,
    {
        // If `DEFERRED_SLOTS_COUNT` is 0, we treat the rollup as having no preferred sequencer.
        // In this case, we just process blobs in the order that they appeared on the DA layer
        if DEFERRED_SLOTS_COUNT == 0 {
            return Ok(self.select_blobs_as_based_sequencer(current_blobs, state));
        }

        // If there's a preferred sequencer, sequence accordingly.
        if let Some(preferred_sender) = self.get_preferred_sequencer(state) {
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
