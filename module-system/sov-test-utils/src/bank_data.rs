use std::rc::Rc;

use sov_bank::{get_token_address, Bank, CallMessage, Coins};
use sov_modules_api::transaction::Transaction;
use sov_modules_api::utils::generate_address;
use sov_modules_api::{
    CryptoSpec, EncodeCall, Gas, GasPrice, Module, PrivateKey as _, PublicKey, Spec,
};

use crate::{Message, MessageGenerator, TestPrivateKey, TestSpec};
type PrivateKey<S> = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;

pub struct TransferData<S: Spec> {
    pub sender_pkey: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    pub receiver_address: S::Address,
    pub token_address: S::Address,
    pub transfer_amount: u64,
}

pub struct MintData<S: Spec> {
    pub token_name: String,
    pub salt: u64,
    pub initial_balance: u64,
    pub minter_address: S::Address,
    pub minter_pkey: Rc<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    pub authorized_minters: Vec<S::Address>,
}

impl<S: Spec> MintData<S> {
    fn get_token_address(&self) -> <S as Spec>::Address {
        get_token_address::<S>(&self.token_name, &self.minter_address, self.salt)
    }
}

pub struct BankMessageGenerator<S: Spec> {
    pub token_mint_txs: Vec<MintData<S>>,
    pub transfer_txs: Vec<TransferData<S>>,
}

const DEFAULT_TOKEN_NAME: &str = "Token1";
const DEFAULT_SALT: u64 = 10;
const DEFAULT_PVT_KEY: &str = "236e80cb222c4ed0431b093b3ac53e6aa7a2273fe1f4351cd354989a823432a27b758bf2e7670fafaf6bf0015ce0ff5aa802306fc7e3f45762853ffc37180fe6";
const DEFAULT_CHAIN_ID: u64 = 0;
const DEFAULT_GAS_TIP: u64 = 0;
const DEFAULT_GAS_LIMIT: u64 = 0;
const DEFAULT_MAX_GAS_PRICE: Option<GasPrice<2>> = None;
const DEFAULT_INIT_BALANCE: u64 = 1000000;

pub fn get_default_token_address() -> <TestSpec as Spec>::Address {
    let minter_key = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
    let minter_address = minter_key.default_address();
    let salt = DEFAULT_SALT;
    let token_name = DEFAULT_TOKEN_NAME.to_owned();
    get_token_address::<TestSpec>(&token_name, &minter_address, salt)
}

pub fn get_default_private_key() -> TestPrivateKey {
    TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap()
}

impl Default for BankMessageGenerator<TestSpec> {
    fn default() -> Self {
        let minter_key = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
        let minter_address = minter_key.default_address();
        let salt = DEFAULT_SALT;
        let token_name = DEFAULT_TOKEN_NAME.to_owned();
        let mint_data = MintData {
            token_name: token_name.clone(),
            salt,
            initial_balance: 1000,
            minter_address,
            minter_pkey: Rc::new(minter_key.clone()),
            authorized_minters: Vec::from([minter_address]),
        };
        Self {
            token_mint_txs: Vec::from([mint_data]),
            transfer_txs: Vec::from([TransferData {
                sender_pkey: Rc::new(minter_key),
                transfer_amount: 15,
                receiver_address: generate_address::<TestSpec>("just_receiver"),
                token_address: get_token_address::<TestSpec>(&token_name, &minter_address, salt),
            }]),
        }
    }
}

impl<S: Spec> BankMessageGenerator<S> {
    /// Gets the default sender address and private key.
    fn random_address_with_pkey() -> (
        <S as Spec>::Address,
        <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    ) {
        let pkey = <<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey>::generate();
        let address = pkey.to_address();
        (address, pkey)
    }

    /// Generates a random create token transaction for default token parameters.
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
        let token_address = generator_with_token.token_mint_txs[0].get_token_address();
        let priv_key: PrivateKey<S> =
            Rc::make_mut(&mut generator_with_token.token_mint_txs[0].minter_pkey).clone();
        let transfer_generator =
            Self::generate_random_transfers(num_transfers, token_address, priv_key);

