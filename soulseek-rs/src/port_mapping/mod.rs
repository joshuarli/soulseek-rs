//! Optional, policy-controlled port mapping so a client behind a home router
//! can become reachable for incoming peer connections (browse/download of
//! firewalled peers and the server-brokered connect-back both need this).
//!
//! Mapping is OFF by default: nothing here is contacted unless the user opts
//! in. [`MappingPolicy::Auto`] tries UPnP-IGD first and then NAT-PMP; the other
//! values select exactly one backend and never contact the other. Everything
//! remains best-effort: a failure is logged and the program continues (the
//! user can still reach non-firewalled peers, and can forward a port manually).
//! Mappings are finite leases, renewed on a timer and removed on shutdown.

mod igd;
mod nat_pmp;

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use igd::{PortMappingProtocol, SearchOptions, search_gateway};
use nat_pmp::MapResponse;
use soulseek_rs::utils::logger::{self, LogLevel};

const MAPPING_DESCRIPTION: &str = "soulseek-rs";
/// Lease we request; both backends renew at half the granted interval.
const LEASE_SECS: u32 = nat_pmp::RECOMMENDED_LIFETIME;
/// A lease at or below this cannot be renewed before it expires.
const MIN_RENEW_SECS: u32 = 30;
/// Interactive NAT-PMP retransmit budget (~3.75s worst case), not the full 9.
const NATPMP_ATTEMPTS: u32 = 4;

/// Which router-mapping backends a session is allowed to contact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MappingPolicy {
    /// Never discover or map anything. The default.
    #[default]
    Off,
    /// UPnP-IGD first, NAT-PMP only if UPnP did not produce a mapping.
    Auto,
    /// UPnP-IGD only.
    Upnp,
    /// NAT-PMP only.
    Natpmp,
}

impl MappingPolicy {
    /// The backends this policy may contact, in order.
    #[must_use]
    pub const fn backends(self) -> &'static [Self] {
        match self {
            Self::Off => &[],
            Self::Auto => &[Self::Upnp, Self::Natpmp],
            Self::Upnp => &[Self::Upnp],
            Self::Natpmp => &[Self::Natpmp],
        }
    }
}

impl fmt::Display for MappingPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Upnp => "upnp",
            Self::Natpmp => "natpmp",
        })
    }
}

impl FromStr for MappingPolicy {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "no" => Ok(Self::Off),
            "auto" | "on" | "true" | "yes" => Ok(Self::Auto),
            "upnp" => Ok(Self::Upnp),
            "natpmp" | "nat-pmp" => Ok(Self::Natpmp),
            other => Err(format!(
                "port_mapping does not accept '{other}'; use one of: off, auto, upnp, natpmp"
            )),
        }
    }
}

/// A live port mapping and its renewal thread. Dropping it stops renewal and
/// removes the mapping (best-effort).
pub struct PortMapper {
    stop: Arc<AtomicBool>,
    done: Receiver<()>,
}

impl PortMapper {
    /// Spawn a background mapper for TCP `port`, or return `None` when mapping
    /// is off. Discovery/mapping happens on the thread so startup never blocks.
    ///
    /// `port` must be the port the listener actually bound, not the configured
    /// one: mapping an occupied port would forward traffic to another process.
    #[must_use]
    pub fn spawn(port: u16, policy: MappingPolicy) -> Option<Self> {
        if policy == MappingPolicy::Off {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (done_tx, done) = mpsc::channel();
        let _ = thread::Builder::new().name("port-mapper".into()).spawn(
            move || {
                run(port, policy, &thread_stop);
                let _ = done_tx.send(());
            },
        );
        Some(Self { stop, done })
    }
}

impl Drop for PortMapper {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wait briefly for the thread to remove its mapping, but NEVER freeze
        // shutdown: if it's still in a blocking discovery/mapping call (which
        // doesn't observe `stop`), detach it and let the finite lease expire.
        let _ = self.done.recv_timeout(Duration::from_secs(1));
    }
}

/// The outcome of a port-mapping probe: a verdict a script can branch on
/// plus the advice a human needs when it fails.
pub struct Probe {
    pub ok: bool,
    /// `upnp`, `natpmp`, or `none`.
    pub backend: &'static str,
    /// External `address:port` the mapping exposes, when the backend reports
    /// one.
    pub external: Option<String>,
    pub advice: String,
}

