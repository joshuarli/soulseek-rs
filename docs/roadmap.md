# Roadmap

## Completion campaign (networking correctness)

Review checkpoint: the follow-up review of `d4a3696`. Base: `d703c30`.
Status: UP-1, FR-1, SH-1, ST-1, TR-1 and NX-1 are accepted on `main`.
`make test` (1094 passed) and clippy are green. Listener threads are owned and
joined by the daemon; idle listeners now wait on OS readiness with an explicit
shutdown wakeup, and a regression counts idle wakeups. A focused standard
(`owned-concurrency`) makes detached long-lived threads a must-fix finding.

NX-1: bidirectional interoperability with headless Nicotine+ 3.3.11 was
verified against a local soulfind in both directions. The earlier `0.0.0.0:0`
peer address was a quoted-username test-fixture fault, not a product defect.

RS-1 remains **incomplete**. The verifier now proves its workload before
accepting evidence: every search/browse/download/upload must succeed, both
copies must hash-match distinct per-round fixtures, and the executed binary's
identity and build are recorded. A deliberately failing client exits 4 and
writes no JSON. A revisioned reduced-profile stress result has all fixtures
ready and all four functional dimensions at 100%
(`benchmarks/stress-functional.json`). The aggregate-resource controls are
still not implemented: actor mailboxes and the client operation channel remain
unbounded, and the default 1024-peer stress profile cannot complete on this
host because thread and socket creation fail. This is core hardening landed,
not a proven resource budget.

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
6. **RS-1** Aggregate resource envelope and reproducible workloads: *open*.
   Revisioned machine-readable evidence and a gating verifier are in place, but
   the unbounded actor mailboxes and operation channel have no admission or
   backpressure control, and the default stress profile is bounded by the
   host. Acceptance remains unmet. Owner: parent.
7. **NX-1** Network contract and headless Nicotine+ interoperability evidence.
   Acceptance: deterministic local checks; independent client attempt recorded
   with exact command or precise blocker. Owner: parent.

### Needs environment

- A local soulfind for server-backed verification (`SOULSEEK_E2E_REQUIRED=1`).
  The run after the closure work failed only in
  `json_records_carry_every_extra_field_the_readme_names`, a login timeout that
  reproduces on the pre-campaign commit; the rest passed. A host with more
  thread and descriptor headroom is needed for the default stress profile.
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
