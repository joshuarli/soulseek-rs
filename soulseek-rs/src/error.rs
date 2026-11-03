//! A plain error type for the persistence and terminal layers.

use std::fmt;

#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    #[must_use]
    pub fn msg(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::msg(error.to_string())
    }
}

impl From<deser::Error> for Error {
    fn from(error: deser::Error) -> Self {
        Self::msg(error.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
