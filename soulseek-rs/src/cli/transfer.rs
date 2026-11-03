//! `transfer`: transfers in flight.

use crate::cli::command::{
    command, CommandId, CommandSpec, PositionalSpec,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

#[derive(Debug)]
pub enum TransferCommand {
    Cancel { user: String, path: String },
}

impl TransferCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        match id {
            CommandId::TransferCancel => Ok(Self::Cancel {
                user: parsed.required_string(0)?,
                path: parsed.required_string(1)?,
            }),
            _ => Err(internal(id)),
        }
    }
}

pub fn spec() -> CommandSpec {
    command(CommandId::Transfer, "transfer", "Transfers in flight")
        .subcommand(
            command(
                CommandId::TransferCancel,
                "cancel",
                "Stop a download or upload; a cancelled download's partial file is deleted",
            )
            .positional(PositionalSpec {
                name: "USER",
                required: true,
                repeatable: false,
                help: "The other side of the transfer",
            })
            .positional(PositionalSpec {
                name: "PATH",
                required: true,
                repeatable: false,
                help: "Remote path exactly as printed by `search`, `browse`, or `serve`",
            }),
        )
        .requires_subcommand()
        .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a transfer command"))
}
