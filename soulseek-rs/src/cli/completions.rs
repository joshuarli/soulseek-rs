//! `completions`: shell completion scripts, generated from the command schema.

#![allow(clippy::format_push_string)]

use crate::cli::command::{
    self, command, repeatable, value, CommandId, CommandSpec, OptionSpec,
    ValueKind, BIN, GLOBAL_OPTIONS,
};
use crate::cli::error::ParseError;
use crate::cli::parse::Parsed;
use std::collections::BTreeSet;
use std::str::FromStr;

pub const SHELL: OptionSpec = repeatable(value(
    None,
    "shell",
    "SHELL",
    ValueKind::Choice(Shell::CHOICES),
    "Shell to act on instead of the ones found on this machine; repeat for several",
));

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    pub const CHOICES: &'static [&'static str] = &["bash", "zsh", "fish"];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
        }
    }
}

impl FromStr for Shell {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "bash" => Self::Bash,
            "zsh" => Self::Zsh,
            "fish" => Self::Fish,
            _ => return Err(()),
        })
    }
}

#[derive(Debug)]
pub enum CompletionsCommand {
    Install(CompletionsArgs),
    Uninstall(CompletionsArgs),
    Print { shell: Shell },
}

impl CompletionsCommand {
    pub fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::CompletionsInstall => {
                Self::Install(CompletionsArgs::from_parsed(parsed)?)
            }
            CommandId::CompletionsUninstall => {
                Self::Uninstall(CompletionsArgs::from_parsed(parsed)?)
            }
            CommandId::CompletionsPrint => {
                let shell = parsed.required_string(0)?;
                Self::Print {
                    shell: shell.parse().map_err(|()| {
                        ParseError::new(format!(
                            "invalid value '{shell}' for '<SHELL>': possible \
                             values: {}",
                            Shell::CHOICES.join(", ")
                        ))
                    })?,
                }
            }
            _ => return Err(internal(id)),
        })
    }
}

#[derive(Debug)]
pub struct CompletionsArgs {
    pub shell: Vec<Shell>,
}

impl CompletionsArgs {
    pub fn from_parsed(parsed: &Parsed) -> Result<Self, ParseError> {
        let mut shell = Vec::new();
        for value in parsed.values(&SHELL)? {
            shell.push(value.parse().map_err(|()| {
                ParseError::new(format!(
                    "invalid value '{value}' for '--shell <SHELL>'"
                ))
            })?);
        }
        Ok(Self { shell })
    }
}

pub fn spec() -> CommandSpec {
    command(
        CommandId::Completions,
        "completions",
        "Tab completion for bash, zsh, and fish",
    )
    .subcommand(
        command(
            CommandId::CompletionsInstall,
            "install",
            "Write the completion script where the shell looks for it",
        )
        .option(SHELL),
    )
    .subcommand(
        command(
            CommandId::CompletionsUninstall,
            "uninstall",
            "Remove the completion script again",
        )
        .option(SHELL),
    )
    .subcommand(
        command(
            CommandId::CompletionsPrint,
            "print",
            "Print the completion script to stdout, for packaging",
        )
        .positional(command::PositionalSpec {
            name: "SHELL",
            required: true,
            repeatable: false,
            help: "Shell to print",
        }),
    )
    .requires_subcommand()
    .with_help()
}

#[must_use]
pub fn script(shell: Shell) -> String {
    let root = command::root_spec();
    match shell {
        Shell::Bash => bash(&root),
        Shell::Zsh => zsh(&root),
        Shell::Fish => fish(&root),
    }
}

fn bash(root: &CommandSpec) -> String {
    let common = words(GLOBAL_OPTIONS.iter()).join(" ");
    let commands = root
        .subcommands
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::new();
    out.push_str("_soulseek-rs() {\n");
    out.push_str("    local cur prev\n");
    out.push_str("    COMPREPLY=()\n");
    out.push_str("    cur=\"${COMP_WORDS[COMP_CWORD]}\"\n");
    out.push_str("    prev=\"${COMP_WORDS[COMP_CWORD-1]}\"\n");
    out.push_str(&format!("    local common=\"{common}\"\n"));
    out.push_str(&format!("    local commands=\"{commands}\"\n"));
    out.push_str("    local opts=\"$common\"\n");
    out.push_str("    case \"${COMP_WORDS[1]}\" in\n");
    for subcommand in &root.subcommands {
        bash_case(&mut out, subcommand, 2, 2);
    }
    out.push_str(
        "        \"\") COMPREPLY=( $(compgen -W \"$commands $common\" -- \"$cur\") ); return;;\n",
    );
    out.push_str("    esac\n");
    out.push_str("    case \"$prev\" in\n");
    for (long, short, kind) in value_options(root) {
        let mut spellings = vec![format!("--{long}")];
        if let Some(short) = short {
            spellings.push(format!("-{short}"));
        }
        out.push_str(&format!(
            "        {})\n",
            spellings.join("|")
        ));
        match kind {
            ValueKind::Path => out.push_str(
                "            COMPREPLY=( $(compgen -f -- \"$cur\") ); return;;\n",
            ),
            ValueKind::Choice(choices) => out.push_str(&format!(
                "            COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") ); return;;\n",
                choices.join(" ")
            )),
            _ => out.push_str("            return;;\n"),
        }
    }
    out.push_str("    esac\n");
    out.push_str("    COMPREPLY=( $(compgen -W \"$opts\" -- \"$cur\") )\n");
    out.push_str("}\n");
    out.push_str(&format!("complete -F _soulseek-rs {BIN}\n"));
    out
}

