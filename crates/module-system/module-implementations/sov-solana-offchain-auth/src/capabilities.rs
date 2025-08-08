use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_modules_api::capabilities::{
    calculate_hash, verify_and_decode_tx, AuthenticationError, AuthenticationOutput,
    BatchFromUnregisteredSequencer, FatalError, TransactionAuthenticator,
    UnregisteredAuthenticationError,
};
use sov_modules_api::prelude::serde_json;
use sov_modules_api::transaction::UnsignedTransaction;
use sov_modules_api::{
    CryptoSpec, DispatchCall, FullyBakedTx, ProvableStateReader, RawTx, Runtime, Spec,
};

/// The application domain for Solana offchain messages (placeholder for now)
pub const APPLICATION_DOMAIN: [u8; 32] = [0u8; 32];

/// The envelope for a signed spec-compliant solana offchain message, where the signed message
/// includes the preamble.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct SolanaOffchainEnvelope<S: Spec> {
    pub signed_message: Vec<u8>,
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
}

/// The envelope for a message signed "raw", without the preable included.
/// The preamble always starts with the \xff byte, whereas our raw message is JSON and so can only
/// start with an ASCII character (normally, '{'), allowing us to unambiguously differentiate them.
/// Without the preamble present, we need to include the pubkey explicitly.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct SolanaOffchainRawMessage<S: Spec> {
    pub signed_message: Vec<u8>,
    pub pubkey: <S::CryptoSpec as CryptoSpec>::PublicKey,
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
}

/// The length of a preamble with a single 32-byte signer.
pub const PREAMBLE_LEN: u64 = 85;

/// The preamble/header required for signing solana offchain messages, supporting a single signer.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct RawSolanaOffchainMessagePreamble {
    pub signing_domain: [u8; 16],
    pub header_version: u8,
    pub application_domain: [u8; 32],
    pub message_format: u8,
    pub signer_count: u8,
    pub signer: [u8; 32],
    pub message_length: [u8; 2],
}

/// Validates a Solana offchain message preamble
fn validate_preamble(
    preamble: &RawSolanaOffchainMessagePreamble,
    actual_message_length: usize,
) -> Result<(), FatalError> {
    if preamble.signing_domain != *b"\xffsolana offchain" {
        return Err(FatalError::DeserializationFailed(
            "Invalid signing domain in preamble".to_string(),
        ));
    }
    // 0 is the only supported header version
    if preamble.header_version != 0 {
        return Err(FatalError::DeserializationFailed(
            "Invalid header version in preamble".to_string(),
        ));
    }
    if preamble.application_domain != APPLICATION_DOMAIN {
        return Err(FatalError::DeserializationFailed(
            "Invalid application domain in preamble".to_string(),
        ));
    }
    // Format 0 is the ASCII, hw-wallet compatible format
    if preamble.message_format != 0 {
        return Err(FatalError::DeserializationFailed(
            "Invalid message format in preamble".to_string(),
        ));
    }
    if preamble.signer_count != 1 {
        return Err(FatalError::DeserializationFailed(
            "Invalid signer count in preamble".to_string(),
        ));
    }
    let expected_length = u16::from_le_bytes(preamble.message_length) as usize;
    if expected_length != actual_message_length {
        return Err(FatalError::DeserializationFailed(format!(
            "Message length mismatch: expected {expected_length}, got {actual_message_length}"
        )));
    }

    Ok(())
}

pub fn authenticate<Accessor, S, D>(
    raw_tx: &[u8],
    chain_hash: &[u8; 32],
    state: &mut Accessor,
) -> Result<AuthenticationOutput<S, D::Decodable>, AuthenticationError>
where
    Accessor: ProvableStateReader<sov_state::User, Spec = S>,
    S: Spec,
    D: DispatchCall<Spec = S>,
    <D as DispatchCall>::Decodable: Serialize + DeserializeOwned,
{
    let raw_tx_hash = calculate_hash::<Accessor, S>(raw_tx, state)
        .map_err(|e| AuthenticationError::OutOfGas(e.to_string()))?;

    let (signer, signature, json_message) = unpack_solana_message::<S>(raw_tx)
        .map_err(|e| AuthenticationError::FatalError(e, raw_tx_hash))?;

    // TODO: add gas metering!
    let unsigned_tx: UnsignedTransaction<D, S> =
        serde_json::from_slice(&json_message).map_err(|e| {
            AuthenticationError::FatalError(
                FatalError::DeserializationFailed(e.to_string()),
                raw_tx_hash,
            )
        })?;

    let tx = unsigned_tx.to_signed_tx(signer, signature);

    verify_and_decode_tx::<S, D>(raw_tx_hash, tx, chain_hash, state)
}

