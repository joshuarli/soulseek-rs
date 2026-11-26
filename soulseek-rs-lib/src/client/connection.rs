use super::{
    ActorSystem, Arc, AtomicBool, Client, ClientContext, ClientOperation,
    ConnectionType, DEFAULT_WORKER_CAPACITY, DownloadPeer, Listen, Peer,
    PeerRegistry, Result, RwLock, RwLockExt, ServerActor, ServerMessage,
    Shares, SoulseekRs, TcpStream, WorkRegistry, error, info, thread, trace,
    warn,
};
use crate::utils::semaphore::{BoundedSender, bounded_channel};
use std::net::TcpListener;
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Maximum operations waiting for the client loop. A full queue refuses the
/// newcomer and reports it; it is a bound on buffered work, not a claim about
/// what a session needs (the stress and RSS workloads supply that evidence).
const OPERATION_QUEUE_DEPTH: usize = 1024;

const LISTENER_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Ceiling on the wait for a login verdict. Generous enough for a slow server
/// and a retrying connect, short enough that an unattended caller ends.
const LOGIN_RESPONSE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(45);

/// Soft open-file limit below which a session should expect "too many open
/// files": every peer holds a socket, and one busy search talks to hundreds
/// of them.
const COMFORTABLE_OPEN_FILES: u64 = 1024;

/// Say up front when the process cannot hold a busy session's sockets, so an
/// operator reads a warning with a number in it instead of an EMFILE storm an
/// hour later. The embedding process owns its rlimits; the library only looks.
fn warn_if_open_file_limit_is_low() {
    // SAFETY: getrlimit only fills the struct it is handed.
    let limit = unsafe {
        let mut limit = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) != 0 {
            return;
        }
        limit.rlim_cur
    };
    if limit < COMFORTABLE_OPEN_FILES {
        warn!(
            "only {limit} open files allowed; a busy session wants \
             {COMFORTABLE_OPEN_FILES} or more — expect \"too many open \
             files\" under load"
        );
    }
}

struct ListenerReleased {
    done: Mutex<bool>,
    wake: Condvar,
}

impl ListenerReleased {
    const fn new() -> Self {
        Self {
            done: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    fn release(&self) {
        let mut done =
            self.done.lock().unwrap_or_else(PoisonError::into_inner);
        *done = true;
        self.wake.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut done =
            self.done.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, _) = self
                .wake
                .wait_timeout(done, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            done = next;
        }
        *done
    }
}

/// Owner of the peer listener. Drop signals stop, dials the listener to wake
/// its parked accept, waits for the port release and joins the thread — the
/// same path on normal disconnect, error and test failure.
pub struct ListenerOwner {
    stop: Arc<AtomicBool>,
    released: Arc<ListenerReleased>,
    // A clone of the bound socket: making it non-blocking unblocks the
    // accept loop even when a direct dial cannot reach it.
    listener: TcpListener,
    port: u16,
    handle: Option<thread::JoinHandle<()>>,
}

impl ListenerOwner {
    /// Explicit, bounded shutdown so a caller need not rely on `Drop` to
    /// block. Idempotent: a second call has no handle left to join.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.listener.set_nonblocking(true);
        if TcpStream::connect(("127.0.0.1", self.port)).is_ok()
            && !self.released.wait(LISTENER_RELEASE_TIMEOUT)
        {
            warn!(
                "[client] listener on {} did not release within {:?}",
                self.port, LISTENER_RELEASE_TIMEOUT
            );
        }
        if let Some(handle) = self.handle.take()
            && let Err(panic) = handle.join()
        {
            error!("[client] peer listener panicked: {panic:?}");
        }
    }
}

impl Drop for ListenerOwner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Disconnect stops the owners in the context for good, so a later connect
/// has to install fresh ones or it would spawn no server actor and no workers.
fn reset_owners(ctx: &mut ClientContext) {
    ctx.actor_system = Arc::new(ActorSystem::new());
    ctx.work = Arc::new(WorkRegistry::new(DEFAULT_WORKER_CAPACITY));
}

