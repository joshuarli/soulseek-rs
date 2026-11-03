//! `config`: read and write the config file.

use crate::cli::command::{
    command, CommandId, CommandSpec, PositionalSpec,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

#[derive(Debug)]
pub enum ConfigCommand {
    Path,
    List,
    Get { key: String },
    Set { key: String, value: String },
}

impl ConfigCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::ConfigPath => Self::Path,
            CommandId::ConfigList => Self::List,
            CommandId::ConfigGet => Self::Get {
                key: parsed.required_string(0)?,
            },
            CommandId::ConfigSet => Self::Set {
                key: parsed.required_string(0)?,
                value: parsed.required_string(1)?,
            },
            _ => return Err(internal(id)),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(CommandId::Config, "config", "Read and write the config file")
        .subcommand(command(
            CommandId::ConfigPath,
            "path",
            "Print the path of the config file in use",
        ))
        .subcommand(command(
            CommandId::ConfigList,
            "list",
            "Print every setting and its effective value",
        ))
        .subcommand(
            command(CommandId::ConfigGet, "get", "Print one setting")
                .positional(PositionalSpec {
                    name: "KEY",
                    required: true,
                    repeatable: false,
                    help: "Setting name, as shown by `config list`",
                }),
        )
        .subcommand(
            command(
                CommandId::ConfigSet,
                "set",
                "Change one setting in the config file",
            )
            .positional(PositionalSpec {
                name: "KEY",
                required: true,
                repeatable: false,
                help: "Setting name, as shown by `config list`",
            })
            .positional(PositionalSpec {
                name: "VALUE",
                required: true,
                repeatable: false,
                help: "New value; an empty string clears it",
            }),
        )
        .requires_subcommand()
        .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a config command"))
}
