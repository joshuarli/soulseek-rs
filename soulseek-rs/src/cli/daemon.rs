//! `daemon`: run as a service other clients connect to, or control one that is.

use crate::cli::command::{
    command, value, with_range, CommandId, CommandSpec, OptionSpec, ValueKind,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

pub const BIND: OptionSpec = value(
    None,
    "bind",
    "ADDR",
    ValueKind::Text,
    "Also accept control connections on this address, for clients on other machines. They need the token; the local socket does not.",
);

pub const UPLOAD_SLOTS: OptionSpec = with_range(
    value(
        None,
        "upload-slots",
        "UPLOAD_SLOTS",
        ValueKind::U16,
        "How many uploads to serve at once",
    ),
    1,
    1000,
);

#[derive(Debug)]
pub struct DaemonArgs {
    pub command: Option<DaemonCommand>,
    pub bind: Option<String>,
    pub upload_slots: Option<u16>,
}

#[derive(Debug)]
pub enum DaemonCommand {
    Token,
    Status,
    Stop,
}

impl DaemonArgs {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        let command = match id {
            CommandId::Daemon => None,
            CommandId::DaemonToken => Some(DaemonCommand::Token),
            CommandId::DaemonStatus => Some(DaemonCommand::Status),
            CommandId::DaemonStop => Some(DaemonCommand::Stop),
            _ => return Err(internal(id)),
        };
        Ok(Self {
            command,
            bind: parsed.value(&BIND)?,
            upload_slots: parsed.u16(&UPLOAD_SLOTS)?,
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Daemon,
        "daemon",
        "Run as a service other clients connect to, or control one that is",
    )
    .option(BIND)
    .option(UPLOAD_SLOTS)
    .subcommand(command(
        CommandId::DaemonToken,
        "token",
        "Print the token remote clients authenticate with",
    ))
    .subcommand(command(
        CommandId::DaemonStatus,
        "status",
        "What a running daemon is doing",
    ))
    .subcommand(command(
        CommandId::DaemonStop,
        "stop",
        "Ask a running daemon to shut down",
    ))
    .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a daemon command"))
}
