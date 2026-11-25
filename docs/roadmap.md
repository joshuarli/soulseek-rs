# Roadmap

## Completion campaign (networking correctness)

Review checkpoint: `79bc539`. Base: `d703c30`. Status: UP-1, FR-1, SH-1,
ST-1 and TR-1 are accepted on `main` and verified by `make test` (1089 passed)
and clippy. Listener threads are now owned and joined by the daemon, and a
focused standard (`owned-concurrency`) makes detached long-lived threads a
must-fix finding. RS-1 has deterministic benchmarks and the stress workload but
no checked-in reproducible daemon RSS measurement. NX-1: headless Nicotine+
3.3.11 connected to a local soulfind and its file was discovered by this
client; direct browse/download into that client remains unverified because
soulfind reported its peer address as `0.0.0.0:0`.

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
  One `cli_e2e` login-timeout case fails on this host at the pre-campaign
  commit as well; the rest pass.
- An isolated Nicotine+ installation with `--headless` for interoperability.
  Headless 3.3.11 was installed and connected locally; soulfind returned peer
  address `0.0.0.0:0` for it, blocking direct peer connections in that setup.

### Decisions still in force

- Sharing on, listener on, mapping off by default.
- Plaintext TCP daemon control stays loopback-only.
- No TLS, remote service, proxy, GUI, or async-runtime migration.
- Local correctness work does not wait on the environment items.
