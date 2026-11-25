Prefix: SOUL
Next: 23

### SOUL-11 · P1 · Same-basename downloads can concatenate or overwrite unrelated partial files

- **Area:** Downloads and durable state
- **Where:** soulseek-rs-lib/src/peer/download_peer.rs:74-89,91-115,146-156,199-200; soulseek-rs-lib/src/client/downloads.rs:122-143; soulseek-rs-lib/src/download_store.rs:14-20,94-115
- **Problem:** A local output path is only the configured directory plus the remote basename. Any shorter existing .part is trusted as a prefix even though username, full remote path, and prior size are not persisted, and live downloads are keyed by token rather than output path. Two files named the same from different folders or peers can therefore append into one .part, truncate each other, race the final rename, or promote mixed bytes as a completed download. The deterministic witness is a four-byte prefix from album-a\01.mp3 resumed as album-b\01.mp3 with a larger size: the length check passes while the content is mixed. Proven by the path and append logic; no network reproduction was run.
- **Recommendation:** Give transfer staging a durable identity and make output installation explicit: reserve each final output path while live, record at least username, full virtual path, and expected size beside or within the staging name, refuse unidentified legacy partials or migrate them deliberately, and choose a no-clobber or unique-name final-install policy. Add regression tests for cross-peer same-basename concurrency and stale partial reuse.
- **Fix with:** durable-state
- **Effort:** M

### SOUL-12 · P1 · Reconnect replay can write operations before login or silently erase them

- **Area:** Server session recovery
- **Where:** soulseek-rs-lib/src/actor/server_actor/handlers.rs:16-23,461-477; soulseek-rs-lib/src/actor/server_actor.rs:559-568,628-633,669-675; soulseek-rs-lib/src/client/connection.rs:263-270; soulseek-rs-lib/src/client/downloads.rs:158-181
- **Problem:** While disconnected, non-login server operations are appended to one FIFO. A reconnecting Login is appended behind them, and connection establishment flushes the FIFO unchanged, so queued operations can reach the server before Login. If the reconnect dial fails, disconnect clears the whole queue. Public operations report success once accepted by the actor, so a download can remain queued indefinitely without its server request either replaying correctly or failing back to the caller. Proven by the queue ordering and clearing code; server enforcement of pre-login rejection was not externally tested.
- **Recommendation:** Make session replay explicit: either fail non-login operations when the session drops and propagate that failure to dependent transfers, or hold them in a post-login replay buffer that is flushed only after a successful login. Login must be the first frame of a new server session. Add ordering and reconnect-failure regression tests.
- **Fix with:** lifecycle-concurrency
- **Effort:** M

### SOUL-13 · P1 · Network-facing work still has no aggregate admission and ownership envelope

- **Area:** Concurrency and resource bounds
- **Where:** docs/roadmap.md:22-26; docs/network-contract.md:57-60; soulseek-rs-lib/src/actor/mod.rs:121; soulseek-rs-lib/src/client/connection.rs:54-57,171-201; soulseek-rs-lib/src/client/operations/transfers.rs:49-81; soulseek-rs-lib/src/client/uploads.rs:162-209,268-282; soulseek-rs-lib/src/peer/listen.rs:333,384-391,477-479; soulseek-rs/src/daemon/server/transport.rs:318-359
- **Problem:** RS-1 remains incomplete: actor mailboxes and the client operation channel are unbounded. The same envelope also excludes transfer threads and accepted control connections: downloads, uploads, and inbound file connections use plain thread::spawn with discarded handles, no global permit, no registry, and no shutdown join; daemon shutdown stops accepting control clients but does not own existing connection threads. A burst of peers or user requests can exhaust threads or memory and work can outlive its owner. The facts are proven directly in source and the roadmap; no new stress measurement was run.
- **Recommendation:** Finish RS-1 as one owned admission design: bounded actor and operation queues with explicit block/reject/drop policy, a bounded transfer registry with cancellation tokens and join ownership, builder-based spawn failure handling, and daemon control-connection shutdown ownership. Validate with the existing daemon RSS and stress workloads rather than claiming a budget first.
- **Fix with:** lifecycle-concurrency
- **Effort:** L

