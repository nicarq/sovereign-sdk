#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

use borsh::{BorshDeserialize, BorshSerialize};
pub use call::*;
pub use event::Event;
pub use ism::Ism;
#[cfg(feature = "native")]
use sov_modules_api::rest::HasRestApi;
use sov_modules_api::{
    Context, DaSpec, Error, GenesisState, HexHash, HexString, Module, ModuleId, ModuleInfo, Spec,
    StateMap, StateValue, TxState,
};
use traits::NoOpPostDispatchHook;

mod call;
mod event;
mod genesis;
mod ism;
mod merkle;
pub use merkle::{Event as MerkleTreeEvent, MerkleTreeHooks};
#[cfg(feature = "test-utils")]
pub mod test_recipient;
pub mod traits;
mod types;
pub use types::Message;

/// The state of the mailbox.
#[derive(Clone, BorshDeserialize, BorshSerialize, Debug, PartialEq, Eq, Hash)]
pub struct DispatchState {
    /// The nonce for the current dispatch.
    pub nonce: u32,
    /// The last message ID that has been dispatched.
    pub last_dispatched_id: HexHash,
}

type MessageId = HexHash;

/// The delivery receipt of a message.
#[derive(Clone, BorshDeserialize, BorshSerialize, Debug, PartialEq, Eq, Hash)]
pub struct Delivery {
    /// The sender of the message.
    pub sender: HexHash,
    /// The block number it was dispatched in.
    pub block_number: u64,
}

/// The mailbox module is the entrypoint for the hyperlane protocol. All messages sent or received are routed through this module.
#[derive(Clone, ModuleInfo)]
pub struct Mailbox<S: Spec, R: Recipient<S>> {
    /// The ID of the module.
    #[id]
    pub id: ModuleId,

    /// The number of messages and latest ID that have been dispatched.
    #[state]
    pub dispatch_state: StateValue<DispatchState>,

    /// A map of message IDs to their delivery status.
    #[state]
    pub deliveries: StateMap<MessageId, Delivery>,

    /// A reference to the merkle tree hooks module.
    #[module]
    pub merkle_tree_hooks: MerkleTreeHooks<S>,

    /// A reference to the recipient module.
    #[module]
    pub recipients: R,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec, R: Recipient<S>> Module for Mailbox<S, R>
where
    S::Address: HyperlaneAddress,
{
    type Spec = S;

    type Config = ();

    type CallMessage = call::CallMessage;

    type Event = Event;

    fn genesis(
        &mut self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        config: &Self::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        // The initialization logic
        Ok(self.init_module(config, state)?)
    }

    fn call(
        &mut self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<(), Error> {
        match msg {
            call::CallMessage::Dispatch {
                domain,
                recipient,
                body,
                metadata,
            } => {
                self.dispatch(
                    domain,
                    recipient,
                    context.sender().to_sender(),
                    HexString::new(body.0.into()),
                    metadata.map(|m| HexString::new(m.0.into())),
                    Option::<NoOpPostDispatchHook>::None,
                    context,
                    state,
                )?;
                Ok(())
            }
            call::CallMessage::Process { metadata, message } => Ok(self.process(
                HexString::new(metadata.0.into()),
                HexString::new(message.0.into()),
                context,
                state,
            )?),
        }
    }
}

/// An address which is compatible with the hyperlane protocol.
///
/// Implementers of this trait must ensure that their addresses can be unambiguously represented in 32 bytes.
/// For example, if the address type is an enum where at least one variant is 32 bytes long, the impelementation
/// must pick one variant and always deserialize into that type (since there's no room to encode the discriminant).
pub trait HyperlaneAddress: Sized {
    /// Convert the address to a Hyperlane sender address.
    fn to_sender(&self) -> HexHash;
    /// Convert a Hyperlane sender address back to the original..
    fn from_sender(recipient: HexHash) -> anyhow::Result<Self>;
}

impl HyperlaneAddress for sov_modules_api::Address {
    fn to_sender(&self) -> HexHash {
        const START_INDEX: usize = 32 - sov_modules_api::Address::LENGTH;
        // Pad the address with leading zeros to 32 bytes. This is the hyperlane convention
        let mut bytes = [0u8; 32];
        bytes[START_INDEX..].copy_from_slice(self.as_ref());
        bytes.into()
    }

    fn from_sender(recipient: HexHash) -> anyhow::Result<Self> {
        const START_INDEX: usize = 32 - sov_modules_api::Address::LENGTH;
        // Check that the address is padded with leading zeros to match the hyperlane convention.
        let (padding, address) = recipient.0.split_at(START_INDEX);

        // Ensure padding is all zeros:
        anyhow::ensure!(
            padding.iter().all(|&byte| byte == 0),
            "Invalid address - not enough leading zeros"
        );

        Ok(Self::new(address.try_into().expect(
            "Infallible conversion failed; this is a bug, please report it",
        )))
    }
}

/// A module that can receive messages via the hyperlane protocol.
///
/// This module may be a "wrapper" module, which internally dispatches to several
/// other recipients.
pub trait Recipient<S: Spec>:
    Module + Clone + std::default::Default + ModuleInfo + HasNativeRestApi<S> + Send + Sync + 'static
{
    /// Get the [`ISM`](Ism) for a given recipient address.
    fn ism(&self, recipient: &HexHash, state: &mut impl TxState<S>) -> anyhow::Result<Option<Ism>>;

    /// Handle an inbound message.
    fn handle(
        &mut self,
        origin: u32,
        sender: HexHash,
        recipient: &HexHash,
        body: HexString,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()>;
}

/// A helper trait which requires `HasRestApi` if the `native` feature is enabled.
#[cfg(feature = "native")]
pub trait HasNativeRestApi<S: Spec>: HasRestApi<S> {}

#[cfg(feature = "native")]
impl<S: Spec, T: HasRestApi<S>> HasNativeRestApi<S> for T {}

/// A helper trait which requires `HasRestApi` if the `native` feature is enabled.
#[cfg(not(feature = "native"))]
pub trait HasNativeRestApi<S: Spec> {}

#[cfg(not(feature = "native"))]
impl<S: Spec, T> HasNativeRestApi<S> for T {}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use sov_modules_api::Address;

    use super::*;

    #[test]
    fn test_address_conversion() {
        let address =
            Address::from_str("sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7qhzze66").unwrap();
        let sender = address.to_sender();
        let recovered = Address::from_sender(sender).unwrap();
        assert_eq!(address, recovered);
    }
}
