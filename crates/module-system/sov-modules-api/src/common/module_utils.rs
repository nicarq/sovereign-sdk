/// We may use [`NotInstantiable`] type for modules that do not support calls.
///
/// ## Details
/// This is a simple struct that implements all the necessary call message traits
/// and that can be used as a placeholder for modules that do not support calls.
#[cfg_attr(
    feature = "native",
    derive(schemars::JsonSchema),
    derive(crate::macros::UniversalWallet),
    universal_wallet(sov_modules_api_path = crate),
)]
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
pub enum NotInstantiable {}

impl borsh::BorshDeserialize for NotInstantiable {
    // It is impossible to deserialize to NotInstantiable.
    fn deserialize_reader<R: std::io::prelude::Read>(
        _reader: &mut R,
    ) -> Result<Self, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "NotInstantiable is not instantiable",
        ))
    }
}

impl borsh::BorshSerialize for NotInstantiable {
    // Since it impossible to have a value of NotInstantiable this code is unreachable.
    fn serialize<W: std::io::Write>(&self, _writer: &mut W) -> Result<(), std::io::Error> {
        unreachable!()
    }
}
