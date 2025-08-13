use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sov_modules_api::capabilities::{
    calculate_hash_metered, extract_authorization_data, verify_chain_id, AuthenticationError,
    AuthenticationOutput, FatalError, UniquenessData,
};
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::transaction::{
    self, AuthenticatedTransactionAndRawHash, TransactionCallable, TxDetails, UnsignedTransaction,
};
use sov_modules_api::{
    CryptoSpec, DispatchCall, GasMeter, GasSpec, MeteredJsonDeserialize,
    MeteredJsonDeserializeError, MeteredSignature, ProvableStateReader, Spec, TxHash,
};

/// The application domain for Solana offchain messages (placeholder for now)
pub const APPLICATION_DOMAIN: [u8; 32] = [0u8; 32];

/// The payload for a solana offchain message.
/// Essentially a wrapper around `sov_modules_api::transaction::UnsignedTransaction` that also
/// includes the chain_hash, in order to ensure the hash gets signed as part of the message.
/// We duplicate the UnsignedTransaction type rather than wrapping it to ensure the JSON displayed
/// to the user doesn't get too nested.
#[serde_with::serde_as]
#[derive(Debug, Serialize, Deserialize, UniversalWallet)]
#[serde(bound = "R::Call: serde::Serialize + serde::de::DeserializeOwned")]
pub struct SolanaOffchainUnsignedTransaction<R: TransactionCallable, S: Spec> {
    /// The runtime call
    pub runtime_call: R::Call,
    /// The uniqueness identifier
    pub uniqueness: UniquenessData,
    /// Data related to fees and gas handling.
    pub details: TxDetails<S>,
    /// The chain hash the transaction data was signed with. Because we're not actually using the
    /// schema to display the transaction, we have to include and validate the hash explicitly.
    #[serde_as(as = "serde_with::hex::Hex")]
    pub chain_hash: [u8; 32],
}

impl<R, S> SolanaOffchainUnsignedTransaction<R, S>
where
    S: Spec,
    R: TransactionCallable,
    <R as TransactionCallable>::Call: Serialize + DeserializeOwned,
{
    fn into_unsigned_tx(self) -> UnsignedTransaction<R, S> {
        UnsignedTransaction {
            runtime_call: self.runtime_call,
            uniqueness: self.uniqueness,
            details: self.details,
        }
    }

    fn unmetered_deserialize_inner(buf: &mut &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice::<SolanaOffchainUnsignedTransaction<R, S>>(buf)
    }
}

impl<R, S> MeteredJsonDeserialize<S> for SolanaOffchainUnsignedTransaction<R, S>
where
    S: Spec,
    R: TransactionCallable,
    <R as TransactionCallable>::Call: Serialize + DeserializeOwned,
{
    fn bias_json_deserialization() -> <S as Spec>::Gas {
        S::tx_bias_json_deserialization()
    }

    fn gas_to_charge_per_byte_json_deserialization() -> <S as Spec>::Gas {
        S::tx_gas_to_charge_per_byte_json_deserialization()
    }

    #[cfg_attr(feature = "bench", crate::cycle_tracker)]
    #[cfg_attr(
        all(feature = "gas-constant-estimation", feature = "native"),
        crate::track_gas_constants_usage
    )]
    fn deserialize(
        buf: &mut &[u8],
        meter: &mut impl GasMeter<Spec = S>,
    ) -> Result<Self, MeteredJsonDeserializeError<<S as GasSpec>::Gas>> {
        Self::charge_gas_to_deserialize(buf, meter)?;

        SolanaOffchainUnsignedTransaction::<R, S>::unmetered_deserialize_inner(buf)
            .map_err(MeteredJsonDeserializeError::SerdeError)
    }

    #[cfg(feature = "native")]
    fn unmetered_deserialize(
        buf: &mut &[u8],
    ) -> Result<Self, MeteredJsonDeserializeError<<S as GasSpec>::Gas>> {
        SolanaOffchainUnsignedTransaction::<R, S>::unmetered_deserialize_inner(buf)
            .map_err(MeteredJsonDeserializeError::SerdeError)
    }
}

