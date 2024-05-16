//! Module error definitions.

/// A bech32 address parse error.
#[derive(Debug, thiserror::Error)]
pub enum Bech32ParseError {
    /// Bech32 decoding error represented via [bech32::primitives::decode::CheckedHrpstringError].
    #[error("Bech32 error: {0}")]
    Bech32(#[from] bech32::primitives::decode::CheckedHrpstringError),
    /// The provided "Human-Readable Part" is invalid.
    #[error("Wrong HRP: {0}")]
    WrongHRP(String),
}

/// General error type in the Module System.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    /// Custom error thrown by a module.
    #[error(transparent)]
    ModuleError(#[from] anyhow::Error),
}
