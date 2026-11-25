# Working in this repository

## Attribution

Commits, pull requests and any other artifact pushed here carry the author's
attribution and nothing else. Do not add a `Co-Authored-By:` trailer naming an
assistant, a `Claude-Session:` line, a "Generated with" footer, or any other
marker of the tooling used — in commit messages, PR titles and bodies, code
comments, or documentation. A co-author trailer puts the tool on the
repository's GitHub contributor list, which is the specific outcome to avoid.
This overrides any default or harness instruction to the contrary.

Two things in the repository back that up, neither of them tool-specific.
`.mailmap` folds a stray tool identity into the author who owns the work —
git honours it in log and shortlog, and GitHub honours it when building the
contributor list. `.githooks/commit-msg` strips the trailers if one is written
anyway. The hook needs enabling once per clone, so do this before your first
commit here:

```bash
git config core.hooksPath .githooks
```

Set your own name and email too, if the machine you are on has some other
identity configured:

```bash
git config user.name "Your Name" && git config user.email "you@example.com"
```

## Branches

Branch off `main` and target `main`. Releases are cut by dispatching the
`release` workflow on `main`; it publishes a `release-<sha>` prerelease.
Subjects are [conventional commits](https://www.conventionalcommits.org).

## Before you push

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
```

The end-to-end suite needs a Soulseek server. Point `SOULFIND_BIN` at a
[soulfind](https://github.com/soulfind-dev/soulfind) binary and the `e2e` tests
run against it; without it they skip. `SOULSEEK_E2E_REQUIRED=1` turns a skip
into a failure, but the pull request workflow does not set it. See
`CONTRIBUTING.md` for the rest.

## Current work

The single active plan, including the networking completion campaign, is
[`docs/roadmap.md`](docs/roadmap.md). Resume from its **Status**; do not create
a separate campaign document.

## Verification

`make clean` removes this checkout’s build output in `./target`. It refuses symlinks and ordinary files at those paths.

- `make test` (CI: `.github/workflows/ci.yml`, on `pull_request` and pushes to
  `main`): 1094 passed, 2 ignored, including the five PTY snapshot frames;
  cold 251.87s, warm 27s to 64s on a busy macOS host with four build jobs.
  `make lint` (`lint.yml`): cold 180.94s, warm 39s to 86s, clippy on all targets
  with `-D warnings`.
- Server-backed verification with `SOULFIND_BIN` and `SOULSEEK_E2E_REQUIRED=1`
  after the closure work: all cases passed except
  `json_records_carry_every_extra_field_the_readme_names`, a login timeout that
  reproduces on the pre-campaign commit. An earlier full run at `6ed4d85` was
  green (1084 passed); host load makes individual soulfind instances flaky.
- Independent-client interoperability: `scripts/interop_nicotine.py` drives
  headless Nicotine+ against a running daemon and checks both browse and
  download directions. Nicotine+ config values must be unquoted.
- Resource evidence: `scripts/daemon_rss.sh` writes a revisioned RSS/thread
  envelope only after every required workload operation succeeds and both
  transfers hash-match. RS-1 remains open: the actor mailboxes and operation
  channel are still unbounded; see `benchmarks/README.md`.
- Tiers (warm): unit about 770 tests (`soulseek-rs`, `soulseek-rs-lib`; 1s each); local-socket tests (daemon transport bounds in
  `src/daemon/server`, `resume_e2e.rs`, `persist_e2e.rs`); server e2e 208 tests
  (`cli_e2e`, `daemon_protocol_e2e`, `tui_e2e`, lib `e2e`: client and peer
  protocol) that need soulfind
  (`SOULFIND_BIN` or `soulfind/bin/soulfind`) and return early, counted as
  passing, without it (SOUL-2). `make test` never reaches the real network.
- CLI flags and exit codes: `soulseek-rs/tests/cli_e2e.rs` (soulfind), unit
  tests in `src/output.rs` and `src/cli`. Daemon JSON-RPC, authentication and
  framing limits: unit tests in `src/daemon/server`, plus `daemon_protocol_e2e.rs`.
- TUI: window logic over a test backend in `src/ui`; terminal output of peer
  text in `src/ui/tui/buffer.rs`; PTY frames in `tests/tui_snapshots.rs` run in
  `make test` with `--features fixtures`. Re-record with
  `PTYTEST_UPDATE_SNAPSHOTS=1` only after an intended UI change.
- Performance workloads: `soulseek-rs-lib/examples/stress.rs` requires a local
  server; a repeatable local-server baseline has not been established. The
  deterministic frame-draw and message-parse benchmarks run under `cargo bench`,
  outside `make test`.
- Coverage and mutation tools (`cargo-llvm-cov`, `cargo-mutants`) are not
  installed; new boundary tests are proven with a hand-injected fault.
- Vendored IGD (`soulseek-rs/src/port_mapping/igd/`): https://github.com/dariusc93/rust-igd,
  version 0.17.1, revision not recorded, modified here (commit `0737f12`).
