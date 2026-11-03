//! The result restrictions shared by every command that searches.

use crate::cli::command::{
    flag, repeatable, value, OptionSpec, ValueKind,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;
use std::str::FromStr;

pub const MIN_BITRATE: OptionSpec = value(
    None,
    "min-bitrate",
    "KBPS",
    ValueKind::U64,
    "Drop results below this bitrate in kbps",
);

pub const FREE_SLOTS: OptionSpec = flag(
    None,
    "free-slots",
    "Keep only peers advertising a free upload slot",
);

pub const EXCLUDE: OptionSpec = repeatable(value(
    None,
    "exclude",
    "TERM",
    ValueKind::Text,
    "Drop results whose path contains this term; repeat for several",
));

pub const EXTENSION: OptionSpec = repeatable(value(
    None,
    "extension",
    "EXT",
    ValueKind::Text,
    "Keep only this file extension, with or without a dot; repeat for several",
));

pub const MIN_SIZE: OptionSpec = value(
    None,
    "min-size",
    "BYTES",
    ValueKind::U64,
    "Drop results smaller than this many bytes",
);

pub const MAX_SIZE: OptionSpec = value(
    None,
    "max-size",
    "BYTES",
    ValueKind::U64,
    "Drop results larger than this many bytes",
);

pub const OPTIONS: [OptionSpec; 6] = [
    MIN_BITRATE,
    FREE_SLOTS,
    EXCLUDE,
    EXTENSION,
    MIN_SIZE,
    MAX_SIZE,
];

#[derive(Clone, Debug, Default)]
pub struct FilterArgs {
    pub min_bitrate: Option<u32>,
    pub free_slots: bool,
    pub exclude: Vec<String>,
    pub extension: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
}

impl FilterArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        let min_bitrate = parsed.u32(&MIN_BITRATE)?;
        Ok(Self {
            min_bitrate,
            free_slots: parsed.flag(&FREE_SLOTS),
            exclude: parsed.values(&EXCLUDE)?,
            extension: parsed.values(&EXTENSION)?,
            min_size: parsed.u64(&MIN_SIZE)?,
            max_size: parsed.u64(&MAX_SIZE)?,
        })
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Best,
    Size,
    Bitrate,
    Speed,
}

impl SortKey {
    pub const CHOICES: &'static [&'static str] =
        &["best", "size", "bitrate", "speed"];
}

impl FromStr for SortKey {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "best" => Self::Best,
            "size" => Self::Size,
            "bitrate" => Self::Bitrate,
            "speed" => Self::Speed,
            _ => return Err(()),
        })
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    #[default]
    Best,
    First,
    All,
}

impl Pick {
    pub const CHOICES: &'static [&'static str] = &["best", "first", "all"];
}

impl FromStr for Pick {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "best" => Self::Best,
            "first" => Self::First,
            "all" => Self::All,
            _ => return Err(()),
        })
    }
}
