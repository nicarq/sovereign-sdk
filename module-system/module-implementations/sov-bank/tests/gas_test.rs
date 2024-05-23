use sov_bank::{Bank, BankConfig, BankGasConfig, CallMessage, GasTokenConfig, GAS_TOKEN_ID};
use sov_modules_api::transaction::{AuthenticatedTransactionData, PriorityFeeBips};
use sov_modules_api::utils::generate_address;
use sov_modules_api::{Context, CredentialId, Gas, GasArray, GasPrice, Module, Spec, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use tempfile::TempDir;

mod helpers;

const CREATE_TOKEN_NATIVE_COST: u64 = 2;
const CREATE_TOKEN_ZK_COST: u64 = 3;

type S = sov_test_utils::TestSpec;
#[test]
fn zeroed_price_wont_deduct_working_set() {
    let sender_balance = 100;
    let remaining_funds = BankGasTestCase::init(sender_balance, GasPrice::from_slice(&[0, 0]))
        .execute()
        .unwrap();

    assert_eq!(
        remaining_funds, sender_balance,
        "the balance should be unchanged with zeroed price"
    );
}

#[test]
fn normal_price_will_deduct_working_set() {
    let sender_balance = 100;

    let native_price = 2;
    let zk_price = 3;
    let remaining_funds = BankGasTestCase::init(
        sender_balance,
        GasPrice::from_slice(&[native_price, zk_price]),
    )
    .override_gas_config()
    .execute()
    .unwrap();

    // compute the expected gas cost, based on the test constants
    let gas_used = native_price * CREATE_TOKEN_NATIVE_COST + zk_price * CREATE_TOKEN_ZK_COST;

    assert_eq!(
        remaining_funds,
        sender_balance - gas_used,
        "the sender balance is enough for this call"
    );
}

#[test]
fn constants_price_is_charged_correctly() {
    let sender_balance = 100;

    let remaining_funds = BankGasTestCase::init(sender_balance, GasPrice::from_slice(&[2, 3]))
        .execute()
        .unwrap();

    // compute the expected gas cost, based on the json constants
    let bank = Bank::<S>::default();
    let config = bank.gas_config();
    let gas_price = <<S as Spec>::Gas as Gas>::Price::from_slice(&[2, 3]);
    let gas_used = config.create_token.value(&gas_price);

    assert_eq!(
        remaining_funds,
        sender_balance - gas_used,
        "the sender balance is enough for this call"
    );
}

#[test]
fn not_enough_gas_wont_panic() {
    let sender_balance = 100;

    let result = BankGasTestCase::init(sender_balance, GasPrice::from_slice(&[2000, 3000]))
        .override_gas_config()
        .execute();

    assert!(
        result.is_err(),
        "the sender balance is not enough for this call"
    );
}

#[test]
fn very_high_gas_price_wont_panic_or_overflow() {
    let sender_balance = 100;

    let result = BankGasTestCase::init(sender_balance, GasPrice::from_slice(&[u64::MAX; 2]))
        .override_gas_config()
        .execute();

    assert!(result.is_err(), "arithmetic overflow shoulnd't panic");
}

#[allow(dead_code)]
pub struct BankGasTestCase {
    ws: WorkingSet<S>,
    bank: Bank<S>,
    ctx: Context<S>,
    message: CallMessage<S>,
    tmpdir: TempDir,
}

impl BankGasTestCase {
    pub fn init(sender_balance: u64, gas_price: <<S as Spec>::Gas as Gas>::Price) -> Self {
        let tmpdir = tempfile::tempdir().unwrap();

        // create a base token with an initial balance to pay for the gas
        let base_token_name = "sov-gas-token";
        let salt = 0;

        // sanity check the token ID
        let base_token_id = GAS_TOKEN_ID;

        // generate a token configuration with the provided arguments
        let sender_address = generate_address::<S>("sender");
        let address_and_balances = vec![(sender_address, sender_balance)];
        let bank_config: BankConfig<S> = BankConfig {
            gas_token_config: GasTokenConfig {
                token_name: base_token_name.to_string(),
                address_and_balances,
                authorized_minters: vec![],
            },
            tokens: vec![],
        };

        // create a context using the generated account as sender
        let height = 1;
        let minter_address = generate_address::<S>("minter");
        let sequencer_address = generate_address::<S>("sequencer");
        let ctx = Context::<S>::new(
            sender_address,
            Default::default(),
            sequencer_address,
            height,
        );

        // create a bank instance
        let bank = Bank::default();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let mut ws = WorkingSet::new(storage);
        bank.genesis(&bank_config, &mut ws).unwrap();

        // sanity test the sender balance
        let balance = bank.get_balance_of(&sender_address, base_token_id, &mut ws);
        assert_eq!(balance, Some(sender_balance));

        let checkpoint = ws.checkpoint().0;

        // generate a create dummy token message
        let token_name = "dummy".to_string();
        let initial_balance = 500;

        let message = CallMessage::CreateToken::<S> {
            salt,
            token_name,
            initial_balance,
            minter_address,
            authorized_minters: vec![minter_address],
        };

        let tx: AuthenticatedTransactionData<S> = AuthenticatedTransactionData {
            credentials: Default::default(),
            max_fee: sender_balance,
            credential_id: CredentialId([0; 32]),
            default_address: Some(sender_address),
            chain_id: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
            gas_limit: None,
            nonce: 0,
        };

        let ws = checkpoint.to_working_set(&tx, &gas_price);

        Self {
            ws,
            bank,
            ctx,
            message,
            tmpdir,
        }
    }

    pub fn override_gas_config(mut self) -> Self {
        self.bank.override_gas_config(BankGasConfig {
            create_token: [CREATE_TOKEN_NATIVE_COST, CREATE_TOKEN_ZK_COST].into(),
            transfer: Gas::zero(),
            burn: Gas::zero(),
            mint: Gas::zero(),
            freeze: Gas::zero(),
        });
        self
    }

    pub fn execute(self) -> anyhow::Result<u64> {
        let Self {
            mut ws,
            bank,
            ctx,
            message,
            tmpdir,
        } = self;

        bank.call(message, &ctx, &mut ws)?;

        // can unlock storage dir
        let _ = tmpdir;

        Ok(ws.gas_remaining_funds())
    }
}
