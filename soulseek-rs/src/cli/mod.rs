//! Command-line surface.
//!
//! Two modes share one binary: no subcommand launches the interactive TUI,
//! any subcommand runs a one-shot operation designed for scripts and agents.
//! The one-shot half follows three rules — stdout carries data only, stderr
//! carries progress, and the exit code carries the verdict (see
//! [`crate::output::Exit`]).

mod account;
mod browse;
pub(crate) mod command;
mod completions;
mod config;
mod daemon;
mod download;
mod error;
mod filter;
mod get;
mod help;
mod message;
mod parse;
mod room;
mod search;
mod serve;
mod shares;
mod skills;
mod transfer;
mod user;
mod version;
mod watch;
mod wish;

pub use account::{AccountCommand, PasswordArgs};
pub use browse::BrowseArgs;
pub use command::MAX_WAIT_SECS;
pub use completions::{CompletionsArgs, CompletionsCommand, Shell};
pub(crate) use completions::script as completion_script;
pub(crate) use help::man_page;
pub use config::ConfigCommand;
pub use daemon::{DaemonArgs, DaemonCommand};
pub use download::DownloadArgs;
pub use error::ParseError;
pub use filter::{FilterArgs, Pick, SortKey};
pub use get::GetArgs;
pub use message::MessageCommand;
pub use room::RoomCommand;
pub use search::SearchArgs;
pub use serve::ServeArgs;
pub use shares::SharesCommand;
pub use skills::{SkillsArgs, SkillsCommand};
pub use transfer::TransferCommand;
pub use user::UserArgs;
pub use version::VERSION;
pub use watch::WatchCommand;
pub use wish::{WishCommand, WishRunArgs};

use command::CommandId;
use parse::Parsed;
use std::path::PathBuf;

/// The parsed command line, or an early successful exit.
pub enum Invocation {
    Run(Box<Cli>),
    Help(String),
    Version(String),
}

/// Every global option, plus the command chosen.
// A flag is a bool; a command line with several switches is not a struct that
// wants splitting up.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
pub struct Cli {
    pub command: Option<Commands>,

    pub username: Option<String>,
    pub password: Option<String>,
    pub password_stdin: bool,
    pub password_cmd: Option<String>,
    pub server: Option<String>,
    pub no_listener: bool,
    pub listener: bool,
    pub listener_port: Option<u16>,
    pub download_dir: Option<String>,
    pub shared_dir: Vec<String>,
    pub max_concurrent_downloads: Option<usize>,
    pub search_timeout: Option<u64>,
    pub daemon: Option<String>,
    pub daemon_token: Option<String>,
    pub no_daemon: bool,
    pub config: Option<PathBuf>,
    pub no_config: bool,
    pub json: bool,
    pub quiet: bool,
    pub verbose: u8,
    pub log_file: Option<PathBuf>,
}

impl Cli {
    fn from_parsed(
        command: Option<Commands>,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(Self {
            command,
            username: parsed.value(&command::USERNAME)?,
            password: parsed.value(&command::PASSWORD)?,
            password_stdin: parsed.flag(&command::PASSWORD_STDIN),
            password_cmd: parsed.value(&command::PASSWORD_CMD)?,
            server: parsed.value(&command::SERVER)?,
            no_listener: parsed.flag(&command::NO_LISTENER),
            listener: parsed.flag(&command::LISTENER),
            listener_port: parsed.u16(&command::LISTENER_PORT)?,
            download_dir: parsed.value(&command::DOWNLOAD_DIR)?,
            shared_dir: parsed.values(&command::SHARED_DIR)?,
            max_concurrent_downloads: parsed
                .usize(&command::MAX_CONCURRENT_DOWNLOADS)?,
            search_timeout: parsed.u64(&command::SEARCH_TIMEOUT)?,
            daemon: parsed.value(&command::DAEMON)?,
            daemon_token: parsed.value(&command::DAEMON_TOKEN)?,
            no_daemon: parsed.flag(&command::NO_DAEMON),
            config: parsed.path(&command::CONFIG),
            no_config: parsed.flag(&command::NO_CONFIG),
            json: parsed.flag(&command::JSON),
            quiet: parsed.flag(&command::QUIET),
            verbose: parsed.count(&command::VERBOSE),
            log_file: parsed.path(&command::LOG_FILE),
        })
    }
}