fn unpack_solana_message<S: Spec>(
    raw_tx: &[u8],
) -> Result<
    (
        <S::CryptoSpec as CryptoSpec>::PublicKey,
        <S::CryptoSpec as CryptoSpec>::Signature,
        Vec<u8>,
    ),
    FatalError,
> {
    // First 4 bytes are the length of the Vec<u8> as u32 (borsh encoding)
    if raw_tx.len() < 5 {
        return Err(FatalError::DeserializationFailed(
            "Message too short".to_string(),
        ));
    }

    // The fifth byte tells us which format we're dealing with
    if raw_tx[4] == 0xff {
        // Spec-compliant message with preamble
        let envelope: SolanaOffchainEnvelope<S> = borsh::from_slice(raw_tx)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        // Verify preamble is present and valid
        if envelope.signed_message.len() < PREAMBLE_LEN as usize {
            return Err(FatalError::DeserializationFailed(
                "Message too short for preamble".to_string(),
            ));
        }

        let preamble: RawSolanaOffchainMessagePreamble =
            borsh::from_slice(&envelope.signed_message[0..PREAMBLE_LEN as usize])
                .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        // Calculate actual message length (excluding preamble)
        let actual_message_length = envelope.signed_message.len() - PREAMBLE_LEN as usize;

        // Validate the preamble
        validate_preamble(&preamble, actual_message_length)?;

        let signer: <S::CryptoSpec as CryptoSpec>::PublicKey = borsh::from_slice(&preamble.signer)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        let json_message = envelope.signed_message[PREAMBLE_LEN as usize..].to_vec();

        Ok((signer, envelope.signature, json_message))
    } else {
        // Raw message without preamble (should start with ASCII character, typically '{')
        let raw_message: SolanaOffchainRawMessage<S> = borsh::from_slice(raw_tx)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        Ok((
            raw_message.pubkey,
            raw_message.signature,
            raw_message.signed_message,
        ))
    }
}

/// Decode bytes as a Sovereign SDK transaction, returning the message and tx info.
pub fn decode_solana_json_tx<S, D>(raw_tx: &[u8]) -> Result<D::Decodable, FatalError>
where
    S: Spec,
    D: DispatchCall<Spec = S>,
    <D as DispatchCall>::Decodable: Serialize + DeserializeOwned,
{
    let (_signer, _signature, json_message) = unpack_solana_message::<S>(raw_tx)?;
    let unsigned_tx: UnsignedTransaction<D, S> = serde_json::from_slice(&json_message)
        .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;
    Ok(unsigned_tx.call())
}

/// Indicates that a runtime supports the `SolanaOffchain` transaction authenticator
/// and provides suitable methods for encoding and decoding solana offchain message transactions.
#[allow(unused)]
pub trait SolanaOffchainAuthenticatorTrait<S: Spec>: Runtime<S> {
    /// Add the Ethereum discriminant to a transaction the runtime.
    fn add_solana_offchain_auth(tx: RawTx) -> <Self::Auth as TransactionAuthenticator<S>>::Input;

    /// Encode a transaction with the Ethereum discriminant for the runtime.
    fn encode_with_solana_offchain_auth(tx: RawTx) -> FullyBakedTx {
        <Self::Auth as TransactionAuthenticator<S>>::encode_authenticator_input(
            &Self::add_solana_offchain_auth(tx),
        )
    }
}

/// See [`TransactionAuthenticator::Input`].
#[derive(std::fmt::Debug, Clone, BorshDeserialize, BorshSerialize)]
// #[borsh(use_discriminant=true)]
// #[repr(u8)]
pub enum SolanaOffchainAuthenticatorInput<T = RawTx> {
    /// Authenticate using the standard `sov-module` authenticator, which uses the default
    /// signature scheme and hashing algorithm defined in the rollup's [`Spec`].
    Standard(T),
    /// Authenticate using the solana offchain authenticator, which expects a standard solana
    /// offchain message version 0 (ASCII, max 1212 bytes); we expect the ASCII message to contain
    /// a JSON-serialized transaction
    SolanaOffchain(T),
}

/// Solana offchain message compatible transaction authenticator. See [`TransactionAuthenticator`].
pub struct SolanaOffchainAuthenticator<S, Rt>(PhantomData<(S, Rt)>);