fn bash_case(
    out: &mut String,
    spec: &CommandSpec,
    word: usize,
    indent: usize,
) {
    let pad = "    ".repeat(indent);
    if spec.subcommands.is_empty() {
        out.push_str(&format!(
            "{pad}{}) opts=\"$common {}\";;\n",
            spec.name,
            words(spec.options.iter()).join(" ")
        ));
        return;
    }

    out.push_str(&format!("{pad}{})\n", spec.name));
    out.push_str(&format!(
        "{pad}    case \"${{COMP_WORDS[{word}]}}\" in\n"
    ));
    for child in &spec.subcommands {
        bash_case(out, child, word + 1, indent + 2);
    }
    let names = spec
        .subcommands
        .iter()
        .map(|child| child.name)
        .collect::<Vec<_>>()
        .join(" ");
    let local = words(spec.options.iter()).join(" ");
    out.push_str(&format!(
        "{pad}        \"\") COMPREPLY=( $(compgen -W \"{names} $common {local}\" -- \"$cur\") ); return;;\n"
    ));
    out.push_str(&format!("{pad}    esac;;\n"));
}

fn zsh(root: &CommandSpec) -> String {
    let mut out = String::from("#compdef soulseek-rs\n\n_soulseek-rs() {\n");
    out.push_str("    local -a commands\n");
    out.push_str("    commands=(\n");
    for subcommand in &root.subcommands {
        out.push_str(&format!(
            "        '{}:{}'\n",
            subcommand.name,
            zsh_quote(subcommand.about)
        ));
    }
    out.push_str("    )\n");
    out.push_str("    _arguments -C \\\n");
    out.push_str("        '(-h --help)'{-h,--help}'[Print help]' \\\n");
    out.push_str(
        "        '(-V --version)'{-V,--version}'[Print version]' \\\n",
    );
    for option in GLOBAL_OPTIONS.iter().filter(|option| option.special.is_none()) {
        out.push_str(&format!("        {} \\\n", zsh_option(option)));
    }
    out.push_str("        '1:command:->command' \\\n");
    out.push_str("        '*::arg:->args'\n");
    out.push_str("    case $state in\n");
    out.push_str("        command)\n");
    out.push_str("            _describe 'command' commands\n");
    out.push_str("            ;;\n");
    out.push_str("        args)\n");
    out.push_str("            case $words[1] in\n");
    for subcommand in &root.subcommands {
        zsh_case(&mut out, subcommand, 1);
    }
    out.push_str("            esac\n");
    out.push_str("            ;;\n");
    out.push_str("    esac\n");
    out.push_str("}\n");
    out.push_str("compdef _soulseek-rs soulseek-rs\n");
    out
}

fn zsh_case(out: &mut String, spec: &CommandSpec, depth: usize) {
    let pad = "    ".repeat(depth + 2);
    out.push_str(&format!("{pad}{})\n", spec.name));
    if spec.subcommands.is_empty() {
        let mut options = words_zsh(GLOBAL_OPTIONS.iter());
        options.extend(words_zsh(spec.options.iter()));
        out.push_str(&format!("{pad}    _arguments {}\n", options.join(" ")));
        out.push_str(&format!("{pad}    ;;\n"));
        return;
    }
    out.push_str(&format!(
        "{pad}    case $words[{}] in\n",
        depth + 1
    ));
    for child in &spec.subcommands {
        if child.subcommands.is_empty() {
            let mut options = words_zsh(GLOBAL_OPTIONS.iter());
            options.extend(words_zsh(spec.options.iter()));
            options.extend(words_zsh(child.options.iter()));
            out.push_str(&format!(
                "{pad}        {}) _arguments {} ;;\n",
                child.name,
                options.join(" ")
            ));
        } else {
            zsh_case(out, child, depth + 1);
        }
    }
    let names = spec
        .subcommands
        .iter()
        .map(|child| child.name)
        .collect::<Vec<_>>()
        .join(" ");
    out.push_str(&format!(
        "{pad}        *) _values 'command' {names} ;;\n"
    ));
    out.push_str(&format!("{pad}    esac\n"));
    out.push_str(&format!("{pad}    ;;\n"));
}