/// Synchronously probe whether automatic port mapping works on this network:
/// actively map `port` (UPnP then NAT-PMP) and then remove the probe mapping.
/// Backs the explicit `portmap` diagnostic; unlike a session, it is not
/// governed by the session's mapping policy and always tries both backends.
#[must_use]
pub fn probe(port: u16) -> Probe {
    let local_ip = local_lan_ip();
    if local_ip.is_none() {
        info("no LAN IP for UPnP; probe will only try NAT-PMP");
    }

    match map_once(local_ip, port, MappingPolicy::Auto) {
        Some((backend, _delay)) => {
            let (name, external) = match &backend {
                Backend::Upnp { gateway, .. } => (
                    "upnp",
                    gateway
                        .get_external_ip()
                        .ok()
                        .map(|ip| format!("{ip}:{port}")),
                ),
                Backend::NatPmp { .. } => ("natpmp", None),
            };
            remove(&backend); // clean up the probe mapping
            Probe {
                ok: true,
                backend: name,
                external,
                advice: format!(
                    "Automatic port mapping works. This probe created and then removed a TCP {port} mapping on \
                     your router, so firewalled peers and the server can \
                     reach you. Sessions only do this when port_mapping is set to auto, upnp or \
                      natpmp (off by default)."
                ),
            }
        }
        None => Probe {
            ok: false,
            backend: "none",
            external: None,
            advice: format!(
                "No UPnP/NAT-PMP router responded for TCP {port}. To \
                 browse/download firewalled peers, forward TCP {port} to this \
                 machine on your router (or enable UPnP there)."
            ),
        },
    }
}

/// Which backend created the active mapping, carrying what's needed to renew and
/// remove it.
enum Backend {
    Upnp {
        gateway: Box<igd::Gateway>,
        local_addr: SocketAddr,
        external_port: u16,
        /// The lease that the router accepted (0 == permanent).
        lease: u32,
    },
    NatPmp {
        gateway: SocketAddr,
        internal_port: u16,
        /// Granted lifetime in seconds. Zero means permanent and is rejected
        /// before a mapping is kept.
        lease: u32,
    },
}

fn run(port: u16, policy: MappingPolicy, stop: &AtomicBool) {
    if stop.load(Ordering::SeqCst) {
        return;
    }
    // Only look up the LAN IP when a backend that needs it is in scope; a
    // NAT-PMP-only session never contacts another backend or guesses an
    // address through a UDP connect.
    let local_ip = if policy
        .backends()
        .contains(&MappingPolicy::Upnp)
    {
        let ip = local_lan_ip();
        if ip.is_none() {
            warn(
                "could not determine this machine's LAN IP; skipping UPnP \
                  (you may need to forward the port manually)",
            );
        }
        ip
    } else {
        None
    };

    let Some((mut backend, delay)) = map_once(local_ip, port, policy) else {
        warn(&format!(
            "port mapping ({policy}) unavailable for port {port}; browsing/\
             downloading firewalled peers may fail unless you forward TCP \
             {port} on your router"
        ));
        return;
    };

    if stop.load(Ordering::SeqCst) {
        remove(&backend);
        return;
    }
    manage(&mut backend, delay, port, stop, sleep_until);
}

/// Half the granted lease, floored at 30s so a short lease cannot spin the
/// mapper into a hot re-map loop.
fn renewal_delay(lease: u32) -> Duration {
    Duration::from_secs(u64::from((lease / 2).max(MIN_RENEW_SECS)))
}

/// Renew `backend`'s mapping until `stop`, then remove it. Each renewal returns
/// the delay for the next one, derived from that renewal's granted lifetime.
fn manage(
    backend: &mut Backend,
    mut delay: Duration,
    port: u16,
    stop: &AtomicBool,
    mut sleep: impl FnMut(Duration, &AtomicBool) -> bool,
) {
    loop {
        if sleep(delay, stop) {
            break; // asked to stop
        }
        let Some(next) = renew(backend, port) else {
            // One failed renewal stops the loop: no tight retry, no worker
            // left behind. Remove what we created so no stale hole remains.
            warn("failed to renew the port mapping; removing it and stopping");
            remove(backend);
            return;
        };
        delay = next;
    }
    remove(backend);
}