impl<S, Rt> TransactionAuthenticator<S> for SolanaOffchainAuthenticator<S, Rt>
where
    S: Spec,
    // S::Address: FromVmAddress<Base58Address>,
    Rt: Runtime<S> + DispatchCall<Spec = S>,
    <Rt as DispatchCall>::Decodable: Serialize + DeserializeOwned,
{
    type Decodable = <Rt as DispatchCall>::Decodable;
    type Input = SolanaOffchainAuthenticatorInput;

    #[cfg(feature = "native")]
    fn decode_serialized_tx(
        tx: &FullyBakedTx,
    ) -> Result<Self::Decodable, sov_modules_api::capabilities::FatalError> {
        let auth_variant: SolanaOffchainAuthenticatorInput =
            borsh::from_slice(&tx.data).map_err(|e| {
                sov_modules_api::capabilities::FatalError::DeserializationFailed(e.to_string())
            })?;

        match auth_variant {
            SolanaOffchainAuthenticatorInput::Standard(raw_tx) => {
                let call = sov_modules_api::capabilities::decode_sov_tx::<S, Rt>(&raw_tx.data)?;
                Ok(call)
            }
            SolanaOffchainAuthenticatorInput::SolanaOffchain(raw_tx) => {
                let call = decode_solana_json_tx::<S, Rt>(&raw_tx.data)?;
                Ok(call)
            }
        }
    }

    fn authenticate<Accessor: ProvableStateReader<sov_state::User, Spec = S>>(
        tx: &FullyBakedTx,
        state: &mut Accessor,
    ) -> Result<
        sov_modules_api::capabilities::AuthenticationOutput<S, Self::Decodable>,
        sov_modules_api::capabilities::AuthenticationError,
    > {
        let input: SolanaOffchainAuthenticatorInput = borsh::from_slice(&tx.data).map_err(|e| {
            sov_modules_api::capabilities::fatal_deserialization_error::<_, S, _>(
                &tx.data, e, state,
            )
        })?;

        match input {
            SolanaOffchainAuthenticatorInput::SolanaOffchain(tx) => {
                let (tx_and_raw_hash, auth_data, runtime_call) =
                    authenticate::<Accessor, S, Rt>(&tx.data, &Rt::CHAIN_HASH, state)?;

                Ok((tx_and_raw_hash, auth_data, runtime_call))
            }
            SolanaOffchainAuthenticatorInput::Standard(tx) => {
                let (tx_and_raw_hash, auth_data, runtime_call) =
                    sov_modules_api::capabilities::authenticate::<_, S, Rt>(
                        &tx.data,
                        &Rt::CHAIN_HASH,
                        state,
                    )?;

                Ok((tx_and_raw_hash, auth_data, runtime_call))
            }
        }
    }

    #[cfg(feature = "native")]
    fn compute_tx_hash(
        tx: &sov_modules_api::FullyBakedTx,
    ) -> anyhow::Result<sov_modules_api::TxHash> {
        let input: SolanaOffchainAuthenticatorInput = borsh::from_slice(&tx.data)?;

        match input {
            SolanaOffchainAuthenticatorInput::SolanaOffchain(tx)
            | SolanaOffchainAuthenticatorInput::Standard(tx) => {
                Ok(sov_modules_api::capabilities::calculate_hash(
                    &tx.data,
                    &mut sov_modules_api::gas::UnlimitedGasMeter::<S>::default(),
                )?)
            }
        }
    }

    fn authenticate_unregistered<Accessor: ProvableStateReader<sov_state::User, Spec = S>>(
        batch: &BatchFromUnregisteredSequencer,
        state: &mut Accessor,
    ) -> Result<
        sov_modules_api::capabilities::AuthenticationOutput<S, Self::Decodable>,
        UnregisteredAuthenticationError,
    > {
        let Self::Input::Standard(input) = borsh::from_slice(&batch.tx.data)
            .map_err(|_| UnregisteredAuthenticationError::InvalidAuthenticationDiscriminant)?
        else {
            return Err(UnregisteredAuthenticationError::InvalidAuthenticationDiscriminant);
        };

        let (tx_and_raw_hash, auth_data, runtime_call) =
            sov_modules_api::capabilities::authenticate::<_, S, Rt>(
                &input.data,
                &Rt::CHAIN_HASH,
                state,
            )
            .map_err(|e| match e {
                AuthenticationError::FatalError(err, hash) => {
                    UnregisteredAuthenticationError::FatalError(err, hash)
                }
                AuthenticationError::OutOfGas(err) => {
                    UnregisteredAuthenticationError::OutOfGas(err)
                }
            })?;

        if Rt::allow_unregistered_tx(&runtime_call) {
            Ok((tx_and_raw_hash, auth_data, runtime_call))
        } else {
            Err(UnregisteredAuthenticationError::FatalError(
                FatalError::Other(
                    "The runtime call included in the transaction was invalid.".to_string(),
                ),
                tx_and_raw_hash.raw_tx_hash,
            ))?
        }
    }

    fn add_standard_auth(tx: RawTx) -> Self::Input {
        SolanaOffchainAuthenticatorInput::Standard(tx)
    }
}

