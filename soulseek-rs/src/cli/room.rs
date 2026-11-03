//! `room`: chat rooms.

use crate::cli::command::{
    command, flag, value, with_default, with_range, CommandId, CommandSpec,
    OptionSpec, PositionalSpec, ValueKind, MAX_WAIT_SECS,
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
            "Seconds to wait for the list",
        ),
        "5",
    ),
    1,
    MAX_WAIT_SECS,
);

pub const USERS_TIMEOUT: OptionSpec = with_range(
    with_default(
        value(
            Some('t'),
            "timeout",
            "TIMEOUT",
            ValueKind::U64,
            "Seconds to wait for the membership list",
        ),
        "10",
    ),
    1,
    MAX_WAIT_SECS,
);

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
pub enum RoomCommand {
    List { timeout: u64 },
    Say { room: String, message: String },
    Users { room: String, timeout: u64 },
    Listen { room: String, duration: u64, follow: bool },
}

impl RoomCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::RoomList => Self::List {
                timeout: parsed.u64(&TIMEOUT)?.unwrap_or(5),
            },
            CommandId::RoomSay => Self::Say {
                room: parsed.required_string(0)?,
                message: parsed.required_string(1)?,
            },
            CommandId::RoomUsers => Self::Users {
                room: parsed.required_string(0)?,
                timeout: parsed.u64(&USERS_TIMEOUT)?.unwrap_or(10),
            },
            CommandId::RoomListen => Self::Listen {
                room: parsed.required_string(0)?,
                duration: parsed.u64(&DURATION)?.unwrap_or(30),
                follow: parsed.flag(&FOLLOW),
            },
            _ => return Err(internal(id)),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(CommandId::Room, "room", "Chat rooms")
        .subcommand(
            command(
                CommandId::RoomList,
                "list",
                "List public rooms and their user counts",
            )
            .option(TIMEOUT),
        )
        .subcommand(
            command(CommandId::RoomSay, "say", "Post a message to a room")
                .positional(PositionalSpec {
                    name: "ROOM",
                    required: true,
                    repeatable: false,
                    help: "Room to post in",
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
                CommandId::RoomUsers,
                "users",
                "List who is currently in a room",
            )
            .positional(PositionalSpec {
                name: "ROOM",
                required: true,
                repeatable: false,
                help: "Room whose members to list",
            })
            .option(USERS_TIMEOUT),
        )
        .subcommand(
            command(
                CommandId::RoomListen,
                "listen",
                "Stream messages and joins/leaves from a room",
            )
            .positional(PositionalSpec {
                name: "ROOM",
                required: true,
                repeatable: false,
                help: "Room to listen to",
            })
            .option(DURATION)
            .option(FOLLOW),
        )
        .requires_subcommand()
        .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a room command"))
}