/// Try exactly the backends the policy names, in order. Returns the backend
/// and the delay until its first renewal. Never falls through to an unnamed
/// backend.
fn map_once(
    local_ip: Option<IpAddr>,
    port: u16,
    policy: MappingPolicy,
) -> Option<(Backend, Duration)> {
    map_once_with(local_ip, port, policy, map_natpmp)
}

/// `natpmp` is injectable so a test can prove a backend is never contacted
/// when the policy does not name it.
fn map_once_with(
    local_ip: Option<IpAddr>,
    port: u16,
    policy: MappingPolicy,
    natpmp: impl Fn(u16) -> Option<(Backend, Duration)>,
) -> Option<(Backend, Duration)> {
    for backend in policy.backends() {
        let result = match backend {
            MappingPolicy::Upnp => local_ip.and_then(|ip| map_upnp(ip, port)),
            MappingPolicy::Natpmp => natpmp(port),
            MappingPolicy::Off | MappingPolicy::Auto => None,
        };
        if result.is_some() {
            return result;
        }
    }
    None
}

fn map_upnp(local_ip: IpAddr, port: u16) -> Option<(Backend, Duration)> {
    let options = SearchOptions {
        bind_addr: SocketAddr::new(local_ip, 0),
        timeout: Some(Duration::from_secs(5)),
        ..SearchOptions::default()
    };
    let gateway = search_gateway(options).ok()?;
    let local_addr = SocketAddr::new(local_ip, port);
    map_via_gateway(gateway, local_addr, port)
}

/// Map `port` on an already-discovered IGD `gateway`. Split out from discovery
/// so it can be exercised against a mock IGD in tests.
fn map_via_gateway(
    gateway: igd::Gateway,
    local_addr: SocketAddr,
    port: u16,
) -> Option<(Backend, Duration)> {
    // We advertise our listen port verbatim to the Soulseek server, so the
    // EXTERNAL port must equal it. If the exact port can't be mapped (e.g.
    // another LAN device already claimed it), a different external port would
    // be unreachable for our advertised address, so we give up rather than map
    // a useless port. A refused finite lease is likewise a refusal: never
    // silently fall back to a permanent mapping we cannot expire on crash.
    if let Err(e) = gateway.add_port(
        PortMappingProtocol::TCP,
        port,
        local_addr,
        LEASE_SECS,
        MAPPING_DESCRIPTION,
    ) {
        warn(&format!(
            "UPnP found a router but refused a {LEASE_SECS}s lease for port \
             {port} ({e}); no mapping was created — try a different \
             --listener-port or forward it manually"
        ));
        return None;
    }
    let delay = renewal_delay(LEASE_SECS);
    let external = gateway
        .get_external_ip()
        .map_or_else(|_| "?".to_string(), |ip| ip.to_string());
    info(&format!(
        "UPnP: mapped external {external}:{port} -> local {port}"
    ));
    Some((
        Backend::Upnp {
            gateway: Box::new(gateway),
            local_addr,
            external_port: port,
            lease: LEASE_SECS,
        },
        delay,
    ))
}

fn map_natpmp(port: u16) -> Option<(Backend, Duration)> {
    let gateway_ip = nat_pmp::default_gateway()?;
    let gateway =
        SocketAddr::new(IpAddr::V4(gateway_ip), nat_pmp::NAT_PMP_PORT);
    map_natpmp_at(gateway, port)
}

fn map_natpmp_at(gateway: SocketAddr, port: u16) -> Option<(Backend, Duration)> {
    let response =
        nat_pmp::map_tcp(gateway, port, port, LEASE_SECS, NATPMP_ATTEMPTS)
            .ok()?;
    let Some(delay) = natpmp_renewal_delay(response, port) else {
        let _ = nat_pmp::unmap_tcp(gateway, port, 2);
        return None;
    };
    info(&format!(
        "NAT-PMP: mapped external port {} -> local {port} (lease {}s)",
        response.external_port, response.lifetime
    ));
    Some((
        Backend::NatPmp {
            gateway,
            internal_port: port,
            lease: response.lifetime,
        },
        delay,
    ))
}

