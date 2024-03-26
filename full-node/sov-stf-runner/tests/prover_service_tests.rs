mod helpers;
use sov_mock_da::{
    MockBlockHeader, MockDaService, MockDaSpec, MockDaVerifier, MockHash, MockValidityCond,
};
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier, MockZkvm};
use sov_modules_api::Zkvm;
use sov_rollup_interface::da::Time;
use sov_rollup_interface::zk::aggregated_proof::AggregatedProofPublicData;
use sov_rollup_interface::zk::StateTransitionWitness;
use sov_stf_runner::mock::MockStf;
use sov_stf_runner::{
    ParallelProverService, ProofAggregationStatus, ProofProcessingStatus, ProverService,
    ProverServiceConfig, ProverServiceError, RollupProverConfig, StateTransitionInfo,
    WitnessSubmissionStatus,
};
use tokio::time;

type StateRoot = Vec<u8>;

#[tokio::test]
async fn test_successful_prover_execution() -> Result<(), ProverServiceError> {
    let TestProver {
        prover_service,
        inner_vm,
        ..
    } = make_new_prover(1);

    let header_hash = MockHash::from([0; 32]);
    prover_service
        .submit_state_transition_info(make_transition_info(header_hash, 1))
        .await;
    prover_service.prove(header_hash).await?;

    inner_vm.make_proof();

    let status = wait_for_aggregated_proof(&[header_hash], &prover_service)
        .await
        .unwrap();

    assert!(matches!(status, ProofAggregationStatus::Success(_)));

    // The proof has already been sent, and the prover_service no longer has a reference to it.
    let err = prover_service
        .create_aggregated_proof(&[header_hash])
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "Missing witness for: 0x0000000000000000000000000000000000000000000000000000000000000000"
    );

    Ok(())
}

#[tokio::test]
async fn test_prover_status_busy() -> Result<(), anyhow::Error> {
    let TestProver {
        prover_service,
        inner_vm,
        num_worker_threads,
        ..
    } = make_new_prover(1);

    let header_hashes = (1..num_worker_threads + 1).map(|hash| MockHash::from([hash as u8; 32]));

    let mut height = 1;
    // Saturate the prover.
    for header_hash in header_hashes.clone() {
        prover_service
            .submit_state_transition_info(make_transition_info(header_hash, height))
            .await;

        let poof_processing_status = prover_service.prove(header_hash).await?;
        assert_eq!(
            ProofProcessingStatus::ProvingInProgress,
            poof_processing_status
        );

        let proof_submission_status = prover_service
            .create_aggregated_proof(&[header_hash])
            .await?;

        assert_eq!(
            ProofAggregationStatus::ProofGenerationInProgress,
            proof_submission_status
        );
        height += 1;
    }

    // Attempting to create another proof while the prover is busy.
    {
        let header_hash = MockHash::from([0; 32]);
        prover_service
            .submit_state_transition_info(make_transition_info(header_hash, height))
            .await;

        let status = prover_service.prove(header_hash).await?;

        height += 1;
        // The prover is busy and won't accept any new jobs.
        assert_eq!(ProofProcessingStatus::Busy, status);

        let err = prover_service
            .create_aggregated_proof(&[header_hash])
            .await
            .unwrap_err();

        // The new job is not triggered.
        assert_eq!(
            err.to_string(),
            "Witness for 0x0000000000000000000000000000000000000000000000000000000000000000 was submitted, but the proof generation is not triggered."
        );
    }

    for _ in 0..header_hashes.len() {
        inner_vm.make_proof();
    }

    for header_hash in header_hashes.clone() {
        let status = wait_for_aggregated_proof(&[header_hash], &prover_service)
            .await
            .unwrap();
        assert!(matches!(status, ProofAggregationStatus::Success(_)));
    }

    // Retry once the prover is available to process new proofs.
    {
        let header_hash = MockHash::from([(num_worker_threads + 1) as u8; 32]);
        prover_service
            .submit_state_transition_info(make_transition_info(header_hash, height))
            .await;

        let status = prover_service.prove(header_hash).await?;
        assert_eq!(ProofProcessingStatus::ProvingInProgress, status);
    }

    Ok(())
}

#[tokio::test]
async fn test_missing_witness() -> Result<(), anyhow::Error> {
    let TestProver { prover_service, .. } = make_new_prover(1);
    let header_hash = MockHash::from([0; 32]);
    let err = prover_service.prove(header_hash).await.unwrap_err();

    assert_eq!(
        err.to_string(),
        "Missing witness for block: 0x0000000000000000000000000000000000000000000000000000000000000000"
    );
    Ok(())
}

#[tokio::test]
async fn test_multiple_witness_submissions() -> Result<(), anyhow::Error> {
    let TestProver { prover_service, .. } = make_new_prover(1);

    let header_hash = MockHash::from([0; 32]);
    let submission_status = prover_service
        .submit_state_transition_info(make_transition_info(header_hash, 1))
        .await;

    assert_eq!(
        WitnessSubmissionStatus::SubmittedForProving,
        submission_status
    );

    let submission_status = prover_service
        .submit_state_transition_info(make_transition_info(header_hash, 2))
        .await;

    assert_eq!(WitnessSubmissionStatus::WitnessExist, submission_status);

    Ok(())
}

#[tokio::test]
async fn test_generate_multiple_proofs_for_the_same_witness() -> Result<(), anyhow::Error> {
    let TestProver { prover_service, .. } = make_new_prover(5);

    let header_hash = MockHash::from([0; 32]);
    prover_service
        .submit_state_transition_info(make_transition_info(header_hash, 1))
        .await;

    let status = prover_service.prove(header_hash).await?;
    assert_eq!(ProofProcessingStatus::ProvingInProgress, status);

    let err = prover_service.prove(header_hash).await.unwrap_err();
    assert_eq!(err.to_string(), "Proof generation for 0x0000000000000000000000000000000000000000000000000000000000000000 still in progress");
    Ok(())
}

