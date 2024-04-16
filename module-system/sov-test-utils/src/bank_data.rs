use std::rc::Rc;

use sov_bank::{get_token_id, Bank, CallMessage, Coins, TokenId};
use sov_modules_api::utils::generate_address;
use sov_modules_api::{CryptoSpec, PrivateKey as _, PublicKey, Spec};

use crate::{Message, MessageGenerator, TestHasher, TestSpec};
type PrivateKey<S> = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;

pub struct TransferData<S: Spec> {
    pub sender_pkey: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    pub receiver_address: S::Address,
    pub token_id: TokenId,
    pub transfer_amount: u64,
}

pub struct TokenCreateData<S: Spec> {
    pub token_name: String,
    pub salt: u64,
    pub initial_balance: u64,
    pub minter_address: S::Address,
    pub minter_pkey: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    pub authorized_minters: Vec<S::Address>,
}

impl<S: Spec> TokenCreateData<S> {
    fn get_token_id(&self) -> TokenId {
        get_token_id::<S>(&self.token_name, &self.minter_address, self.salt)
    }
}

pub struct BankMessageGenerator<S: Spec> {
    pub token_create_txs: Vec<TokenCreateData<S>>,
    pub transfer_txs: Vec<TransferData<S>>,
}

const DEFAULT_TOKEN_NAME: &str = "Token1";
const DEFAULT_SALT: u64 = 10;
const DEFAULT_INIT_BALANCE: u64 = 1000000;

pub fn get_default_token_id<S: Spec>(minter_address: &<S as Spec>::Address) -> TokenId {
    get_token_id::<S>(DEFAULT_TOKEN_NAME, minter_address, DEFAULT_SALT)
}

impl<S: Spec> BankMessageGenerator<S> {
    /// Gets the default sender address and private key.
    fn random_address_with_pkey() -> (
        <S as Spec>::Address,
        <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    ) {
        let pkey = <<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey>::generate();
        let address = pkey.to_address::<TestHasher, _>();
        (address, pkey)
    }

    /// Generates a random [`CallMessage::CreateToken`] transaction for default token parameters.
    pub fn random_create_token_generator() -> Self {
        let (minter_address, pk) = Self::random_address_with_pkey();
        Self::generate_create_token(
            DEFAULT_TOKEN_NAME.to_owned(),
            DEFAULT_SALT,
            pk.into(),
            vec![minter_address],
            DEFAULT_INIT_BALANCE,
        )
    }

    /// Create two message generators - one which creates a token, and one which generates random transfers for the token.
    /// The token generator is returned in the first position.
    pub fn generate_token_and_random_transfers(num_transfers: u64) -> (Self, Self) {
        let mut generator_with_token = Self::random_create_token_generator();
        let token_id = generator_with_token.token_create_txs[0].get_token_id();
        let priv_key: PrivateKey<S> =
            Rc::make_mut(&mut generator_with_token.token_create_txs[0].minter_pkey).clone();
        let transfer_generator = Self::generate_random_transfers(num_transfers, token_id, priv_key);

        (generator_with_token, transfer_generator)
    }

    /// Generates a [`CallMessage::CreateToken`] transaction.
    pub fn generate_create_token(
        token_name: String,
        salt: u64,
        minter_pkey: Rc<<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey>,
        authorized_minters: Vec<<S as Spec>::Address>,
        initial_balance: u64,
    ) -> Self {
        Self {
            token_create_txs: vec![TokenCreateData {
                token_name,
                salt,
                initial_balance,
                minter_address: minter_pkey.to_address::<TestHasher, _>(),
                minter_pkey,
                authorized_minters,
            }],
            transfer_txs: vec![],
        }
    }

    /// Generates random [`CallMessage::Transfer`] messages between the default sender and random receivers.
    pub fn generate_random_transfers(n: u64, token_id: TokenId, sender_pk: PrivateKey<S>) -> Self {
        let mut transfer_txs = vec![];
        for _ in 1..(n + 1) {
            let priv_key = PrivateKey::<S>::generate();
            let address: <S as Spec>::Address = priv_key.pub_key().to_address::<TestHasher, _>();

            transfer_txs.push(TransferData {
                sender_pkey: Rc::new(sender_pk.clone()),
                receiver_address: address,
                token_id,
                transfer_amount: 1,
            });
        }

        BankMessageGenerator {
            token_create_txs: vec![],
            transfer_txs,
        }
    }

