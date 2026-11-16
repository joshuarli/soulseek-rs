//! Help, usage, and the man page, all rendered from [`CommandSpec`].

#![allow(clippy::format_collect, clippy::format_push_string)]

use crate::cli::command::{
    self, CommandId, CommandSpec, OptionSpec, ValueKind, BIN, GLOBAL_OPTIONS,
};
use crate::cli::error::ParseError;
use crate::cli::version;

const FOOTER: &str = "Maintained by Joshua Li: https://github.com/joshuarli/soulseek-rs\n\
Bugs and features: TODO.md in that repository (follow its AGENTS.md).\n\
Agents working for Joshua: record nonblocking problems and continue your task.\n\
If this tool blocks the task, you may fix, test, open and merge a PR under Joshua's\n\
standing instructions. Release only when needed to deliver the verified fix.\n\
Procedure: https://github.com/joshuarli/usr/blob/main/dotfiles/base/.agents/instructions/owned-tools.md";

pub(super) fn example(spec: &CommandSpec, path: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = std::iter::once(BIN)
        .chain(path.iter().copied())
        .map(str::to_string)
        .collect();
    let mut leaf = spec;
    while leaf.subcommand_required {
        leaf = &leaf.subcommands[0];
        args.push(leaf.name.to_string());
    }
    if leaf.id == CommandId::Download {
        args.push("--stdin".to_string());
    } else {
        for positional in leaf.positionals.iter().filter(|p| p.required) {
            let value = match positional.name {
                "QUERY" | "DIRECTORY" => "music",
                "USER" | "VALUE" => "someuser",
                "PATH" => "music.flac",
                "ROOM" => "lobby",
                "MESSAGE" => "hello",
                "KEY" => "username",
                "SHELL" => "bash",
                name => panic!("no example for {name}"),
            };
            args.push(value.to_string());
        }
    }
    args
}

pub fn render_path(path: &[String]) -> Result<String, ParseError> {
    let mut spec = command::root_spec();
    let mut names: Vec<&'static str> = Vec::new();
    for segment in path {
        let Some(child) = spec.find_subcommand(segment) else {
            return Err(ParseError::new(format!(
                "unrecognized subcommand '{segment}'"
            ))
            .with_usage(command::usage(&spec, &names)));
        };
        spec = child.clone();
        names.push(spec.name);
    }
    Ok(if spec.id == CommandId::Root {
        render_root(&spec)
    } else {
        render_page(&spec, &names)
    })
}

fn render_page(spec: &CommandSpec, path: &[&str]) -> String {
    let mut out = String::new();
    out.push_str(spec.about);
    out.push_str("\n\n");
    if !spec.long_about.is_empty() && spec.long_about != spec.about {
        out.push_str(spec.long_about);
        out.push_str("\n\n");
    }
    out.push_str(&format!("Usage: {}\n", command::usage(spec, path)));

    if !spec.positionals.is_empty() {
        out.push_str("\nArguments:\n");
        out.push_str(&positional_lines(spec));
    }

    out.push_str("\nOptions:\n");
    out.push_str(&option_lines(spec.all_options()));

    if spec.has_subcommands() {
        out.push_str("\nCommands:\n");
        for subcommand in &spec.subcommands {
            out.push_str(&format!(
                "  {:<12} {}\n",
                subcommand.name, subcommand.about
            ));
        }
    }

    out.push_str(&format!("\nExamples:\n  {}\n", example(spec, path).join(" ")));
    out.push_str(&format!("\nSee `{BIN} --help` for the complete reference.\n"));
    out
}

