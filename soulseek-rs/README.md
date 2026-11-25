# soulseek-rs

A Soulseek client for the terminal: a full-screen TUI, and a one-shot command
surface built for scripts and agents.

Website: <https://re-invention.nl/soulseek-rs/>

## Installation

```bash
brew install michel/tap/soulseek-rs   # macOS and Linux, prebuilt
cargo install soulseek-rs             # anywhere with a Rust toolchain
```

## Usage

Run it with no subcommand for the interactive TUI:

```bash
soulseek-rs
```

Anything else is a one-shot command that runs headless. stdout carries records
only, stderr carries progress, and the exit code carries the verdict:

```bash
soulseek-rs search <QUERY>                # print matching files
soulseek-rs get <QUERY>                   # search, pick the best, download
soulseek-rs download <USER> <PATH>        # fetch one known file
soulseek-rs download --stdin              # fetch files listed on stdin
soulseek-rs browse <USER>                 # list a user's shared files
soulseek-rs serve [--follow]              # stay online sharing, stream uploads
soulseek-rs whoami                        # confirm credentials and connection
soulseek-rs user <NAME>                   # a peer's status and share counts
soulseek-rs room list|say|users|listen
soulseek-rs message send|read
soulseek-rs shares list|add|remove|status|reindex
soulseek-rs config path|list|get|set
soulseek-rs --port-mapping auto serve     # opt in to UPnP then NAT-PMP mapping
soulseek-rs portmap                       # actively probe and undo a mapping
soulseek-rs completions install           # tab completion for bash/zsh/fish
```

Add `--json` for newline-delimited JSON instead of tab-separated text, so
commands compose:

```bash
soulseek-rs search "rjd2" --json \
  | jq -c 'select(.size > 5000000 and .free_slot)' \
  | soulseek-rs download --stdin
```

Run `soulseek-rs --help` for the complete command reference, including nested
commands, their arguments and options, and an example for each. Command-specific
help is also available, for example `soulseek-rs room listen --help`. Running
`soulseek-rs` without a command still starts the interactive TUI.

Exit codes: `0` success, `2` usage or config, `3` connect or login, `4` nothing
found, `5` timed out, `6` transfer failed, `7` session lost mid-command, `1`
unexpected error.

The [full README](https://github.com/michel/soulseek-rs#readme) documents every
command, record shape, flag, environment variable and `config.toml` key.

## Building from Source

```bash
git clone https://github.com/michel/soulseek-rs.git
cd soulseek-rs
cargo build --release
```

## Library

To build your own Soulseek client, use the
[soulseek-rs-lib](https://crates.io/crates/soulseek-rs-lib) crate: the protocol
implementation this client is built on.
