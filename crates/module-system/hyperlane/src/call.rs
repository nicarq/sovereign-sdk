use std::fmt::Debug;

use anyhow::{Context as _, Result};
use schemars::JsonSchema;
use sov_modules_api::macros::{config_value, UniversalWallet};
use sov_modules_api::{Context, EventEmitter, HexHash, HexString, SafeVec, Spec, TxState};
use strum::{EnumDiscriminants, EnumIs, VariantArray};

use super::Mailbox;
use crate::event::Event;
use crate::ism::Ism;
use crate::traits::PostDispatchHook;
use crate::types::{keccak256_hash, Message};
use crate::{Delivery, DispatchState, HyperlaneAddress, Recipient};

/// The version of the hyperlane message format used by the mailbox module.
pub const MESSAGE_VERSION: u8 = 3;

/// The maximum size of a message body.
// Currently set to 8KB
pub const MAX_MESSAGE_BODY_SIZE: usize = 8192;
/// The maximum size of a message metadata.
// Currently set to 8KB
pub const MAX_MESSAGE_METADATA_SIZE: usize = 8192;

/// This enumeration represents the available call messages for interacting with the `sov-value-setter` module.
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Eq,
    Clone,
    JsonSchema,
    EnumDiscriminants,
    EnumIs,
    UniversalWallet,
)]
#[serde(rename_all = "snake_case")]
#[strum_discriminants(derive(VariantArray, EnumIs))]
pub enum CallMessage {
    /// Sends an outbound message to the specified recipient.
    Dispatch {
        /// The destination domain (aka "Chain ID")
        domain: u32,
        /// The recipient address. Must implement the `handle` function - i.e. be a smart contract
        recipient: HexHash,
        /// The message body. For example, if the recipient is a warp route, this will encode the amount/type of funds being transferred
        body: HexString<SafeVec<u8, MAX_MESSAGE_BODY_SIZE>>,
        /// The "metadata" which is used to verify the message or control hooks. Currently set to `None` at construction since none of
        /// our currently implemented ISMs require metadata. This will change in an upcoming version.
        metadata: Option<HexString<SafeVec<u8, MAX_MESSAGE_METADATA_SIZE>>>,
    },
    /// Receive an inbound message. This is called *on the desitination chain* by the relayer after
    /// a `dispatch` call has been made on the source chain.
    ///
    /// Passes the message metadata and body to the security module (ISM) for verification,
    /// then calls the recipient's `handle` function with the message body.
    Process {
        /// Metadata used to verify the message.
        metadata: HexString<SafeVec<u8, MAX_MESSAGE_BODY_SIZE>>,
        /// The serialized [`Message`] struct
        message: HexString<SafeVec<u8, MAX_MESSAGE_METADATA_SIZE>>,
    },
}
impl<S: Spec, R: Recipient<S>> Mailbox<S, R>
where
    S::Address: HyperlaneAddress,
{
    /// Dispatches (aka "sends") a message to the specified recipient.
    // Compare with https://github.com/eigerco/hyperlane-monorepo/blob/b68fe264b3585ecd9d95a5ec2ec2d7defbe907d2/solidity/contracts/Mailbox.sol#L276
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dispatch(
        &mut self,
        destination_domain: u32,
        recipient_address: HexHash,
        sender: HexHash,
        message_body: HexString,
        metadata: Option<HexString>,
        _custom_hook: Option<impl PostDispatchHook<S>>,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<HexString> {
        let mut dispatch_state = self
            .dispatch_state
            .borrow_mut(state)?
            .unwrap_or(DispatchState {
                nonce: 0,
                last_dispatched_id: HexHash::new([0; 32]),
            });
        let message = Message {
            version: MESSAGE_VERSION,
            nonce: dispatch_state.nonce,
            origin_domain: config_value!("HYPERLANE_BRIDGE_DOMAIN"),
            sender,
            dest_domain: destination_domain,
            recipient: recipient_address,
            body: message_body,
        };
        let message_id = message.id();

        dispatch_state.nonce += 1;
        dispatch_state.last_dispatched_id = message_id;
        dispatch_state.save(state)?;

        let message_hex: HexString = message.encode();
        self.emit_event(
            state,
            Event::Dispatch {
                sender: message.sender,
                destination_domain,
                recipient_address,
                message: message_hex.clone(),
            },
        );
        self.emit_event(state, Event::DispatchId { id: message_id });

        let metadata = metadata.unwrap_or_else(|| HexString::new(vec![]));

        self.merkle_tree_hooks
            .post_dispatch(&metadata, &message_hex, state)?;

        // TODO: Add default hook and use custom hook or remove
        // let hook = match custom_hook {
        //     Some(hook) => Some(hook),
        //     None => self.default_hook.get_or_err(state)??,
        // };
        // hook.map(|hook| {
        //     self.hook_registry
        //         .post_dispatch(&metadata, &message_hex, state, context)
        // });

        Ok(message_hex)
    }

    /// Processes an incoming message.
    // Compare with https://github.com/eigerco/hyperlane-monorepo/blob/b68fe264b3585ecd9d95a5ec2ec2d7defbe907d2/solidity/contracts/Mailbox.sol#L202
    pub(crate) fn process(
        &mut self,
        metadata: HexString,
        message: HexString,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let message_id = keccak256_hash(&message.0);
        let message = Message::decode(message.as_ref())
            .context(format!("Failed to decode message {}", message_id))?;
        // Ensure message version is correct
        anyhow::ensure!(
            message.version == MESSAGE_VERSION,
            "Invalid message version: {} had version {} but only {} is supported",
            message_id,
            message.version,
            MESSAGE_VERSION,
        );
        let bridge_domain = config_value!("HYPERLANE_BRIDGE_DOMAIN");

        // Ensure the message is intended for us
        anyhow::ensure!(
            message.dest_domain == bridge_domain,
            "Invalid message destination domain. Message ID: {} had domain: {} but only {} is supported",
            message_id,
            message.dest_domain,
            bridge_domain
        );

        // Check if message has already been processed. If not, mark it as processed.
        let delivery = self.deliveries.borrow(&message_id, state)?;
        if delivery.is_some() {
            return Err(anyhow::anyhow!("Message {} already processed", message_id));
        }
        self.deliveries.set(
            &message_id,
            &Delivery {
                sender: message.sender,
                block_number: state.current_visible_slot_number().get(),
            },
            state,
        )?;

        self.emit_event(
            state,
            Event::Process {
                origin_domain: message.origin_domain,
                sender_address: message.sender,
                recipient_address: message.recipient,
            },
        );
        self.emit_event(state, Event::ProcessId { id: message.id() });

        // Try and get ISM from recipient registry, if not found, use default ISM
        let ism = match self.recipients.ism(&message.recipient, state)? {
            Some(ism) => ism,
            None => Ism::AlwaysTrust, // TODO: Add a less insecure default ISM
        };

        ism.verify(context, &message, &metadata, state)?;
        self.recipients.handle(
            message.origin_domain,
            message.sender,
            &message.recipient,
            message.body,
            state,
        )?;

        Ok(())
    }
}