#[tokio::test]
async fn test_aggregated_proof() -> Result<(), ProverServiceError> {
    let total_nb_of_blocks: usize = 10;
    let jump = 5;
    let end_block = jump + 1;

    let TestProver {
        prover_service,
        inner_vm,
        ..
    } = make_new_prover(jump);

    let header_hashes: Vec<_> = (0..total_nb_of_blocks)
        .map(|h| MockHash::from([h as u8; 32]))
        .collect();

    // Prove blocks form 0 to jump, where the number of submitted witnesses is equal to end_block.
    {
        for (height, hash) in header_hashes[0..end_block].iter().enumerate() {
            prover_service
                .submit_state_transition_info(make_transition_info(*hash, height as u64))
                .await;

            prover_service.prove(*hash).await?;
        }

        let status = wait_for_aggregated_proof(&header_hashes[0..jump], &prover_service)
            .await
            .unwrap();
        // Waiting for the proof.
        assert!(matches!(
            status,
            ProofAggregationStatus::ProofGenerationInProgress
        ));

        // Make proof for each submitted block.
        for _ in 0..end_block {
            inner_vm.make_proof();
        }

        let status = wait_for_aggregated_proof(&header_hashes[0..jump], &prover_service)
            .await
            .unwrap();

        match status {
            ProofAggregationStatus::Success(proof) => {
                let public_data = <MockZkVerifier as Zkvm>::verify::<AggregatedProofPublicData>(
                    proof.raw_aggregated_proof.as_ref(),
                    &MockCodeCommitment::default(),
                )
                .unwrap();
                assert_eq!(public_data.initial_slot_number, 0);
                assert_eq!(public_data.final_slot_number, (jump - 1) as u64);
            }
            ProofAggregationStatus::ProofGenerationInProgress => panic!("Prover should succeed"),
        }
    }

    // Prove remaining blocks.
    {
        for (height, hash) in header_hashes[end_block..total_nb_of_blocks]
            .iter()
            .enumerate()
        {
            prover_service
                .submit_state_transition_info(make_transition_info(
                    *hash,
                    (height + end_block) as u64,
                ))
                .await;

            prover_service.prove(*hash).await?;
            inner_vm.make_proof();
        }

        let status =
            wait_for_aggregated_proof(&header_hashes[jump..total_nb_of_blocks], &prover_service)
                .await
                .unwrap();

        match status {
            ProofAggregationStatus::Success(proof) => {
                let public_data = <MockZkVerifier as Zkvm>::verify::<AggregatedProofPublicData>(
                    proof.raw_aggregated_proof.as_ref(),
                    &MockCodeCommitment::default(),
                )
                .unwrap();
                assert_eq!(public_data.initial_slot_number as usize, jump);
                assert_eq!(
                    public_data.final_slot_number as usize,
                    total_nb_of_blocks - 1
                );
            }
            ProofAggregationStatus::ProofGenerationInProgress => panic!("Proves should succeed"),
        }
    }

    Ok(())
}

struct TestProver {
    prover_service: ParallelProverService<
        StateRoot,
        Vec<u8>,
        MockDaService,
        MockZkvm,
        MockZkvm,
        MockStf<MockValidityCond>,
    >,
    inner_vm: MockZkvm,
    num_worker_threads: usize,
}

async fn wait_for_aggregated_proof(
    header_hashes: &[MockHash],
    prover_service: &ParallelProverService<
        StateRoot,
        Vec<u8>,
        MockDaService,
        MockZkvm,
        MockZkvm,
        MockStf<MockValidityCond>,
    >,
) -> Result<ProofAggregationStatus, anyhow::Error> {
    let mut counter = 0;
    loop {
        let status = prover_service
            .create_aggregated_proof(header_hashes)
            .await?;

        if let ProofAggregationStatus::Success(_) = &status {
            return Ok(status);
        }

        if counter == 10 {
            return Ok(status);
        }

        time::sleep(time::Duration::from_millis(1000)).await;
        counter += 1;
    }
}

fn make_new_prover(jump: usize) -> TestProver {
    let num_threads = 10;
    let inner_vm = MockZkvm::new();
    let outer_vm = MockZkvm::new_non_blocking();

    let prover_config = RollupProverConfig::Execute;
    let zk_stf = MockStf::<MockValidityCond>::default();
    let da_verifier = MockDaVerifier::default();
    TestProver {
        prover_service: ParallelProverService::new(
            inner_vm.clone(),
            outer_vm,
            zk_stf,
            da_verifier,
            prover_config,
            (),
            num_threads,
            ProverServiceConfig {
                aggregated_proof_block_jump: jump,
            },
            Default::default(),
        ),
        inner_vm,
        num_worker_threads: num_threads,
    }
}

fn make_transition_info(
    header_hash: MockHash,
    height: u64,
) -> StateTransitionInfo<StateRoot, Vec<u8>, MockDaSpec> {
    StateTransitionInfo::new(
        StateTransitionWitness {
            initial_state_root: Vec::default(),
            final_state_root: Vec::default(),
            da_block_header: MockBlockHeader {
                prev_hash: [0; 32].into(),
                hash: header_hash,
                height,
                time: Time::now(),
            },
            inclusion_proof: [0; 32],
            completeness_proof: (),
            blobs: vec![],
            witness: vec![],
        },
        height,
    )
}