/// The envelope for a signed spec-compliant solana offchain message, where the signed message
/// includes the preamble.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct SolanaOffchainSpecCompliantMessage<S: Spec> {
    /// The message is a JSON-serialized SolanaOffchainUnsignedTransaction with the standard preamble prepended.
    pub signed_message: Vec<u8>,
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
}

/// The envelope for a message signed "raw", without the preable included.
/// The preamble always starts with the \xff byte, whereas our raw message is JSON and so can only
/// start with an ASCII character (normally, '{'), allowing us to unambiguously differentiate them.
/// Without the preamble present, we need to include the pubkey explicitly.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct SolanaOffchainSimpleMessage<S: Spec> {
    /// The message is a JSON-serialized SolanaOffchainUnsignedTransaction, unaltered.
    pub signed_message: Vec<u8>,
    pub pubkey: <S::CryptoSpec as CryptoSpec>::PublicKey,
    pub signature: <S::CryptoSpec as CryptoSpec>::Signature,
}

/// The length of a preamble with a single 32-byte signer.
pub const PREAMBLE_LEN: usize = std::mem::size_of::<RawSolanaOffchainMessagePreamble>();
// Sanity check - this will only fail if the format or encoding is changed somehow. In that case,
// adjust this check, once we're sure the versioning is handled correctly.
const _: () = assert!(
    PREAMBLE_LEN == 85,
    "Preamble size is known to be exactly 85 bytes, unless the format changed"
);

/// The preamble/header required for signing solana offchain messages, supporting a single signer.
/// See https://docs.anza.xyz/proposals/off-chain-message-signing#message-preamble
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

// TODO: json_bytes and signed_bytes will have a good amount of duplication, maybe we can reduce
// the copying involved
struct UnpackedSolanaMessage<S: Spec> {
    pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
    signature: <S::CryptoSpec as CryptoSpec>::Signature,
    json_bytes: Vec<u8>,
    signed_bytes: Vec<u8>,
}

/// Verifies a signature over the signed bytes with gas metering
fn verify_solana_signature<S: Spec>(
    pub_key: &<S::CryptoSpec as CryptoSpec>::PublicKey,
    signature: &<S::CryptoSpec as CryptoSpec>::Signature,
    signed_bytes: &[u8],
    raw_tx_hash: TxHash,
    meter: &mut impl GasMeter<Spec = S>,
) -> Result<(), AuthenticationError> {
    MeteredSignature::new::<S>(signature.clone())
        .verify(pub_key, signed_bytes, meter)
        .map_err(|e| match e {
            sov_modules_api::MeteredSigVerificationError::BadSignature(err) => {
                AuthenticationError::FatalError(
                    FatalError::SigVerificationFailed(err.to_string()),
                    raw_tx_hash,
                )
            }
            sov_modules_api::MeteredSigVerificationError::GasError(err) => {
                AuthenticationError::OutOfGas(format!(
                    "Signature verification ran out of gas: {err}"
                ))
            }
        })
}

