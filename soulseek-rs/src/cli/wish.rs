//! `wish`: standing searches the server re-runs on its own schedule.

use crate::cli::command::{
    command, value, with_default, CommandId, CommandSpec, OptionSpec,
    PositionalSpec, ValueKind,
};
use crate::cli::error::ParseError;
use crate::cli::filter::{self, FilterArgs, SortKey};
use crate::cli::parse::Parsed;

pub const ONLY: OptionSpec = value(
    None,
    "only",
    "QUERY",
    ValueKind::Text,
    "Only run this wish instead of all of them",
);

pub const LIMIT: OptionSpec = with_default(
    value(
        Some('n'),
        "limit",
        "LIMIT",
        ValueKind::Usize,
        "Print at most this many results per wish (0 means all)",
    ),
    "0",
);

pub const SORT: OptionSpec = with_default(
    value(
        None,
        "sort",
        "SORT",
        ValueKind::Choice(SortKey::CHOICES),
        "Result order",
    ),
    "best",
);

#[derive(Debug)]
pub enum WishCommand {
    Add { query: String },
    Remove { query: String },
    List,
    Run(WishRunArgs),
}

impl WishCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::WishAdd => Self::Add {
                query: parsed.required_string(0)?,
            },
            CommandId::WishRemove => Self::Remove {
                query: parsed.required_string(0)?,
            },
            CommandId::WishList => Self::List,
            CommandId::WishRun => Self::Run(WishRunArgs::from_parsed(parsed)?),
            _ => return Err(internal(id)),
        })
    }
}

#[derive(Debug)]
pub struct WishRunArgs {
    pub only: Option<String>,
    pub filter: FilterArgs,
    pub limit: usize,
    pub sort: SortKey,
}

impl WishRunArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            only: parsed.value(&ONLY)?,
            filter: FilterArgs::from_parsed(parsed)?,
            limit: parsed.usize(&LIMIT)?.unwrap_or(0),
            sort: parsed.enum_value(&SORT, SortKey::Best)?,
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Wish,
        "wish",
        "Standing searches the server re-runs on its own schedule",
    )
    .subcommand(
        command(CommandId::WishAdd, "add", "Store a query and search for it from now on")
            .positional(PositionalSpec {
                name: "QUERY",
                required: true,
                repeatable: false,
                help: "What to keep looking for",
            }),
    )
    .subcommand(
        command(CommandId::WishRemove, "remove", "Stop looking for a query")
            .positional(PositionalSpec {
                name: "QUERY",
                required: true,
                repeatable: false,
                help: "The wish to drop, as shown by `wish list`",
            }),
    )
    .subcommand(command(
        CommandId::WishList,
        "list",
        "Print the stored queries",
    ))
    .subcommand(
        command(
            CommandId::WishRun,
            "run",
            "Search every stored query once and print what came back",
        )
        .option(ONLY)
        .options(filter::OPTIONS)
        .option(LIMIT)
        .option(SORT),
    )
    .requires_subcommand()
    .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a wish command"))
}
