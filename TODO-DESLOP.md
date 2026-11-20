Prefix: DSLP
Next: 18

Deslop campaign backlog ([plan](https://github.com/laputa-systems/stuff/blob/main/docs/campaigns/deslop.md), tier E). One agent works this file with the `deslop` skill: take the lowest-numbered open ticket whose area does not start with `Needs owner ·`. After DSLP-5, work the open P1 and P2 findings, then fill in `## Report` below.

## Report

Base: 8d8b654 (start of this pass; DSLP-1 closed in 3e12318). Host: busy macOS, four build jobs, no soulfind.

**Scorecard** (before, after)

- Source and tests: net +77 lines in 20 files (728 added, 651 removed); about 390 lines of dead vendored IGD code and one dependency (`fastrand`) removed, about 440 lines of tests and fixes added.
- `make test`: 981 passed, 2 ignored, warm 27s, cold 251.87s; after 1002 passed, 2 ignored, warm 33s to 53s on the busy host. `make lint` clean, warm 39s to 86s, cold 180.94s.
- Coverage and mutation: not measured, `cargo-llvm-cov` and `cargo-mutants` are not installed (Needs environment). New boundary tests are proven by hand-injected faults: 2 + 4 + 3 + 4 + 5 faults across the poll helper, daemon transport, TUI buffer, CLI output and IGD limits, every one failing the intended test and restored.
- Escape hatches in our changed files (non-test unsafe, allow, unwrap, expect): 33 to 32; one `unsafe` block deleted (select to poll). Adopted lint settings: none. Rejected with counts (our files): indexing_slicing 88, wildcard_enum_match_arm 50, as_conversions 31, needless_pass_by_value 19, cast_possible_truncation 16, cast_sign_loss 5, expect_used 6, unsafe_code forbid (7 sites, 4 without a safe equivalent).
- Dead code removed: 17 rustc dead-code items in the vendored IGD module and its `#![allow(dead_code)]`; the `fastrand` dependency.
- Findings: filed 2 P1/P2 security defects reproduced and fixed (DSLP-7 P1, DSLP-8 P2), 1 P2 fixed (DSLP-9), 1 P2 needing the owner (DSLP-10), P3 filed: DSLP-6, 11, 12, 13, 15, 16, 17.
- Perf verdict: the stress workload needs soulfind (blocked, Needs environment). Hot path `Buffer::set_stringn` micro-timing, six interleaved release pairs: old 6.45 to 6.87 us, new 6.41 to 7.06 us per 170-character call, median paired ratio about 1.04 on a busy host (about 0.25 us per call); undecided absolute number deferred.
- Escaped regressions: none known.

**Per step**

- Ground (c603fd0): DSLP-1 was already closed; re-measured and recorded costs. CI runs `make test` on `pull_request` (ci.yml) and `make lint` (lint.yml). 10 minutes.
- Diagnose (3f6aba0): reviewed peer text to the terminal, peer paths, daemon framing, IGD parsing, secrets. Reproduced against the built fixture binary in a PTY: OSC, CSI 2J, 8-bit CSI and BEL from room, chat and name text reached the terminal raw (DSLP-7). Reproduced a stack-overflow abort from a 700 kB nested XML (DSLP-9). Found sound: download names (last path segment), per-user directory names, daemon request cap and connection cap, credentials mode.
- Types (b4f238b): profile measured and rejected as above; `input_ready` uses safe `rustix::poll`, three pipe tests.
- Tests (8621119): daemon framing and connection limits had only soulfind-dependent tests; three local-socket tests added. The commit message says four tests, it is three (history is not amended).
- Dead code (8387836): vendored IGD unused API removed with the allow; cargo-machete and cargo-udeps missing (Needs environment), dependencies checked by hand.
- Bench (222f113): one benchmark (`scripts/stress.sh`, baseline 97.78), needs soulfind; no `make bench` entrypoint could be proven (DSLP-15, DSLP-16).
- Findings: DSLP-7 (f86757c), DSLP-8 (377fb5b), DSLP-9 (075d7c8), each with a regression test that failed before the fix.
- Report: self-review of the cumulative diff (no second reviewer available): found and fixed a `const fn` that needs Rust 1.97 against the 1.91 MSRV before it was committed; no other defects.

**Needs owner**

- DSLP-10 (P2): the upstream library logger writes peer text to stderr unescaped; fixing it edits upstream code, which this pass may not do.
- Commit trailers: the task asked for a `Co-Authored-By` trailer, but this repository's AGENTS.md forbids assistant trailers and its hook strips them, so none were written.

**Needs environment**

- soulfind: `cli_e2e`, `daemon_protocol_e2e`, `tui_e2e`, lib `e2e` (208 tests skipped as passing, DSLP-6) and `scripts/stress.sh` (DSLP-15).
- `cargo-llvm-cov`, `cargo-mutants`, `cargo-machete`, `cargo-udeps`.

**Method feedback**

- A skipped-as-passing test tier hides the most important boundaries; `make test` count alone said nothing about server coverage. The Verification section should state which tier is skipped.
- Hand-injected faults found two weak first drafts: the oversized-line test passed without the check (the junk line closed an unauthenticated connection anyway), and the endless-body test passed without the read cap. Both were rewritten to observe the bound, not the error.
- The PTY frame tests were stale and unused by `make test` (DSLP-13); no step of the procedure asks whether the declared boundary test even passes.

**Shared-file gaps**

- None in stuff or usr; the deslop skill's trailer instruction conflicts with repositories that forbid assistant trailers.

## Tickets

### DSLP-10 · P2 · Library logger writes peer-supplied text to stderr unescaped

- **Area:** Needs owner · security · upstream logger
- **Where:** soulseek-rs-lib/src/utils/logger.rs (log sinks); soulseek-rs-lib/src/message/peer/upload_denied.rs:16
- **Problem:** log() formats peer-controlled strings (for example the upload-denied reason, file names, user names) into stderr lines and into the buffered lines the TUI flushes to the real terminal on exit, without removing control characters. The file is upstream code, outside this pass's diff, so it is not changed here.
- **Recommendation:** Owner decision: allow a minimal change to the upstream logger (replace control characters other than newline in message) or carry it as a fork patch; add a test through log().
- **Fix with:** security
- **Effort:** S

### DSLP-6 · P3 · Make skipped server scenarios visible in CI

- **Area:** verification · server scenarios
- **Where:** .github/workflows/ci.yml; soulseek-rs-lib/tests/e2e.rs:208; README.md:757
- **Problem:** The pull request workflow runs make test without a local soulfind or SOULSEEK_E2E_REQUIRED. Server-dependent tests return early and are counted as passing; README claims CI supplies the server. With SOULFIND_BIN pointing to a missing path, connect_and_login_succeed reports a skip and exits successfully.
- **Recommendation:** Provide a local server in an authorized CI change and require it, or explicitly report the missing scenario coverage; correct the CI description alongside that change.
- **Fix with:** test-pyramid
- **Effort:** M

### DSLP-11 · P3 · Daemon token file is world-readable until chmod

- **Area:** security · daemon token
- **Where:** soulseek-rs/src/daemon/mod.rs:load_or_create_token
- **Problem:** The token is written with std::fs::write (mode from the umask) and restricted to 0600 afterwards, leaving a window in which another local user can read the control token. persist/secret.rs already creates its file with mode 0600 and renames it into place.
- **Recommendation:** Create the file with OpenOptions mode 0o600 (create_new) as secret.rs does; test the mode of a fresh token.
- **Fix with:** os-boundaries
- **Effort:** S

### DSLP-12 · P3 · Bidirectional overrides in peer text can reorder what the terminal shows

- **Area:** security · terminal rendering
- **Where:** soulseek-rs/src/ui/tui/buffer.rs; soulseek-rs/src/output.rs
- **Problem:** Format characters such as U+202E are not controls, so a file name like 'song\u202Egnp.exe' displays reordered in the TUI and in text output, which can disguise a file's extension. Not a terminal-control injection.
- **Recommendation:** After the control-character fix, decide whether to replace bidi override and isolate characters (U+202A..U+202E, U+2066..U+2069) the same way.
- **Fix with:** security
- **Effort:** S

### DSLP-13 · P3 · TUI snapshot tests are stale and outside make test

- **Area:** verification · tui snapshots
- **Where:** soulseek-rs/tests/tui_snapshots.rs; soulseek-rs/tests/snapshots/*.ptytest
- **Problem:** cargo test -p soulseek-rs --features fixtures --test tui_snapshots fails 4 of 5 tests at HEAD without any change of this pass (verified by restoring the previous event.rs): the recorded frames lack the Status pane the window now draws (narrow_window_still_fits expects Results at row 0, the window draws Status first), and the tiny-window test times out. The tests need the fixtures feature, so make test and CI never run them.
- **Recommendation:** Decide whether the window or the frames are right, re-record with PTYTEST_UPDATE_SNAPSHOTS=1, and run the suite in make test (or CI) once green.
- **Fix with:** test-pyramid
- **Effort:** M

### DSLP-15 · P3 · Stress benchmark needs a local soulfind and has no make entrypoint

- **Area:** verification · bench
- **Where:** scripts/stress.sh; benchmarks/README.md; Makefile
- **Problem:** The only benchmark (scripts/stress.sh, a mixed-load score out of 100 against benchmarks/stress-baseline.json at 97.78) spawns a local soulfind that is not installed on this host, so neither an Edit-size nor a Change-size run could be proven. The Makefile has no bench target, and CI never runs it. The score covers the library under peer load only.
- **Recommendation:** Where soulfind is available, add make bench running scripts/stress.sh and prove it at one Edit and one Change workload; never run the full profile in CI.
- **Fix with:** bench-pyramid
- **Effort:** S

### DSLP-16 · P3 · No benchmark for the TUI frame path or peer message parsing

- **Area:** verification · bench
- **Where:** soulseek-rs/src/ui/tui/buffer.rs; soulseek-rs/src/ui/tui/terminal.rs; soulseek-rs-lib/src/message
- **Problem:** Every frame redraws through Buffer::set_stringn and AnsiBackend::draw, and every peer message goes through the message readers, but no benchmark covers either; the stress score exercises only network concurrency, so a render or parse regression would not move it.
- **Recommendation:** Add one deterministic benchmark each (a full-window draw over the fixture session with the TestBackend; parsing a captured file-search response) outside make test.
- **Fix with:** bench-pyramid
- **Effort:** M

### DSLP-17 · P3 · Port mapping follows any address named in an SSDP reply

- **Area:** security · port mapping
- **Where:** soulseek-rs/src/port_mapping/igd/common/parsing.rs (parse_location); soulseek-rs/src/port_mapping/igd/search.rs
- **Problem:** Discovery fetches the description from the host:port in the LOCATION header of whichever datagram arrives first, not from the address that sent it, and a bad datagram aborts the search with ? instead of being skipped. A LAN host can steer the client's HTTP requests to another address or end discovery early.
- **Recommendation:** Take the gateway address from the datagram's source (recv_from) and accept only a LOCATION that names it; continue past an unparsable reply.
- **Fix with:** security
- **Effort:** S