fn unpack_solana_message<S: Spec>(raw_tx: &[u8]) -> Result<UnpackedSolanaMessage<S>, FatalError> {
    // First 4 bytes are the length of the Vec<u8> as u32 (borsh encoding)
    if raw_tx.len() < 5 {
        return Err(FatalError::DeserializationFailed(
            "Message too short".to_string(),
        ));
    }

    // The fifth byte tells us which format we're dealing with
    if raw_tx[4] == 0xff {
        // Spec-compliant message with preamble
        let envelope: SolanaOffchainSpecCompliantMessage<S> = borsh::from_slice(raw_tx)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        // Verify preamble is present and valid
        if envelope.signed_message.len() < PREAMBLE_LEN {
            return Err(FatalError::DeserializationFailed(
                "Message too short for preamble".to_string(),
            ));
        }

        let preamble: RawSolanaOffchainMessagePreamble =
            borsh::from_slice(&envelope.signed_message[0..PREAMBLE_LEN])
                .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        // Calculate actual message length (excluding preamble)
        let actual_message_length = envelope.signed_message.len() - PREAMBLE_LEN;

        // Validate the preamble
        validate_preamble(&preamble, actual_message_length)?;

        let signer: <S::CryptoSpec as CryptoSpec>::PublicKey = borsh::from_slice(&preamble.signer)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        let json_bytes = envelope.signed_message[PREAMBLE_LEN..].to_vec();

        Ok(UnpackedSolanaMessage {
            pub_key: signer,
            signature: envelope.signature,
            json_bytes,
            signed_bytes: envelope.signed_message,
        })
    } else {
        // Raw message without preamble (should start with ASCII character, typically '{')
        let raw_message: SolanaOffchainSimpleMessage<S> = borsh::from_slice(raw_tx)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;

        Ok(UnpackedSolanaMessage {
            pub_key: raw_message.pubkey,
            signature: raw_message.signature,
            json_bytes: raw_message.signed_message.clone(),
            signed_bytes: raw_message.signed_message,
        })
    }
}

/// Decode bytes as a Sovereign SDK transaction, returning the message and tx info.
pub fn decode_solana_json_tx<S, D>(raw_tx: &[u8]) -> Result<D::Decodable, FatalError>
where
    S: Spec,
    D: DispatchCall<Spec = S>,
    <D as DispatchCall>::Decodable: Serialize + DeserializeOwned,
{
    let unpacked_message = unpack_solana_message::<S>(raw_tx)?;
    let solana_unsigned_tx: SolanaOffchainUnsignedTransaction<D, S> =
        serde_json::from_slice(&unpacked_message.json_bytes)
            .map_err(|e| FatalError::DeserializationFailed(e.to_string()))?;
    Ok(solana_unsigned_tx.into_unsigned_tx().call())
}

pub fn authenticate<Accessor, S, D>(
    raw_tx: &[u8],
    runtime_chain_hash: &[u8; 32],
    state: &mut Accessor,
) -> Result<AuthenticationOutput<S, D::Decodable>, AuthenticationError>
where
    Accessor: ProvableStateReader<sov_state::User, Spec = S>,
    S: Spec,
    D: DispatchCall<Spec = S>,
    <D as DispatchCall>::Decodable: Serialize + DeserializeOwned,
{
    let raw_tx_hash = calculate_hash_metered::<Accessor, S>(raw_tx, state)
        .map_err(|e| AuthenticationError::OutOfGas(e.to_string()))?;

    let unpacked_message = unpack_solana_message::<S>(raw_tx)
        .map_err(|e| AuthenticationError::FatalError(e, raw_tx_hash))?;

    let solana_unsigned_tx =
        match <SolanaOffchainUnsignedTransaction<D, S> as MeteredJsonDeserialize<S>>::deserialize(
            &mut unpacked_message.json_bytes.as_slice(),
            state,
        ) {
            Ok(ok) => ok,

            Err(MeteredJsonDeserializeError::GasError(e)) => {
                return Err(AuthenticationError::OutOfGas(format!(
                    "Transaction deserialization run out of gas {e}, tx hash {raw_tx_hash}"
                )))
            }
            Err(MeteredJsonDeserializeError::SerdeError(e)) => {
                return Err(AuthenticationError::FatalError(
                    FatalError::DeserializationFailed(e.to_string()),
                    raw_tx_hash,
                ));
            }
        };

    let provided_chain_hash = solana_unsigned_tx.chain_hash;
    let unsigned_tx = solana_unsigned_tx.into_unsigned_tx();
    // This is useful to be able to reuse some of the standard authenticator's logic
    let reconstructed_tx_v0 = transaction::Version0 {
        runtime_call: unsigned_tx.runtime_call,
        uniqueness: unsigned_tx.uniqueness,
        details: unsigned_tx.details,
        signature: unpacked_message.signature,
        pub_key: unpacked_message.pub_key,
    };

    if provided_chain_hash != *runtime_chain_hash {
        return Err(AuthenticationError::FatalError(
            FatalError::InvalidChainHash {
                expected: hex::encode(runtime_chain_hash),
                got: hex::encode(provided_chain_hash),
            },
            raw_tx_hash,
        ));
    }

    verify_chain_id(&reconstructed_tx_v0.details, raw_tx_hash)?;

    verify_solana_signature::<S>(
        &reconstructed_tx_v0.pub_key,
        &reconstructed_tx_v0.signature,
        &unpacked_message.signed_bytes,
        raw_tx_hash,
        state,
    )?;

    let authorization_data =
        extract_authorization_data::<S, D>(&reconstructed_tx_v0, raw_tx_hash, state)?;

    let tx_and_raw_hash = AuthenticatedTransactionAndRawHash {
        raw_tx_hash,
        authenticated_tx: reconstructed_tx_v0.details.clone().into(),
    };

    Ok((
        tx_and_raw_hash,
        authorization_data,
        reconstructed_tx_v0.runtime_call,
    ))
}