#[cfg(test)]
mod test {
    use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
    use sov_mock_zkvm::crypto::Ed25519Signature;
    use sov_modules_api::PrivateKey;
    use sov_test_utils::TestSpec;

    use super::*;

    fn message_preamble(pubkey: &[u8; 32], message_length: u16) -> [u8; 85] {
        let mut header = Vec::<u8>::new();
        // Signing domain (pre-defined constant)
        header.extend(b"\xffsolana offchain");
        // Header version (only 0 is valid)
        header.push(0);
        // Application domain
        header.extend(APPLICATION_DOMAIN);
        // Message format - 0 is for ASCII, hardware wallet compatible
        header.push(0);
        // Signer count
        header.push(1);
        header.extend(pubkey);
        // Message length as little-endian u16
        header.extend(&message_length.to_le_bytes());
        return header.try_into().unwrap();
    }

    #[test]
    fn test_unpack_with_preamble() {
        let message = b"{\"test\":\"abcd\"}";
        let message_len = message.len() as u16;

        // Placeholder pubkey and signature, this is only testing parsing and not authentication
        let pubkey = Ed25519PrivateKey::generate().pub_key();
        let signature: Ed25519Signature = [4u8; 64].as_slice().try_into().unwrap();

        let preamble = message_preamble(&pubkey.bytes(), message_len);

        let mut signed_message = Vec::new();
        signed_message.extend_from_slice(&preamble);
        signed_message.extend_from_slice(message);

        let envelope = SolanaOffchainEnvelope::<TestSpec> {
            signed_message,
            signature: signature.clone(),
        };

        let serialized = borsh::to_vec(&envelope).unwrap();

        let result = unpack_solana_message::<TestSpec>(&serialized);
        assert!(result.is_ok());

        let (unpacked_pubkey, unpacked_signature, unpacked_message) = result.unwrap();
        assert_eq!(unpacked_pubkey, pubkey);
        assert_eq!(unpacked_signature, signature);
        assert_eq!(unpacked_message, message);
    }

    #[test]
    fn test_unpack_raw_message() {
        let message = b"{\"test\":\"abcd\"}";

        // Placeholder pubkey and signature, this is only testing parsing and not authentication
        let pubkey = Ed25519PrivateKey::generate().pub_key();
        let signature: Ed25519Signature = [4u8; 64].as_slice().try_into().unwrap();

        let raw_message = SolanaOffchainRawMessage::<TestSpec> {
            signed_message: message.to_vec(),
            pubkey: pubkey.clone(),
            signature: signature.clone(),
        };

        let serialized = borsh::to_vec(&raw_message).unwrap();

        let result = unpack_solana_message::<TestSpec>(&serialized);
        assert!(result.is_ok());

        let (unpacked_pubkey, unpacked_signature, unpacked_message) = result.unwrap();
        assert_eq!(unpacked_pubkey, pubkey);
        assert_eq!(unpacked_signature, signature);
        assert_eq!(unpacked_message, message);
    }

    #[test]
    fn test_invalid_preamble() {
        let message = b"{\"test\":\"abcd\"}";

        let pubkey = Ed25519PrivateKey::generate().pub_key();
        let signature: Ed25519Signature = [4u8; 64].as_slice().try_into().unwrap();

        // Create invalid preamble
        let mut header = Vec::<u8>::new();
        header.extend(b"\xffsolanaXoffchain"); // Wrong domain
        header.push(0);
        header.extend(APPLICATION_DOMAIN);
        header.push(0);
        header.push(1);
        header.extend(pubkey.bytes());
        header.extend(&(message.len() as u16).to_le_bytes());

        let mut signed_message = Vec::new();
        signed_message.extend_from_slice(&header);
        signed_message.extend_from_slice(message);

        let envelope = SolanaOffchainEnvelope::<TestSpec> {
            signed_message,
            signature: signature.clone(),
        };

        let serialized = borsh::to_vec(&envelope).unwrap();

        // Should fail validation
        let result = unpack_solana_message::<TestSpec>(&serialized);
        assert!(result.is_err());
        assert!(matches!(result, Err(FatalError::DeserializationFailed(_))));
    }
}