        (generator_with_token, transfer_generator)
    }

    /// Generates a create token transaction.
    pub fn generate_create_token(
        token_name: String,
        salt: u64,
        minter_pkey: std::rc::Rc<<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey>,
        authorized_minters: Vec<<S as Spec>::Address>,
        initial_balance: u64,
    ) -> Self {
        Self {
            token_mint_txs: vec![MintData {
                token_name,
                salt,
                initial_balance,
                minter_address: minter_pkey.to_address(),
                minter_pkey,
                authorized_minters,
            }],
            transfer_txs: vec![],
        }
    }

    /// Generates random transfers between the default sender and random receivers.
    pub fn generate_random_transfers(
        n: u64,
        token_address: <S as Spec>::Address,
        sender_pk: PrivateKey<S>,
    ) -> Self {
        let mut transfer_txs = vec![];
        for _ in 1..(n + 1) {
            let priv_key = PrivateKey::<S>::generate();
            let address: <S as Spec>::Address = priv_key.pub_key().to_address();

            transfer_txs.push(TransferData {
                sender_pkey: Rc::new(sender_pk.clone()),
                receiver_address: address,
                token_address: token_address.clone(),
                transfer_amount: 1,
            });
        }

        BankMessageGenerator {
            token_mint_txs: vec![],
            transfer_txs,
        }
    }
}

impl BankMessageGenerator<TestSpec> {
    /// Gets the default sender address and private key.
    fn default_address_with_pkey() -> (<TestSpec as Spec>::Address, TestPrivateKey) {
        let pkey = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
        let address = pkey.default_address();
        (address, pkey)
    }

    /// Generates random transfers between the default sender and random receivers for default token parameters.
    pub fn default_generate_random_transfers(n: u64) -> Self {
        let priv_key = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
        let token_address =
            get_token_address::<TestSpec>(DEFAULT_TOKEN_NAME, &priv_key.to_address(), DEFAULT_SALT);
        Self::generate_random_transfers(n, token_address, priv_key)
    }

    /// Generates a create token transaction for default token parameters.
    pub fn default_generate_create_token() -> Self {
        let (minter_address, pk) = Self::default_address_with_pkey();
        Self::generate_create_token(
            DEFAULT_TOKEN_NAME.to_owned(),
            DEFAULT_SALT,
            pk.into(),
            vec![minter_address],
            DEFAULT_INIT_BALANCE,
        )
    }

    pub fn create_invalid_transfer() -> Self {
        let minter_key = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
        let minter_address = minter_key.default_address();
        let salt = DEFAULT_SALT;
        let token_name = DEFAULT_TOKEN_NAME.to_owned();
        let mint_data = MintData {
            token_name: token_name.clone(),
            salt,
            initial_balance: 1000,
            minter_address,
            minter_pkey: Rc::new(minter_key),
            authorized_minters: Vec::from([minter_address]),
        };
        Self {
            token_mint_txs: Vec::from([mint_data]),
            transfer_txs: Vec::from([
                TransferData {
                    sender_pkey: Rc::new(TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap()),
                    transfer_amount: 15,
                    receiver_address: generate_address::<TestSpec>("just_receiver"),
                    token_address: get_token_address::<TestSpec>(
                        &token_name,
                        &minter_address,
                        salt,
                    ),
                },
                TransferData {
                    sender_pkey: Rc::new(TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap()),
                    // invalid transfer because transfer_amount > minted supply
                    transfer_amount: 5000,
                    receiver_address: generate_address::<TestSpec>("just_receiver"),
                    token_address: get_token_address::<TestSpec>(
                        &token_name,
                        &minter_address,
                        salt,
                    ),
                },
            ]),
        }
    }
}

pub(crate) fn mint_token_tx<S: Spec>(mint_data: &MintData<S>) -> CallMessage<S> {
    CallMessage::CreateToken {
        salt: mint_data.salt,
        token_name: mint_data.token_name.clone(),
        initial_balance: mint_data.initial_balance,
        minter_address: mint_data.minter_address.clone(),
        authorized_minters: mint_data.authorized_minters.clone(),
    }
}

pub(crate) fn transfer_token_tx<S: Spec>(transfer_data: &TransferData<S>) -> CallMessage<S> {
    CallMessage::Transfer {
        to: transfer_data.receiver_address.clone(),
        coins: Coins {
            amount: transfer_data.transfer_amount,
            token_address: transfer_data.token_address.clone(),
        },
    }
}