/// Non-exhaustive: this enum is public through the crate's library target so
/// the integration tests can reach it, and a new subcommand must not read as a
/// breaking change of that library.
#[derive(Debug)]
#[non_exhaustive]
pub enum Commands {
    Search(SearchArgs),
    Download(DownloadArgs),
    Transfer(TransferCommand),
    Get(GetArgs),
    Browse(BrowseArgs),
    Wish(WishCommand),
    Room(RoomCommand),
    Message(MessageCommand),
    Serve(ServeArgs),
    Daemon(DaemonArgs),
    User(UserArgs),
    Watch(WatchCommand),
    Whoami,
    Account(AccountCommand),
    Shares(SharesCommand),
    Config(ConfigCommand),
    Portmap,
    Skills(SkillsCommand),
    Completions(CompletionsCommand),
    Man,
}

impl Commands {
    fn from_parsed(
        id: CommandId,
        parsed: &Parsed,
    ) -> Result<Self, ParseError> {
        Ok(match id {
            CommandId::Search => Self::Search(SearchArgs::from_parsed(parsed)?),
            CommandId::Download => {
                Self::Download(DownloadArgs::from_parsed(parsed)?)
            }
            CommandId::Transfer
            | CommandId::TransferCancel => {
                Self::Transfer(TransferCommand::from_parsed(id, parsed)?)
            }
            CommandId::Get => Self::Get(GetArgs::from_parsed(parsed)?),
            CommandId::Browse => Self::Browse(BrowseArgs::from_parsed(parsed)?),
            CommandId::Wish
            | CommandId::WishAdd
            | CommandId::WishRemove
            | CommandId::WishList
            | CommandId::WishRun => {
                Self::Wish(WishCommand::from_parsed(id, parsed)?)
            }
            CommandId::Room
            | CommandId::RoomList
            | CommandId::RoomSay
            | CommandId::RoomUsers
            | CommandId::RoomListen => {
                Self::Room(RoomCommand::from_parsed(id, parsed)?)
            }
            CommandId::Message | CommandId::MessageSend | CommandId::MessageRead => {
                Self::Message(MessageCommand::from_parsed(id, parsed)?)
            }
            CommandId::Serve => Self::Serve(ServeArgs::from_parsed(parsed)?),
            CommandId::Daemon
            | CommandId::DaemonToken
            | CommandId::DaemonStatus
            | CommandId::DaemonStop => {
                Self::Daemon(DaemonArgs::from_parsed(id, parsed)?)
            }
            CommandId::User => Self::User(UserArgs::from_parsed(parsed)?),
            CommandId::Watch
            | CommandId::WatchAdd
            | CommandId::WatchRemove
            | CommandId::WatchList => {
                Self::Watch(WatchCommand::from_parsed(id, parsed)?)
            }
            CommandId::Whoami => Self::Whoami,
            CommandId::Account
            | CommandId::AccountPassword
            | CommandId::AccountLogout => {
                Self::Account(AccountCommand::from_parsed(id, parsed)?)
            }
            CommandId::Shares
            | CommandId::SharesList
            | CommandId::SharesAdd
            | CommandId::SharesRemove
            | CommandId::SharesFiles
            | CommandId::SharesReindex
            | CommandId::SharesStatus => {
                Self::Shares(SharesCommand::from_parsed(id, parsed)?)
            }
            CommandId::Config
            | CommandId::ConfigPath
            | CommandId::ConfigList
            | CommandId::ConfigGet
            | CommandId::ConfigSet => {
                Self::Config(ConfigCommand::from_parsed(id, parsed)?)
            }
            CommandId::Portmap => Self::Portmap,
            CommandId::Skills
            | CommandId::SkillsInstall
            | CommandId::SkillsUninstall
            | CommandId::SkillsList => {
                Self::Skills(SkillsCommand::from_parsed(id, parsed)?)
            }
            CommandId::Completions
            | CommandId::CompletionsInstall
            | CommandId::CompletionsUninstall
            | CommandId::CompletionsPrint => {
                Self::Completions(CompletionsCommand::from_parsed(id, parsed)?)
            }
            CommandId::Man => Self::Man,
            CommandId::Root | CommandId::Help => {
                return Err(ParseError::new(format!(
                    "internal: {id:?} is not a runnable command"
                )));
            }
        })
    }
}

