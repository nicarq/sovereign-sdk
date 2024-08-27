use rand::Rng;
use sov_bank::{Bank, Coins, TokenId};
use sov_modules_api::capabilities::Authenticator;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{DaSpec, EncodeCall, RawTx, Spec};
use sov_modules_stf_blueprint::Runtime;

use crate::module_message_generators::get_prepared_call_message;
use crate::{
    get_gas_funding_txs, AccountPool, GasFundingConfig, PreparedCallMessage,
    SerializedPreparedCallMessage,
};

/// The [`TokenTransferMessageGenerator`] structure holds all that is required to prepare
/// [`Bank`] module token-creation call messages, that are sign- and broadcast-able by accounts
/// from the [`AccountPool`].
#[derive(Clone)]
pub struct TokenTransferMessageGenerator<S: Spec> {
    message_count: u64,
    account_pool: AccountPool<S>,
    account_pool_index: u64,
    token_id: TokenId,
}

impl<S: Spec> TokenTransferMessageGenerator<S> {
    /// Creates a [`TokenTransferMessageGenerator`] with an [`AccountPool`] capable of signing
    /// token-transfer module messages for the given [`TokenId`].
    pub fn new_from_account_pool(
        account_pool: AccountPool<S>,
        token_id: TokenId,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            message_count: 0,
            account_pool_index: 0,
            account_pool,
            token_id,
        })
    }
}

impl<S: Spec> Iterator for TokenTransferMessageGenerator<S> {
    type Item = PreparedCallMessage<S, Bank<S>>;

    fn next(&mut self) -> Option<Self::Item> {
        let account_pool_index = self.account_pool_index;
        let account = self
            .account_pool
            .get_by_index(&account_pool_index)
            .expect("could not get account from account pool at index: {index}");
        let address = account.address().clone();

        let mut rng = rand::thread_rng();
        // TODO @gskapka have user define the min and max? Also keep ref to rng around for more efficiency.
        let amount = rng.gen_range(1..10_000);

        let prepared_call_message = get_prepared_call_message(
            get_token_transfer_call_message(address, amount, self.token_id)
                .expect("could not get token transfer call message"),
            account_pool_index,
            None,
        );

        // NOTE So that we iterate through the account pool when sending the messages.
        self.message_count += 1;

        // NOTE: So that we iterate over the account pool indefinitely.
        if self.account_pool.len() as u64 >= self.account_pool_index {
            self.account_pool_index = 0;
        } else {
            self.account_pool_index += 1;
        };

        Some(prepared_call_message)
    }
}

