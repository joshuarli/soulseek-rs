//! The lexopt boundary: tokens in, typed command models out.

use crate::cli::command::{
    self, CommandId, CommandSpec, Constraint, OptionSpec, PositionalSpec, Special,
    ValueKind, ValueSpec,
};
use crate::cli::error::ParseError;
use crate::cli::{help, version, Cli, Commands, Invocation};
use lexopt::{Arg, Parser};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Default)]
struct ParseState {
    values: BTreeMap<&'static str, Vec<OsString>>,
    flags: BTreeSet<&'static str>,
    counts: BTreeMap<&'static str, u8>,
    positionals: Vec<OsString>,
    path: Vec<&'static str>,
    leaf: Option<CommandSpec>,
}

enum Level {
    Command(CommandId),
    HelpPath(Vec<String>),
    Version,
}

pub struct Parsed {
    spec: CommandSpec,
    usage: String,
    values: BTreeMap<&'static str, Vec<OsString>>,
    flags: BTreeSet<&'static str>,
    counts: BTreeMap<&'static str, u8>,
    positionals: Vec<OsString>,
}

pub fn parse<I, T>(args: I) -> Result<Invocation, ParseError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut parser = Parser::from_iter(args);
    let root = command::root_spec();
    let mut state = ParseState::default();
    match parse_level(&root, &mut parser, &mut state)? {
        Level::Command(id) => {
            let spec = state.leaf.clone().unwrap_or_else(command::root_spec);
            let parsed = Parsed {
                usage: command::usage(&spec, &state.path),
                spec,
                values: state.values,
                flags: state.flags,
                counts: state.counts,
                positionals: state.positionals,
            };
            let command = if id == CommandId::Root {
                None
            } else {
                Some(Commands::from_parsed(id, &parsed)?)
            };
            Ok(Invocation::Run(Box::new(Cli::from_parsed(
                command, &parsed,
            )?)))
        }
        Level::HelpPath(path) => Ok(Invocation::Help(help::render_path(&path)?)),
        Level::Version => Ok(Invocation::Version(version::line())),
    }
}

fn parse_level(
    spec: &CommandSpec,
    parser: &mut Parser,
    state: &mut ParseState,
) -> Result<Level, ParseError> {
    loop {
        let arg = match parser.next() {
            Ok(Some(arg)) => arg,
            Ok(None) => break,
            Err(error) => return Err(lexopt_error(error, spec, state)),
        };
        match arg {
            Arg::Short(short) => {
                let option = spec.find_short(short).ok_or_else(|| {
                    unknown_option(&format!("-{short}"), spec, state)
                })?;
                if let Some(level) = handle_option(option, spec, parser, state)? {
                    return Ok(level);
                }
            }
            Arg::Long(name) => {
                let option = spec.find_long(name).ok_or_else(|| {
                    unknown_option(&format!("--{name}"), spec, state)
                })?;
                if let Some(level) = handle_option(option, spec, parser, state)? {
                    return Ok(level);
                }
            }
            Arg::Value(value) => {
                if spec.has_subcommands() {
                    let name = value.to_str().ok_or_else(|| {
                        ParseError::new(
                            "invalid UTF-8 in a subcommand name",
                        )
                        .with_usage(command::usage(spec, &state.path))
                    })?;
                    let child = spec.find_subcommand(name).ok_or_else(|| {
                        ParseError::new(format!(
                            "unrecognized subcommand '{name}'"
                        ))
                        .with_usage(command::usage(spec, &state.path))
                    })?;
                    state.path.push(child.name);
                    return parse_level(child, parser, state);
                }
                state.positionals.push(value);
            }
        }
    }

    if spec.id == CommandId::Help {
        return help_path(state);
    }

    if spec.has_subcommands() && spec.subcommand_required {
        return Err(ParseError::new(
            "a subcommand is required but none was provided",
        )
        .with_usage(command::usage(spec, &state.path)));
    }

    state.leaf = Some(spec.clone());
    apply_env(state, spec);
    validate(spec, state)?;
    Ok(Level::Command(spec.id))
}