/// Parse the full command line, including the binary name.
pub fn parse<I, T>(args: I) -> Result<Invocation, ParseError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    parse::parse(args)
}

/// Split `host:port`, the only shape the server setting accepts.
pub fn parse_server_address(server: &str) -> Result<(String, u16), String> {
    let (host, port) = server.rsplit_once(':').ok_or_else(|| {
        format!("Invalid server '{server}': expected host:port")
    })?;
    if host.is_empty() {
        return Err(format!("Invalid server '{server}': missing host"));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("Invalid server '{server}': bad port '{port}'"))?;
    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, Commands, Invocation, MessageCommand, ParseError, Pick, RoomCommand,
        Shell, SkillsCommand, command, completions, help, parse,
        parse_server_address, version,
    };
    use crate::cli::command::CommandSpec;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn parsed(args: &[&str]) -> Cli {
        match parse(args.iter().map(ToString::to_string))
            .expect("should parse")
        {
            Invocation::Run(cli) => *cli,
            _ => panic!("expected a command"),
        }
    }

    fn error(args: &[&str]) -> ParseError {
        match parse(args.iter().map(ToString::to_string)) {
            Err(error) => error,
            Ok(_) => panic!("expected a parse error"),
        }
    }

    fn specs(
        spec: &CommandSpec,
        path: &mut Vec<&'static str>,
        out: &mut Vec<(CommandSpec, Vec<&'static str>)>,
    ) {
        out.push((spec.clone(), path.clone()));
        for child in &spec.subcommands {
            path.push(child.name);
            specs(child, path, out);
            path.pop();
        }
    }

    #[test]
    fn no_subcommand_means_tui() {
        assert!(parsed(&["soulseek-rs"]).command.is_none());
    }

    #[test]
    fn global_flags_parse_after_the_subcommand() {
        let cli = parsed(&[
            "soulseek-rs",
            "search",
            "query",
            "--username",
            "alice",
            "--json",
            "--search-timeout",
            "30",
        ]);
        assert_eq!(cli.username.as_deref(), Some("alice"));
        assert!(cli.json);
        assert_eq!(cli.search_timeout, Some(30));
    }

    #[test]
    fn global_flags_parse_before_the_subcommand() {
        let cli =
            parsed(&["soulseek-rs", "--username", "alice", "browse", "bob"]);
        assert_eq!(cli.username.as_deref(), Some("alice"));
        assert!(matches!(cli.command, Some(Commands::Browse(_))));
    }

    #[test]
    fn shared_dir_repeats_into_a_list() {
        let cli =
            parsed(&["soulseek-rs", "--shared-dir", "/a", "--shared-dir", "/b"]);
        assert_eq!(cli.shared_dir, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn download_requires_user_and_path_without_stdin() {
        assert!(parse(["soulseek-rs", "download"]).is_err());
        assert!(parse(["soulseek-rs", "download", "bob"]).is_err());
        assert!(
            parse(["soulseek-rs", "download", "bob", "a\\b.mp3"]).is_ok()
        );
    }

    #[test]
    fn download_stdin_needs_no_positionals() {
        let cli = parsed(&["soulseek-rs", "download", "--stdin"]);
        let Some(Commands::Download(args)) = cli.command else {
            panic!("expected download");
        };
        assert!(args.stdin);
        assert!(args.user.is_none());
    }

    #[test]
    fn download_stdin_conflicts_with_positionals() {
        assert!(
            parse(["soulseek-rs", "download", "--stdin", "bob", "a.mp3"])
                .is_err()
        );
        assert!(
            parse(["soulseek-rs", "download", "--stdin", "--size", "4"])
                .is_err()
        );
    }

    #[test]
    fn room_and_message_groups_parse() {
        assert!(matches!(
            parsed(&["soulseek-rs", "room", "list"]).command,
            Some(Commands::Room(RoomCommand::List { .. }))
        ));
        assert!(matches!(
            parsed(&["soulseek-rs", "room", "say", "lobby", "hi"]).command,
            Some(Commands::Room(RoomCommand::Say { .. }))
        ));
        assert!(matches!(
            parsed(&["soulseek-rs", "message", "read", "--follow"]).command,
            Some(Commands::Message(MessageCommand::Read { follow: true, .. }))
        ));
    }

    #[test]
    fn skills_dir_repeats_into_a_list() {
        let cli = parsed(&[
            "soulseek-rs",
            "skills",
            "install",
            "--dir",
            "/a",
            "--dir",
            "/b",
        ]);
        let Some(Commands::Skills(SkillsCommand::Install(args))) = cli.command
        else {
            panic!("expected skills install");
        };
        assert_eq!(args.dir, [PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn get_defaults_to_picking_the_best_result() {
        let cli = parsed(&["soulseek-rs", "get", "query"]);
        let Some(Commands::Get(args)) = cli.command else {
            panic!("expected get");
        };
        assert_eq!(args.pick, Pick::Best);
        assert_eq!(args.limit, 0);
        assert_eq!(args.timeout, 300);
    }

    #[test]
    fn an_absurd_wait_is_rejected_rather_than_overflowing_a_deadline() {
        let too_long = error(&[
            "soulseek-rs",
            "download",
            "bob",
            "a.mp3",
            "--timeout",
            "99999999999999",
        ]);
        assert!(too_long.to_string().contains("1..=86400"), "{too_long}");
        let zero = error(&[
            "soulseek-rs",
            "--search-timeout",
            "0",
            "search",
            "q",
        ]);
        assert!(zero.to_string().contains("1..=86400"), "{zero}");
        assert!(
            parse(["soulseek-rs", "browse", "bob", "-t", "20"]).is_ok()
        );
    }

    #[test]
    fn server_address_splits_host_and_port() {
        assert_eq!(
            parse_server_address("server.slsknet.org:2416"),
            Ok(("server.slsknet.org".to_string(), 2416))
        );
        assert!(parse_server_address("noport").is_err());
        assert!(parse_server_address("host:70000").is_err());
        assert!(parse_server_address(":2416").is_err());
    }

    #[test]
    fn long_options_accept_equals_and_short_options_accept_clusters() {
        let cli = parsed(&[
            "soulseek-rs",
            "-qv",
            "--username=alice",
            "search",
            "q",
            "--limit=5",
        ]);
        assert!(cli.quiet);
        assert_eq!(cli.verbose, 1);
        assert_eq!(cli.username.as_deref(), Some("alice"));
        let Some(Commands::Search(args)) = cli.command else {
            panic!("expected search");
        };
        assert_eq!(args.limit, 5);
    }

    #[test]
    fn a_double_dash_keeps_option_like_positionals() {
        let cli = parsed(&[
            "soulseek-rs",
            "room",
            "say",
            "lobby",
            "--",
            "-hello",
        ]);
        let Some(Commands::Room(RoomCommand::Say { message, .. })) = cli.command
        else {
            panic!("expected room say");
        };
        assert_eq!(message, "-hello");
    }

    #[test]
    fn an_unknown_option_is_a_usage_error() {
        let error = error(&["soulseek-rs", "--definitely-not-a-flag"]);
        assert!(error.to_string().contains("unexpected argument"));
        assert!(error.usage.is_some());
    }

    #[test]
    fn an_unknown_command_is_a_usage_error() {
        let error = error(&["soulseek-rs", "nope"]);
        assert!(error.to_string().contains("unrecognized subcommand"));
    }

    #[test]
    fn missing_and_invalid_values_name_the_option() {
        let missing =
            error(&["soulseek-rs", "browse", "bob", "--timeout"]);
        assert!(missing.to_string().contains("--timeout"));
        let invalid = error(&[
            "soulseek-rs",
            "search",
            "q",
            "--sort",
            "nope",
        ]);
        assert!(invalid.to_string().contains("possible values"));
        let duplicate = error(&[
            "soulseek-rs",
            "browse",
            "bob",
            "-t",
            "5",
            "--timeout",
            "6",
        ]);
        assert!(duplicate.to_string().contains("cannot be used multiple times"));
    }

    #[test]
    fn short_and_long_help_work_before_required_arguments() {
        for args in [
            vec!["soulseek-rs", "search", "-h"],
            vec!["soulseek-rs", "room", "list", "--help"],
            vec!["soulseek-rs", "room", "-h"],
        ] {
            match parse(args.iter().map(ToString::to_string)) {
                Ok(Invocation::Help(help)) => {
                    assert!(help.contains("Usage:"), "{help}");
                }
                _ => panic!("expected help for {args:?}"),
            }
        }
    }

    #[test]
    fn version_is_an_early_success() {
        for args in [vec!["soulseek-rs", "--version"], vec!["soulseek-rs", "-V"]]
        {
            match parse(args.iter().map(ToString::to_string)) {
                Ok(Invocation::Version(version)) => {
                    assert!(version.contains("(git "), "{version}");
                }
                _ => panic!("expected version for {args:?}"),
            }
        }
    }

    #[test]
    fn help_is_available_for_every_registered_command() {
        let mut all = Vec::new();
        specs(&command::root_spec(), &mut Vec::new(), &mut all);
        assert!(all.len() > 40);
        for (spec, path) in all {
            let help = help::render_path(
                &path.iter().map(|name| (*name).to_string()).collect::<Vec<_>>(),
            )
            .expect("registered command");
            let usage = command::usage(&spec, &path);
            assert!(
                help.contains(&usage),
                "{} should show usage {usage}, got:\n{help}",
                spec.name
            );
            assert!(!spec.about.is_empty(), "{} has no description", spec.name);

            let mut args = vec!["soulseek-rs".to_string()];
            args.extend(path.iter().map(|name| (*name).to_string()));
            args.push("--help".to_string());
            match parse(args) {
                Ok(Invocation::Help(help)) => assert!(
                    help.contains(&usage),
                    "{} --help should show usage {usage}",
                    path.join(" ")
                ),
                _ => panic!("{} --help should be a successful early exit", path.join(" ")),
            }
        }
    }

    #[test]
    fn the_help_subcommand_resolves_nested_paths() {
        match parse(["soulseek-rs", "help", "room", "list"]) {
            Ok(Invocation::Help(help)) => {
                assert!(help.contains("soulseek-rs room list"));
            }
            _ => panic!("expected help"),
        }
    }

    #[test]
    fn the_schema_has_unique_names_and_explained_options() {
        fn check(spec: &CommandSpec) {
            let mut names = BTreeSet::new();
            for child in &spec.subcommands {
                assert!(names.insert(child.name), "duplicate command {}", child.name);
            }
            let mut longs = BTreeSet::new();
            let mut shorts = BTreeSet::new();
            for option in spec.all_options() {
                assert!(
                    longs.insert(option.long),
                    "duplicate option --{} under {}",
                    option.long,
                    spec.name
                );
                if let Some(short) = option.short {
                    assert!(
                        shorts.insert(short),
                        "duplicate option -{short} under {}",
                        spec.name
                    );
                }
                assert!(!option.help.is_empty(), "--{} has no help", option.long);
            }
            for positional in &spec.positionals {
                assert!(
                    !positional.help.is_empty(),
                    "{} has no help",
                    positional.name
                );
            }
            for child in &spec.subcommands {
                check(child);
            }
        }
        check(&command::root_spec());
    }

    #[test]
    fn the_version_carries_a_hex_git_sha() {
        assert!(!version::GIT_SHA.is_empty());
        assert!(
            version::GIT_SHA.chars().all(|c| c.is_ascii_hexdigit()),
            "{}",
            version::GIT_SHA
        );
        assert!(version::line().contains("(git "));
    }

    #[test]
    fn completion_scripts_come_from_the_same_schema() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let script = completions::script(shell);
            assert!(
                script.contains("download-dir"),
                "{shell:?} is missing a global option"
            );
            assert!(script.contains("search"), "{shell:?}");
        }
        let zsh = completions::script(Shell::Zsh);
        assert!(zsh.contains("#compdef soulseek-rs"));
        assert!(zsh.contains("compdef _soulseek-rs soulseek-rs"));
        let bash = completions::script(Shell::Bash);
        assert!(bash.contains("complete -F _soulseek-rs"));
        let fish = completions::script(Shell::Fish);
        assert!(fish.contains("complete -c soulseek-rs"));
        assert!(fish.contains("-l download-dir"));
    }

    #[test]
    fn a_non_utf8_positional_is_rejected() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let args = vec![
            OsString::from("soulseek-rs"),
            OsString::from("search"),
            OsString::from_vec(vec![0xff]),
        ];
        let Err(error) = parse(args) else {
            panic!("expected a parse error");
        };
        assert!(error.to_string().contains("UTF-8"), "{error}");
    }
}
