//! `shares`: the folders this client shares.

use crate::cli::command::{
    command, value, CommandId, CommandSpec, OptionSpec, PositionalSpec,
    ValueKind,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

pub const FILTER: OptionSpec = value(
    None,
    "filter",
    "FILTER",
    ValueKind::Text,
    "Only paths containing this text, case-insensitively",
);

#[derive(Debug)]
pub enum SharesCommand {
    List,
    Add { directory: String },
    Remove { directory: String },
    Files { filter: Option<String> },
    Reindex,
    Status,
}

impl SharesCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::SharesList => Self::List,
            CommandId::SharesAdd => Self::Add {
                directory: parsed.required_string(0)?,
            },
            CommandId::SharesRemove => Self::Remove {
                directory: parsed.required_string(0)?,
            },
            CommandId::SharesFiles => Self::Files {
                filter: parsed.value(&FILTER)?,
            },
            CommandId::SharesReindex => Self::Reindex,
            CommandId::SharesStatus => Self::Status,
            _ => return Err(internal(id)),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(CommandId::Shares, "shares", "The folders this client shares")
        .subcommand(command(
            CommandId::SharesList,
            "list",
            "List the configured share folders",
        ))
        .subcommand(
            command(
                CommandId::SharesAdd,
                "add",
                "Add a folder to the shares and save it to the config file",
            )
            .positional(PositionalSpec {
                name: "DIRECTORY",
                required: true,
                repeatable: false,
                help: "Folder to start sharing",
            }),
        )
        .subcommand(
            command(
                CommandId::SharesRemove,
                "remove",
                "Stop sharing a folder and save that to the config file",
            )
            .positional(PositionalSpec {
                name: "DIRECTORY",
                required: true,
                repeatable: false,
                help: "Folder to stop sharing",
            }),
        )
        .subcommand(
            command(
                CommandId::SharesFiles,
                "files",
                "List every indexed file, the way `browse` lists a peer's",
            )
            .option(FILTER),
        )
        .subcommand(command(
            CommandId::SharesReindex,
            "reindex",
            "Re-index the shares and report what the network will see",
        ))
        .subcommand(command(
            CommandId::SharesStatus,
            "status",
            "Report how many folders and files are indexed",
        ))
        .requires_subcommand()
        .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a shares command"))
}