fn help_path(state: &ParseState) -> Result<Level, ParseError> {
    let Some((_, parent)) = state.path.split_last() else {
        return Err(ParseError::new("cannot resolve the help command"));
    };
    let mut path: Vec<String> = parent
        .iter()
        .map(|segment| (*segment).to_string())
        .collect();
    for value in &state.positionals {
        let name = value.to_str().ok_or_else(|| {
            ParseError::new("invalid UTF-8 in a help command name")
        })?;
        path.push(name.to_string());
    }
    Ok(Level::HelpPath(path))
}

fn handle_option(
    option: &OptionSpec,
    spec: &CommandSpec,
    parser: &mut Parser,
    state: &mut ParseState,
) -> Result<Option<Level>, ParseError> {
    match option.special {
        Some(Special::Help) => {
            return Ok(Some(Level::HelpPath(
                state.path.iter().map(|name| (*name).to_string()).collect(),
            )));
        }
        Some(Special::Version) => return Ok(Some(Level::Version)),
        None => {}
    }

    if option.value.is_some() {
        let value = parser.value().map_err(|error| {
            lexopt_error(error, spec, state)
        })?;
        if !option.repeatable && state.values.contains_key(option.long) {
            return Err(ParseError::new(format!(
                "the argument '{}' cannot be used multiple times",
                command::option_label(option)
            ))
            .with_usage(command::usage(spec, &state.path)));
        }
        state
            .values
            .entry(option.long)
            .or_default()
            .push(value);
        return Ok(None);
    }

    if option.count {
        let count = state.counts.entry(option.long).or_insert(0);
        *count = count.saturating_add(1);
    } else {
        state.flags.insert(option.long);
    }
    Ok(None)
}

fn apply_env(state: &mut ParseState, spec: &CommandSpec) {
    for option in spec.all_options() {
        let Some(name) = option.env else {
            continue;
        };
        let Some(value) = std::env::var_os(name) else {
            continue;
        };
        if option.value.is_some() {
            if !state.values.contains_key(option.long) {
                state.values.insert(option.long, vec![value]);
            }
        } else if !state.flags.contains(option.long) && env_truthy(&value) {
            state.flags.insert(option.long);
        }
    }
}