impl<S: Spec> MessageGenerator for BankMessageGenerator<S> {
    type Module = Bank<S>;
    type Spec = S;

    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        let mut messages = Vec::<Message<S, Bank<S>>>::new();

        let mut nonce = 0;

        for mint_message in &self.token_mint_txs {
            let max_gas_price = None;
            messages.push(Message::new(
                mint_message.minter_pkey.clone(),
                mint_token_tx::<S>(mint_message),
                DEFAULT_CHAIN_ID,
                DEFAULT_GAS_TIP,
                DEFAULT_GAS_LIMIT,
                max_gas_price,
                nonce,
            ));
            nonce += 1;
        }

        for transfer_message in &self.transfer_txs {
            let max_gas_price = None;
            messages.push(Message::new(
                transfer_message.sender_pkey.clone(),
                transfer_token_tx::<S>(transfer_message),
                DEFAULT_CHAIN_ID,
                DEFAULT_GAS_TIP,
                DEFAULT_GAS_LIMIT,
                max_gas_price,
                nonce,
            ));
            nonce += 1;
        }

        messages
    }

    fn create_tx<Encoder: EncodeCall<Self::Module>>(
        &self,
        sender: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
        message: <Self::Module as Module>::CallMessage,
        chain_id: u64,
        gas_tip: u64,
        gas_limit: u64,
        max_gas_price: Option<<S::Gas as Gas>::Price>,
        nonce: u64,
        _is_last: bool,
    ) -> sov_modules_api::transaction::Transaction<S> {
        let message = Encoder::encode_call(message);
        Transaction::<S>::new_signed_tx(
            sender,
            message,
            chain_id,
            gas_tip,
            gas_limit,
            max_gas_price,
            nonce,
        )
    }
}

pub struct BadSerializationBankCallMessages;

impl BadSerializationBankCallMessages {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for BadSerializationBankCallMessages {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageGenerator for BadSerializationBankCallMessages {
    type Module = Bank<Self::Spec>;
    type Spec = TestSpec;

    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        let mut messages = Vec::<Message<Self::Spec, Bank<Self::Spec>>>::new();
        let minter_key = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
        let minter_address = minter_key.default_address();
        let salt = DEFAULT_SALT;
        let token_name = DEFAULT_TOKEN_NAME.to_owned();
        messages.push(Message::new(
            Rc::new(TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap()),
            CallMessage::CreateToken {
                salt,
                token_name,
                initial_balance: 1000,
                minter_address,
                authorized_minters: Vec::from([minter_address]),
            },
            DEFAULT_CHAIN_ID,
            DEFAULT_GAS_TIP,
            DEFAULT_GAS_LIMIT,
            DEFAULT_MAX_GAS_PRICE,
            0,
        ));
        messages.push(Message::new(
            Rc::new(TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap()),
            CallMessage::Transfer {
                to: generate_address::<Self::Spec>("just_receiver"),
                coins: Coins {
                    amount: 50,
                    token_address: get_default_token_address(),
                },
            },
            DEFAULT_CHAIN_ID,
            DEFAULT_GAS_TIP,
            DEFAULT_GAS_LIMIT,
            DEFAULT_MAX_GAS_PRICE,
            0,
        ));
        messages
    }

