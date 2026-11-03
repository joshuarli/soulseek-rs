//! `account`: its password, and what this machine stores for it.

use crate::cli::command::{
    command, flag, value, with_env, CommandId, CommandSpec, OptionSpec,
    ValueKind,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;

pub const NEW_PASSWORD: OptionSpec = with_env(
    value(
        None,
        "new-password",
        "PASSWORD",
        ValueKind::Text,
        "The new password (visible in `ps`; prefer --new-password-stdin)",
    ),
    "SOULSEEK_NEW_PASSWORD",
);

pub const NEW_PASSWORD_STDIN: OptionSpec = flag(
    None,
    "new-password-stdin",
    "Read the new password from the first line of stdin (takes precedence over --new-password)",
);

#[derive(Debug)]
pub enum AccountCommand {
    Password(PasswordArgs),
    Logout,
}

impl AccountCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::AccountPassword => {
                Self::Password(PasswordArgs::from_parsed(parsed)?)
            }
            CommandId::AccountLogout => Self::Logout,
            _ => return Err(internal(id)),
        })
    }
}

#[derive(Debug)]
pub struct PasswordArgs {
    pub new_password: Option<String>,
    pub new_password_stdin: bool,
}

impl PasswordArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        Ok(Self {
            new_password: parsed.value(&NEW_PASSWORD)?,
            new_password_stdin: parsed.flag(&NEW_PASSWORD_STDIN),
        })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Account,
        "account",
        "This account: its password, and what this machine stores for it",
    )
    .subcommand(
        command(
            CommandId::AccountPassword,
            "password",
            "Change the account's password on the server",
        )
        .option(NEW_PASSWORD)
        .option(NEW_PASSWORD_STDIN),
    )
    .subcommand(command(
        CommandId::AccountLogout,
        "logout",
        "Forget the password stored on this machine, so the next start asks for it again",
    ))
    .requires_subcommand()
    .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not an account command"))
}
