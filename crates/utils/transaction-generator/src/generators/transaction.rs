use std::sync::Arc;

use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::transaction::TxDetails;
use sov_modules_api::{BlobDataWithId, CryptoSpec, DispatchCall, Gas, Runtime, Spec};
use sov_modules_stf_blueprint::get_gas_used;
use sov_state::{DefaultStorageSpec, ProverStorage};
use sov_test_utils::runtime::traits::MinimalGenesis;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    TransactionAssertContext, TransactionType, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
};

use super::basic::{BasicChangeLogEntry, BasicModuleRef, BasicTag};
use crate::{Distribution, GeneratedMessage, MessageValidity, State};

pub trait PrepareEnv {
    type Input;

    fn prepare_env(&mut self, input: &mut Self::Input);
}

pub trait AssertOutcome {
    type Output;

    fn assert_outcome(&self, output: &Self::Output);
}

pub trait GeneratedTransaction
where
    Self: Sized + PrepareEnv + AssertOutcome,
{
    type Transaction;

    type Context<'a>;

    fn new(context: &mut Self::Context<'_>) -> Self;

    fn transaction(&self) -> Self::Transaction;
}

#[derive(Debug)]
pub enum MaxFeeOutcome {
    Insufficient,
    Exact,
    Excess,
}

#[derive(Debug)]
pub enum TransactionOutcome {
    MaxFee(MaxFeeOutcome),
}

type DefaultSpecWithHasher<S> = DefaultStorageSpec<<<S as Spec>::CryptoSpec as CryptoSpec>::Hasher>;

#[derive(Clone)]
pub struct SovereignGeneratedTransaction<
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>, Da = MockDaSpec>,
    RT: Runtime<S, BlobType = BlobDataWithId> + MinimalGenesis<S> + DispatchCall,
> {
    pub tx: TransactionType<RT, S>,
    pub outcome: Arc<TransactionOutcome>,
    pub msg: GeneratedMessage<S, <RT as DispatchCall>::Decodable, BasicChangeLogEntry<S>>,
}

impl<S, RT> PrepareEnv for SovereignGeneratedTransaction<S, RT>
where
    RT: Runtime<S, BlobType = BlobDataWithId> + MinimalGenesis<S> + DispatchCall,
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>, Da = MockDaSpec>,
{
    type Input = TestRunner<RT, S>;

    fn prepare_env(&mut self, input: &mut Self::Input) {
        let (simulated, _) = input.simulate(self.tx.clone());
        let batch_receipt = simulated.batch_receipts[0].clone();
        let tx_receipt = &simulated.batch_receipts[0].tx_receipts[0].clone();
        let gas_used = get_gas_used(tx_receipt);
        let gas_price = batch_receipt.inner.gas_price.clone();
        let gas_used = gas_used.value(&gas_price);

        let max_fee = match &*self.outcome {
            TransactionOutcome::MaxFee(gas_outcome) => match gas_outcome {
                MaxFeeOutcome::Insufficient => gas_used - 2000,
                MaxFeeOutcome::Exact => gas_used,
                MaxFeeOutcome::Excess => gas_used + 2000,
            },
        };

        self.tx = self.transaction().with_max_fee(max_fee);
    }
}

impl<S, RT> AssertOutcome for SovereignGeneratedTransaction<S, RT>
where
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>, Da = MockDaSpec>,
    RT: Runtime<S, BlobType = BlobDataWithId> + MinimalGenesis<S> + DispatchCall,
{
    type Output = TransactionAssertContext<S, RT>;

    fn assert_outcome<'a>(&self, output: &Self::Output) {
        match &*self.outcome {
            TransactionOutcome::MaxFee(max_fee_outcome) => match max_fee_outcome {
                MaxFeeOutcome::Insufficient => assert!(
                    !output.tx_receipt.is_successful(),
                    "Insufficient expected reverted receipt, found {:?}",
                    output.tx_receipt
                ),
                MaxFeeOutcome::Exact => assert!(
                    output.tx_receipt.is_successful(),
                    "Exact expected successful receipt, found {:?}",
                    output.tx_receipt
                ),
                MaxFeeOutcome::Excess => assert!(
                    output.tx_receipt.is_successful(),
                    "Excess expected successful receipt, found {:?}",
                    output.tx_receipt
                ),
            },
        };
    }
}

pub struct SovereignContext<'a, S: Spec, RT: DispatchCall + Runtime<S>> {
    pub modules: Distribution<BasicModuleRef<S, RT>>,
    pub u: &'a mut arbitrary::Unstructured<'a>,
    pub call_generator_state: &'a mut State<S, BasicTag, ()>,
    pub outcomes: Distribution<Arc<TransactionOutcome>>,
}

impl<S, RT> GeneratedTransaction for SovereignGeneratedTransaction<S, RT>
where
    RT: Runtime<S, BlobType = BlobDataWithId> + MinimalGenesis<S> + DispatchCall,
    S: Spec<Storage = ProverStorage<DefaultSpecWithHasher<S>>, Da = MockDaSpec>,
{
    type Transaction = TransactionType<RT, S>;

    type Context<'a> = SovereignContext<'a, S, RT>;

    fn new(context: &mut Self::Context<'_>) -> Self {
        let module = context.modules.select_value(context.u).unwrap();
        let msg = module
            .generate_call_message(
                context.u,
                context.call_generator_state,
                // we always want these messages to be valid
                // we want the outcome to be based on transaction level attributes
                // i.e gas, nonces, etc
                MessageValidity::Valid,
            )
            .unwrap();
        let outcome = context.outcomes.select_value(context.u).unwrap();
        let tx = TransactionType::<RT, S>::Plain {
            message: msg.message.clone(),
            key: msg.sender.clone(),
            details: TxDetails {
                max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
                max_fee: TEST_DEFAULT_MAX_FEE,
                gas_limit: None,
                chain_id: config_chain_id(),
            },
        };

        Self {
            tx,
            outcome: outcome.clone(),
            msg,
        }
    }

    fn transaction(&self) -> Self::Transaction {
        self.tx.clone()
    }
}