    /// Generates [`CallMessage::CreateToken`] and single [`CallMessage::Transfer`] transactions.
    pub fn with_minter_and_transfer(
        minter_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    ) -> Self {
        let minter_address: <S as Spec>::Address = minter_key.to_address::<TestHasher, _>();
        let salt = DEFAULT_SALT;
        let token_name = DEFAULT_TOKEN_NAME.to_owned();
        let create_data = TokenCreateData {
            token_name: token_name.clone(),
            salt,
            initial_balance: 1000,
            minter_address: minter_address.clone(),
            minter_pkey: Rc::new(minter_key.clone()),
            authorized_minters: Vec::from([minter_address.clone()]),
        };
        Self {
            token_create_txs: Vec::from([create_data]),
            transfer_txs: Vec::from([TransferData {
                sender_pkey: Rc::new(minter_key),
                transfer_amount: 15,
                receiver_address: generate_address::<S>("just_receiver"),
                token_id: get_token_id::<S>(&token_name, &minter_address, salt),
            }]),
        }
    }

    /// Generates single [`CallMessage::CreateToken`] transaction with a specified minter.
    pub fn with_minter(minter_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey) -> Self {
        let minter_address: <S as Spec>::Address = minter_key.to_address::<TestHasher, _>();
        Self::generate_create_token(
            DEFAULT_TOKEN_NAME.to_owned(),
            DEFAULT_SALT,
            Rc::new(minter_key),
            vec![minter_address],
            DEFAULT_INIT_BALANCE,
        )
    }
}

impl BankMessageGenerator<TestSpec> {
    pub fn create_invalid_transfer(
        minter_key: <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    ) -> Self {
        let minter_address = minter_key.to_address::<TestHasher, _>();
        let salt = DEFAULT_SALT;
        let token_name = DEFAULT_TOKEN_NAME.to_owned();
        let token_create_data = TokenCreateData {
            token_name: token_name.clone(),
            salt,
            initial_balance: 1000,
            minter_address,
            minter_pkey: Rc::new(minter_key.clone()),
            authorized_minters: Vec::from([minter_address]),
        };
        Self {
            token_create_txs: Vec::from([token_create_data]),
            transfer_txs: Vec::from([
                TransferData {
                    sender_pkey: Rc::new(minter_key.clone()),
                    transfer_amount: 15,
                    receiver_address: generate_address::<TestSpec>("just_receiver"),
                    token_id: get_token_id::<TestSpec>(&token_name, &minter_address, salt),
                },
                TransferData {
                    sender_pkey: Rc::new(minter_key.clone()),
                    // invalid transfer because transfer_amount > minted supply
                    transfer_amount: 5000,
                    receiver_address: generate_address::<TestSpec>("just_receiver"),
                    token_id: get_token_id::<TestSpec>(&token_name, &minter_address, salt),
                },
            ]),
        }
    }
}

pub(crate) fn create_token_tx<S: Spec>(input: &TokenCreateData<S>) -> CallMessage<S> {
    CallMessage::CreateToken {
        salt: input.salt,
        token_name: input.token_name.clone(),
        initial_balance: input.initial_balance,
        minter_address: input.minter_address.clone(),
        authorized_minters: input.authorized_minters.clone(),
    }
}

pub(crate) fn transfer_token_tx<S: Spec>(transfer_data: &TransferData<S>) -> CallMessage<S> {
    CallMessage::Transfer {
        to: transfer_data.receiver_address.clone(),
        coins: Coins {
            amount: transfer_data.transfer_amount,
            token_id: transfer_data.token_id,
        },
    }
}

impl<S: Spec> MessageGenerator for BankMessageGenerator<S> {
    type Module = Bank<S>;
    type Spec = S;

    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        let mut messages = Vec::<Message<S, Bank<S>>>::new();

        let mut nonce = 0;

        for create_message in &self.token_create_txs {
            let gas_limit = None;
            messages.push(Message::new(
                create_message.minter_pkey.clone(),
                create_token_tx::<S>(create_message),
                Self::DEFAULT_CHAIN_ID,
                Self::DEFAULT_MAX_PRIORITY_FEE,
                Self::DEFAULT_MAX_FEE,
                gas_limit,
                nonce,
            ));
            nonce += 1;
        }

        for transfer_message in &self.transfer_txs {
            let gas_limit = None;
            messages.push(Message::new(
                transfer_message.sender_pkey.clone(),
                transfer_token_tx::<S>(transfer_message),
                Self::DEFAULT_CHAIN_ID,
                Self::DEFAULT_MAX_PRIORITY_FEE,
                Self::DEFAULT_MAX_FEE,
                gas_limit,
                nonce,
            ));
            nonce += 1;
        }

        messages
    }
}
