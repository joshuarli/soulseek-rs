Prefix: SOUL
Next: 23

### SOUL-13 · P1 · Network-facing work still has no aggregate admission and ownership envelope

- **Area:** Concurrency and resource bounds
- **Where:** docs/roadmap.md:22-26; docs/network-contract.md:57-60; soulseek-rs-lib/src/actor/mod.rs:121; soulseek-rs-lib/src/client/connection.rs:54-57,171-201; soulseek-rs-lib/src/client/operations/transfers.rs:49-81; soulseek-rs-lib/src/client/uploads.rs:162-209,268-282; soulseek-rs-lib/src/peer/listen.rs:333,384-391,477-479; soulseek-rs/src/daemon/server/transport.rs:318-359
- **Problem:** RS-1 remains incomplete: actor mailboxes and the client operation channel are unbounded. The same envelope also excludes transfer threads and accepted control connections: downloads, uploads, and inbound file connections use plain thread::spawn with discarded handles, no global permit, no registry, and no shutdown join; daemon shutdown stops accepting control clients but does not own existing connection threads. A burst of peers or user requests can exhaust threads or memory and work can outlive its owner. The facts are proven directly in source and the roadmap; no new stress measurement was run.
- **Recommendation:** Finish RS-1 as one owned admission design: bounded actor and operation queues with explicit block/reject/drop policy, a bounded transfer registry with cancellation tokens and join ownership, builder-based spawn failure handling, and daemon control-connection shutdown ownership. Validate with the existing daemon RSS and stress workloads rather than claiming a budget first.
- **Fix with:** lifecycle-concurrency
- **Effort:** L

### SOUL-21 · P2 · Server backpressure disconnects the control session instead of deferring writes

- **Area:** Server transport reliability
- **Where:** soulseek-rs-lib/src/actor/server_actor.rs:372,592-601; soulseek-rs-lib/src/actor/peer_actor.rs:688-724
- **Problem:** The server socket is nonblocking, but each outbound frame is written with one write_all plus flush and any error disconnects. On a congested or slow server connection, WouldBlock is expected backpressure rather than a fatal session error. The peer actor already has the correct partial-write/pending-write pattern, so the server actor is inconsistent with the transport contract. Structurally proven; no socket-pressure reproduction was run.
- **Recommendation:** Give the server actor the same bounded pending-write state machine as peer actors: account for partial writes, treat WouldBlock as retryable backpressure, bound queued bytes with explicit disconnect semantics, and test partial writes and recovery.
- **Fix with:** lifecycle-concurrency
- **Effort:** M

### SOUL-9 · P3 · Verify one controlled session against the public Soulseek network

- **Area:** Release verification
- **Where:** TODO.md working checklist; docs/protocol-coverage.md; scripts/interop_nicotine.py
- **Problem:** The carried-over checklist says the public-network smoke test was not run because no purpose-provided credentials exist. Local soulfind and Nicotine+ interoperability are verified, but live server behavior, real NAT conditions, and public peer behavior remain outside the checked evidence. Parked: no purpose-provided public-network credentials or authority to use the live Soulseek network are available in this run. Unblock when the owner supplies credentials and authorizes one bounded live verification; keep the local suites green meanwhile.
- **Recommendation:** When suitable credentials are available, run one bounded manual verification of login, search, browse, download, upload serving, and daemon reconnection against the public network, recording the client revision and any protocol deltas. Do not weaken the local suites to wait for it.
- **Fix with:** reference-parity
- **Effort:** M

### SOUL-10 · P3 · Decide the distribution contract the release workflow supports

- **Area:** Release and packaging
- **Where:** .github/workflows/release.yml:388-423,475-479; scripts/package-linux.sh:1-54; packaging/nfpm.yaml; README.md installation
- **Problem:** The workflow publishes only three tar.xz archives and SHA256SUMS as release-<sha> prereleases, while the tree still carries an uncalled nfpm package path and previously documented stable, nightly, installer, package-manager, and develop-channel artifacts. The supported installation contract is therefore an owner decision, not a documentation detail. Evidence is proven by the workflow and missing caller for package-linux.sh. Parked: the linked RFC is still a draft and the distribution contract is an owner decision (tarball-only prereleases, first-class distribution packages, or a restored installer/nightly channel). Unblock when the owner accepts one RFC option; then implement that contract and delete the RFC in the ticket's closing commit.
- **Recommendation:** Decide among tarball-only prereleases, first-class distribution packages, or a restored installer/nightly channel using the linked RFC; then make the workflow, packaging files, metadata, and user docs implement that one contract.
- **Fix with:** dead-code-archaeology
- **Effort:** M
- **Design:** docs/rfc/SOUL-10-decide-the-supported-distribution-contract.md
