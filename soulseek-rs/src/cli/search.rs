//! `search`: query the network and print matching files.

use crate::cli::command::{
    command, value, with_default, CommandId, CommandSpec, OptionSpec,
    PositionalSpec, ValueKind,
};
use crate::cli::error::ParseError;
use crate::cli::filter::{self, FilterArgs, SortKey};
use crate::cli::parse::Parsed;

pub const LIMIT: OptionSpec = with_default(
    value(
        Some('n'),
        "limit",
        "LIMIT",
        ValueKind::Usize,
        "Print at most this many results (0 means all)",
    ),
    "0",
);

pub const SORT: OptionSpec = with_default(
    value(
        None,
        "sort",
        "SORT",
        ValueKind::Choice(SortKey::CHOICES),
        "Result order; best prefers lossless, then bit depth, sample rate, bitrate",
    ),
    "best",
);

#[derive(Debug)]
pub struct SearchArgs {
    pub query: String,
    pub filter: FilterArgs,
    pub limit: usize,
    pub sort: SortKey,
}

impl SearchArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            query: parsed.required_string(0)?,
            filter: FilterArgs::from_parsed(parsed)?,
            limit: parsed.usize(&LIMIT)?.unwrap_or(0),
            sort: parsed.enum_value(&SORT, SortKey::Best)?,
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Search,
        "search",
        "Search the network and print matching files",
    )
    .positional(PositionalSpec {
        name: "QUERY",
        required: true,
        repeatable: false,
        help: "What to search for",
    })
    .options(filter::OPTIONS)
    .option(LIMIT)
    .option(SORT)
}
