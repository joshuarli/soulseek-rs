//! The command surface as data: every command, option, positional, default,
//! environment input, and constraint the parser and help renderer share.

use crate::cli::{
    account, browse, completions, config, daemon, download, get, message, room,
    search, serve, shares, skills, transfer, user, watch, wish,
};

pub const BIN: &str = "soulseek-rs";

/// Upper bound for every wait expressed in seconds.
///
/// A one-shot command that runs longer than a day is a mistyped flag, and
/// rejecting it here keeps the deadline arithmetic in the command layer from
/// overflowing.
pub const MAX_WAIT_SECS: u64 = 24 * 60 * 60;

pub const EXAMPLES: &str = "\
Examples:
  soulseek-rs                                   launch the interactive TUI
  soulseek-rs search 'aphex twin xtal'          print matching files
  soulseek-rs get 'aphex twin xtal'             search, pick the best, download
  soulseek-rs search 'rjd2' --json | jq -c 'select(.bitrate >= 320)' \\
    | soulseek-rs download --stdin              filter, then fetch
  soulseek-rs browse someuser --json            list a peer's shares
  soulseek-rs room listen lobby --follow        stream room chat
  soulseek-rs serve --follow --json             share files, stream upload events
  soulseek-rs daemon                            run as a service; later commands use it
  soulseek-rs user someuser                     is this peer online and worth it
  soulseek-rs skills install                    teach your coding agent this CLI
  soulseek-rs completions install               tab completion for your shell

Exit codes:
  0 success   2 usage/config   3 connect or login   4 no results
  5 timed out   6 transfer failed   7 session lost   1 unexpected error";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CommandId {
    Root,
    Search,
    Download,
    Transfer,
    TransferCancel,
    Get,
    Browse,
    Wish,
    WishAdd,
    WishRemove,
    WishList,
    WishRun,
    Room,
    RoomList,
    RoomSay,
    RoomUsers,
    RoomListen,
    Message,
    MessageSend,
    MessageRead,
    Serve,
    Daemon,
    DaemonToken,
    DaemonStatus,
    DaemonStop,
    User,
    Watch,
    WatchAdd,
    WatchRemove,
    WatchList,
    Whoami,
    Account,
    AccountPassword,
    AccountLogout,
    Shares,
    SharesList,
    SharesAdd,
    SharesRemove,
    SharesFiles,
    SharesReindex,
    SharesStatus,
    Config,
    ConfigPath,
    ConfigList,
    ConfigGet,
    ConfigSet,
    Portmap,
    Skills,
    SkillsInstall,
    SkillsUninstall,
    SkillsList,
    Completions,
    CompletionsInstall,
    CompletionsUninstall,
    CompletionsPrint,
    Man,
    Help,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Special {
    Help,
    Version,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Text,
    Usize,
    U64,
    U16,
    U8,
    Path,
    Choice(&'static [&'static str]),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub min: u64,
    pub max: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ValueSpec {
    pub name: &'static str,
    pub kind: ValueKind,
    pub range: Option<Range>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OptionSpec {
    pub short: Option<char>,
    pub long: &'static str,
    pub value: Option<ValueSpec>,
    pub repeatable: bool,
    pub count: bool,
    pub default: Option<&'static str>,
    pub env: Option<&'static str>,
    pub help: &'static str,
    pub special: Option<Special>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PositionalSpec {
    pub name: &'static str,
    pub required: bool,
    pub repeatable: bool,
    pub help: &'static str,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Constraint {
    RequiredUnless {
        target: &'static str,
        unless: &'static str,
    },
    Conflicts {
        option: &'static str,
        with: &'static [&'static str],
    },
}

#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub id: CommandId,
    pub name: &'static str,
    pub about: &'static str,
    pub long_about: &'static str,
    pub options: Vec<OptionSpec>,
    pub positionals: Vec<PositionalSpec>,
    pub subcommands: Vec<Self>,
    pub constraints: Vec<Constraint>,
    pub examples: &'static str,
    pub subcommand_required: bool,
}

pub const fn flag(
    short: Option<char>,
    long: &'static str,
    help: &'static str,
) -> OptionSpec {
    OptionSpec {
        short,
        long,
        value: None,
        repeatable: false,
        count: false,
        default: None,
        env: None,
        help,
        special: None,
    }
}

