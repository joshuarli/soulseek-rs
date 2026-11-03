//! `download`: fetch files by user and remote path, or from stdin.

use crate::cli::command::{
    command, flag, value, with_default, with_range, CommandId, CommandSpec,
    Constraint, OptionSpec, PositionalSpec, ValueKind, MAX_WAIT_SECS,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

const USER_NAME: &str = "USER";
const PATH_NAME: &str = "PATH";
const STDIN_CONFLICTS: &[&str] = &[USER_NAME, PATH_NAME, SIZE.long];

pub const SIZE: OptionSpec = value(
    None,
    "size",
    "SIZE",
    ValueKind::U64,
    "File size in bytes, as reported by `search` or `browse`",
);

pub const STDIN: OptionSpec = flag(
    None,
    "stdin",
    "Read records from stdin: `search --json` output, or TAB-separated `search` text lines",
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
pub struct DownloadArgs {
    pub user: Option<String>,
    pub path: Option<String>,
    pub size: Option<u64>,
    pub stdin: bool,
    pub timeout: u64,
}

impl DownloadArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            user: parsed.string(0)?,
            path: parsed.string(1)?,
            size: parsed.u64(&SIZE)?,
            stdin: parsed.flag(&STDIN),
            timeout: parsed.u64(&TIMEOUT)?.unwrap_or(300),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Download,
        "download",
        "Download files by user and remote path",
    )
    .positional(PositionalSpec {
        name: USER_NAME,
        required: false,
        repeatable: false,
        help: "Peer holding the file",
    })
    .positional(PositionalSpec {
        name: PATH_NAME,
        required: false,
        repeatable: false,
        help: "Remote path exactly as printed by `search` or `browse`",
    })
    .option(SIZE)
    .option(STDIN)
    .option(TIMEOUT)
    .constraint(Constraint::RequiredUnless {
        target: USER_NAME,
        unless: STDIN.long,
    })
    .constraint(Constraint::RequiredUnless {
        target: PATH_NAME,
        unless: STDIN.long,
    })
    .constraint(Constraint::Conflicts {
        option: STDIN.long,
        with: STDIN_CONFLICTS,
    })
}