#[cfg(test)]
pub mod test {
    use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
    use sov_mock_zkvm::crypto::Ed25519Signature;
    use sov_modules_api::PrivateKey;
    use sov_test_utils::TestSpec;

    use super::*;
    use crate::utils::make_preamble_for_message;

    #[test]
    fn test_unpack_with_preamble() {
        let message = b"{\"test\":\"abcd\"}";
        let message_len = message.len() as u16;

        // Placeholder pubkey and signature, this is only testing parsing and not authentication
        let pubkey = Ed25519PrivateKey::generate().pub_key();
        let signature: Ed25519Signature = [4u8; 64].as_slice().try_into().unwrap();

        let preamble = make_preamble_for_message(pubkey.bytes(), message_len);

        let mut signed_message = Vec::new();
        signed_message.extend_from_slice(&preamble);
        signed_message.extend_from_slice(message);

        let envelope = SolanaOffchainSpecCompliantMessage::<TestSpec> {
            signed_message: signed_message.clone(),
            signature: signature.clone(),
        };

        let serialized = borsh::to_vec(&envelope).unwrap();

        let result = unpack_solana_message::<TestSpec>(&serialized);
        assert!(result.is_ok());

        let UnpackedSolanaMessage {
            pub_key: unpacked_pubkey,
            signature: unpacked_signature,
            json_bytes: unpacked_message,
            signed_bytes,
        } = result.unwrap();
        assert_eq!(unpacked_pubkey, pubkey);
        assert_eq!(unpacked_signature, signature);
        assert_eq!(unpacked_message, message);
        assert_eq!(signed_bytes, signed_message);
    }

    #[test]
    fn test_unpack_raw_message() {
        let message = b"{\"test\":\"abcd\"}";

        // Placeholder pubkey and signature, this is only testing parsing and not authentication
        let pubkey = Ed25519PrivateKey::generate().pub_key();
        let signature: Ed25519Signature = [4u8; 64].as_slice().try_into().unwrap();

        let raw_message = SolanaOffchainSimpleMessage::<TestSpec> {
            signed_message: message.to_vec(),
            pubkey: pubkey.clone(),
            signature: signature.clone(),
        };

        let serialized = borsh::to_vec(&raw_message).unwrap();

        let result = unpack_solana_message::<TestSpec>(&serialized);
        assert!(result.is_ok());

        let UnpackedSolanaMessage {
            pub_key: unpacked_pubkey,
            signature: unpacked_signature,
            json_bytes: unpacked_message,
            signed_bytes,
        } = result.unwrap();
        assert_eq!(unpacked_pubkey, pubkey);
        assert_eq!(unpacked_signature, signature);
        assert_eq!(unpacked_message, message);
        assert_eq!(signed_bytes, message);
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

        let envelope = SolanaOffchainSpecCompliantMessage::<TestSpec> {
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