pub const fn value(
    short: Option<char>,
    long: &'static str,
    name: &'static str,
    kind: ValueKind,
    help: &'static str,
) -> OptionSpec {
    OptionSpec {
        short,
        long,
        value: Some(ValueSpec {
            name,
            kind,
            range: None,
        }),
        repeatable: false,
        count: false,
        default: None,
        env: None,
        help,
        special: None,
    }
}

pub const fn count(
    short: char,
    long: &'static str,
    help: &'static str,
) -> OptionSpec {
    OptionSpec {
        short: Some(short),
        long,
        value: None,
        repeatable: false,
        count: true,
        default: None,
        env: None,
        help,
        special: None,
    }
}

pub const fn special(
    short: Option<char>,
    long: &'static str,
    special: Special,
    help: &'static str,
) -> OptionSpec {
    OptionSpec {
        short,
        long,
        value: None,
        repeatable: false,
        count: false,
        default: None,
        env: None,
        help,
        special: Some(special),
    }
}

pub const fn with_env(mut spec: OptionSpec, env: &'static str) -> OptionSpec {
    spec.env = Some(env);
    spec
}

pub const fn repeatable(mut spec: OptionSpec) -> OptionSpec {
    spec.repeatable = true;
    spec
}

pub const fn with_default(
    mut spec: OptionSpec,
    default: &'static str,
) -> OptionSpec {
    spec.default = Some(default);
    spec
}

pub const fn with_range(
    mut spec: OptionSpec,
    min: u64,
    max: u64,
) -> OptionSpec {
    if let Some(value) = &mut spec.value {
        value.range = Some(Range { min, max });
    }
    spec
}

pub const HELP_OPTION: OptionSpec = special(
    Some('h'),
    "help",
    Special::Help,
    "Print help",
);

pub const VERSION_OPTION: OptionSpec = special(
    Some('V'),
    "version",
    Special::Version,
    "Print version",
);

pub const USERNAME: OptionSpec = with_env(
    value(
        Some('u'),
        "username",
        "USERNAME",
        ValueKind::Text,
        "Soulseek username",
    ),
    "SOULSEEK_USERNAME",
);

pub const PASSWORD: OptionSpec = with_env(
    value(
        Some('p'),
        "password",
        "PASSWORD",
        ValueKind::Text,
        "Password (visible in `ps`; prefer --password-stdin or the saved password)",
    ),
    "SOULSEEK_PASSWORD",
);

pub const PASSWORD_STDIN: OptionSpec = flag(
    None,
    "password-stdin",
    "Read the password from the first line of stdin (takes precedence over --password)",
);

pub const PASSWORD_CMD: OptionSpec = with_env(
    value(
        None,
        "password-cmd",
        "PASSWORD_CMD",
        ValueKind::Text,
        "Shell command whose stdout is the password, used when nothing else supplies one",
    ),
    "SOULSEEK_PASSWORD_CMD",
);

pub const SERVER: OptionSpec = with_env(
    value(
        None,
        "server",
        "SERVER",
        ValueKind::Text,
        "Server as host:port",
    ),
    "SOULSEEK_SERVER",
);

pub const NO_LISTENER: OptionSpec = with_env(
    flag(
        None,
        "no-listener",
        "Do not accept incoming peer connections",
    ),
    "SOULSEEK_NO_LISTENER",
);

pub const LISTENER: OptionSpec = flag(
    None,
    "listener",
    "Accept incoming peer connections, overriding the config file",
);

pub const LISTENER_PORT: OptionSpec = with_env(
    value(
        None,
        "listener-port",
        "LISTENER_PORT",
        ValueKind::U16,
        "Port to accept peer connections on",
    ),
    "SOULSEEK_LISTENER_PORT",
);

pub const DOWNLOAD_DIR: OptionSpec = with_env(
    value(
        Some('d'),
        "download-dir",
        "DOWNLOAD_DIR",
        ValueKind::Text,
        "Directory downloads are saved to",
    ),
    "SOULSEEK_DOWNLOAD_DIR",
);

