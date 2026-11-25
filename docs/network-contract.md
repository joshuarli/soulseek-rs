# Network contract

This document describes the connections this application may make, what
triggers them, and the limits that keep a laptop predictable. All components
(runtime, daemon, CLI, TUI, index and transfers) stay on the same machine.
There is no relay, proxy, hosted service or remote download box.

Native Soulseek traffic is **not** encrypted by the daemon control transport
described below; the protocol frames are the reference client's frames.

## Connection classes

| Class | Purpose | Trigger | Transport / bind scope | Destination source | Default? | Concurrency, timeout, retry |
| --- | --- | --- | --- | --- | --- | --- |
| Central server | Login, search coordination, peer addresses, brokered connects | One session start (CLI/TUI/`serve`/daemon) | TCP outbound; no bind | `server` setting, default `server.slsknet.org:2416` | Yes, when a session opens | One per session; connect 10s, login verdict 30s, caller wait 45s; daemon reconnect backs off 1→60s and stops after 12 failures or an auth rejection |
| Soulseek peer listener | Incoming peer control and file-transfer connections | Session start with `enable_listen` | TCP listener on `0.0.0.0:<listener_port>`; ephemeral fallback when occupied | Local bind only | **Yes** | Port advertised is the bound port; 128 half-open handshakes, refused when full; 10s peer-init deadline, 4096-byte init cap; 100ms accept-error backoff |
| Established peer control | Browse, search replies, chat, transfer offers | Direct dial or accepted handshake | TCP, ephemeral local port | Server address or accepted socket | Yes, on demand | 512 peers; per-user replacement after 30s; 5s dial / 20s connect-state / 5min idle; 32MiB pending-write cap; 256KiB actor stacks |
| File transfers | Upload and download bytes | Accepted transfer offer or resume | TCP, ephemeral local port | Peer address, server-brokered connect, or `PierceFirewall` | Yes, on demand | Upload slots 10; queued uploads 1024 total / 16 per peer; pending brokered connects 256; pending upload serves 256 / 64 per peer, 60s TTL; dial 20s, read/write 5–30s; cancelled on shutdown; `.part` resume |
| Distributed parent candidates / parent | Leaf participation in distributed search | Server offers candidate parents | TCP outbound, ephemeral local port | Server's candidate list | Yes (leaf) | 10 candidates, at most one parent; dial 10s, idle link 1min, 64KiB frame cap; 50 searches/s admitted from the parent |
| Distributed children | Serve search stream to peers below us | Explicit `accept_children`, peer connects | TCP accepted on the peer listener | Incoming child socket | **No** | 10 children, speed-derived cap; 5s write timeout; searches broadcast only when fed |
| Pending brokered requests | Server asks the other peer to connect back | Direct dial failed | TCP via server coordination | Server's `ConnectToPeer` message | Yes, on demand | 20s token deadline, swept each second; caps above |
| Upload/download queues | Pair peers with transfer slots | Transfer request or user command | No socket of their own | Local state | Yes, on demand | Bounded queues above; explicit refusal when full; fair privileged-first then first-come ordering |
| Daemon clients / events / writers | Local control of the resident session | CLI/TUI attach | **Unix socket**, `<state-dir>/daemon.sock`, mode 0600 | Local filesystem path | Yes for a daemon | 64 clients, 1024-event and 1024-writer queues, 1MiB frame cap, 30s auth timeout, 1s bad-token delay; full event queue disconnects the subscriber; idle listeners wait on OS readiness with an explicit shutdown wakeup |
| Optional TCP daemon control | Control from a tunnel endpoint | Explicit `--bind ADDR` only | TCP, **loopback only**, plaintext | Operator-supplied `ADDR` | **No** | Same limits as Unix; token authentication; non-loopback binds are rejected before listening |
| Router mapping (UPnP-IGD / NAT-PMP) | Make the listener reachable behind a home router | Explicit `port_mapping` setting | UDP SSDP 239.255.255.250:1900 or NAT-PMP gateway; HTTP only to the gateway named in the SSDP reply from the same host | LAN gateway discovered on demand | **No** | `off` starts nothing; `auto` = UPnP then NAT-PMP; `upnp`/`natpmp` select one; 5s SSDP search, ~3.75s NAT-PMP transaction, finite lease renewed at half life, single failed renewal removes the mapping and stops; no fallback to a permanent lease |

The `portmap` subcommand is a separate, explicit diagnostic. It **actively
creates and then removes** a temporary mapping (UPnP first, then NAT-PMP),
independent of the session's `port_mapping`. It is not a passive reachability
check and is never run by startup, attachment, or a local-only command.

There are no public "what is my IP" calls or internet connectivity probes. A
UDP `connect()` used only to choose a local LAN address does not emit a
datagram. SSDP replies naming an address other than their sender are ignored;
unparsable replies are skipped rather than ending discovery.

## Defaults at a glance

Sharing is on and shares only the dedicated download directory
(`~/Sync/M/Soulseek`) unless roots are configured; an explicitly empty share
list disables sharing. The peer listener is on. Router mapping is off.
Distributed leaf participation is on; accepting children is off. Daemon control
is a private Unix socket; network-accessible control is off. Nothing here adds
telemetry or unrelated service discovery.

## Resource notes

Bounds are enforced before threads or sockets are spawned where the resource is
untrusted. Accepted half-open handshakes are refused (not queued) at capacity.
Upload and download work is bounded by slots, queue caps and deadlines; a
stalled peer cannot hold an upload slot forever. Search matching is budgeted per
source so a busy distributed parent cannot starve ordinary leaf searches.
Daemon event and writer queues disconnect a client that falls behind instead of
buffering without limit. A rejected relogin is terminal and is never retried;
transient failures keep a bounded slow retry, and daemon stop cancels an active
login wait. Broker-admission refusal fails the queued downloads it would
otherwise strand, with a retryable cause. Actor mailboxes and the client
operation channel are still unbounded: their admission control is outstanding
work, not a claimed resource bound.

The stress workload (`scripts/stress.sh`, `soulseek-rs-lib/examples/stress.rs`)
reports its thread count and peak RSS for the **whole process**, which includes
the mock peer swarm and local server fixture. Those numbers are a trend line,
not the application's own resource use. `scripts/daemon_rss.sh` measures the
daemon process alone under a mixed local load and records the revision and
measurement boundaries. Separate the application from mock peers and OS
`TIME_WAIT` entries when measuring sockets.

## Reachability limits

With mapping off, two firewalled peers may be unable to communicate; that is a
normal Soulseek limitation. Server-brokered reverse connections are part of
native connection coordination and are not a public file-relay service. A
mapping created by this session is removed on clean shutdown; a crash can only
rely on the finite lease expiring.
