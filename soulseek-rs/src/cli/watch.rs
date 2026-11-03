//! `watch`: keep a daemon updated about users you care about.

use crate::cli::command::{
    command, CommandId, CommandSpec, PositionalSpec,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

#[derive(Debug)]
pub enum WatchCommand {
    Add { user: String },
    Remove { user: String },
    List,
}

impl WatchCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::WatchAdd => Self::Add {
                user: parsed.required_string(0)?,
            },
            CommandId::WatchRemove => Self::Remove {
                user: parsed.required_string(0)?,
            },
            CommandId::WatchList => Self::List,
            _ => return Err(internal(id)),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Watch,
        "watch",
        "Keep a daemon updated about users you care about",
    )
    .subcommand(
        command(CommandId::WatchAdd, "add", "Start watching a user")
            .positional(PositionalSpec {
                name: "USER",
                required: true,
                repeatable: false,
                help: "The user to watch",
            }),
    )
    .subcommand(
        command(CommandId::WatchRemove, "remove", "Stop watching a user")
            .positional(PositionalSpec {
                name: "USER",
                required: true,
                repeatable: false,
                help: "The user to stop watching",
            }),
    )
    .subcommand(command(
        CommandId::WatchList,
        "list",
        "List the users being watched",
    ))
    .requires_subcommand()
    .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a watch command"))
}