/// A grant is only usable if it keeps the advertised port and stays finite and
/// long enough to be renewed before it expires. Returns the next renewal delay.
fn natpmp_renewal_delay(response: MapResponse, port: u16) -> Option<Duration> {
    if response.external_port != port {
        warn(&format!(
            "NAT-PMP granted external port {} instead of {port}; removing the \
             mapping rather than advertising the wrong port — forward TCP \
             {port} manually if you need it",
            response.external_port
        ));
        return None;
    }
    if response.lifetime == 0 {
        warn(
            "NAT-PMP granted a permanent mapping instead of a finite lease; \
             removing it rather than leaving a mapping that cannot expire",
        );
        return None;
    }
    if response.lifetime <= MIN_RENEW_SECS {
        warn(&format!(
            "NAT-PMP granted a {}s lease, at or below the {MIN_RENEW_SECS}s \
             renewal floor; removing it rather than letting it expire unmanaged",
            response.lifetime
        ));
        return None;
    }
    Some(renewal_delay(response.lifetime))
}

fn renew(backend: &mut Backend, port: u16) -> Option<Duration> {
    match backend {
        Backend::Upnp {
            gateway,
            local_addr,
            external_port,
            lease,
        } => {
            if gateway
                .add_port(
                    PortMappingProtocol::TCP,
                    *external_port,
                    *local_addr,
                    *lease,
                    MAPPING_DESCRIPTION,
                )
                .is_err()
            {
                warn("UPnP refused to renew the port mapping");
                return None;
            }
            Some(renewal_delay(*lease))
        }
        Backend::NatPmp {
            gateway,
            internal_port,
            lease,
        } => {
            let response = nat_pmp::map_tcp(
                *gateway,
                *internal_port,
                port,
                *lease,
                NATPMP_ATTEMPTS,
            )
            .ok()?;
            let delay = natpmp_renewal_delay(response, port)?;
            *lease = response.lifetime;
            Some(delay)
        }
    }
}

fn remove(backend: &Backend) {
    match backend {
        Backend::Upnp {
            gateway,
            external_port,
            ..
        } => {
            let _ =
                gateway.remove_port(PortMappingProtocol::TCP, *external_port);
        }
        Backend::NatPmp {
            gateway,
            internal_port,
            ..
        } => {
            let _ = nat_pmp::unmap_tcp(*gateway, *internal_port, 2);
        }
    }
}

/// Sleep for `dur`, checking `stop` every 500ms. Returns `true` if `stop` was
/// signalled (so the caller should exit promptly).
fn sleep_until(dur: Duration, stop: &AtomicBool) -> bool {
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return true;
        }
        thread::sleep(Duration::from_millis(500));
    }
    stop.load(Ordering::SeqCst)
}

/// The LAN IP of the interface that routes to the internet, found by asking the
/// OS which local address a UDP socket would use to reach a public address. No
/// packets are sent (UDP `connect` only sets the default route).
fn local_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    // A loopback/unspecified address can't be a forward target.
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}

fn info(message: &str) {
    logger::log(LogLevel::Info, &format!("[port-mapping] {message}"));
}

fn warn(message: &str) {
    logger::log(LogLevel::Warn, &format!("[port-mapping] {message}"));
}

#[cfg(test)]
impl PortMapper {
    /// Spawn a mapper that uses ONLY the NAT-PMP backend against an injected
    /// gateway (no UPnP, no routing-table discovery), so the full spawn → map →
    /// renew → cleanup-on-drop lifecycle can be driven against a mock gateway.
    fn spawn_natpmp_test(port: u16, gateway: SocketAddr) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (done_tx, done) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("port-mapper-test".into())
            .spawn(move || {
                if let Some((mut backend, delay)) = map_natpmp_at(gateway, port)
                {
                    manage(&mut backend, delay, port, &thread_stop, sleep_until);
                }
                let _ = done_tx.send(());
            });
        Self { stop, done }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;