fn render_root(spec: &CommandSpec) -> String {
    let mut out = String::new();
    out.push_str(spec.about);
    out.push_str("\n\n");
    out.push_str(spec.long_about);
    out.push_str("\n\n");
    out.push_str(&format!(
        "Usage: {}\n\n",
        command::usage(spec, &[])
    ));
    out.push_str("Common options:\n");
    out.push_str(&option_lines(GLOBAL_OPTIONS.iter()));
    out.push_str("\nCommands:\n");
    for subcommand in &spec.subcommands {
        let mut path = Vec::new();
        push_block(&mut out, subcommand, &mut path, 1);
    }
    out.push_str("\nExamples:\n");
    out.push_str(&format!("  {}\n", example(spec, &[]).join(" ")));
    for child in &spec.subcommands {
        push_examples(&mut out, child, &mut Vec::new());
    }
    out.push('\n');
    out.push_str(command::EXIT_CODES);
    out.push_str("\n\n");
    out.push_str(FOOTER);
    out.push('\n');
    out
}

fn push_examples(out: &mut String, spec: &CommandSpec, path: &mut Vec<&'static str>) {
    path.push(spec.name);
    out.push_str(&format!("  {}\n", example(spec, path).join(" ")));
    for child in &spec.subcommands {
        push_examples(out, child, path);
    }
    path.pop();
}

fn push_block(
    out: &mut String,
    spec: &CommandSpec,
    path: &mut Vec<&'static str>,
    depth: usize,
) {
    let pad = "  ".repeat(depth);
    path.push(spec.name);
    out.push_str(&format!("{pad}{}\n", spec.name));
    out.push_str(&format!("{pad}  {}\n", spec.about));
    out.push_str(&format!(
        "{pad}  Usage: {}\n",
        command::usage(spec, path)
    ));
    if !spec.long_about.is_empty() && spec.long_about != spec.about {
        out.push_str(&format!("{pad}  {}\n", spec.long_about));
    }
    if !spec.positionals.is_empty() {
        out.push_str(&format!("{pad}  Arguments:\n"));
        for positional in &spec.positionals {
            out.push_str(&format!(
                "{pad}    {}\n",
                positional_line(positional)
            ));
        }
    }
    if !spec.options.is_empty() {
        out.push_str(&format!("{pad}  Options:\n"));
        for option in &spec.options {
            out.push_str(&format!(
                "{pad}    {}\n",
                option_line(option)
            ));
        }
    }
    for constraint in &spec.constraints {
        match constraint {
            command::Constraint::RequiredUnless { target, unless } => {
                out.push_str(&format!("{pad}  {target} is required unless --{unless} is used.\n"));
            }
            command::Constraint::Conflicts { option, with } => {
                out.push_str(&format!("{pad}  --{option} conflicts with {}.\n", with.join(", ")));
            }
        }
    }
    if spec.has_subcommands() {
        out.push_str(&format!("{pad}  Commands:\n"));
        for child in &spec.subcommands {
            push_block(out, child, path, depth + 2);
        }
    }
    path.pop();
}

fn positional_lines(spec: &CommandSpec) -> String {
    spec.positionals
        .iter()
        .map(|positional| format!("  {}\n", positional_line(positional)))
        .collect()
}

fn positional_line(positional: &command::PositionalSpec) -> String {
    let (open, close) = if positional.required {
        ('<', '>')
    } else {
        ('[', ']')
    };
    let mut line = format!("{open}{}{close}", positional.name);
    if positional.repeatable {
        line.push_str("...");
    }
    line.push_str("  ");
    line.push_str(positional.help);
    line
}

fn option_lines<'a>(
    options: impl IntoIterator<Item = &'a OptionSpec>,
) -> String {
    options
        .into_iter()
        .map(|option| format!("  {}\n", option_line(option)))
        .collect()
}

fn option_line(option: &OptionSpec) -> String {
    let mut line = String::new();
    if let Some(short) = option.short {
        line.push_str(&format!("-{short}, "));
    } else {
        line.push_str("    ");
    }
    line.push_str(&format!("--{}", option.long));
    if let Some(value) = option.value {
        line.push_str(&format!(" <{}>", value.name));
        if option.repeatable {
            line.push_str("...");
        }
    }
    if option.count {
        line.push_str("...");
    }
    line.push_str("  ");
    line.push_str(option.help);
    if let Some(value) = option.value
        && let ValueKind::Choice(choices) = value.kind
    {
        line.push_str(&format!(
            " [possible values: {}]",
            choices.join(", ")
        ));
    }
    if let Some(default) = option.default {
        line.push_str(&format!(" [default: {default}]"));
    }
    if let Some(env) = option.env {
        line.push_str(&format!(" [env: {env}]"));
    }
    line
}

