Prefix: SOUL
Next: 9

## Working checklist — conservative networking campaign

- [x] Mapping policy (`off`/`auto`/`upnp`/`natpmp`) wired through CLI, env,
      file, help, completions and all session entry points; `off` starts no
      worker; invalid combinations rejected before sockets open.
- [x] Default sharing and peer listener preserved; share roots confined at
      file-open time; `.part` files excluded.
- [x] Resource bounds added/refreshed: frame buffer cap, handshake refusal,
      queue caps, pending-serve TTL, search budgets, reconnect stop.
- [x] Daemon control kept local; non-loopback plaintext TCP refused; token
      created 0600 atomically; socket mode tested.
- [x] Docs, network contract, CI server-backed job, `make bench` target.
- [x] Server-backed verification: local soulfind built from source; full
      `SOULSEEK_E2E_REQUIRED=1` suite passed on macOS with two test threads
      (one earlier run failed only under default parallelism, when 100
      concurrent soulfind instances exhausted this host).
- [ ] Independent-client (Nicotine+) interoperability: GUI client not
      available in this environment; unverified.
- [ ] Public-network smoke test: no purpose-provided credentials; not run.

### SOUL-4 · P3 · Bidirectional overrides in peer text can reorder what the terminal shows

- **Area:** security · terminal rendering
- **Where:** soulseek-rs/src/ui/tui/buffer.rs; soulseek-rs/src/output.rs
- **Problem:** Format characters such as U+202E are not controls, so a file name like 'song\u202Egnp.exe' displays reordered in the TUI and in text output, which can disguise a file's extension. Not a terminal-control injection.
- **Recommendation:** After the control-character fix, decide whether to replace bidi override and isolate characters (U+202A..U+202E, U+2066..U+2069) the same way.
- **Fix with:** security
- **Effort:** S

### SOUL-5 · P3 · TUI snapshot tests are stale and outside make test

- **Area:** verification · tui snapshots
- **Where:** soulseek-rs/tests/tui_snapshots.rs; soulseek-rs/tests/snapshots/*.ptytest
- **Problem:** cargo test -p soulseek-rs --features fixtures --test tui_snapshots fails 4 of 5 tests at HEAD without any change of this pass (verified by restoring the previous event.rs): the recorded frames lack the Status pane the window now draws (narrow_window_still_fits expects Results at row 0, the window draws Status first), and the tiny-window test times out. The tests need the fixtures feature, so make test and CI never run them.
- **Recommendation:** Decide whether the window or the frames are right, re-record with PTYTEST_UPDATE_SNAPSHOTS=1, and run the suite in make test (or CI) once green.
- **Fix with:** test-pyramid
- **Effort:** M

### SOUL-7 · P3 · No benchmark for the TUI frame path or peer message parsing

- **Area:** verification · bench
- **Where:** soulseek-rs/src/ui/tui/buffer.rs; soulseek-rs/src/ui/tui/terminal.rs; soulseek-rs-lib/src/message
- **Problem:** Every frame redraws through Buffer::set_stringn and AnsiBackend::draw, and every peer message goes through the message readers, but no benchmark covers either; the stress score exercises only network concurrency, so a render or parse regression would not move it.
- **Recommendation:** Add one deterministic benchmark each (a full-window draw over the fixture session with the TestBackend; parsing a captured file-search response) outside make test.
- **Fix with:** bench-pyramid
- **Effort:** M

