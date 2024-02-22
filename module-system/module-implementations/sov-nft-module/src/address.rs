use std::fmt;

use sov_modules_macros::address_type;

/// Address representing a simple user capable of owning an NFT.
#[address_type]
pub struct UserAddress;

/// Derived Address representing an NFT collection - Derived from CreatorAddress(S::Address) and collection_name: String.
#[address_type]
pub struct CollectionAddress;

/// Address representing the owner of an NFT.
#[address_type]
pub struct OwnerAddress;

/// Address representing a creator of an NFT collection.
#[address_type]
pub struct CreatorAddress;
