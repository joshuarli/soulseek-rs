Prefix: SOUL
Next: 27

### SOUL-23 · P2 · Reconnect leaves report_cant_connect sending on a dead dispatcher

- **Area:** Recovery and error propagation
- **Where:** soulseek-rs-lib/src/client/connection.rs:339-352; soulseek-rs-lib/src/client/operations/mod.rs:253
- **Problem:** report_cant_connect clones ctx.server_sender, which is set once when a dispatcher starts and is never cleared on disconnect. After a server reconnect the old receiver is gone, send fails, and the error is discarded, so a peer that could not be reached never gets CantConnectToPeer. Proven by source flow; no reconnect reproduction was run.
- **Recommendation:** Clear and replace server_sender with session state, or route the report through the current server actor handle, and surface send failures instead of discarding them. Add a regression covering disconnect, successful relogin, then a failed peer connection.
- **Fix with:** lifecycle-concurrency
- **Effort:** M

### SOUL-24 · P2 · Peer actor carries stale pending_write bytes across a reconnect

- **Area:** Peer transport lifecycle
- **Where:** soulseek-rs-lib/src/actor/peer_actor.rs:895-902; soulseek-rs-lib/src/actor/server_actor.rs:707
- **Problem:** disconnect() clears the peer stream and state but not pending_write, while the server actor now clears its equivalent queue. A partially written frame held as backpressure can be flushed onto a new socket after reconnect, mixing bytes from two sessions. Proven structurally; no reconnect reproduction was run.
- **Recommendation:** Clear pending_write and related backpressure state in peer_actor::disconnect(), and add a regression that disconnects mid-frame, reconnects, and asserts the new socket starts at a frame boundary.
- **Fix with:** lifecycle-concurrency
- **Effort:** S

### SOUL-25 · P2 · Locally restored paused downloads cannot be resumed

- **Area:** Durable transfer state
- **Where:** soulseek-rs/src/ui/main_tui/mod.rs:110-140; soulseek-rs/src/ui/main_tui/downloads.rs:125-140; soulseek-rs/src/api.rs:122
- **Problem:** A locally owned TUI restores paused rows as display-only entries with a detached sender and no receiver, then calls client.resume_download on a transfer that was never added to the library store. The user's resume action is a silent no-op; daemon-attached resume works through the restored paused hold from SOUL-16. Proven by construction.
- **Recommendation:** Route local resume through the same held-row start path used by the daemon, or refuse the action with a visible message until that path exists; add a restart/resume regression for local mode.
- **Fix with:** durable-state
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

### SOUL-26 · P3 · Broken rustdoc link fails a strict library doc build

- **Area:** Documentation build
- **Where:** soulseek-rs-lib/src/client/social.rs:234; soulseek-rs-lib/src/client/mod.rs
- **Problem:** The queue-position doc links to Client::downloads, which does not exist. RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc -p soulseek-rs-lib --no-deps exits 101 with 'unresolved link to Client::downloads'. Proven by the command.
- **Recommendation:** Point the link at the real accessor or use plain text, then rerun the strict doc build.
- **Fix with:** none
- **Effort:** S