impl Client {
    pub fn connect(&mut self) -> Result<()> {
        // Re-entering would replace the actor handle and leave the first
        // server actor, socket and login verdict thread running unowned.
        if self.server_handle.is_some() {
            return Err(SoulseekRs::AlreadyConnected);
        }
        warn_if_open_file_limit_is_low();

        let (sender, message_reader): (
            BoundedSender<ClientOperation>,
            std::sync::mpsc::Receiver<ClientOperation>,
        ) = bounded_channel(OPERATION_QUEUE_DEPTH, "client operations");

        let mut ctx = self.context.write_safe()?;
        reset_owners(&mut ctx);
        let peer_registry = PeerRegistry::with_capacity(
            ctx.actor_system.clone(),
            sender.clone(),
            self.username.clone(),
            ctx.max_peers.clone(),
        );
        ctx.peer_registry = Some(peer_registry);
        ctx.operations = Some(sender.clone());

        let listen_sender = sender.clone();

        // Scan the shared directories once into the read-only index, and
        // report the real folder/file counts to the server on login.
        let roots: Vec<std::path::PathBuf> = self
            .shared_directories
            .iter()
            .filter(|dir| !dir.trim().is_empty())
            .map(std::path::PathBuf::from)
            .collect();
        let shares = if roots.is_empty() {
            Arc::new(Shares::empty())
        } else {
            let scanned = Shares::scan_many(&roots);
            info!(
                "Sharing {} files in {} folders from {} directories",
                scanned.file_count(),
                scanned.folder_count(),
                roots.len()
            );
            Arc::new(scanned)
        };
        let shared_folder_count = shares.folder_count();
        let shared_file_count = shares.file_count();
        ctx.shares = shares;
        ctx.shared_directories.clone_from(&self.shared_directories);

        if self.cancel.is_cancelled() {
            return Err(SoulseekRs::NotConnected);
        }

        // Bind before logging in: the port we advertise has to be the port we
        // hold, and a bind that fails outright is the caller's to see rather
        // than a panic on a thread nobody is watching.
        let listener = if self.enable_listen {
            let listener = Listen::bind(self.listen_port)?;
            self.bound_port = Some(listener.local_addr()?.port());
            Some(listener)
        } else {
            None
        };
        let mut server_actor = ServerActor::new(
            self.address.clone(),
            sender,
            self.bound_port.unwrap_or(self.listen_port),
            self.enable_listen,
            shared_folder_count,
            shared_file_count,
        );
        server_actor.set_session_watch(self.session.clone());
        server_actor.set_cancel(self.cancel.clone());

        self.server_handle = Some(ctx.actor_system.spawn_with_handle(
            server_actor,
            |actor, handle| {
                actor.set_self_handle(handle);
            },
        ));
        // A guard with a destructor lives to the end of its scope, not its
        // last use. Release it before starting threads that read the context.
        drop(ctx);

        if let Some(listener) = listener {
            let client_sender = listen_sender;
            let context = self.context.clone();
            let own_username = self.username.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let released = Arc::new(ListenerReleased::new());
            let loop_stop = Arc::clone(&stop);
            let loop_released = Arc::clone(&released);
            let port = listener.local_addr()?.port();
            let owner_listener = listener.try_clone()?;

            // Builder over thread::spawn: a thread the OS refuses must come
            // back as an error rather than panic through `connect`.
            let spawned = thread::Builder::new()
                .name("soulseek-peer-listener".to_string())
                .spawn(move || {
                    Listen::serve(
                        &listener,
                        client_sender,
                        context,
                        own_username,
                        loop_stop,
                    );
                    drop(listener);
                    loop_released.release();
                });
            match spawned {
                Ok(handle) => {
                    self.listener_owner = Some(ListenerOwner {
                        stop,
                        released,
                        listener: owner_listener,
                        port,
                        handle: Some(handle),
                    });
                }
                Err(e) => {
                    // Undo the channel and registry installed above: without
                    // a listener, neither may outlive this failed connect.
                    if let Ok(mut ctx) = self.context.write_safe() {
                        ctx.operations = None;
                        ctx.peer_registry = None;
                    }
                    return Err(e.into());
                }
            }
        }

        Self::listen_to_client_operations(
            message_reader,
            self.context.clone(),
            self.username.clone(),
        );

        Ok(())
    }

