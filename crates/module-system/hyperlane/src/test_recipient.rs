//! Defines a module that can receive messages for testing.

use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{
    Context, Error, EventEmitter, HexHash, HexString, Module, ModuleId, ModuleInfo, ModuleRestApi,
    Spec, StateMap, TxState,
};

use crate::ism::Ism;
use crate::{HyperlaneAddress, Recipient};

/// A magic domain number used to signal that the sender is a Sovereign SDK chain.
pub const MAGIC_SOV_CHAIN_DOMAIN: u32 = 12345;

/// A module that can receive messages for testing.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct TestRecipient<S: Spec> {
    /// The ID of the module.
    #[id]
    pub id: ModuleId,

    /// A from registered recipient addresses to their ISM.
    #[state]
    pub isms: StateMap<HexHash, Ism>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

/// Events emitted by the test recipient module
#[derive(
    Clone,
    BorshDeserialize,
    BorshSerialize,
    Debug,
    PartialEq,
    Eq,
    Hash,
    JsonSchema,
    Serialize,
    Deserialize,
)]
pub enum Event<S: Spec> {
    /// A generic "message received" event when the sending chain is unknown
    MessageReceivedGeneric {
        #[allow(missing_docs)]
        origin: u32,
        #[allow(missing_docs)]
        sender: HexHash,
        #[allow(missing_docs)]
        body: HexString,
    },
    /// A "message received" event used when the sending chain is known to be a Sovereign SDK chain.
    MessageReceived {
        #[allow(missing_docs)]
        origin: u32,
        #[allow(missing_docs)]
        sender: S::Address,
        #[allow(missing_docs)]
        body: String,
    },
}

/// Call messages for the test recipient module.
#[derive(
    Clone,
    BorshDeserialize,
    BorshSerialize,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    UniversalWallet,
)]
pub enum CallMessage {
    /// Register a recipient and its ISM.
    Register {
        #[allow(missing_docs)]
        address: HexHash,
        #[allow(missing_docs)]
        ism: Ism,
    },
}

impl<S: Spec> Module for TestRecipient<S> {
    type Spec = S;
    type Config = ();
    type CallMessage = CallMessage;
    type Event = Event<S>;

    fn call(
        &mut self,
        msg: Self::CallMessage,
        _context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<(), Error> {
        match msg {
            CallMessage::Register { address, ism } => {
                self.register(address, ism, state)?;
            }
        }
        Ok(())
    }
}

impl<S: Spec> TestRecipient<S> {
    fn register(
        &mut self,
        address: HexHash,
        ism: Ism,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        if self.isms.get(&address, state)?.is_some() {
            anyhow::bail!("ISM already registered");
        }
        self.isms.set(&address, &ism, state)?;
        Ok(())
    }
}

impl<S: Spec> Recipient<S> for TestRecipient<S>
where
    S::Address: HyperlaneAddress,
{
    fn ism(&self, recipient: &HexHash, state: &mut impl TxState<S>) -> anyhow::Result<Option<Ism>> {
        self.isms.get(recipient, state).map_err(anyhow::Error::new)
    }

    /// Handles an inbound message. Note that this deviates from more standard Hyperlane `handle` API because all messages
    /// are dispatched through this module regardless of their ultimate destination, so we need to explicitly pass the recipient as an argument.
    fn handle(
        &mut self,
        origin: u32,
        sender: HexHash,
        recipient: &HexHash,
        body: HexString,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        if self.isms.get(recipient, state)?.is_none() {
            anyhow::bail!("Recipient not registered");
        }
        if origin == MAGIC_SOV_CHAIN_DOMAIN {
            self.emit_event(
                state,
                Event::MessageReceived {
                    origin,
                    sender: S::Address::from_sender(sender)?,
                    body: body.to_string(),
                },
            );
        } else {
            self.emit_event(
                state,
                Event::MessageReceivedGeneric {
                    origin,
                    sender,
                    body,
                },
            );
        }
        Ok(())
    }
}