pub const SHARED_DIR: OptionSpec = with_env(
    repeatable(value(
        None,
        "shared-dir",
        "SHARED_DIR",
        ValueKind::Text,
        "Directory shared with other peers; repeat for several",
    )),
    "SOULSEEK_SHARED_DIR",
);

pub const MAX_CONCURRENT_DOWNLOADS: OptionSpec = with_env(
    value(
        Some('c'),
        "max-concurrent-downloads",
        "MAX_CONCURRENT_DOWNLOADS",
        ValueKind::Usize,
        "Maximum simultaneous downloads",
    ),
    "SOULSEEK_MAX_CONCURRENT_DOWNLOADS",
);

pub const SEARCH_TIMEOUT: OptionSpec = with_range(
    with_env(
        value(
            None,
            "search-timeout",
            "SEARCH_TIMEOUT",
            ValueKind::U64,
            "Seconds a search collects results before reporting",
        ),
        "SOULSEEK_SEARCH_TIMEOUT",
    ),
    1,
    MAX_WAIT_SECS,
);

pub const DAEMON: OptionSpec = with_env(
    value(
        None,
        "daemon",
        "ADDR",
        ValueKind::Text,
        "Control a daemon at this address (host:port) or Unix socket path",
    ),
    "SOULSEEK_DAEMON",
);

pub const DAEMON_TOKEN: OptionSpec = with_env(
    value(
        None,
        "daemon-token",
        "DAEMON_TOKEN",
        ValueKind::Text,
        "Token for a remote daemon; a local socket needs none",
    ),
    "SOULSEEK_DAEMON_TOKEN",
);

pub const NO_DAEMON: OptionSpec = flag(
    None,
    "no-daemon",
    "Ignore any running daemon and open this run's own session",
);

pub const CONFIG: OptionSpec = with_env(
    value(
        None,
        "config",
        "FILE",
        ValueKind::Path,
        "Config file to read instead of the default location",
    ),
    "SOULSEEK_CONFIG",
);

pub const NO_CONFIG: OptionSpec =
    flag(None, "no-config", "Ignore the config file entirely");

pub const JSON: OptionSpec = flag(
    None,
    "json",
    "Emit newline-delimited JSON instead of tab-separated text",
);

pub const QUIET: OptionSpec = flag(
    Some('q'),
    "quiet",
    "Suppress progress and status output on stderr",
);

pub const VERBOSE: OptionSpec =
    count('v', "verbose", "Increase log verbosity (repeat up to -vvvv)");

pub const LOG_FILE: OptionSpec = with_env(
    value(
        None,
        "log-file",
        "LOG_FILE",
        ValueKind::Path,
        "Write logs to a file instead of stderr",
    ),
    "SOULSEEK_LOG_FILE",
);

pub const GLOBAL_OPTIONS: &[OptionSpec] = &[
    USERNAME,
    PASSWORD,
    PASSWORD_STDIN,
    PASSWORD_CMD,
    SERVER,
    NO_LISTENER,
    LISTENER,
    LISTENER_PORT,
    DOWNLOAD_DIR,
    SHARED_DIR,
    MAX_CONCURRENT_DOWNLOADS,
    SEARCH_TIMEOUT,
    DAEMON,
    DAEMON_TOKEN,
    NO_DAEMON,
    CONFIG,
    NO_CONFIG,
    JSON,
    QUIET,
    VERBOSE,
    LOG_FILE,
    HELP_OPTION,
    VERSION_OPTION,
];

pub const fn command(
    id: CommandId,
    name: &'static str,
    about: &'static str,
) -> CommandSpec {
    CommandSpec {
        id,
        name,
        about,
        long_about: "",
        options: Vec::new(),
        positionals: Vec::new(),
        subcommands: Vec::new(),
        constraints: Vec::new(),
        examples: "",
        subcommand_required: false,
    }
}

impl CommandSpec {
    pub const fn long_about(mut self, text: &'static str) -> Self {
        self.long_about = text;
        self
    }

    pub fn option(mut self, spec: OptionSpec) -> Self {
        self.options.push(spec);
        self
    }

    pub fn options(mut self, specs: impl IntoIterator<Item = OptionSpec>) -> Self {
        self.options.extend(specs);
        self
    }

    pub fn positional(mut self, spec: PositionalSpec) -> Self {
        self.positionals.push(spec);
        self
    }

