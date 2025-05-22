use sov_rollup_interface::common::SlotNumber;

use crate::SequencerNotReadyDetails;

pub struct NodeDeltaWatcher {
    pub sequencer_slot_number: SlotNumber,
    pub node_slot_number: SlotNumber,
    max_delta: u64,
}

impl NodeDeltaWatcher {
    pub fn new(max_delta: u64) -> Self {
        Self {
            // The height fields are initialized by the
            // `update_state()` call when first initializing the sequencer
            sequencer_slot_number: SlotNumber::GENESIS,
            node_slot_number: SlotNumber::GENESIS,
            max_delta,
        }
    }

    pub fn check_delta(&self) -> Result<(), SequencerNotReadyDetails> {
        let seq_slot_num = self.sequencer_slot_number.get();
        let node_slot_num = self.node_slot_number.get();

        let Some(delta) = seq_slot_num.checked_sub(node_slot_num) else {
            return Ok(());
        };

        if delta >= self.max_delta {
            Err(SequencerNotReadyDetails::WaitingOnNode {
                current_node_slot_number: node_slot_num,
                current_sequencer_slot_number: seq_slot_num,
                current_delta: delta,
                max_allowed_delta: self.max_delta,
            })
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_rollup_interface::common::IntoSlotNumber;

    use super::*;

    #[test]
    fn test_check_node_delta() {
        let mut tracker = NodeDeltaWatcher {
            sequencer_slot_number: 10.to_slot_number(),
            node_slot_number: 5.to_slot_number(),
            max_delta: 5,
        };
        // delta equal to max delta
        assert!(tracker.check_delta().is_err());
        tracker.node_slot_number = 4.to_slot_number();
        // delta greater than max delta
        assert!(tracker.check_delta().is_err());
        // no delta
        tracker.node_slot_number = 10.to_slot_number();
        assert!(tracker.check_delta().is_ok());
        // node ahead
        tracker.node_slot_number = 11.to_slot_number();
        assert!(tracker.check_delta().is_ok());
    }

    #[test]
    fn test_check_node_delta_returned_fields() {
        let tracker = NodeDeltaWatcher {
            sequencer_slot_number: 10.to_slot_number(),
            node_slot_number: 2.to_slot_number(),
            max_delta: 5,
        };
        if let Err(SequencerNotReadyDetails::WaitingOnNode {
            current_node_slot_number,
            current_sequencer_slot_number,
            current_delta,
            max_allowed_delta,
        }) = tracker.check_delta()
        {
            assert_eq!(current_node_slot_number, 2);
            assert_eq!(current_sequencer_slot_number, 10);
            assert_eq!(current_delta, 8);
            assert_eq!(max_allowed_delta, 5);
        } else {
            panic!("expected WaitingOnNode error");
        }
    }
}
