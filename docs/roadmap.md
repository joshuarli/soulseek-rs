# Roadmap

## Completion campaign (networking correctness)

Review checkpoint: `79bc539`. Base: `d703c30`. Status: UP-1, FR-1, SH-1,
ST-1, TR-1 and NX-1 are accepted on `main`; `make test` (1089 passed) and clippy
are green. Listener threads are now owned and joined by the daemon, and a
focused standard (`owned-concurrency`) makes detached long-lived threads a
must-fix finding. NX-1: bidirectional interoperability with headless Nicotine+
3.3.11 was verified against a local soulfind — this client browsed and
downloaded `nico-share\nico-track.mp3` from Nicotine+, and a Python-driven
Nicotine+ `core` browsed this daemon's `rustshare` and downloaded
`share-rust\rust-track.mp3` to `Finished`. The earlier `0.0.0.0:0` peer address
was a test-fixture fault: the Nicotine+ config file used Python-style quoted
values (`login = 'nicouser'`), so the client logged in with quotes in its
username and soulfind reported an address for a different name. RS-1 is now
accepted: `scripts/daemon_rss.sh` writes a revisioned, machine-readable RSS and
thread envelope (`benchmarks/daemon-rss.json`: 9456 KiB peak, 12 threads over
121 samples at `45d53c2`), and the mixed-load harness reports peak RSS and the
revision behind its baseline. Unbounded actor mailboxes and the operation
channel remain a documented boundary, not a claimed memory budget.

### Sequence

1. **UP-1** Upload admission and lifecycle: no recursive lock on brokered-cap
   rejection; queue rejection sends a protocol denial and frees no slot;
   pending-serve expiry removes the matching upload job. Acceptance:
   `cargo test -p soulseek-rs-lib --lib client` with new regression tests on
   reachable production branches. Owner: `campaign/uploads`.
2. **FR-1** Framing: consistent payload/header/buffer accounting; drain does not
   swallow capacity errors after consuming bytes; socket-level tests for
   boundaries, fragmentation, coalescing, and EOF. Acceptance:
   `cargo test -p soulseek-rs-lib --lib message::message_reader` and actor
   tests. Owner: `campaign/framing`.
3. **SH-1** Share serving: no blocking open on a replaced FIFO; precise root
   anchoring policy with descriptor-based identity preserved. Acceptance:
   `cargo test -p soulseek-rs-lib --lib shares` with a deadline regression.
   Owner: `campaign/shares`.
4. **ST-1** Local startup and settings: fresh headless daemon creates its
   default share directory and actually shares; config get/set and daemon
   control work despite an invalid listener/mapping combination; transient
   outages recover without restarting. Acceptance:
   `cargo test -p soulseek-rs --lib` plus targeted CLI tests.
   Owner: `campaign/startup`.
5. **TR-1** Transport and mapping: bind the validated socket addresses;
   NAT-PMP renewals validate granted port and lifetime. Acceptance: targeted
   unit tests in `daemon/server/transport` and `port_mapping`.
   Owner: `campaign/startup`.
6. **RS-1** Aggregate resource envelope and reproducible workloads: audit
   unbounded queues, add machine-readable evidence with exact revisions, and
   measure under mixed load. Acceptance: documented commands and results with
   explicit measurement boundaries. Owner: parent.
7. **NX-1** Network contract and headless Nicotine+ interoperability evidence.
   Acceptance: deterministic local checks; independent client attempt recorded
   with exact command or precise blocker. Owner: parent.

### Needs environment

- A local soulfind for server-backed verification (`SOULSEEK_E2E_REQUIRED=1`).
  Server startup is sensitive to host load: one `lib/e2e` case failed when its
  soulfind instance did not listen and passed on a serial retry, and a
  `cli_e2e` login-timeout case has failed at the pre-campaign commit too.
  Neither is a campaign regression.
- An isolated Nicotine+ installation with `--headless` for interoperability.
  Headless 3.3.11 connected to a local soulfind. Its config values must be
  unquoted: `login = nicouser`, not `login = 'nicouser'`, or the quotes become
  part of the login name and peer lookups fail. With that fixed, both browse and
  download directions succeed.

### Decisions still in force

- Sharing on, listener on, mapping off by default.
- Plaintext TCP daemon control stays loopback-only.
- No TLS, remote service, proxy, GUI, or async-runtime migration.
- Local correctness work does not wait on the environment items.