    pub fn subcommand(mut self, spec: Self) -> Self {
        self.subcommands.push(spec);
        self
    }

    pub fn constraint(mut self, constraint: Constraint) -> Self {
        self.constraints.push(constraint);
        self
    }

    pub const fn examples(mut self, text: &'static str) -> Self {
        self.examples = text;
        self
    }

    pub const fn requires_subcommand(mut self) -> Self {
        self.subcommand_required = true;
        self
    }

    pub fn with_help(mut self) -> Self {
        self.subcommands.push(help_spec());
        self
    }

    #[must_use]
    pub fn find_subcommand(&self, name: &str) -> Option<&Self> {
        self.subcommands.iter().find(|spec| spec.name == name)
    }

    #[must_use]
    pub fn find_long(&self, name: &str) -> Option<&OptionSpec> {
        self.options
            .iter()
            .chain(GLOBAL_OPTIONS)
            .find(|spec| spec.long == name)
    }

    #[must_use]
    pub fn find_short(&self, short: char) -> Option<&OptionSpec> {
        self.options
            .iter()
            .chain(GLOBAL_OPTIONS)
            .find(|spec| spec.short == Some(short))
    }

    pub fn all_options(&self) -> impl Iterator<Item = &OptionSpec> {
        self.options.iter().chain(GLOBAL_OPTIONS)
    }

    #[must_use]
    pub const fn has_subcommands(&self) -> bool {
        !self.subcommands.is_empty()
    }
}

pub fn help_spec() -> CommandSpec {
    command(
        CommandId::Help,
        "help",
        "Print this message or the help of the given subcommand(s)",
    )
    .positional(PositionalSpec {
        name: "COMMAND",
        required: false,
        repeatable: true,
        help: "Command to show help for",
    })
}

pub fn root_spec() -> CommandSpec {
    command(CommandId::Root, BIN, "Soulseek client in Rust 🦀")
        .long_about(
            "Run without a subcommand for the interactive TUI, or use a \
             subcommand for a scriptable one-shot operation: stdout is data, \
             stderr is progress, and the exit code says what happened.",
        )
        .subcommand(search::spec())
        .subcommand(download::spec())
        .subcommand(transfer::spec())
        .subcommand(get::spec())
        .subcommand(browse::spec())
        .subcommand(wish::spec())
        .subcommand(room::spec())
        .subcommand(message::spec())
        .subcommand(serve::spec())
        .subcommand(daemon::spec())
        .subcommand(user::spec())
        .subcommand(watch::spec())
        .subcommand(command(CommandId::Whoami, "whoami", "Confirm the credentials and connection work"))
        .subcommand(account::spec())
        .subcommand(shares::spec())
        .subcommand(config::spec())
        .subcommand(command(
            CommandId::Portmap,
            "portmap",
            "Check whether the router opens the listen port (UPnP/NAT-PMP)",
        ))
        .subcommand(skills::spec())
        .subcommand(completions::spec())
        .subcommand(command(
            CommandId::Man,
            "man",
            "Print the man page to stdout, for packaging",
        ))
        .examples(EXAMPLES)
        .with_help()
}

#[must_use]
pub fn usage(spec: &CommandSpec, path: &[&str]) -> String {
    let mut usage = String::from(BIN);
    for segment in path {
        usage.push(' ');
        usage.push_str(segment);
    }
    usage.push_str(" [OPTIONS]");
    for positional in &spec.positionals {
        usage.push(' ');
        let (open, close) = if positional.required {
            ('<', '>')
        } else {
            ('[', ']')
        };
        usage.push(open);
        usage.push_str(positional.name);
        usage.push(close);
        if positional.repeatable {
            usage.push_str("...");
        }
    }
    if spec.has_subcommands() {
        usage.push_str(if spec.subcommand_required {
            " <COMMAND>"
        } else {
            " [COMMAND]"
        });
    }
    usage
}

#[must_use]
pub fn option_label(spec: &OptionSpec) -> String {
    let mut label = format!("--{}", spec.long);
    if let Some(value) = spec.value {
        label.push_str(" <");
        label.push_str(value.name);
        label.push('>');
        if spec.repeatable {
            label.push_str("...");
        }
    }
    if spec.count {
        label.push_str("...");
    }
    label
}
