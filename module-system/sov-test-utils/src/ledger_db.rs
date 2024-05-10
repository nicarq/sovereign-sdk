use std::str::FromStr;

use sov_bank::utils::TokenHolder;
use sov_bank::{Coins, TokenId};
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_mock_da::{MockBlock, MockDaSpec};
use sov_modules_api::{AggregatedProofPublicData, CodeCommitment, ModuleId, StoredEvent};
use sov_modules_stf_blueprint::BatchReceipt;
use sov_rollup_interface::stf::TransactionReceipt;
use sov_rollup_interface::zk::aggregated_proof::{AggregatedProof, SerializedAggregatedProof};

use crate::TestSpec;

/// Very, very simple utility function: it just persists some dummy data to the
/// [`LedgerDb`], so that it's not empty when you read it within tests.
pub async fn add_data_to_ledger_db(ledger_db: &LedgerDb) -> anyhow::Result<()> {
    let block_a = MockBlock::default();

    let mut slot: SlotCommit<MockBlock, i32, i32> = SlotCommit::new(block_a);

    let tx_receipts = vec![TransactionReceipt {
        tx_hash: [1; 32],
        body_to_save: Some(b"tx-body".to_vec()),
        events: events(),
        receipt: 0,
        gas_used: vec![0, 1, u64::MAX],
    }];

    slot.add_batch(BatchReceipt {
        batch_hash: [10; 32],
        tx_receipts,
        inner: 0,
        gas_price: vec![0, 1, u64::MAX],
    });

    ledger_db.commit_slot(slot, b"state-root")?;

    ledger_db.save_finalized_aggregated_proof(AggregatedProof::new(
        SerializedAggregatedProof {
            raw_aggregated_proof: b"aggregated-proof".to_vec(),
        },
        // By filling all the fields, clients can more thoroughly test
        // (de)serialization logic.
        //
        // This data doesn't make any sense (they're not even hashes...), but
        // it's just for testing.
        AggregatedProofPublicData {
            validity_conditions: vec![],
            initial_slot_number: u64::MAX,
            final_slot_number: u64::MAX,
            genesis_state_root: b"genesis-state-root".to_vec(),
            initial_state_root: b"initial-state-root".to_vec(),
            final_state_root: b"final-state-root".to_vec(),
            initial_slot_hash: b"initial-slot-hash".to_vec(),
            final_slot_hash: b"final-slot-hash".to_vec(),
            code_commitment: CodeCommitment(b"code-commitment".to_vec()),
        },
    ))?;

    Ok(())
}

fn events() -> Vec<StoredEvent> {
    let holder = TokenHolder::Module(ModuleId::from([0; 32]));
    let token_id =
        TokenId::from_str("token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6")
            .unwrap();

    let event_value1 = demo_stf::runtime::RuntimeEvent::<TestSpec, MockDaSpec>::bank(
        sov_bank::event::Event::TokenCreated {
            token_name: "token".to_string(),
            coins: Coins {
                amount: 0,
                token_id,
            },
            minter: holder.clone(),
            authorized_minters: vec![],
        },
    );
    let event_value2 = demo_stf::runtime::RuntimeEvent::<TestSpec, MockDaSpec>::bank(
        sov_bank::event::Event::TokenFrozen {
            token_id,
            freezer: holder,
        },
    );

    vec![
        StoredEvent::new("foo".as_bytes(), &borsh::to_vec(&event_value1).unwrap()),
        StoredEvent::new("bar".as_bytes(), &borsh::to_vec(&event_value2).unwrap()),
    ]
}

#[cfg(test)]
mod tests {
    use sov_mock_da::MockDaSpec;

    use super::*;
    use crate::TestSpec;

    #[test]
    fn events_deserialize_correctly() {
        let events = events();
        for event in events {
            <demo_stf::runtime::RuntimeEvent<TestSpec, MockDaSpec> as borsh::BorshDeserialize>::deserialize(
                &mut &event.value().inner()[..]).unwrap();
        }
    }
}
