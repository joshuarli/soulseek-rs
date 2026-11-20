Prefix: DSLP
Next: 13

Deslop campaign backlog ([plan](https://github.com/laputa-systems/stuff/blob/main/docs/campaigns/deslop.md), tier E). One agent works this file with the `deslop` skill: take the lowest-numbered open ticket whose area does not start with `Needs owner ·`. After DSLP-5, work the open P1 and P2 findings, then fill in `## Report` below.

## Report

Not started.

## Tickets

### DSLP-7 · P1 · Peer chat and room text reaches the terminal with raw control sequences

- **Area:** security · terminal rendering
- **Where:** soulseek-rs/src/ui/tui/buffer.rs (set_stringn, glyph); soulseek-rs/src/ui/tui/terminal.rs (AnsiBackend::draw)
- **Problem:** Any peer or room member can make the TUI emit raw ESC, BEL and C1 bytes. Reproduced against the built tui-fixture binary in a PTY with a temporary fixture change (restored): a room message 'been on repeat \x1b]0;PWNED-ROOM\x07all week' and a chat text containing OSC 0, CSI 2J and U+009B reached the terminal byte-for-byte inside the cell stream (OSC title count 1 each, C1 CSI count 1). unicode-width gives control characters width 1 as a str, so Buffer::glyph stores them as cell symbols and AnsiBackend::draw writes them. The same path renders usernames, file names, paths and room names. Impact: screen and title spoofing, and terminal-specific escapes such as OSC 52 clipboard writes and answerback injection.
- **Recommendation:** Neutralize every control character (is_control: C0, DEL, C1) at the one place all widgets draw through, Buffer::set_stringn, by drawing U+FFFD instead; keep width accounting. Add a regression test through the TestBackend and the AnsiBackend byte stream that fails before the fix.
- **Fix with:** security
- **Effort:** S

### DSLP-3 · P2 · Types: strict profile and typing findings

- **Area:** deslop · types
- **Where:** type checker and lint configuration; Rust sources
- **Problem:** The strict-profile candidates in the deslop skill's tools reference have not been measured here.
- **Recommendation:** Run the deslop Types step. Accept: clean at the adopted profile, escape hatches not increased, witnesses recorded.
- **Fix with:** deslop
- **Effort:** M

### DSLP-4 · P2 · Tests: coverage, boundary gaps, mutation

- **Area:** deslop · tests
- **Where:** test suites and CI
- **Problem:** Coverage and mutation results are unmeasured; no deslop mutation lane exists.
- **Recommendation:** Run the deslop Tests step. Accept: no undispositioned survivor in critical code, the test cost budget holds, and the lane is green where the tools are mirrored.
- **Fix with:** deslop
- **Effort:** L

### DSLP-5 · P2 · Dead code: tools and debloat

- **Area:** deslop · dead code
- **Where:** whole repository (our changes)
- **Problem:** No dead-code scan has run.
- **Recommendation:** Run the deslop Dead code step. Accept: tools clean or allowlisted with reasons; walk-backs filed as Needs owner tickets.
- **Fix with:** deslop
- **Effort:** M

### DSLP-8 · P2 · CLI text records and status lines pass peer control sequences to the terminal

- **Area:** security · cli output
- **Where:** soulseek-rs/src/output.rs (sanitize, Out::status, Out::warn, Out::error)
- **Problem:** sanitize only replaces tab, CR and LF, so a peer user name, file path, room name or chat message containing ESC or C1 controls reaches stdout in text mode (search, browse, room and message watch commands) unchanged; Out::status, warn and error print their message verbatim to stderr, and callers embed peer names and paths in them. A terminal then executes the sequence (title change, screen clear, OSC 52 clipboard write). JSON mode is unaffected because serde escapes controls.
- **Recommendation:** Make sanitize replace every control character (is_control) with a space, and give status, warn and error a message sanitizer that keeps only newline and tab. Test both with ESC and U+009B inputs.
- **Fix with:** security
- **Effort:** S

### DSLP-9 · P2 · Port mapping trusts unbounded, arbitrarily nested XML from LAN devices

- **Area:** security · port mapping
- **Where:** soulseek-rs/src/port_mapping/igd/http.rs (send: read_to_end); soulseek-rs/src/port_mapping/igd/common/parsing.rs (parse_control_urls, parse_schemas, parse_response)
- **Problem:** Gateway discovery fetches the device description and SCPD from whatever host answers the SSDP search, so any LAN host controls the body. send() reads the whole body with read_to_end and no cap, and xmltree::Element::parse is recursive: a temporary test parsing a 700 kB document of 100,000 nested elements aborted the process with 'has overflowed its stack' (SIGABRT, probe restored). A stack-overflow abort skips the terminal restore, and an endless body exhausts memory.
- **Recommendation:** Cap the response body (1 MiB) and reject XML nested deeper than 64 elements before parsing, in every parse entry point. Test the cap with a local listener bound to port 0 and the depth limit with a generated document.
- **Fix with:** security
- **Effort:** S

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

