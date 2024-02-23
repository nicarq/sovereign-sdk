//! This module defines the following tables:
//!
//!
//! Slot Tables:
//! - `SlotNumber -> StoredSlot`
//! - `SlotNumber -> Vec<BatchNumber>`
//!
//! Batch Tables:
//! - `BatchNumber -> StoredBatch`
//! - `BatchHash -> BatchNumber`
//!
//! Tx Tables:
//! - `TxNumber -> (TxHash,Tx)`
//! - `TxHash -> TxNumber`
//!
//! Event Tables:
//! - `(EventKey, TxNumber) -> EventNumber`
//! - `EventNumber -> (EventKey, EventValue)`
//!
//! JMT Tables:
//! - `KeyHash -> Key`
//! - `(Key, Version) -> JmtValue`
//! - `NodeKey -> Node`
//!
//! Module Accessory State Table:
//! - `(ModuleAddress, Key) -> Value`

use borsh::{maybestd, BorshDeserialize, BorshSerialize};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use jmt::Version;
use sov_rollup_interface::stf::{EventKey, StoredEvent};
use sov_schema_db::schema::{KeyDecoder, KeyEncoder, ValueCodec};
use sov_schema_db::{CodecError, SeekKeyEncoder};

use super::types::{
    AccessoryKey, AccessoryStateValue, BatchNumber, DbHash, EventNumber, ModuleAddress,
    ProofUniqueId, SlotNumber, StoredAggregatedProof, StoredBatch, StoredSlot, StoredTransaction,
    TxNumber,
};

/* Other tables used by the Rollup */

/// A list of all tables used by the LedgerDB. These tables store rollup "history" - meaning
/// transaction, events, receipts, etc.
pub const LEDGER_TABLES: &[&str] = &[
    SlotByNumber::table_name(),
    SlotByHash::table_name(),
    BatchByHash::table_name(),
    BatchByNumber::table_name(),
    TxByHash::table_name(),
    TxByNumber::table_name(),
    EventByKey::table_name(),
    EventByNumber::table_name(),
    EventByModuleAddress::table_name(),
    ProofByUniqueId::table_name(),
];

/// A list of all tables used by the NativeDB. These tables store
/// "accessory" state only accessible from a native execution context, to be
/// used for JSON-RPC and other tooling.
pub const NATIVE_TABLES: &[&str] = &[ModuleAccessoryState::table_name()];

/// Macro to define a table that implements [`sov_schema_db::Schema`].
/// KeyCodec<Schema> and ValueCodec<Schema> must be implemented separately.
///
/// ```ignore
/// define_table_without_codec!(
///  /// A table storing keys and value
///  (MyTable) MyKey => MyValue
/// )
///
/// // This impl must be written by hand
/// impl KeyCodec<MyTable> for MyKey {
/// // ...
/// }
///
/// // This impl must be written by hand
/// impl ValueCodec<MyTable> for MyValue {
/// // ...
/// }
/// ```
macro_rules! define_table_without_codec {
    ($(#[$docs:meta])+ ( $table_name:ident ) $key:ty => $value:ty) => {
        $(#[$docs])+
        ///
        #[doc = concat!("Takes [`", stringify!($key), "`] as a key and returns [`", stringify!($value), "`]")]
        #[derive(Clone, Copy, Debug, Default)]
        pub(crate) struct $table_name;

        impl ::sov_schema_db::schema::Schema for $table_name {
            const COLUMN_FAMILY_NAME: &'static str = $table_name::table_name();
            type Key = $key;
            type Value = $value;
        }

        impl $table_name {
            #[doc=concat!("Return ", stringify!($table_name), " as it is present inside the database.")]
            pub const fn table_name() -> &'static str {
                ::core::stringify!($table_name)
            }
        }

        impl ::std::fmt::Display for $table_name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::core::write!(f, "{}", stringify!($table_name))
            }
        }
    };
}

macro_rules! impl_borsh_value_codec {
    ($table_name:ident, $value:ty) => {
        impl ::sov_schema_db::schema::ValueCodec<$table_name> for $value {
            fn encode_value(
                &self,
            ) -> ::std::result::Result<
                ::sov_rollup_interface::maybestd::vec::Vec<u8>,
                ::sov_schema_db::CodecError,
            > {
                ::borsh::BorshSerialize::try_to_vec(self).map_err(Into::into)
            }

            fn decode_value(
                data: &[u8],
            ) -> ::std::result::Result<Self, ::sov_schema_db::CodecError> {
                ::borsh::BorshDeserialize::deserialize_reader(&mut &data[..]).map_err(Into::into)
            }
        }
    };
}

