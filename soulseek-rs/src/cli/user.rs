//! `user`: what the server knows about another user.

use crate::cli::command::{
    command, value, with_default, with_range, CommandId, CommandSpec, OptionSpec,
    PositionalSpec, ValueKind, MAX_WAIT_SECS,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

pub const TIMEOUT: OptionSpec = with_range(
    with_default(
        value(
            Some('t'),
            "timeout",
            "TIMEOUT",
            ValueKind::U64,
            "Seconds to wait for the server's answer",
        ),
        "10",
    ),
    1,
    MAX_WAIT_SECS,
);

#[derive(Debug)]
pub struct UserArgs {
    pub user: String,
    pub timeout: u64,
}

impl UserArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            user: parsed.required_string(0)?,
            timeout: parsed.u64(&TIMEOUT)?.unwrap_or(10),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::User,
        "user",
        "What the server knows about another user",
    )
    .positional(PositionalSpec {
        name: "USER",
        required: true,
        repeatable: false,
        help: "The user to ask the server about",
    })
    .option(TIMEOUT)
}