### SOUL-14 · P2 · Unsolicited compressed peer frames can allocate hundreds of MiB before correlation

- **Area:** Peer input resource boundaries
- **Where:** soulseek-rs-lib/src/utils/zlib.rs:62-70,88-120,134-153; soulseek-rs-lib/src/types.rs:113-118; soulseek-rs-lib/src/message/peer/shared_file_list.rs:40-47,69-73,139-149; soulseek-rs-lib/src/actor/peer_actor.rs:205-220
- **Problem:** Compressed search, folder, and browse payloads are fully materialized before parsing or outstanding-request correlation. The per-frame ceilings are 128 MiB and 2 GiB, with no process-wide or per-peer decompression budget, and a small valid deflate stream can reach those sizes. Several hostile peers can multiply the allocation even when later domain caps discard the results. Proven by the input-to-allocation path; no bomb payload was executed.
- **Recommendation:** Correlate compressed responses with outstanding requests before expensive inflation where the protocol permits it, reduce per-response plaintext budgets to domain sizes, add a process-wide decompression byte/concurrency budget, and move large listings toward streaming parse rather than one Vec. Measure the chosen ceilings with the benchmark workload.
- **Fix with:** security
- **Effort:** M

### SOUL-15 · P2 · Attached clients keep treating a recovered daemon session as lost

- **Area:** Remote session lifecycle
- **Where:** soulseek-rs/src/daemon/hub.rs:275-286; soulseek-rs/src/remote/mirror.rs:95-100; soulseek-rs/src/remote/mod.rs:471-478; soulseek-rs/src/commands/mod.rs:72-85; soulseek-rs/src/daemon/mod.rs:465-472; soulseek-rs-lib/src/types.rs:572-575
- **Problem:** The daemon reconnects a disconnected session and a successful login clears the library session watch, but a remote mirror stores the first session_loss event with get_or_insert and never clears it. The remote session then prefers that stale latch over live daemon status, so later truthful no-results, timeout, or transfer failures can be rewritten into exit 7 indefinitely. Proven by the state flow; no end-to-end reconnect was run.
- **Recommendation:** Make connectivity one canonical state owned by the daemon, with Disconnected to Live permitted and Displaced terminal. Publish recovery or update the mirror from live status instead of latching the first loss, and add a regression covering disconnect, successful relogin, then an ordinary empty result.
- **Fix with:** audit-implicit-state-machine
- **Effort:** M

### SOUL-16 · P2 · Paused downloads restart automatically after daemon recovery

- **Area:** Durable transfer state
- **Where:** soulseek-rs/src/persist/state.rs:24-45; soulseek-rs/src/daemon/mod.rs:578-594; soulseek-rs/src/daemon/proto/dto.rs:102-123
- **Problem:** The live lifecycle distinguishes Queued, InProgress, Paused, Completed, Cancelled, Failed, and TimedOut, but persistence collapses every unfinished state to completed false. Restore calls download for every incomplete row, so a transfer the user deliberately paused consumes queue position and bandwidth after a daemon restart without a new user decision. Proven by construction.
- **Recommendation:** Persist explicit resume intent rather than a boolean, with a migrated representation for paused transfers that restores them as paused until download.resume. Keep completed rows terminal and name any automatic retry policy for failed rows.
- **Fix with:** durable-state
- **Effort:** M

### SOUL-17 · P2 · Durable-state writes can fail once and silently lose restart state