impl<S: Spec> TokenTransferMessageGenerator<S> {
    /// Get an iterator which yields valid signed transactions that can be
    /// sent to a sequencer or formed into a blob to send direct to the DA layer.
    /// Example:
    ///
    /// ```
    /// use std::str::FromStr;
    ///
    /// use sov_bank::Bank;
    /// use sov_bank::TokenId;
    /// use sov_modules_api::DaSpec;
    /// use sov_modules_api::PrivateKey;
    /// use sov_modules_api::EncodeCall;
    /// use sov_modules_stf_blueprint::Runtime;
    /// use sov_test_harness::AccountPoolConfig;
    /// use sov_modules_api::{Spec, CryptoSpec};
    /// use sov_modules_api::capabilities::Authenticator;
    /// use sov_modules_api::transaction::PriorityFeeBips;
    /// use sov_test_harness::{TokenTransferMessageGenerator, AccountPool, GasFundingConfig};
    ///
    /// async fn foo<R, S, Da, Auth>() -> anyhow::Result<()>
    /// where
    ///     R: Runtime<S, Da> + EncodeCall<Bank<S>>,
    ///     Da: DaSpec,
    ///     Auth: Authenticator,
    ///     S: Spec,
    /// {
    ///     // First you need an account pool...
    ///     let num_accounts_to_generate = 10;
    ///     let rpc_url = "your rollup rpc url".to_string();
    ///     let genesis_dir = "path to genesis file".to_string();
    ///     let private_keys_dir = "path to test-net private keys".to_string();
    ///     let gas_funding_config = GasFundingConfig::new(genesis_dir, rpc_url.clone());
    ///     let gas_token_authorized_minters = vec![/* NOTE: At least one must exist!*/];
    ///
    ///     let account_pool_config = AccountPoolConfig::new(
    ///         private_keys_dir,
    ///         rpc_url,
    ///         num_accounts_to_generate,
    ///         gas_token_authorized_minters,
    ///     );
    ///     let account_pool = AccountPool::<S>::new_from_config(account_pool_config).await.unwrap();
    ///
    ///     /// Now you can create the message generator...
    ///     let chain_id = 1337;
    ///     let token_id = TokenId::from_str("my token id").unwrap();
    ///     let max_priority_fee_bips = PriorityFeeBips::from_percentage(1);
    ///     let message_generator = TokenTransferMessageGenerator::new_from_account_pool(
    ///         account_pool,
    ///         token_id
    ///     ).unwrap();
    ///     
    ///     // Now you can get the signed version of the iterator. Note the gas funding configuration.
    ///     // Adding this means the iterator will begin with set of signed transactions that mint
    ///     // and distribute the gas token of the rollup to all the accounts in the pool that need it.
    ///     let signed_message_iterator = message_generator.into_signed_tx_iter::<R, Da, Auth>(
    ///         chain_id,
    ///         max_priority_fee_bips,
    ///         Some(gas_funding_config),
    ///     ).await?;
    ///
    ///     /// And finally, you have an iterator of valid signed transactions...
    ///     for signed_message in signed_message_iterator {
    ///         // To do with whatever you please...
    ///     }
    ///     # Ok(())
    /// }
    /// ```
    pub async fn into_signed_tx_iter<R, Da, Auth>(
        self,
        chain_id: u64,
        max_priority_fee_bips: PriorityFeeBips,
        maybe_gas_funding_config: Option<GasFundingConfig>,
    ) -> anyhow::Result<Box<impl Iterator<Item = RawTx>>>
    where
        R: Runtime<S, Da> + EncodeCall<Bank<S>>,
        Da: DaSpec,
        Auth: Authenticator,
    {
        // NOTE: We need to clone the account pool here since the `Iterator` impl consumes self. Whilst not ideal,
        // this method also consumes self rending the account pool unusable elsewhere anyway.
        let cloned_account_pool = self.account_pool.clone();

        let gas_funding_txs = if let Some(gas_funding_config) = maybe_gas_funding_config {
            get_gas_funding_txs(gas_funding_config, &self.account_pool).await?
        } else {
            vec![]
        };

        let prepared_txs_iter = gas_funding_txs.into_iter().chain(self.into_iter());

        let signed_txs_iter = prepared_txs_iter
            .map(|prepared_call_message| SerializedPreparedCallMessage {
                max_fee: *prepared_call_message.max_fee(),
                account_pool_index: *prepared_call_message.account_pool_index(),
                call_message: <R as EncodeCall<Bank<S>>>::encode_call(
                    prepared_call_message.call_message,
                ),
            })
            .map(move |serialized_call_message| {
                let (serialized_message, account_pool_index, max_fee) =
                    serialized_call_message.dissolve();

                let account = cloned_account_pool
                    .get_by_index(&account_pool_index)
                    .expect("no account @ index {account_pool_index}");
                let nonce = account.nonce().load(std::sync::atomic::Ordering::Relaxed);

                let unsigned_tx = UnsignedTransaction::<S>::new(
                    serialized_message,
                    chain_id,
                    max_priority_fee_bips,
                    max_fee,
                    nonce,
                    None,
                );

                let signed_tx = Transaction::<S>::new_signed_tx(account.private_key(), unsigned_tx);

                cloned_account_pool.inc_nonce(&account_pool_index);

                Auth::encode(
                    borsh::to_vec(&signed_tx)
                        .expect("could not borsh serialized signed transaction!"),
                )
                .expect("could not `Auth::encode` serialized transaction")
            });

        Ok(Box::new(signed_txs_iter))
    }
}

fn get_token_transfer_call_message<S: Spec>(
    to: S::Address,
    amount: u64,
    token_id: TokenId,
) -> anyhow::Result<sov_bank::CallMessage<S>> {
    Ok(sov_bank::CallMessage::Transfer {
        to,
        coins: Coins { amount, token_id },
    })
}
