//! `serve`: stay online sharing files, reporting each upload as it happens.

use crate::cli::command::{
    command, flag, value, with_default, with_range, CommandId, CommandSpec,
    OptionSpec, ValueKind, MAX_WAIT_SECS,
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
            "Seconds to stay online before exiting",
        ),
        "3600",
    ),
    1,
    MAX_WAIT_SECS,
);

pub const FOLLOW: OptionSpec =
    flag(None, "follow", "Stay online until interrupted, ignoring --duration");

pub const WISHLIST: OptionSpec = flag(
    None,
    "wishlist",
    "Also re-run the wishlist on the interval the server sets, reporting matches as they arrive",
);

pub const UPLOAD_SLOTS: OptionSpec = with_range(
    value(
        None,
        "upload-slots",
        "N",
        ValueKind::U8,
        "How many uploads to run at once [default: 10]; the rest queue, privileged users first",
    ),
    1,
    64,
);

#[derive(Debug)]
pub struct ServeArgs {
    pub duration: u64,
    pub follow: bool,
    pub wishlist: bool,
    pub upload_slots: Option<u8>,
}

impl ServeArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            duration: parsed.u64(&DURATION)?.unwrap_or(3600),
            follow: parsed.flag(&FOLLOW),
            wishlist: parsed.flag(&WISHLIST),
            upload_slots: parsed.u8(&UPLOAD_SLOTS)?,
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Serve,
        "serve",
        "Stay online sharing files, reporting each upload as it happens",
    )
    .option(DURATION)
    .option(FOLLOW)
    .option(WISHLIST)
    .option(UPLOAD_SLOTS)
}