    /// A mock NAT-PMP gateway that answers every map-TCP request with the same
    /// granted values and forwards each raw request to the test.
    fn mock_gateway() -> (SocketAddr, mpsc::Receiver<Vec<u8>>) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = socket.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 64];
            // Answer many requests (initial map + cleanup unmap + any renews).
            while let Ok((n, from)) = socket.recv_from(&mut buf) {
                let request = buf[..n].to_vec();
                let internal = u16::from_be_bytes([request[4], request[5]]);
                let lifetime = u32::from_be_bytes([
                    request[8],
                    request[9],
                    request[10],
                    request[11],
                ]);
                // Response: version 0, opcode 130, result 0, echo internal port
                // and grant the SAME external port + requested lifetime.
                let mut reply = vec![0u8, 130, 0, 0];
                reply.extend_from_slice(&7u32.to_be_bytes()); // epoch
                reply.extend_from_slice(&internal.to_be_bytes());
                reply.extend_from_slice(&internal.to_be_bytes());
                reply.extend_from_slice(&lifetime.to_be_bytes());
                if tx.send(request).is_err() {
                    break;
                }
                let _ = socket.send_to(&reply, from);
            }
        });
        (addr, rx)
    }

    enum Reply {
        Grant { external: u16, lifetime: u32 },
        Refuse(u16),
    }

    fn map_reply(
        result: u16,
        internal: u16,
        external: u16,
        lifetime: u32,
    ) -> Vec<u8> {
        let mut reply = vec![0u8, 130];
        reply.extend_from_slice(&result.to_be_bytes());
        reply.extend_from_slice(&7u32.to_be_bytes());
        reply.extend_from_slice(&internal.to_be_bytes());
        reply.extend_from_slice(&external.to_be_bytes());
        reply.extend_from_slice(&lifetime.to_be_bytes());
        reply
    }

    /// The gateway thread and its recorded requests. Dropping the guard stops
    /// and joins the thread instead of leaving it parked in `recv_from`.
    struct GatewayGuard {
        requests: mpsc::Receiver<Vec<u8>>,
        stop: Arc<AtomicBool>,
        task: Option<thread::JoinHandle<()>>,
    }

    impl Drop for GatewayGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(task) = self.task.take() {
                let _ = task.join();
            }
        }
    }

    impl std::ops::Deref for GatewayGuard {
        type Target = mpsc::Receiver<Vec<u8>>;

        fn deref(&self) -> &Self::Target {
            &self.requests
        }
    }

    /// A gateway whose map replies are scripted in order. Unmap requests are
    /// always acknowledged and never consume a scripted reply.
    fn scripted_gateway(replies: Vec<Reply>) -> (SocketAddr, GatewayGuard) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        socket
            .set_read_timeout(Some(StdDuration::from_millis(20)))
            .unwrap();
        let addr = socket.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = Arc::clone(&stop);
        let task = thread::spawn(move || {
            let mut replies = replies.into_iter();
            let mut buf = [0u8; 64];
            while !task_stop.load(Ordering::Relaxed) {
                let (n, from) = match socket.recv_from(&mut buf) {
                    Ok(received) => received,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => break,
                };
                let request = buf[..n].to_vec();
                let internal =
                    u16::from_be_bytes(request[4..6].try_into().unwrap());
                let lifetime =
                    u32::from_be_bytes(request[8..12].try_into().unwrap());
                let reply = if lifetime == 0 {
                    map_reply(0, internal, 0, 0)
                } else {
                    match replies.next() {
                        Some(Reply::Grant { external, lifetime }) => {
                            map_reply(0, internal, external, lifetime)
                        }
                        Some(Reply::Refuse(code)) => {
                            map_reply(code, internal, 0, 0)
                        }
                        None => map_reply(5, internal, 0, 0),
                    }
                };
                if tx.send(request).is_err() {
                    break;
                }
                let _ = socket.send_to(&reply, from);
            }
        });
        (
            addr,
            GatewayGuard {
                requests: rx,
                stop,
                task: Some(task),
            },
        )
    }

    fn collect_requests(requests: &mpsc::Receiver<Vec<u8>>) -> Vec<Vec<u8>> {
        requests.try_iter().collect()
    }

    fn is_unmap(request: &[u8]) -> bool {
        requested_lifetime(request) == 0
    }

    fn requested_lifetime(request: &[u8]) -> u32 {
        u32::from_be_bytes(request[8..12].try_into().unwrap())
    }

    /// Let the first wait elapse, then stop. Reveals the deadlines `manage`
    /// computed without waiting for them in real time.
    fn stop_after_one_renewal(
        backend: &mut Backend,
        delay: Duration,
        port: u16,
    ) -> Vec<Duration> {
        let stop = AtomicBool::new(false);
        let mut delays = Vec::new();
        let mut calls = 0;
        manage(backend, delay, port, &stop, |duration, _| {
            delays.push(duration);
            calls += 1;
            calls >= 2
        });
        delays
    }

    #[test]
    fn renewal_recomputes_next_deadline_from_granted_lifetime() {
        let (gateway, sent) = scripted_gateway(vec![
            Reply::Grant {
                external: 2234,
                lifetime: 600,
            },
            Reply::Grant {
                external: 2234,
                lifetime: 120,
            },
        ]);
        let (mut backend, delay) = map_natpmp_at(gateway, 2234)
            .expect("the initial mapping should succeed");
        assert_eq!(delay, Duration::from_secs(300));

        let delays = stop_after_one_renewal(&mut backend, delay, 2234);
        assert_eq!(
            delays,
            [Duration::from_secs(300), Duration::from_secs(60)],
            "the next deadline must come from the granted 120s lease"
        );

        let sent = collect_requests(&sent);
        let maps: Vec<_> = sent.iter().filter(|r| !is_unmap(r)).collect();
        assert_eq!(maps.len(), 2);
        assert_eq!(requested_lifetime(maps[0]), LEASE_SECS);
        assert_eq!(
            requested_lifetime(maps[1]),
            600,
            "the renewal must ask for the currently granted lease"
        );
        assert_eq!(sent.iter().filter(|r| is_unmap(r)).count(), 1);
    }

    #[test]
    fn renewal_detects_a_changed_external_port_and_removes_the_mapping() {
        let (gateway, sent) = scripted_gateway(vec![
            Reply::Grant {
                external: 2234,
                lifetime: 600,
            },
            Reply::Grant {
                external: 40000,
                lifetime: 600,
            },
        ]);
        let (mut backend, delay) =
            map_natpmp_at(gateway, 2234).expect("initial map should succeed");

        let delays = stop_after_one_renewal(&mut backend, delay, 2234);
        assert_eq!(delays, [Duration::from_secs(300)]);

        let sent = collect_requests(&sent);
        assert_eq!(sent.iter().filter(|r| !is_unmap(r)).count(), 2);
        assert_eq!(
            sent.iter().filter(|r| is_unmap(r)).count(),
            1,
            "a mapping at an unadvertised port must be removed"
        );
    }

    #[test]
    fn renewal_rejects_a_permanent_grant_and_removes_the_mapping() {
        let (gateway, sent) = scripted_gateway(vec![
            Reply::Grant {
                external: 2234,
                lifetime: 600,
            },
            Reply::Grant {
                external: 2234,
                lifetime: 0,
            },
        ]);
        let (mut backend, delay) =
            map_natpmp_at(gateway, 2234).expect("initial map should succeed");

        stop_after_one_renewal(&mut backend, delay, 2234);
        let sent = collect_requests(&sent);
        assert_eq!(
            sent.iter().filter(|r| is_unmap(r)).count(),
            1,
            "a permanent mapping cannot be left behind"
        );
    }

    #[test]
    fn a_lease_at_or_below_the_renewal_floor_is_never_kept() {
        let (gateway, sent) = scripted_gateway(vec![Reply::Grant {
            external: 2234,
            lifetime: MIN_RENEW_SECS,
        }]);
        assert!(map_natpmp_at(gateway, 2234).is_none());
        let sent = collect_requests(&sent);
        assert_eq!(sent.len(), 2, "a refused mapping must be unmapped");
        assert!(is_unmap(&sent[1]));

        let (gateway, sent) = scripted_gateway(vec![
            Reply::Grant {
                external: 2234,
                lifetime: 600,
            },
            Reply::Grant {
                external: 2234,
                lifetime: MIN_RENEW_SECS,
            },
        ]);
        let (mut backend, delay) =
            map_natpmp_at(gateway, 2234).expect("initial map should succeed");
        stop_after_one_renewal(&mut backend, delay, 2234);
        let sent = collect_requests(&sent);
        assert_eq!(
            sent.iter().filter(|r| is_unmap(r)).count(),
            1,
            "a too-short renewal must be removed"
        );
    }

    #[test]
    fn renewal_failure_removes_the_mapping() {
        let (gateway, sent) = scripted_gateway(vec![
            Reply::Grant {
                external: 2234,
                lifetime: 600,
            },
            Reply::Refuse(2),
        ]);
        let (mut backend, delay) =
            map_natpmp_at(gateway, 2234).expect("initial map should succeed");

        stop_after_one_renewal(&mut backend, delay, 2234);
        let sent = collect_requests(&sent);
        assert_eq!(
            sent.iter().filter(|r| is_unmap(r)).count(),
            1,
            "a refused renewal must leave no stale mapping"
        );
    }

    #[test]
    fn cancellation_during_the_wait_removes_without_renewing() {
        let (gateway, sent) = scripted_gateway(vec![Reply::Grant {
            external: 2234,
            lifetime: 600,
        }]);
        let (mut backend, delay) =
            map_natpmp_at(gateway, 2234).expect("initial map should succeed");

        let stop = AtomicBool::new(true);
        manage(&mut backend, delay, 2234, &stop, sleep_until);
        let sent = collect_requests(&sent);
        assert_eq!(sent.len(), 2, "only the initial map and cleanup unmap");
        assert_eq!(requested_lifetime(&sent[0]), LEASE_SECS);
        assert!(is_unmap(&sent[1]));
    }

    #[test]
    fn service_maps_on_start_and_unmaps_on_drop() {
        let (gateway, requests) = mock_gateway();
        let mapper = PortMapper::spawn_natpmp_test(2234, gateway);

        // The first request must be a map with our full requested lifetime.
        let map_req = requests
            .recv_timeout(StdDuration::from_secs(3))
            .expect("mapper should send an initial map request");
        assert_eq!(u16::from_be_bytes([map_req[4], map_req[5]]), 2234);
        assert_eq!(
            u32::from_be_bytes([
                map_req[8],
                map_req[9],
                map_req[10],
                map_req[11]
            ]),
            LEASE_SECS,
            "initial map should request the full lease"
        );

        // Dropping the mapper must remove the mapping (a lifetime-0 request).
        // A map the gateway was slow to answer is sent again, and that resend
        // can arrive first.
        drop(mapper);
        let unmapped = std::iter::from_fn(|| {
            requests.recv_timeout(StdDuration::from_secs(3)).ok()
        })
        .any(|request| {
            u32::from_be_bytes([
                request[8],
                request[9],
                request[10],
                request[11],
            ]) == 0
        });
        assert!(unmapped, "cleanup should request lifetime 0");
    }

    /// A minimal in-process UPnP IGD: an HTTP server that answers every SOAP
    /// control request with a canned envelope containing the Add/Delete/GetIP
    /// response elements, and forwards each request body to the test.
    fn mock_igd() -> (SocketAddr, mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                // Read headers, then Content-Length bytes of body.
                let body = loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break String::new(),
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if let Some(hdr_end) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| {
                                l.strip_prefix("Content-Length: ").or_else(
                                    || l.strip_prefix("content-length: "),
                                )
                            })
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= hdr_end + 4 + len {
                            break text[hdr_end + 4..].to_string();
                        }
                    }
                };
                if tx.send(body).is_err() {
                    break;
                }
                let soap = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
