//! Error types, mirroring the Python `DROTrimmerException` / `DROFileException`.

use thiserror::Error;

/// Anything that can go wrong inside `dro-core`.
///
/// The Python original distinguished `DROFileException` (a subclass) from the
/// `DROTrimmerException` base. Here that distinction is a variant, which callers
/// match on rather than downcast.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum Error {
    /// The bytes handed to a reader are not a valid file of the expected format.
    #[error("{0}")]
    File(String),

    /// A configuration source could not be parsed.
    #[error("{0}")]
    Config(String),
}

impl Error {
    /// Convenience constructor for [`Error::File`].
    pub fn file(message: impl Into<String>) -> Self {
        Self::File(message.into())
    }

    /// Convenience constructor for [`Error::Config`].
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}

pub type Result<T, E = Error> = core::result::Result<T, E>;
