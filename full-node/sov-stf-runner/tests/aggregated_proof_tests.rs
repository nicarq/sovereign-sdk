use sov_mock_da::{MockBlock, MockBlockHeader};
use sov_rollup_interface::zk::aggregated_proof::AggregatedProofPublicInput;
use sov_stf_runner::InitVariant;
mod helpers;
use helpers::runner_init::{initialize_runner, TestNode};

#[tokio::test]
async fn fetch_aggregated_proof_test() -> Result<(), anyhow::Error> {
    for jump in [1, 7] {
        let test_case = TestCase::new(jump);
        for threads in [1, 3] {
            run_make_proof_sync(test_case, threads).await?;
            run_make_proof_async(test_case, threads).await?;
        }
    }

    Ok(())
}

// In this test proofs are created just after batch is submitted to the DA.
async fn run_make_proof_sync(
    test_case: TestCase,
    aggregated_proof_block_jump: usize,
) -> Result<(), anyhow::Error> {
    let mut test_node = spawn(test_case.jump(), aggregated_proof_block_jump);
    // Clear the notification from the genesis slot
    test_node.wait_for_all_slots().await;

    for i in 0..test_case.input.nb_of_transactions {
        test_node.send_transaction(true).await?;
        test_node.make_proof();
        if (i + 1) % test_case.jump() == 0 {
            test_node.wait_for_aggregated_proof_in_da().await;
        }
    }

    test_node.send_transaction(true).await?;
    let public_input = test_node.get_latest_public_input_proof()?.unwrap();
    test_case.assert(&public_input);
    Ok(())
}

// In this test proofs are created after multiple batches are submitted to the DA.
async fn run_make_proof_async(
    test_case: TestCase,
    aggregated_proof_block_jump: usize,
) -> Result<(), anyhow::Error> {
    let mut test_node = spawn(test_case.jump(), aggregated_proof_block_jump);

    for _ in 0..test_case.input.nb_of_transactions {
        test_node.send_transaction(false).await?;
    }

    for i in 0..test_case.input.nb_of_transactions {
        test_node.make_proof();
        if (i + 1) % test_case.jump() == 0 {
            test_node.wait_for_aggregated_proof_in_da().await;
        }
    }

    test_node.wait_for_all_slots().await;
    test_node.send_transaction(true).await?;

    let public_input = test_node.get_latest_public_input_proof()?.unwrap();
    test_case.assert(&public_input);
    Ok(())
}

fn spawn(jump: usize, aggregated_proof_block_jump: usize) -> TestNode {
    let genesis_block = MockBlock {
        header: MockBlockHeader::from_height(0),
        validity_cond: Default::default(),
        blobs: vec![],
    };
    let init_variant = InitVariant::Genesis {
        block: genesis_block,
        genesis_params: vec![1],
    };

    let (mut runner, test_node) =
        initialize_runner(init_variant, jump, aggregated_proof_block_jump);
    tokio::spawn(async move {
        runner.run_in_process().await.unwrap();
    });

    test_node
}

#[derive(Clone, Copy)]
struct Input {
    jump: usize,
    nb_of_transactions: usize,
}

#[derive(Clone, Copy)]
struct Output {
    initial_slot_number: u64,
}

#[derive(Clone, Copy)]
struct TestCase {
    input: Input,
    output: Output,
}

impl TestCase {
    fn new(jump: usize) -> Self {
        // Generate 7 aggregate-proofs worth of blocks
        let nb_of_transactions = 7 * jump;
        // The initial slot number of the final proof
        // The first proof covers blocks 1..=jump, the second jump+1..=(2*jump), etc.
        let initial_slot_number = (6 * jump + 1) as u64;
        Self {
            input: Input {
                jump,
                nb_of_transactions,
            },
            output: Output {
                initial_slot_number,
            },
        }
    }

    fn jump(&self) -> usize {
        self.input.jump
    }

    fn final_slot_number(&self) -> u64 {
        self.output.initial_slot_number + (self.input.jump as u64) - 1
    }

    fn assert(&self, public_input: &AggregatedProofPublicInput) {
        assert_eq!(
            self.output.initial_slot_number,
            public_input.initial_slot_number,
        );

        assert_eq!(self.final_slot_number(), public_input.final_slot_number);
    }
}
