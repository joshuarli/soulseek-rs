//! `browse`: list the files a user shares.

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
            "Seconds to wait for the listing",
        ),
        "20",
    ),
    1,
    MAX_WAIT_SECS,
);

#[derive(Debug)]
pub struct BrowseArgs {
    pub user: String,
    pub timeout: u64,
}

impl BrowseArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            user: parsed.required_string(0)?,
            timeout: parsed.u64(&TIMEOUT)?.unwrap_or(20),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Browse,
        "browse",
        "List the files a user shares",
    )
    .positional(PositionalSpec {
        name: "USER",
        required: true,
        repeatable: false,
        help: "User whose shares to list",
    })
    .option(TIMEOUT)
}