fn env_truthy(value: &OsStr) -> bool {
    let Some(text) = value.to_str() else {
        return true;
    };
    !matches!(
        text.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

fn validate(spec: &CommandSpec, state: &ParseState) -> Result<(), ParseError> {
    let usage = command::usage(spec, &state.path);
    let last_repeats = spec
        .positionals
        .last()
        .is_some_and(|positional| positional.repeatable);
    if state.positionals.len() > spec.positionals.len() && !last_repeats {
        let extra = &state.positionals[spec.positionals.len()];
        return Err(ParseError::new(format!(
            "unexpected argument '{}' found",
            show(extra)
        ))
        .with_usage(usage));
    }

    for (index, positional) in spec.positionals.iter().enumerate() {
        if positional.required && index >= state.positionals.len() {
            return Err(missing_positional(positional, &usage));
        }
    }

    for constraint in &spec.constraints {
        match *constraint {
            Constraint::RequiredUnless { target, unless } => {
                if !has_named(spec, state, target)
                    && !has_named(spec, state, unless)
                {
                    let positional = spec
                        .positionals
                        .iter()
                        .find(|positional| positional.name == target);
                    return Err(match positional {
                        Some(positional) => {
                            missing_positional(positional, &usage)
                        }
                        None => ParseError::new(format!(
                            "the argument '{target}' is required unless \
                             '{unless}' is used"
                        ))
                        .with_usage(usage),
                    });
                }
            }
            Constraint::Conflicts { option, with } => {
                if !has_named(spec, state, option) {
                    continue;
                }
                if let Some(conflict) = with
                    .iter()
                    .find(|name| has_named(spec, state, name))
                {
                    return Err(ParseError::new(format!(
                        "the argument '{}' cannot be used with '{}'",
                        named_label(spec, option),
                        named_label(spec, conflict)
                    ))
                    .with_usage(usage));
                }
            }
        }
    }
    Ok(())
}

fn missing_positional(
    positional: &PositionalSpec,
    usage: &str,
) -> ParseError {
    ParseError::new(format!(
        "the following required arguments were not provided:\n  <{}>",
        positional.name
    ))
    .with_usage(usage)
}

fn has_named(spec: &CommandSpec, state: &ParseState, name: &str) -> bool {
    state.values.contains_key(name)
        || state.flags.contains(name)
        || state.counts.contains_key(name)
        || spec
            .positionals
            .iter()
            .position(|positional| positional.name == name)
            .is_some_and(|index| index < state.positionals.len())
}

fn named_label(spec: &CommandSpec, name: &str) -> String {
    if let Some(option) = spec.find_long(name) {
        return command::option_label(option);
    }
    if let Some(positional) = spec
        .positionals
        .iter()
        .find(|positional| positional.name == name)
    {
        return format!("<{}>", positional.name);
    }
    name.to_string()
}

fn unknown_option(
    spelling: &str,
    spec: &CommandSpec,
    state: &ParseState,
) -> ParseError {
    ParseError::new(format!("unexpected argument '{spelling}' found"))
        .with_usage(command::usage(spec, &state.path))
}

fn lexopt_error(
    error: lexopt::Error,
    spec: &CommandSpec,
    state: &ParseState,
) -> ParseError {
    use lexopt::Error;
    let message = match error {
        Error::MissingValue { option: Some(option) } => {
            format!("a value is required for '{option}' but none was supplied")
        }
        Error::MissingValue { option: None } => {
            "a value is required but none was supplied".to_string()
        }
        Error::UnexpectedValue { option, value } => format!(
            "unexpected value '{}' for '{option}' found; no more were expected",
            show(&value)
        ),
        Error::UnexpectedOption(option) => {
            format!("unexpected argument '{option}' found")
        }
        Error::UnexpectedArgument(value) => {
            format!("unexpected argument '{}' found", show(&value))
        }
        other => other.to_string(),
    };
    ParseError::new(message).with_usage(command::usage(spec, &state.path))
}

#[allow(clippy::unnecessary_debug_formatting)]
fn show(value: &OsStr) -> String {
    value
        .to_str()
        .map_or_else(|| format!("{value:?}"), ToString::to_string)
}

impl Parsed {
    #[must_use]
    pub const fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    #[must_use]
    pub fn flag(&self, spec: &OptionSpec) -> bool {
        self.flags.contains(spec.long)
    }

    #[must_use]
    pub fn count(&self, spec: &OptionSpec) -> u8 {
        self.counts.get(spec.long).copied().unwrap_or(0)
    }

    pub fn value(
        &self,
        spec: &OptionSpec,
    ) -> Result<Option<String>, ParseError> {
        let Some(raw) = self.values.get(spec.long).and_then(|values| {
            values.last()
        }) else {
            return Ok(None);
        };
        let text = raw.to_str().ok_or_else(|| {
            self.invalid(format!("invalid UTF-8 in '{}'", spec.long))
        })?;
        self.validate_choice(spec, text)?;
        Ok(Some(text.to_string()))
    }

    pub fn values(
        &self,
        spec: &OptionSpec,
    ) -> Result<Vec<String>, ParseError> {
        let Some(values) = self.values.get(spec.long) else {
            return Ok(Vec::new());
        };
        values
            .iter()
            .map(|raw| {
                let text = raw.to_str().ok_or_else(|| {
                    self.invalid(format!("invalid UTF-8 in '{}'", spec.long))
                })?;
                self.validate_choice(spec, text)?;
                Ok(text.to_string())
            })
            .collect()
    }

    pub fn path(&self, spec: &OptionSpec) -> Option<PathBuf> {
        self.values
            .get(spec.long)
            .and_then(|values| values.last())
            .map(PathBuf::from)
    }

    pub fn paths(&self, spec: &OptionSpec) -> Vec<PathBuf> {
        self.values
            .get(spec.long)
            .map(|values| values.iter().map(PathBuf::from).collect())
            .unwrap_or_default()
    }

    pub fn u64(&self, spec: &OptionSpec) -> Result<Option<u64>, ParseError> {
        self.number(spec)
    }

    pub fn u32(&self, spec: &OptionSpec) -> Result<Option<u32>, ParseError> {
        self.number(spec)
    }

    pub fn u16(&self, spec: &OptionSpec) -> Result<Option<u16>, ParseError> {
        self.number(spec)
    }

    pub fn u8(&self, spec: &OptionSpec) -> Result<Option<u8>, ParseError> {
        self.number(spec)
    }

    pub fn usize(&self, spec: &OptionSpec) -> Result<Option<usize>, ParseError> {
        self.number(spec)
    }

    fn number<T>(&self, spec: &OptionSpec) -> Result<Option<T>, ParseError>
    where
        T: TryFrom<u64>,
    {
        let Some(text) = self.value(spec)? else {
            return Ok(None);
        };
        let parsed = text.parse::<u64>().map_err(|_| {
            self.invalid(format!(
                "invalid value '{text}' for '{}': expected a number",
                command::option_label(spec)
            ))
        })?;
        if let Some(value) = spec.value
            && let Some(range) = value.range
            && (parsed < range.min || parsed > range.max)
        {
            return Err(self.range_error(spec, &text, range));
        }
        T::try_from(parsed)
            .map(Some)
            .map_err(|_| self.invalid(format!(
                "invalid value '{text}' for '{}'",
                command::option_label(spec)
            )))
    }

    fn range_error(
        &self,
        spec: &OptionSpec,
        text: &str,
        range: command::Range,
    ) -> ParseError {
        self.invalid(format!(
            "invalid value '{text}' for '{}': {text} is not in {}..={}",
            command::option_label(spec),
            range.min,
            range.max
        ))
    }

    fn validate_choice(
        &self,
        spec: &OptionSpec,
        text: &str,
    ) -> Result<(), ParseError> {
        let Some(ValueSpec {
            kind: ValueKind::Choice(choices),
            ..
        }) = spec.value
        else {
            return Ok(());
        };
        if choices.contains(&text) {
            return Ok(());
        }
        Err(self.invalid(format!(
            "invalid value '{text}' for '{}': possible values: {}",
            command::option_label(spec),
            choices.join(", ")
        )))
    }

    fn invalid(&self, message: String) -> ParseError {
        ParseError::new(message).with_usage(self.usage.clone())
    }

    #[must_use]
    pub fn positional(&self, index: usize) -> Option<&OsStr> {
        self.positionals.get(index).map(OsString::as_os_str)
    }

    pub fn string(&self, index: usize) -> Result<Option<String>, ParseError> {
        let Some(raw) = self.positionals.get(index) else {
            return Ok(None);
        };
        raw.to_str().map(|text| Some(text.to_string())).ok_or_else(|| {
            self.invalid(format!(
                "invalid UTF-8 in <{}>",
                self.spec
                    .positionals
                    .get(index)
                    .map_or("ARGUMENT", |positional| positional.name)
            ))
        })
    }

    pub fn required_string(&self, index: usize) -> Result<String, ParseError> {
        self.string(index)?.ok_or_else(|| {
            let name = self
                .spec
                .positionals
                .get(index)
                .map_or("ARGUMENT", |positional| positional.name);
            self.invalid(format!(
                "the following required arguments were not provided:\n  <{name}>"
            ))
        })
    }

    pub fn choice(
        &self,
        spec: &OptionSpec,
    ) -> Result<Option<String>, ParseError> {
        self.value(spec)
    }

    pub fn enum_value<T: FromStr>(
        &self,
        spec: &OptionSpec,
        default: T,
    ) -> Result<T, ParseError> {
        match self.value(spec)? {
            Some(text) => text.parse().map_err(|_| {
                self.invalid(format!(
                    "invalid value '{text}' for '{}'",
                    command::option_label(spec)
                ))
            }),
            None => Ok(default),
        }
    }

}