<s:Body>
<u:AddPortMappingResponse xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1"></u:AddPortMappingResponse>
<u:DeletePortMappingResponse xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1"></u:DeletePortMappingResponse>
<u:GetExternalIPAddressResponse xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1"><NewExternalIPAddress>203.0.113.7</NewExternalIPAddress></u:GetExternalIPAddressResponse>
</s:Body>
</s:Envelope>"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{soap}",
                    soap.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (addr, rx)
    }

    fn igd_control_schema() -> std::collections::HashMap<String, Vec<String>> {
        let mut schema = std::collections::HashMap::new();
        schema.insert(
            "AddPortMapping".to_string(),
            [
                "NewRemoteHost",
                "NewExternalPort",
                "NewProtocol",
                "NewInternalPort",
                "NewInternalClient",
                "NewEnabled",
                "NewPortMappingDescription",
                "NewLeaseDuration",
            ]
            .iter()
            .map(ToString::to_string)
            .collect(),
        );
        schema.insert(
            "DeletePortMapping".to_string(),
            ["NewRemoteHost", "NewExternalPort", "NewProtocol"]
                .iter()
                .map(ToString::to_string)
                .collect(),
        );
        schema
    }

    #[test]
    fn upnp_maps_a_port_against_a_mock_igd() {
        let (igd_addr, requests) = mock_igd();
        let gateway = igd::Gateway {
            addr: igd_addr,
            control_url: "/ctl".to_string(),
            control_schema: igd_control_schema(),
        };

        let local_addr =
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 2234);
        let result = map_via_gateway(gateway, local_addr, 2234);

        let (backend, _delay) =
            result.expect("mapping against a healthy IGD should succeed");
        match backend {
            Backend::Upnp { external_port, .. } => {
                assert_eq!(
                    external_port, 2234,
                    "must map the exact listen port"
                );
            }
            Backend::NatPmp { .. } => panic!("expected a UPnP backend"),
        }

        // The IGD must have received an AddPortMapping SOAP call quoting our
        // external and internal port and the LAN client address.
        let body = requests
            .recv_timeout(StdDuration::from_secs(3))
            .expect("IGD should receive an AddPortMapping request");
        assert!(
            body.contains("<NewExternalPort>2234</NewExternalPort>"),
            "request should map external port 2234: {body}"
        );
        assert!(
            body.contains("<NewInternalPort>2234</NewInternalPort>"),
            "request should target internal port 2234: {body}"
        );
        assert!(
            body.contains("192.168.1.50"),
            "request should point at the LAN client: {body}"
        );
    }

    #[test]
    fn off_starts_no_worker_and_selects_no_backend() {
        assert!(PortMapper::spawn(2234, MappingPolicy::Off).is_none());
        assert!(MappingPolicy::Off.backends().is_empty());
        assert_eq!(
            MappingPolicy::Auto.backends(),
            &[MappingPolicy::Upnp, MappingPolicy::Natpmp]
        );
        assert_eq!(MappingPolicy::Upnp.backends(), &[MappingPolicy::Upnp]);
        assert_eq!(MappingPolicy::Natpmp.backends(), &[MappingPolicy::Natpmp]);
    }

    #[test]
    fn selecting_one_backend_never_contacts_the_other() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let natpmp = move |port: u16| {
            counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(port, 2234);
            None
        };
        assert!(map_once_with(None, 2234, MappingPolicy::Off, &natpmp).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(map_once_with(None, 2234, MappingPolicy::Upnp, &natpmp).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "upnp must not try nat-pmp");
        assert!(map_once_with(None, 2234, MappingPolicy::Natpmp, &natpmp).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(map_once_with(None, 2234, MappingPolicy::Auto, &natpmp).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 2, "auto falls through once");
    }

    /// A router that refuses a finite lease must not receive a second request
    /// asking for a permanent one.
    #[test]
    fn a_refused_finite_lease_does_not_become_a_permanent_mapping() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                let body = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break String::new();
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if let Some(header_end) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("Content-Length: ")
                                    .or_else(|| {
                                        line.strip_prefix("content-length: ")
                                    })
                            })
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= header_end + 4 + len {
                            break text[header_end + 4..].to_string();
                        }
                    }
                };
                if tx.send(body).is_err() {
                    break;
                }
                let body = "lease refused";
                let response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let gateway = igd::Gateway {
            addr,
            control_url: "/ctl".to_string(),
            control_schema: igd_control_schema(),
        };
        let local_addr =
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 2234);
        assert!(map_via_gateway(gateway, local_addr, 2234).is_none());
        let first = rx
            .recv_timeout(StdDuration::from_secs(3))
            .expect("one lease request should reach the router");
        assert!(
            first.contains(&LEASE_SECS.to_string()),
            "the request should ask for a finite lease: {first}"
        );
        assert!(
            rx.recv_timeout(StdDuration::from_millis(300)).is_err(),
            "a refused lease must not be retried as a permanent mapping"
        );
    }

    #[test]
    fn drop_does_not_hang_when_gateway_is_unresponsive() {
        // A bound-but-silent gateway: the mapper thread blocks in the NAT-PMP
        // retransmit budget (~3.75s). Dropping the mapper must NOT wait that
        // long — it detaches after a bounded grace period.
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = silent.local_addr().unwrap();
        let mapper = PortMapper::spawn_natpmp_test(2234, addr);
        // Ensure the thread is mid-transaction before we drop.
        thread::sleep(StdDuration::from_millis(100));

        let start = Instant::now();
        drop(mapper);
        let elapsed = start.elapsed();
        assert!(
            elapsed < StdDuration::from_millis(1500),
            "drop blocked for {elapsed:?}; it should detach promptly"
        );
        drop(silent);
    }
}
