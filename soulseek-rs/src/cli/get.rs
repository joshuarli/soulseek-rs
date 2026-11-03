//! `get`: search, pick automatically, and download in one step.

use crate::cli::command::{
    command, value, with_default, with_range, CommandId, CommandSpec, OptionSpec,
    PositionalSpec, ValueKind, MAX_WAIT_SECS,
};
use crate::cli::error::ParseError;
use crate::cli::filter::{self, FilterArgs, Pick};
use crate::cli::parse::Parsed;

pub const PICK: OptionSpec = with_default(
    value(
        None,
        "pick",
        "PICK",
        ValueKind::Choice(Pick::CHOICES),
        "Which results to download",
    ),
    "best",
);

pub const LIMIT: OptionSpec = with_default(
    value(
        Some('n'),
        "limit",
        "LIMIT",
        ValueKind::Usize,
        "With `--pick all`, download at most this many (0 means all)",
    ),
    "0",
);

pub const TIMEOUT: OptionSpec = with_range(
    with_default(
        value(
            Some('t'),
            "timeout",
            "TIMEOUT",
            ValueKind::U64,
            "Seconds to wait for each transfer before giving up",
        ),
        "300",
    ),
    1,
    MAX_WAIT_SECS,
);

#[derive(Debug)]
pub struct GetArgs {
    pub query: String,
    pub pick: Pick,
    pub limit: usize,
    pub filter: FilterArgs,
    pub timeout: u64,
}

impl GetArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            query: parsed.required_string(0)?,
            pick: parsed.enum_value(&PICK, Pick::Best)?,
            limit: parsed.usize(&LIMIT)?.unwrap_or(0),
            filter: FilterArgs::from_parsed(parsed)?,
            timeout: parsed.u64(&TIMEOUT)?.unwrap_or(300),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Get,
        "get",
        "Search, pick automatically, and download in one step",
    )
    .positional(PositionalSpec {
        name: "QUERY",
        required: true,
        repeatable: false,
        help: "What to search for",
    })
    .option(PICK)
    .option(LIMIT)
    .options(filter::OPTIONS)
    .option(TIMEOUT)
}