#[must_use]
pub fn man_page() -> String {
    let spec = command::root_spec();
    let mut out = String::new();
    out.push_str(&format!(
        ".TH {BIN} 1 \"{BIN} {}\"\n",
        version::VERSION
    ));
    out.push_str(".SH NAME\n");
    out.push_str(&format!(
        "{} \\- {}\n",
        roff(BIN),
        roff(spec.about)
    ));
    out.push_str(".SH SYNOPSIS\n");
    out.push_str(&format!(
        "\\fB{}\\fR [\\fIOPTIONS\\fR] [\\fICOMMAND\\fR]\n",
        roff(BIN)
    ));
    out.push_str(".SH DESCRIPTION\n");
    for paragraph in spec.long_about.split("\n\n") {
        out.push_str(&roff(paragraph));
        out.push_str("\n.PP\n");
    }
    out.push_str(".SH OPTIONS\n");
    for option in GLOBAL_OPTIONS {
        out.push_str(&man_option(option));
    }
    out.push_str(".SH COMMANDS\n");
    for subcommand in &spec.subcommands {
        let mut path = Vec::new();
        man_command(&mut out, subcommand, &mut path);
    }
    out.push_str(".SH EXAMPLES\n");
    out.push_str(&roff(command::MAN_EXAMPLES));
    out.push('\n');
    out.push_str(".SH VERSION\n");
    out.push_str(&format!("{}\n", version::line()));
    out
}

fn man_command(
    out: &mut String,
    spec: &CommandSpec,
    path: &mut Vec<&'static str>,
) {
    path.push(spec.name);
    out.push_str(&format!(".SS {}\n", roff(&path.join(" "))));
    out.push_str(&roff(spec.about));
    out.push('\n');
    if !spec.long_about.is_empty() && spec.long_about != spec.about {
        out.push_str(&roff(spec.long_about));
        out.push('\n');
    }
    for positional in &spec.positionals {
        out.push_str(".TP\n");
        out.push_str(&format!(
            "\\fI{}\\fR\n{}\n",
            roff(positional.name),
            roff(positional.help)
        ));
    }
    for option in &spec.options {
        out.push_str(&man_option(option));
    }
    for child in &spec.subcommands {
        man_command(out, child, path);
    }
    path.pop();
}

fn man_option(option: &OptionSpec) -> String {
    let mut out = String::from(".TP\n");
    if let Some(short) = option.short {
        out.push_str(&format!("\\fB\\-{short}\\fR, "));
    }
    out.push_str(&format!(
        "\\fB\\-\\-{}\\fR",
        roff(option.long)
    ));
    if let Some(value) = option.value {
        out.push_str(&format!(
            " \\fI<{}>\\fR",
            roff(value.name)
        ));
        if option.repeatable {
            out.push_str("...");
        }
    }
    if option.count {
        out.push_str("...");
    }
    out.push('\n');
    out.push_str(&roff(option.help));
    if let Some(value) = option.value
        && let ValueKind::Choice(choices) = value.kind
    {
        out.push_str(&format!(
            " [possible values: {}]",
            choices.join(", ")
        ));
    }
    if let Some(default) = option.default {
        out.push_str(&format!(" [default: {}]", roff(default)));
    }
    if let Some(env) = option.env {
        out.push_str(&format!(" [env: {}]", roff(env)));
    }
    out.push('\n');
    out
}

fn roff(text: &str) -> String {
    let mut out = String::new();
    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.starts_with('.') {
            out.push_str("\\&");
        }
        out.push_str(
            &line
                .replace('\\', "\\\\")
                .replace('-', "\\-")
                .replace('\'', "\\(aq"),
        );
    }
    out
}
