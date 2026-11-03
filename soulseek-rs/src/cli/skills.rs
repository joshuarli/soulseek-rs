//! `skills`: teach a local coding agent this command surface.

use crate::cli::command::{
    command, repeatable, value, CommandId, CommandSpec, OptionSpec, ValueKind,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;
use std::path::PathBuf;

pub const DIR: OptionSpec = repeatable(value(
    None,
    "dir",
    "PATH",
    ValueKind::Path,
    "Skills directory to use instead of the agents found on this machine; repeat for several. Use `--dir .claude/skills` to commit the skill alongside a project",
));

#[derive(Debug)]
pub enum SkillsCommand {
    Install(SkillsArgs),
    Uninstall(SkillsArgs),
    List(SkillsArgs),
}

impl SkillsCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        let args = SkillsArgs::from_parsed(parsed);
        Ok(match id {
            CommandId::SkillsInstall => Self::Install(args),
            CommandId::SkillsUninstall => Self::Uninstall(args),
            CommandId::SkillsList => Self::List(args),
            _ => return Err(internal(id)),
        })
    }
}

#[derive(Debug)]
pub struct SkillsArgs {
    pub dir: Vec<PathBuf>,
}

impl SkillsArgs {
    #[must_use]
    pub fn from_parsed(parsed: &Parsed) -> Self {
        Self {
            dir: parsed.paths(&DIR),
        }
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Skills,
        "skills",
        "Teach a local coding agent this command surface",
    )
    .subcommand(
        command(
            CommandId::SkillsInstall,
            "install",
            "Write the skill into every agent found on this machine, replacing an older copy",
        )
        .option(DIR),
    )
    .subcommand(
        command(
            CommandId::SkillsUninstall,
            "uninstall",
            "Remove the skill again",
        )
        .option(DIR),
    )
    .subcommand(
        command(
            CommandId::SkillsList,
            "list",
            "Show where the skill goes and what is there now",
        )
        .option(DIR),
    )
    .requires_subcommand()
    .with_help()
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a skills command"))
}