    /// Close the server connection and every peer connection.
    ///
    /// The server sees the socket close and marks us offline, which is what
    /// releases the username: a client that simply goes out of scope without
    /// this leaves a session the server still believes in, so mail addressed
    /// to us is delivered to a connection nobody is reading and later logins
    /// collide with our own ghost.
    ///
    /// Called automatically when the client is dropped. The peer listener's
    /// bound port, if one was opened, is free again when this returns, unless
    /// the listener cannot be woken within two seconds.
    pub fn disconnect(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            let _ = handle.stop();
        }
        let (registry, actors, workers) = match self.context.write_safe() {
            Ok(mut ctx) => {
                // Dropping our end of the operations channel is what lets its
                // thread finish once the peers and the listener have gone.
                ctx.operations = None;
                (
                    ctx.peer_registry.clone(),
                    Some(Arc::clone(&ctx.actor_system)),
                    Some(Arc::clone(&ctx.work)),
                )
            }
            Err(_) => (None, None, None),
        };
        if let Some(registry) = registry {
            registry.stop_all();
        }
        if let Some(actors) = actors {
            actors.stop_all();
        }
        if let Some(workers) = workers {
            workers.stop_all();
        }
        // The listener owner signals stop, dials the accept loop open, waits
        // for the port and joins its thread. Dropping it here runs that path
        // on every disconnect, including the implicit one in `Client::drop`.
        self.listener_owner.take();
        self.bound_port.take();
    }

    /// Log in and wait for the server's verdict.
    ///
    /// The wait is bounded: the actor only answers once it has a working
    /// connection, so a server that never completes the TCP handshake would
    /// otherwise block the caller forever. A caller that hits
    /// [`SoulseekRs::Timeout`] can retry or give up, but it always regains
    /// control.
    pub fn login(&self) -> Result<bool> {
        self.login_inner(None)
    }

    /// Log in, but also give up when `stop` is set. A daemon shuts down on a
    /// flag it owns; a server that withholds the login verdict must not hold
    /// the process open while the caller waits for its worker threads.
    pub fn login_with_stop(&self, stop: &AtomicBool) -> Result<bool> {
        self.login_inner(Some(stop))
    }

    fn login_inner(&self, stop: Option<&AtomicBool>) -> Result<bool> {
        info!("Logging in as {}", self.username);
        let Some(handle) = &self.server_handle else {
            return Err(SoulseekRs::NotConnected);
        };
        if self.cancel.is_cancelled()
            || stop.is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            return Err(SoulseekRs::NotConnected);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = handle.send(ServerMessage::Login {
            username: self.username.clone(),
            password: self.password.clone(),
            version: self.version,
            response: tx,
        });

        let deadline = Instant::now() + LOGIN_RESPONSE_TIMEOUT;
        loop {
            match rx.recv_timeout(LOGIN_POLL_INTERVAL) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout)
                    if self.cancel.is_cancelled()
                        || stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) =>
                {
                    return Err(SoulseekRs::NotConnected);
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {
                }
                Err(_) => return Err(SoulseekRs::Timeout),
            }
        }
    }

    /// Ask the server for a peer's address and open a direct control
    /// connection to it. Downloads queued for that peer are sent automatically
    /// once the connection is established.
    ///
    /// # Errors
    /// Returns [`SoulseekRs::NotConnected`] if the client is not connected.
    pub fn connect_peer(&self, username: &str) -> Result<()> {
        let handle = self
            .server_handle
            .as_ref()
            .ok_or(SoulseekRs::NotConnected)?;
        handle
            .send(ServerMessage::GetPeerAddress(username.to_string()))
            .map_err(|_| SoulseekRs::NotConnected)?;
        Ok(())
    }

    pub fn set_max_peers(&self, max_peers: usize) {
        if let Ok(ctx) = self.context.read_safe() {
            ctx.max_peers
                .store(max_peers.max(1), std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[allow(dead_code)]
    pub fn remove_peer(&self, username: &str) {
        let context = match self.context.read_safe() {
            Ok(c) => c,
            Err(e) => {
                error!("[client] remove_peer: {}", e);
                return;
            }
        };
        if let Some(ref registry) = context.peer_registry
            && let Some(handle) = registry.remove_peer(username)
        {
            let _ = handle.stop();
        }
    }

    pub(crate) fn register_peer_or_fail(
        client_context: &Arc<RwLock<ClientContext>>,
        peer: Peer,
        stream: Option<TcpStream>,
        reader: Option<crate::message::MessageReader>,
    ) -> bool {
        let username = peer.username.clone();
        let registered = {
            let context = match client_context.read_safe() {
                Ok(c) => c,
                Err(e) => {
                    error!("[client] register peer read: {}", e);
                    return false;
                }
            };
            if let Some(ref registry) = context.peer_registry {
                let protected = context.protected_peers();
                match registry
                    .register_peer_protected(peer, stream, reader, &protected)
                {
                    Ok(_) => true,
                    Err(e) => {
                        warn!(
                            "[client] refusing peer connection for {:?}: {:?}",
                            username, e
                        );
                        false
                    }
                }
            } else {
                trace!("PeerRegistry not initialized");
                false
            }
        };
        if !registered {
            Self::fail_queued_downloads(client_context, &username);
        }
        registered
    }

    /// Tell the server we could not reach the peer it asked us to connect to,
    /// quoting the token that peer is waiting on.
    pub(crate) fn report_cant_connect(
        client_context: &Arc<RwLock<ClientContext>>,
        token: u32,
        username: &str,
    ) {
        let sender = match client_context.read_safe() {
            Ok(ctx) => ctx.server_sender.clone(),
            Err(_) => return,
        };
        let Some(sender) = sender else { return };
        let message =
            crate::message::server::MessageFactory::build_cant_connect_to_peer(
                token, username,
            );
        let _ = sender.send(ServerMessage::SendMessage(message));
    }

    pub(crate) fn connect_to_peer(
        peer: Peer,
        client_context: Arc<RwLock<ClientContext>>,
        own_username: String,
        stream: Option<TcpStream>,
    ) {
        let peer_clone = peer.clone();
        trace!(
            "[client] connecting to {}, with connection_type: {}, and token {:?}",
            peer.username, peer.connection_type, peer.token
        );
        match peer.connection_type {
            ConnectionType::P => {
                Self::register_peer_or_fail(
                    &client_context,
                    peer_clone,
                    stream,
                    None,
                );
            }

            ConnectionType::F => {
                trace!(
                    "[client] downloading from: {}, {:?}",
                    peer.username, peer.token
                );
                let Some(token) = peer.token else {
                    error!(
                        "[client] F connection from {} without token",
                        peer.username
                    );
                    return;
                };
                let download_peer = DownloadPeer::new(
                    peer.username,
                    peer.host,
                    peer.port,
                    token,
                    false,
                    own_username,
                );

                let brokered = peer_clone.brokered;
                let username = peer_clone.username;
                match download_peer.download_file(
                    client_context.clone(),
                    None,
                    None,
                ) {
                    Ok((download, filename)) => {
                        trace!(
                            "[client] downloaded {} bytes {:?} ",
                            filename, download.size
                        );
                    }
                    Err(e) => {
                        trace!("[client] failed to download: {}", e);
                        // A file connection the server asked us to make and
                        // we could not complete owes that peer a
                        // CantConnectToPeer, exactly as a control connection
                        // does: it is waiting on this token.
                        if brokered {
                            Self::report_cant_connect(
                                &client_context,
                                token,
                                &username,
                            );
                        }
                    }
                }
            }
            ConnectionType::D => {
                // ponytail: we never dial out to adopt a child — children
                // reach us through the listener. Tell the peer rather than
                // leave it waiting; wire the outbound half if firewalled
                // children turn out to matter.
                if let (true, Some(token)) = (peer.brokered, peer.token) {
                    Self::report_cant_connect(
                        &client_context,
                        token,
                        &peer.username,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_operation_channel_reports_a_full_queue() {
        let (sender, _receiver) = bounded_channel::<ClientOperation>(
            OPERATION_QUEUE_DEPTH,
            "client operations",
        );
        for sequence in 0..OPERATION_QUEUE_DEPTH {
            sender
                .send(ClientOperation::OwnPrivileges(sequence as u32))
                .expect("every slot accepts one operation");
        }
        assert!(
            sender.send(ClientOperation::OwnPrivileges(u32::MAX)).is_err(),
            "one operation past the bound must be refused, not buffered"
        );
    }

    #[test]
    fn dropping_the_listener_owner_stops_and_joins_its_thread() {
        let bound = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = bound.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let released = Arc::new(ListenerReleased::new());
        let exited = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let loop_stop = Arc::clone(&stop);
        let loop_released = Arc::clone(&released);
        let loop_exited = Arc::clone(&exited);
        let handle = thread::Builder::new()
            .name("test-peer-listener".to_string())
            .spawn(move || {
                entered_tx.send(()).unwrap();
                while !loop_stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                loop_released.release();
                loop_exited.store(true, Ordering::Release);
            })
            .unwrap();
        let owner = ListenerOwner {
            stop,
            released,
            listener: bound.try_clone().unwrap(),
            port,
            handle: Some(handle),
        };
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(owner);
        assert!(
            exited.load(Ordering::Acquire),
            "the owner must join a thread it signalled to stop"
        );
    }

    #[test]
    fn disconnect_then_connect_installs_fresh_owners() {
        struct TestActor;
        impl crate::actor::Actor for TestActor {
            type Message = ();
            fn handle(&mut self, (): ()) {}
        }

        let mut client = Client::new("peer", "secret");
        client.disconnect();

        {
            let ctx = client.context.read().expect("context");
            assert!(
                ctx.actor_system
                    .try_spawn_with_handle(TestActor, |_, _| {})
                    .is_err(),
                "disconnect must stop the actor system"
            );
            assert!(
                ctx.work.try_spawn("test", None, |_| {}).is_err(),
                "disconnect must stop the worker registry"
            );
        }

        {
            let mut ctx = client.context.write().expect("context");
            reset_owners(&mut ctx);
        }

        let ctx = client.context.read().expect("context");
        assert!(
            ctx.actor_system
                .try_spawn_with_handle(TestActor, |_, _| {})
                .is_ok(),
            "a new session must get a live actor system"
        );
        assert!(
            ctx.work.try_spawn("test", None, |_| {}).is_ok(),
            "a new session must get a live worker registry"
        );
    }
}
