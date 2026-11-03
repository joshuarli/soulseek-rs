//! `message`: private messages.

use crate::cli::command::{
    command, flag, value, with_default, with_range, CommandId, CommandSpec,
    OptionSpec, PositionalSpec, ValueKind, MAX_WAIT_SECS,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

pub const DURATION: OptionSpec = with_range(
    with_default(
        value(
            None,
            "duration",
            "DURATION",
            ValueKind::U64,
            "Seconds to listen before exiting",
        ),
        "30",
    ),
    1,
    MAX_WAIT_SECS,
);

pub const FOLLOW: OptionSpec =
    flag(None, "follow", "Listen until interrupted, ignoring --duration");

#[derive(Debug)]
pub enum MessageCommand {
    Send { user: String, message: String },
    Read { duration: u64, follow: bool },
}

impl MessageCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::MessageSend => Self::Send {
                user: parsed.required_string(0)?,
                message: parsed.required_string(1)?,
            },
            CommandId::MessageRead => Self::Read {
                duration: parsed.u64(&DURATION)?.unwrap_or(30),
                follow: parsed.flag(&FOLLOW),
            },
            _ => return Err(internal(id)),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(CommandId::Message, "message", "Private messages")
        .subcommand(
            command(CommandId::MessageSend, "send", "Send a private message")
                .positional(PositionalSpec {
                    name: "USER",
                    required: true,
                    repeatable: false,
                    help: "Recipient username",
                })
                .positional(PositionalSpec {
                    name: "MESSAGE",
                    required: true,
                    repeatable: false,
                    help: "Message text",
                }),
        )
        .subcommand(
            command(
                CommandId::MessageRead,
                "read",
                "Stream incoming private messages",
            )
            .option(DURATION)
            .option(FOLLOW),
        )
        .requires_subcommand()
        .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a message command"))
}