    fn create_tx<Encoder: EncodeCall<Self::Module>>(
        &self,
        sender: &TestPrivateKey,
        message: <Bank<Self::Spec> as Module>::CallMessage,
        chain_id: u64,
        gas_tip: u64,
        gas_limit: u64,
        max_gas_price: Option<<<Self::Spec as Spec>::Gas as Gas>::Price>,
        nonce: u64,
        is_last: bool,
    ) -> Transaction<Self::Spec> {
        // just some random bytes that won't deserialize to a valid txn
        let call_data = if is_last {
            vec![1, 2, 3]
        } else {
            Encoder::encode_call(message)
        };

        Transaction::<Self::Spec>::new_signed_tx(
            sender,
            call_data,
            chain_id,
            gas_tip,
            gas_limit,
            max_gas_price,
            nonce,
        )
    }
}

pub struct BadSignatureBankCallMessages;

impl BadSignatureBankCallMessages {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for BadSignatureBankCallMessages {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageGenerator for BadSignatureBankCallMessages {
    type Spec = TestSpec;
    type Module = Bank<Self::Spec>;

    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        let mut messages = Vec::<Message<Self::Spec, Bank<Self::Spec>>>::new();
        let minter_key = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
        let minter_address = minter_key.default_address();
        let salt = DEFAULT_SALT;
        let token_name = DEFAULT_TOKEN_NAME.to_owned();
        messages.push(Message::new(
            Rc::new(TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap()),
            CallMessage::CreateToken {
                salt,
                token_name,
                initial_balance: 1000,
                minter_address,
                authorized_minters: Vec::from([minter_address]),
            },
            DEFAULT_CHAIN_ID,
            DEFAULT_GAS_TIP,
            DEFAULT_GAS_LIMIT,
            DEFAULT_MAX_GAS_PRICE,
            0,
        ));
        messages
    }

    fn create_tx<Encoder: EncodeCall<Self::Module>>(
        &self,
        sender: &TestPrivateKey,
        message: <Bank<Self::Spec> as Module>::CallMessage,
        chain_id: u64,
        gas_tip: u64,
        gas_limit: u64,
        max_gas_price: Option<<<Self::Spec as Spec>::Gas as Gas>::Price>,
        nonce: u64,
        is_last: bool,
    ) -> Transaction<Self::Spec> {
        let call_data = Encoder::encode_call(message);

        if is_last {
            let tx = Transaction::<Self::Spec>::new_signed_tx(
                sender,
                call_data.clone(),
                chain_id,
                gas_tip,
                gas_limit,
                max_gas_price.clone(),
                nonce,
            );
            Transaction::new(
                TestPrivateKey::generate().pub_key(),
                call_data,
                tx.signature().clone(),
                chain_id,
                gas_tip,
                gas_limit,
                max_gas_price,
                nonce,
            )
        } else {
            Transaction::<Self::Spec>::new_signed_tx(
                sender,
                call_data,
                chain_id,
                gas_tip,
                gas_limit,
                max_gas_price,
                nonce,
            )
        }
    }
}

pub struct BadNonceBankCallMessages;

impl BadNonceBankCallMessages {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for BadNonceBankCallMessages {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageGenerator for BadNonceBankCallMessages {
    type Module = Bank<Self::Spec>;
    type Spec = TestSpec;

    fn create_messages(&self) -> Vec<Message<Self::Spec, Self::Module>> {
        let mut messages = Vec::<Message<Self::Spec, Bank<Self::Spec>>>::new();
        let minter_key = TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap();
        let minter_address = minter_key.default_address();
        let salt = DEFAULT_SALT;
        let token_name = DEFAULT_TOKEN_NAME.to_owned();
        messages.push(Message::new(
            Rc::new(TestPrivateKey::from_hex(DEFAULT_PVT_KEY).unwrap()),
            CallMessage::CreateToken {
                salt,
                token_name,
                initial_balance: 1000,
                minter_address,
                authorized_minters: Vec::from([minter_address]),
            },
            DEFAULT_CHAIN_ID,
            DEFAULT_GAS_TIP,
            DEFAULT_GAS_LIMIT,
            DEFAULT_MAX_GAS_PRICE,
            0,
        ));
        messages
    }

    fn create_tx<Encoder: EncodeCall<Self::Module>>(
        &self,
        sender: &TestPrivateKey,
        message: <Bank<Self::Spec> as Module>::CallMessage,
        chain_id: u64,
        gas_tip: u64,
        gas_limit: u64,
        max_gas_price: Option<<<Self::Spec as Spec>::Gas as Gas>::Price>,
        _nonce: u64,
        _is_last: bool,
    ) -> Transaction<Self::Spec> {
        let message = Encoder::encode_call(message);
        // hard-coding the nonce to 1000
        Transaction::<Self::Spec>::new_signed_tx(
            sender,
            message,
            chain_id,
            gas_tip,
            gas_limit,
            max_gas_price,
            1000,
        )
    }
}