/// Macro to define a table that implements [`sov_schema_db::schema::Schema`].
/// Automatically generates KeyCodec<...> and ValueCodec<...> implementations
/// using the Encode and Decode traits from sov_rollup_interface
///
/// ```ignore
/// define_table_with_default_codec!(
///  /// A table storing keys and value
///  (MyTable) MyKey => MyValue
/// )
/// ```
macro_rules! define_table_with_default_codec {
    ($(#[$docs:meta])+ ($table_name:ident) $key:ty => $value:ty) => {
        define_table_without_codec!($(#[$docs])+ ( $table_name ) $key => $value);

        impl ::sov_schema_db::schema::KeyEncoder<$table_name> for $key {
            fn encode_key(&self) -> ::std::result::Result<::sov_rollup_interface::maybestd::vec::Vec<u8>, ::sov_schema_db::CodecError> {
                ::borsh::BorshSerialize::try_to_vec(self).map_err(Into::into)
            }
        }

        impl ::sov_schema_db::schema::KeyDecoder<$table_name> for $key {
            fn decode_key(data: &[u8]) -> ::std::result::Result<Self, ::sov_schema_db::CodecError> {
                ::borsh::BorshDeserialize::deserialize_reader(&mut &data[..]).map_err(Into::into)
            }
        }

        impl_borsh_value_codec!($table_name, $value);
    };
}

/// Macro similar to [`define_table_with_default_codec`], but to be used when
/// your key type should be [`SeekKeyEncoder`]. Borsh serializes integers as
/// little-endian, but RocksDB uses lexicographic ordering which is only
/// compatible with big-endian, so we use [`bincode`] with the big-endian option
/// here.
macro_rules! define_table_with_seek_key_codec {
    ($(#[$docs:meta])+ ($table_name:ident) $key:ty => $value:ty) => {
        define_table_without_codec!($(#[$docs])+ ( $table_name ) $key => $value);

        impl ::sov_schema_db::schema::KeyEncoder<$table_name> for $key {
            fn encode_key(&self) -> ::std::result::Result<::sov_rollup_interface::maybestd::vec::Vec<u8>, ::sov_schema_db::CodecError> {
                use ::anyhow::Context as _;
                use ::bincode::Options as _;

                let bincode_options = ::bincode::options()
                    .with_fixint_encoding()
                    .with_big_endian();

                bincode_options.serialize(self).context("Failed to serialize key").map_err(Into::into)
            }
        }

        impl ::sov_schema_db::schema::KeyDecoder<$table_name> for $key {
            fn decode_key(data: &[u8]) -> ::std::result::Result<Self, ::sov_schema_db::CodecError> {
                use ::anyhow::Context as _;
                use ::bincode::Options as _;

                let bincode_options = ::bincode::options()
                    .with_fixint_encoding()
                    .with_big_endian();

                bincode_options.deserialize_from(&mut &data[..]).context("Failed to deserialize key").map_err(Into::into)
            }
        }

        impl ::sov_schema_db::SeekKeyEncoder<$table_name> for $key {
            fn encode_seek_key(&self) -> ::std::result::Result<::sov_rollup_interface::maybestd::vec::Vec<u8>, ::sov_schema_db::CodecError> {
                <Self as ::sov_schema_db::schema::KeyEncoder<$table_name>>::encode_key(self)
            }
        }

        impl_borsh_value_codec!($table_name, $value);
    };
}

define_table_with_seek_key_codec!(
    /// The primary source for slot data
    (SlotByNumber) SlotNumber => StoredSlot
);

define_table_with_default_codec!(
    /// A "secondary index" for slot data by hash
    (SlotByHash) DbHash => SlotNumber
);

define_table_with_seek_key_codec!(
    /// The primary source for batch data
    (BatchByNumber) BatchNumber => StoredBatch
);

define_table_with_default_codec!(
    /// A "secondary index" for batch data by hash
    (BatchByHash) DbHash => BatchNumber
);

define_table_with_seek_key_codec!(
    /// The primary source for transaction data
    (TxByNumber) TxNumber => StoredTransaction
);

define_table_with_default_codec!(
    /// A "secondary index" for transaction data by hash
    (TxByHash) DbHash => TxNumber
);

define_table_with_seek_key_codec!(
    /// The primary store for event data
    (EventByNumber) EventNumber => StoredEvent
);

define_table_with_seek_key_codec!(
    /// A "secondary index" for event data by key
    (EventByKey) (EventKey, ModuleAddress, TxNumber, EventNumber) => ()
);

define_table_with_seek_key_codec!(
    /// A "tertiary index" for event data by module
    (EventByModuleAddress) (ModuleAddress, TxNumber, EventNumber) => ()
);

define_table_with_seek_key_codec!(
    /// The primary source for proof data
    (ProofByUniqueId) ProofUniqueId => StoredAggregatedProof
);

define_table_without_codec!(
    /// Non-JMT state stored by a module for JSON-RPC use.
    (ModuleAccessoryState) (AccessoryKey, Version) => AccessoryStateValue
);

impl KeyEncoder<ModuleAccessoryState> for (AccessoryKey, Version) {
    fn encode_key(&self) -> sov_schema_db::schema::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.0.len() + std::mem::size_of::<Version>() + 8);
        self.0
            .as_slice()
            .serialize(&mut out)
            .map_err(CodecError::from)?;
        // Write the version in big-endian order so that sorting order is based on the most-significant bytes of the key
        out.write_u64::<BigEndian>(self.1)
            .expect("serialization to vec is infallible");
        Ok(out)
    }
}

impl SeekKeyEncoder<ModuleAccessoryState> for (AccessoryKey, Version) {
    fn encode_seek_key(&self) -> sov_schema_db::schema::Result<Vec<u8>> {
        <(Vec<u8>, u64) as KeyEncoder<ModuleAccessoryState>>::encode_key(self)
    }
}

impl KeyDecoder<ModuleAccessoryState> for (AccessoryKey, Version) {
    fn decode_key(data: &[u8]) -> sov_schema_db::schema::Result<Self> {
        let mut cursor = maybestd::io::Cursor::new(data);
        let key = Vec::<u8>::deserialize_reader(&mut cursor)?;
        let version = cursor.read_u64::<BigEndian>()?;
        Ok((key, version))
    }
}

impl ValueCodec<ModuleAccessoryState> for AccessoryStateValue {
    fn encode_value(&self) -> sov_schema_db::schema::Result<Vec<u8>> {
        self.try_to_vec().map_err(CodecError::from)
    }

    fn decode_value(data: &[u8]) -> sov_schema_db::schema::Result<Self> {
        Ok(Self::deserialize_reader(&mut &data[..])?)
    }
}