- **Area:** Durable state and errors
- **Where:** soulseek-rs/src/ui/main_tui/persist.rs:158-183; soulseek-rs/src/daemon/mod.rs:620-638; soulseek-rs/src/persist/config.rs:281-286; soulseek-rs/src/persist/secret.rs:80-102; soulseek-rs/src/persist/state.rs:169-172
- **Problem:** The TUI marks a snapshot saved even when either write fails, suppressing later retries. The daemon discards state-save results entirely. config.toml is still written directly even though malformed config is fatal and other durable files already use temp-file plus rename. A full disk, permission failure, crash, or concurrent writer can therefore erase restart state or leave config unreadable without an operator-visible error. Proven structurally; no fault injection was run.
- **Recommendation:** Advance saved snapshots only after successful writes, keep dirty state eligible for retry, surface rate-limited daemon save failures, and use the existing atomic persistence primitive for config. Decide whether cross-process config mutation needs a lock or a single owning writer.
- **Fix with:** durable-state
- **Effort:** M

### SOUL-20 · P2 · Transfer direction is flattened into an unconditional download accept path

- **Area:** Peer protocol correctness
- **Where:** soulseek-rs-lib/src/types.rs:165-172,472-485; soulseek-rs-lib/src/message/server/message_factory.rs:250-275; soulseek-rs-lib/src/client/operations/transfers.rs:82-140
- **Problem:** Transfer.direction is an unconstrained u32, the parser always reads a direction-dependent size, and UpdateDownloadTokens never branches on direction. The codec writes direction 0 without size for a download request and direction 1 with size for an upload offer, yet an incoming direction-0 request is handled through the same download-token update path. Even when no local download matches, the peer receives an allowed transfer response rather than a denial. Proven statically from the wire shapes and control flow; no live peer reproduction was run.
- **Recommendation:** Model TransferRequest direction as a closed type that carries size only where the wire format does, dispatch download requests and upload offers separately, and deny requests that do not match a live queued transfer. Add protocol-level regression tests for direction 0, direction 1, unknown direction, truncation, and no matching token.
- **Fix with:** reference-parity
- **Effort:** M

### SOUL-21 · P2 · Server backpressure disconnects the control session instead of deferring writes

- **Area:** Server transport reliability
- **Where:** soulseek-rs-lib/src/actor/server_actor.rs:372,592-601; soulseek-rs-lib/src/actor/peer_actor.rs:688-724
- **Problem:** The server socket is nonblocking, but each outbound frame is written with one write_all plus flush and any error disconnects. On a congested or slow server connection, WouldBlock is expected backpressure rather than a fatal session error. The peer actor already has the correct partial-write/pending-write pattern, so the server actor is inconsistent with the transport contract. Structurally proven; no socket-pressure reproduction was run.
- **Recommendation:** Give the server actor the same bounded pending-write state machine as peer actors: account for partial writes, treat WouldBlock as retryable backpressure, bound queued bytes with explicit disconnect semantics, and test partial writes and recovery.
- **Fix with:** lifecycle-concurrency
- **Effort:** M

### SOUL-22 · P2 · Browse and search responses monopolize the singleton client operations loop

- **Area:** Performance and lifecycle
- **Where:** soulseek-rs-lib/src/client/operations/mod.rs:35-64,770-783; soulseek-rs-lib/src/shares/mod.rs:193-197,217-243; soulseek-rs-lib/src/message/peer/shared_file_list.rs:52-62; soulseek-rs-lib/src/client/operations/search.rs:71-81
- **Problem:** One thread receives every client operation while also sweeping and polling. Browse construction rebuilds the complete directory listing and compresses it synchronously inside that operation, and search responses scan and collect matches before applying response caps. A large share or hostile burst of metadata requests can delay unrelated transfers, queue polling, connection sweeps, and user operations behind serialization work. The cost path is structurally proven; no latency benchmark was run.
- **Recommendation:** Move listing construction/compression out of the singleton decision loop or precompute incremental share indexes and cached browse/search responses with explicit invalidation. Bound scan work per operation and measure a large-share browse/search burst against ordinary transfer progress before choosing the design.
- **Fix with:** make-it-fast
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