fn fish(root: &CommandSpec) -> String {
    let mut out = String::from("complete -c soulseek-rs -f\n");
    for option in GLOBAL_OPTIONS.iter().filter(|option| option.special.is_none()) {
        out.push_str(&fish_option(option, None));
    }
    fish_commands(&mut out, root, &[]);
    out
}

fn fish_commands(out: &mut String, spec: &CommandSpec, path: &[&str]) {
    for subcommand in &spec.subcommands {
        let condition = if path.is_empty() {
            " -n '__fish_use_subcommand'".to_string()
        } else {
            fish_condition(path)
        };
        out.push_str(&format!(
            "complete -c {BIN}{condition} -a {} -d '{}'\n",
            subcommand.name,
            fish_quote(subcommand.about)
        ));
        let mut child_path = path.to_vec();
        child_path.push(subcommand.name);
        for option in &subcommand.options {
            out.push_str(&fish_option(option, Some(&child_path)));
        }
        if subcommand.has_subcommands() {
            fish_commands(out, subcommand, &child_path);
        }
    }
}

fn fish_option(option: &OptionSpec, path: Option<&[&str]>) -> String {
    let condition = path.map_or_else(String::new, fish_condition);
    let mut line = format!("complete -c {BIN}{condition}");
    if let Some(short) = option.short {
        line.push_str(&format!(" -s {short}"));
    }
    line.push_str(&format!(" -l {}", option.long));
    if let Some(value) = option.value {
        line.push_str(" -r");
        if value.kind == ValueKind::Path {
            line.push_str(" -F");
        }
        if let ValueKind::Choice(choices) = value.kind {
            line.push_str(&format!(" -a '{}'", choices.join(" ")));
        }
    }
    line.push_str(&format!(" -d '{}'\n", fish_quote(option.help)));
    line
}

fn fish_condition(path: &[&str]) -> String {
    if path.is_empty() {
        return String::new();
    }
    let conditions = path
        .iter()
        .map(|segment| format!("__fish_seen_subcommand_from {segment}"))
        .collect::<Vec<_>>()
        .join("; and ");
    format!(" -n '{conditions}'")
}

fn value_options(
    root: &CommandSpec,
) -> Vec<(&'static str, Option<char>, ValueKind)> {
    let mut seen = BTreeSet::new();
    let mut values = Vec::new();
    collect_value_options(root, &mut seen, &mut values);
    values
}

fn collect_value_options(
    spec: &CommandSpec,
    seen: &mut BTreeSet<&'static str>,
    values: &mut Vec<(&'static str, Option<char>, ValueKind)>,
) {
    for option in spec.all_options() {
        if let Some(value) = option.value
            && seen.insert(option.long)
        {
            values.push((option.long, option.short, value.kind));
        }
    }
    for child in &spec.subcommands {
        collect_value_options(child, seen, values);
    }
}

fn words<'a>(
    options: impl IntoIterator<Item = &'a OptionSpec>,
) -> Vec<String> {
    let mut words = Vec::new();
    for option in options {
        words.push(format!("--{}", option.long));
        if let Some(short) = option.short {
            words.push(format!("-{short}"));
        }
    }
    words
}

fn words_zsh<'a>(
    options: impl IntoIterator<Item = &'a OptionSpec>,
) -> Vec<String> {
    options.into_iter().map(zsh_option).collect()
}

fn zsh_option(option: &OptionSpec) -> String {
    let help = zsh_quote(option.help);
    let value = match option.value {
        Some(value) => match value.kind {
            ValueKind::Choice(choices) => {
                format!(":{}:({})", value.name, choices.join(" "))
            }
            ValueKind::Path => format!(":{}:_files", value.name),
            _ => format!(":{}:", value.name),
        },
        None => String::new(),
    };
    match option.short {
        Some(short) => format!(
            "'(-{short} --{})'{{-{short},--{}}}'[{help}]{value}'",
            option.long, option.long
        ),
        None => format!("'--{}[{help}]{value}'", option.long),
    }
}

fn zsh_quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\'', "'\\''")
}

fn fish_quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\'', "\\'")
}

fn internal(id: CommandId) -> ParseError {
    ParseError::new(format!("internal: {id:?} is not a completions command"))
}
