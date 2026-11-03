//! Argument errors: a message naming the problem and, where there is a
//! command to point at, its usage line.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub usage: Option<String>,
}

impl ParseError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            usage: None,
        }
    }

    #[must_use]
    pub fn with_usage(mut self, usage: impl Into<String>) -> Self {
        self.usage = Some(usage.into());
        self
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.message)?;
        if let Some(usage) = &self.usage {
            writeln!(f)?;
            writeln!(f, "Usage: {usage}")?;
        }
        write!(f, "\nFor more information, try '--help'.")
    }
}

impl std::error::Error for ParseError {}
