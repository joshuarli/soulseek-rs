//! The resident service: one logged-in session, controlled over a socket.
//!
//! Where a one-shot command logs in, does its job and exits, the daemon logs in
//! once and stays. That is what makes cross-invocation work possible — a queue
//! that survives the command that filled it, a chat window that was open while
//! nobody was watching — and what lets several clients share one session
//! instead of fighting the server's one-login-per-account rule.
//!
//! Two transports, two answers to "who is this":
//!
//! * A Unix socket at `<state-dir>/daemon.sock`, mode 0600. The socket's own
//!   permissions are the authentication, the same bargain Docker and ssh-agent
//!   make, so a local client — an agent especially — needs no flags and no
//!   token.
//! * TCP, opt-in and loopback-only, always requiring the token in
//!   `<state-dir>/daemon.token`. There is no TLS support: a non-loopback bind
//!   is rejected before the listener opens rather than sending the token in
//!   clear text.

pub mod hub;
/// Generates `docs/openrpc.json` from the wire types and checks it is current.
/// Test-only: the published document is served at runtime by `include_str!`.
#[cfg(test)]
mod openrpc;
pub mod proto;
pub mod server;

use crate::api::SessionApi;
use crate::commands::{Ctx, Session};
use crate::output::{CliError, CliResult, Out};
use crate::persist::state::{
    PersistedDownload, PersistedMessage, ResumeIntent, SAVE_WARN_INTERVAL,
    StateStore,
};
use hub::{Hub, PendingBrowses, PendingDownload};
use proto::ChatMessageDto;
use server::Daemon;
use soulseek_rs::SessionLoss;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The control socket, where a local client looks without being told to.
pub const SOCKET_NAME: &str = "daemon.sock";
/// The shared secret for TCP clients — the cookie-file convention, so a local
/// client can pick it up without the operator handling it at all.
pub const TOKEN_NAME: &str = "daemon.token";

/// How often daemon-owned state is written back. Frequent enough that a kill
/// loses little, rare enough not to churn the disk on a busy queue.
const SAVE_INTERVAL: Duration = Duration::from_secs(5);

const RECONNECT_BACKOFF_CAP: Duration = Duration::from_mins(1);
/// Fast retries cover an ordinary drop: they recover in seconds and never
/// hammer the server. Past this budget the outage is long, so the worker
/// switches to a slow loop rather than giving up — a restored network must
/// bring the daemon back without a restart.
const FAST_RETRY_BUDGET: u32 = 12;
/// A long outage costs one login attempt per minute, bounded in rate but not
/// in count, until the server answers or the daemon is stopped.
const SLOW_RETRY_INTERVAL: Duration = RECONNECT_BACKOFF_CAP;

/// Deliberately paused transfers, held outside the session until `resume`.
/// Bounded so a restart cannot accumulate decisions without limit.
pub type PausedDownloads = Arc<Mutex<Vec<PersistedDownload>>>;

/// Full-queue policy for paused transfers: reject the newcomer and say so,
/// keeping every earlier decision intact.
const PAUSED_LIMIT: usize = 4096;

fn remember_paused(paused: &Mutex<Vec<PersistedDownload>>, row: PersistedDownload) {
    let mut paused = match paused.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            soulseek_rs::warn!(
                "paused transfer store was poisoned; recovering its contents"
            );
            poisoned.into_inner()
        }
    };
    let key = (row.username.as_str(), row.filename.as_str());
    if paused
        .iter()
        .any(|p| (p.username.as_str(), p.filename.as_str()) == key)
    {
        return;
    }
    if paused.len() >= PAUSED_LIMIT {
        soulseek_rs::warn!(
            "paused transfer store is full; not restoring {}",
            row.filename
        );
        return;
    }
    paused.push(row);
}

#[must_use]
pub fn socket_path() -> Option<PathBuf> {
    crate::persist::paths::state_dir().map(|dir| dir.join(SOCKET_NAME))
}

#[must_use]
pub fn token_path() -> Option<PathBuf> {
    crate::persist::paths::state_dir().map(|dir| dir.join(TOKEN_NAME))
}

/// The token a local client should present, when there is one to find.
#[must_use]
pub fn stored_token() -> Option<String> {
    let token = std::fs::read_to_string(token_path()?).ok()?;
    let token = token.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Read the token, or mint one on first start.
///
/// Written 0600 and never printed to stdout, so `daemon` output stays usable in
/// a pipe while the secret is still one `daemon token` away.
pub fn load_or_create_token(path: &Path) -> std::io::Result<String> {
    if let Some(existing) = std::fs::read_to_string(path)
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
    {
        // An older token file may have been created under a permissive umask.
        restrict(path)?;
        return Ok(existing);
    }

    let token = mint_token();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Created 0600 from the first write, then renamed into place: there is no
    // window in which another local user can read the token from a file made
    // with the process umask.
    let staging = path.with_extension(format!("token-{}", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        file.write_all(format!("{token}\n").as_bytes())?;
    }
    std::fs::rename(&staging, path)?;
    Ok(token)
}

/// 256 bits from the OS, hex-encoded. Long enough that the one-second penalty
/// on a wrong answer is the only rate limit the daemon needs.
fn mint_token() -> String {
    use std::fmt::Write;

    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS always has randomness");
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut token, byte| {
            let _ = write!(token, "{byte:02x}");
            token
        })
}

/// Make a secret readable by nobody else.
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Whether a daemon is listening on the local socket right now.
///
/// This is what makes daemon mode seamless: a client that finds a live socket
/// uses it, and one that does not behaves exactly as it always has. A socket
/// file left by a crash answers `None`, so a stale file cannot strand a client.
#[must_use]
pub fn local_daemon() -> Option<PathBuf> {
    let path = socket_path()?;
    std::os::unix::net::UnixStream::connect(&path)
        .ok()
        .map(|_| path)
}

/// Run the daemon until it is asked to stop.
pub fn run(
    ctx: &Ctx,
    bind: Option<&str>,
    upload_slots: Option<u16>,
) -> CliResult {
    let state_dir = crate::persist::paths::state_dir().ok_or_else(|| {
        CliError::usage("no state directory: set SOULSEEK_STATE_DIR")
    })?;
    std::fs::create_dir_all(&state_dir).map_err(|e| {
        CliError::usage(format!(
            "cannot use state directory {}: {e}",
            state_dir.display()
        ))
    })?;

    let token = load_or_create_token(&state_dir.join(TOKEN_NAME))
        .map_err(|e| CliError::usage(format!("cannot read the token: {e}")))?;

    // Bind before logging in: a second daemon should fail in a millisecond,
    // not after a full login handshake against the server.
    let mut listeners = Vec::new();
    let path = state_dir.join(SOCKET_NAME);
    listeners.push(server::bind_unix(&path).map_err(|e| {
        CliError::usage(format!("cannot listen on {}: {e}", path.display()))
    })?);
    ctx.out.status(&format!("listening on {}", path.display()));
    if let Some(address) = bind {
        listeners.push(server::bind_tcp(address).map_err(|e| {
            CliError::connection(format!("cannot listen on {address}: {e}"))
        })?);
        ctx.out.status(&format!(
            "listening on {address} (remote clients need the token from \
             `soulseek-rs daemon token`)"
        ));
    }
    let session = Session::open(ctx)?;
    if let Some(slots) = upload_slots {
        session.client.set_upload_slots(usize::from(slots));
    }

    let store = StateStore::new(state_dir);
    let hub = Arc::new(Hub::new());
    let browses = Arc::new(PendingBrowses::default());
    let downloads = Arc::new(Mutex::new(Vec::new()));
    let paused: PausedDownloads = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));

    let rooms = restore(
        ctx,
        &store,
        session.client.as_ref(),
        &hub,
        &downloads,
        &paused,
    );

    let daemon = Arc::new(Daemon {
        session: Arc::clone(&session.client),
        secrets: Arc::new(crate::persist::secret::FileStore::default()),
        hub: Arc::clone(&hub),
        browses: Arc::clone(&browses),
        downloads: Arc::clone(&downloads),
        paused: Arc::clone(&paused),
        download_dir: Mutex::new(ctx.download_dir.clone()),
        use_username_dirs: ctx.use_username_dirs,
        server: ctx.settings.server_address.to_string(),
        token: token.clone(),
        started: Instant::now(),
        stop: Arc::clone(&stop),
        open: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        rooms: Mutex::new(rooms),
        searches: Mutex::new(std::collections::HashMap::new()),
    });

    // Every long-lived thread the daemon starts is owned here. On shutdown
    // the same flag that ends this loop ends them, and joining keeps a test
    // or a second daemon start from inheriting stale listeners.
    let mut tasks: Vec<(&'static str, std::thread::JoinHandle<()>)> = Vec::new();

    {
        let session = Arc::clone(&session.client);
        let hub = Arc::clone(&hub);
        let stop = Arc::clone(&stop);
        tasks.push((
            "hub",
            std::thread::spawn(move || {
                hub::run(session, hub, browses, downloads, stop);
            }),
        ));
    }

    {
        let session = Arc::clone(&session.client);
        let daemon = Arc::clone(&daemon);
        let stop = Arc::clone(&stop);
        let out = ctx.out.clone();
        let address = ctx.settings.server_address.to_string();
        tasks.push((
            "reconnect",
            std::thread::spawn(move || {
                reconnect_loop(&session, &daemon, &out, &address, &stop);
            }),
        ));
    }

    let (wakeup, wake_read) = server::ListenerWakeup::new()
        .expect("a wakeup pipe for the control listeners");
    for listener in listeners {
        let daemon = Arc::clone(&daemon);
        let stop = Arc::clone(&stop);
        let wake_read = wake_read
            .try_clone()
            .expect("a per-listener wakeup descriptor");
        tasks.push((
            "control listener",
            std::thread::spawn(move || {
                listener.serve(daemon, &stop, Some(wake_read), None);
            }),
        ));
    }

    let (folders, files) = session.client.shared_counts();
    ctx.out.status(&format!(
        "logged in as {}, sharing {files} files in {folders} folders",
        session.client.username()
    ));
    ready(ctx, bind, &token);

    // The listeners run on their own threads; this one only has to notice a
    // `daemon.stop` and write state back on the way past.
    let mut last_save = Instant::now();
    let mut last_save_warning = None;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(hub::POLL);
        if last_save.elapsed() >= SAVE_INTERVAL {
            last_save = Instant::now();
            report_save_failures(
                &ctx.out,
                &save(&store, session.client.as_ref(), &hub, &daemon),
                &mut last_save_warning,
                Instant::now(),
            );
        }
    }

    report_save_failures(
        &ctx.out,
        &save(&store, session.client.as_ref(), &hub, &daemon),
        &mut last_save_warning,
        Instant::now(),
    );
    ctx.out.status("stopping");
    // Wake listeners before joining: they are parked in readiness waiting and
    // would otherwise spend the whole stop-check interval still accepting.
    wakeup.wake();
    // Joining keeps a half-written state file from racing the process exit
    // and leaves no listener or reconnect thread behind.
    for (name, task) in tasks {
        if let Err(panic) = task.join() {
            ctx.out.warn(&format!("the {name} thread panicked: {panic:?}"));
        }
    }
    cleanup();
    Ok(())
}

/// Say what is now possible, once, on stderr.
///
/// The token goes here rather than only into a file because the moment you
/// need it is the moment you bind a port, and hunting for `daemon token`
/// afterwards is a poor first experience. stdout stays empty either way, so
/// `daemon` still composes in a pipeline.
fn ready(ctx: &Ctx, bind: Option<&str>, token: &str) {
    const BANNER: &str = "
ready — leave this running.

  On this machine, nothing else to do. Other commands find
  it on their own and share this one login:

      soulseek-rs search 'aphex twin'
      soulseek-rs                       (the TUI attaches)
      soulseek-rs daemon status
      soulseek-rs daemon stop
";
    const LOCAL_ONLY: &str =
        "  Optional TCP control: --bind 127.0.0.1:PORT, reachable over an
  encrypted tunnel; plaintext remote binds are refused.
";

    ctx.out.status(BANNER);

    let Some(address) = bind else {
        ctx.out.status(LOCAL_ONLY);
        return;
    };

    ctx.out.status(&format!(
        "  Over an encrypted tunnel, point a client at {address} with
  this token:

      {token}

      soulseek-rs config set daemon 127.0.0.1:PORT
      soulseek-rs config set daemon_token <token above>

  This port is plaintext loopback only. Non-loopback binds are refused; \
         reach remote machines over an encrypted tunnel to loopback.
"
    ));
}

/// Remove the socket file so the next start does not have to reason about
/// whether it is stale.
fn cleanup() {
    if let Some(path) = socket_path() {
        let _ = std::fs::remove_file(path);
    }
}

const fn should_reconnect(loss: Option<SessionLoss>, owned: bool) -> bool {
    owned && matches!(loss, Some(SessionLoss::Disconnected))
}

/// The slice of a session the reconnect worker needs. Keeping it narrow lets
/// the retry policy be tested without a logged-in client or a server.
trait ReconnectSession {
    fn is_owned(&self) -> bool;
    fn loss(&self) -> Option<SessionLoss>;
    fn relogin(&self, stop: &AtomicBool) -> soulseek_rs::Result<bool>;
}

impl ReconnectSession for Arc<dyn SessionApi> {
    fn is_owned(&self) -> bool {
        self.daemon_endpoint().is_none()
    }
    fn loss(&self) -> Option<SessionLoss> {
        SessionApi::session_loss(self.as_ref())
    }
    fn relogin(&self, stop: &AtomicBool) -> soulseek_rs::Result<bool> {
        SessionApi::relogin_with_stop(self.as_ref(), stop)
    }
}

trait ReconnectReporter {
    fn status(&self, message: &str);
    fn warn(&self, message: &str);
}

impl ReconnectReporter for Out {
    fn status(&self, message: &str) {
        Self::status(self, message);
    }
    fn warn(&self, message: &str) {
        Self::warn(self, message);
    }
}

/// Waiting, injected so a long outage can be tested without waiting minutes.
trait ReconnectTimings {
    fn poll(&self) -> Duration;
    /// Wait out `duration`, returning `false` if the daemon was asked to stop.
    fn wait(&self, duration: Duration, stop: &AtomicBool) -> bool;
}

struct SystemTimings;

impl ReconnectTimings for SystemTimings {
    fn poll(&self) -> Duration {
        hub::POLL
    }
    fn wait(&self, duration: Duration, stop: &AtomicBool) -> bool {
        wait_or_stop(duration, stop)
    }
}

fn reconnect_loop(
    session: &Arc<dyn SessionApi>,
    daemon: &Daemon,
    out: &Out,
    address: &str,
    stop: &AtomicBool,
) {
    reconnect(
        session,
        address,
        stop,
        &SystemTimings,
        out,
        || rejoin(out, session.as_ref(), daemon),
    );
}

/// One worker relogins at a time. A rejected login and a displaced name are
/// terminal; a transport failure is not — after the fast budget the worker
/// keeps a slow, bounded rate going until the session returns or we stop.
fn reconnect<S: ReconnectSession, R: ReconnectReporter, T: ReconnectTimings>(
    session: &S,
    address: &str,
    stop: &AtomicBool,
    timings: &T,
    reporter: &R,
    rejoin: impl Fn(),
) {
    if !session.is_owned() {
        return;
    }
    let mut failures = 0u32;
    let mut announced = false;
    let mut slow = false;
    while !stop.load(Ordering::Relaxed) {
        if !timings.wait(timings.poll(), stop) {
            return;
        }
        let loss = session.loss();
        if !should_reconnect(loss, true) {
            if matches!(loss, Some(SessionLoss::Displaced)) {
                reporter.warn(&format!(
                    "another login displaced this session at {address}; not \
                     fighting for the name — fix the other client and restart"
                ));
                return;
            }
            failures = 0;
            announced = false;
            slow = false;
            continue;
        }
        if !announced {
            reporter.status(&format!(
                "the connection to {address} dropped; logging back in…"
            ));
            announced = true;
        }
        match session.relogin(stop) {
            Ok(true) => {
                reporter.status(&format!("logged back in to {address}"));
                rejoin();
                failures = 0;
                announced = false;
                slow = false;
            }
            Ok(false) => {
                reporter.warn(
                    "the server rejected the login; the daemon will not retry \
                     with rejected credentials — fix the password and restart",
                );
                return;
            }
            Err(e) => {
                // The server's rejection arrives as an error, not as
                // `Ok(false)`. Retrying it would keep fighting a name we
                // cannot log into, so it is terminal just like a refusal
                // reported as a verdict.
                if matches!(e, soulseek_rs::SoulseekRs::AuthenticationFailed) {
                    reporter.warn(
                        "the server rejected the login; the daemon will not \
                         retry with rejected credentials — fix the password \
                         and restart",
                    );
                    return;
                }
                if !slow && failures >= FAST_RETRY_BUDGET {
                    slow = true;
                    reporter.status(&format!(
                        "still disconnected from {address}; retrying every {}s \
                         until it comes back",
                        SLOW_RETRY_INTERVAL.as_secs()
                    ));
                }
                let wait = if slow {
                    SLOW_RETRY_INTERVAL
                } else {
                    backoff(failures)
                };
                failures = failures.saturating_add(1);
                reporter.warn(&format!(
                    "cannot log back in: {e}; retrying in {}s (attempt {failures}{})",
                    wait.as_secs(),
                    if slow { ", slow retries" } else { "" }
                ));
                if !timings.wait(wait, stop) {
                    return;
                }
            }
        }
    }
}

fn backoff(failures: u32) -> Duration {
    Duration::from_secs(1 << failures.min(6)).min(RECONNECT_BACKOFF_CAP)
}

fn wait_or_stop(total: Duration, stop: &AtomicBool) -> bool {
    let deadline = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(hub::POLL));
    }
    false
}

fn rejoin(out: &Out, session: &dyn SessionApi, daemon: &Daemon) {
    join_rooms(out, session, &daemon.rooms());
    for user in session.watched_users() {
        if let Err(e) = session.watch_user(&user) {
            out.warn(&format!("cannot watch {user} again: {e}"));
        }
    }
}

fn join_rooms(out: &Out, session: &dyn SessionApi, rooms: &[String]) {
    for room in rooms {
        if let Err(e) = session.join_room(room) {
            out.warn(&format!("cannot rejoin {room}: {e}"));
        }
    }
}

/// Split persisted transfers into the rows to re-queue and the rows to hold.
/// Completed rows are terminal; paused rows are the user's decision and must
/// not consume a queue position or bandwidth on restart.
fn plan_restore(
    rows: Vec<PersistedDownload>,
    paused: &Mutex<Vec<PersistedDownload>>,
) -> Vec<PersistedDownload> {
    let mut retries = Vec::new();
    for row in rows {
        match row.intent {
            ResumeIntent::Completed => {}
            ResumeIntent::Paused => remember_paused(paused, row),
            ResumeIntent::AutoRetry => retries.push(row),
        }
    }
    retries
}

/// Bring back what the last run left: re-queue transfers whose resume intent
/// is `AutoRetry`, hold paused ones until `download.resume`, rejoin rooms, and
/// reload the conversation.
fn restore(
    ctx: &Ctx,
    store: &StateStore,
    session: &dyn SessionApi,
    hub: &Hub,
    downloads: &Mutex<Vec<PendingDownload>>,
    paused: &Mutex<Vec<PersistedDownload>>,
) -> Vec<String> {
    hub.restore(
        store
            .load_messages()
            .into_iter()
            .map(|message| ChatMessageDto {
                peer: message.peer,
                outgoing: message.outgoing,
                text: message.text,
                at: message.at.unix(),
            })
            .collect(),
    );

    let rooms = store.load_rooms();
    join_rooms(&ctx.out, session, &rooms);

    let retries = plan_restore(store.load_downloads(), paused);
    if retries.is_empty() {
        return rooms;
    }
    ctx.out.status(&format!(
        "automatically retrying {} unfinished transfers",
        retries.len()
    ));
    for download in retries {
        match session.download(
            download.filename.clone(),
            download.username.clone(),
            download.size,
            download.download_directory.clone(),
        ) {
            Ok((_, updates)) => {
                if let Ok(mut pending) = downloads.lock() {
                    pending.push(PendingDownload {
                        username: download.username,
                        filename: download.filename,
                        updates,
                    });
                }
            }
            Err(e) => ctx.out.warn(&format!(
                "cannot retry {}: {e}",
                download.filename
            )),
        }
    }
    rooms
}

/// Which state files a save cycle failed to write.
#[derive(Debug, Default, PartialEq, Eq)]
struct SaveFailures(Vec<&'static str>);

impl SaveFailures {
    fn describe(&self) -> String {
        self.0.join(", ")
    }
}

/// Tell the operator about failed writes, at most once per interval. A full
/// disk fails every cycle; silence would hide lost restart state and a warning
/// per cycle would bury everything else in the log.
fn report_save_failures<R: ReconnectReporter>(
    reporter: &R,
    failures: &SaveFailures,
    last: &mut Option<Instant>,
    now: Instant,
) {
    if failures.0.is_empty() {
        return;
    }
    if last.is_some_and(|at| now.saturating_duration_since(at) < SAVE_WARN_INTERVAL)
    {
        return;
    }
    *last = Some(now);
    reporter.warn(&format!(
        "cannot save daemon state ({}); retrying on the next cycle",
        failures.describe()
    ));
}

/// Write daemon-owned state back. Every failure is carried out so the owner can
/// report it; none is a reason to drop a working session.
fn save(
    store: &StateStore,
    session: &dyn SessionApi,
    hub: &Hub,
    daemon: &Daemon,
) -> SaveFailures {
    let mut failures = SaveFailures::default();
    let mut downloads: Vec<PersistedDownload> = session
        .get_all_downloads()
        .iter()
        .filter_map(PersistedDownload::capture)
        .collect();
    let paused = match daemon.paused.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            soulseek_rs::warn!(
                "paused transfer store was poisoned; recovering its contents"
            );
            poisoned.into_inner()
        }
    };
    for row in paused.iter() {
        let listed = downloads.iter().any(|download| {
            download.username == row.username
                && download.filename == row.filename
        });
        if !listed {
            downloads.push(row.clone());
        }
    }
    if store.save_downloads(&downloads).is_err() {
        failures.0.push("downloads");
    }

    let messages: Vec<PersistedMessage> = hub
        .history()
        .into_iter()
        .map(|message| PersistedMessage {
            peer: message.peer,
            outgoing: message.outgoing,
            text: message.text,
            at: crate::time::Timestamp::from_unix(message.at),
        })
        .collect();
    if store.save_messages(&messages).is_err() {
        failures.0.push("messages");
    }
    if store.save_rooms(&daemon.rooms()).is_err() {
        failures.0.push("rooms");
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicUsize};

    #[test]
    fn a_token_is_minted_once_and_then_reused() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join(TOKEN_NAME);

        let first = load_or_create_token(&path).expect("a token is minted");
        let second = load_or_create_token(&path).expect("the token is reused");

        assert_eq!(
            first, second,
            "restarting must not lock existing clients out"
        );
        assert_eq!(first.len(), 64, "256 bits, hex encoded");
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn only_an_owned_dropped_session_reconnects() {
        assert!(should_reconnect(Some(SessionLoss::Disconnected), true));
        assert!(
            !should_reconnect(Some(SessionLoss::Displaced), true),
            "reconnecting a displaced name starts a login war with whoever \
             took it"
        );
        assert!(
            !should_reconnect(None, true),
            "a live session is not retried"
        );
        assert!(
            !should_reconnect(Some(SessionLoss::Disconnected), false),
            "a borrowed session is the daemon it came from to mend"
        );
    }

    #[test]
    fn the_backoff_doubles_and_then_holds_at_the_cap() {
        let seconds: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(seconds, [1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(
            backoff(u32::MAX),
            RECONNECT_BACKOFF_CAP,
            "a very long outage must not overflow into a tiny wait"
        );
    }

    #[test]
    fn two_tokens_are_not_the_same() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let first = load_or_create_token(&dir.path().join("a")).unwrap();
        let second = load_or_create_token(&dir.path().join("b")).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn a_token_file_is_readable_by_nobody_else() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join(TOKEN_NAME);
        load_or_create_token(&path).expect("a token is minted");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the token is a secret on disk");
    }

    #[test]
    fn an_existing_permissive_token_file_is_tightened_on_load() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join(TOKEN_NAME);
        std::fs::write(&path, "abcdef\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert_eq!(load_or_create_token(&path).unwrap(), "abcdef");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_blank_token_file_is_replaced_rather_than_trusted() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join(TOKEN_NAME);
        std::fs::write(&path, "   \n").unwrap();
        let token = load_or_create_token(&path).expect("a token is minted");
        assert_eq!(token.len(), 64, "an empty secret must never authenticate");
    }

    #[test]
    fn a_socket_left_by_a_dead_daemon_is_replaced_not_refused() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join(SOCKET_NAME);
        // A plain file standing in for what a crash left behind: nothing is
        // listening on it, so binding must succeed.
        std::fs::write(&path, "").unwrap();
        assert!(
            server::bind_unix(&path).is_ok(),
            "a stale socket must not strand the daemon"
        );
    }

    #[test]
    fn a_second_daemon_refuses_to_steal_a_live_socket() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join(SOCKET_NAME);
        let _first = server::bind_unix(&path).expect("the first daemon binds");
        assert!(
            server::bind_unix(&path).is_err(),
            "taking over a live socket would silently steal its clients"
        );
    }

    struct ScriptedSession {
        loss: Mutex<Option<SessionLoss>>,
        outcomes: Mutex<std::collections::VecDeque<soulseek_rs::Result<bool>>>,
        attempts: AtomicU32,
    }

    impl ScriptedSession {
        fn with(
            outcomes: impl IntoIterator<Item = soulseek_rs::Result<bool>>,
        ) -> Self {
            Self {
                loss: Mutex::new(Some(SessionLoss::Disconnected)),
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                attempts: AtomicU32::new(0),
            }
        }
    }

    impl ReconnectSession for ScriptedSession {
        fn is_owned(&self) -> bool {
            true
        }
        fn loss(&self) -> Option<SessionLoss> {
            *self.loss.lock().expect("session loss")
        }
        fn relogin(&self, _stop: &AtomicBool) -> soulseek_rs::Result<bool> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            let outcome = self
                .outcomes
                .lock()
                .expect("script")
                .pop_front()
                .unwrap_or(Ok(true));
            if matches!(outcome, Ok(true)) {
                *self.loss.lock().expect("loss") = None;
            }
            outcome
        }
    }

    /// A stub clock never sleeps, so a test cannot spend wall time waiting.
    /// It also stops a test whose script would otherwise keep it alive — zero
    /// duration polls must be counted too, or a zero backoff spins forever.
    const MAX_SCRIPTED_WAITS: usize = 64;
    const MAX_SCRIPTED_POLLS: usize = 256;

    #[derive(Default)]
    struct ScriptedTimings {
        waits: Mutex<Vec<Duration>>,
        polls: AtomicUsize,
    }

    impl ReconnectTimings for ScriptedTimings {
        fn poll(&self) -> Duration {
            Duration::ZERO
        }
        fn wait(&self, duration: Duration, stop: &AtomicBool) -> bool {
            if self.polls.fetch_add(1, Ordering::Relaxed) >= MAX_SCRIPTED_POLLS
            {
                stop.store(true, Ordering::Relaxed);
                return false;
            }
            let mut waits = self.waits.lock().expect("waits");
            if !duration.is_zero() {
                waits.push(duration);
            }
            if waits.len() >= MAX_SCRIPTED_WAITS {
                stop.store(true, Ordering::Relaxed);
                return false;
            }
            drop(waits);
            !stop.load(Ordering::Relaxed)
        }
    }

    #[derive(Default)]
    struct RecordingReporter {
        status: Mutex<Vec<String>>,
        warnings: Mutex<Vec<String>>,
    }

    impl ReconnectReporter for RecordingReporter {
        fn status(&self, message: &str) {
            self.status.lock().expect("status").push(message.to_string());
        }
        fn warn(&self, message: &str) {
            self.warnings.lock().expect("warnings").push(message.to_string());
        }
    }

    #[test]
    fn an_outage_past_the_old_budget_recovers_without_a_restart() {
        let outage = (0..13)
            .map(|_| Err(soulseek_rs::SoulseekRs::NotConnected))
            .chain(std::iter::once(Ok(true)));
        let session = ScriptedSession::with(outage);
        let timings = ScriptedTimings::default();
        let reporter = RecordingReporter::default();
        let stop = AtomicBool::new(false);
        let rejoined = AtomicU32::new(0);

        reconnect(
            &session,
            "server.test:2416",
            &stop,
            &timings,
            &reporter,
            || {
                rejoined.fetch_add(1, Ordering::Relaxed);
                stop.store(true, Ordering::Relaxed);
            },
        );

        assert_eq!(session.attempts.load(Ordering::Relaxed), 14);
        assert_eq!(rejoined.load(Ordering::Relaxed), 1, "rooms come back");
        assert!(
            timings
                .waits
                .lock()
                .expect("waits")
                .contains(&SLOW_RETRY_INTERVAL),
            "long outages fall back to the slow interval"
        );
        let status = reporter.status.lock().expect("status");
        assert!(status.iter().any(|m| m.contains("dropped")));
        assert!(status.iter().any(|m| m.contains("retrying every 60s")));
        assert!(status.iter().any(|m| m.contains("logged back in")));
        let warnings = reporter.warnings.lock().expect("warnings");
        assert!(warnings.iter().any(|m| m.contains("slow retries")));
        assert!(
            !warnings.iter().any(|m| m.contains("giving up")),
            "a transport failure is not a reason to stop forever"
        );
    }

    #[test]
    fn rejected_credentials_are_not_retried() {
        // Production reports a rejected login as `Err(AuthenticationFailed)`;
        // `Ok(false)` is not the contract this policy has to honour.
        let session = ScriptedSession::with([
            Err(soulseek_rs::SoulseekRs::AuthenticationFailed),
            Ok(true),
        ]);
        let reporter = RecordingReporter::default();
        reconnect(
            &session,
            "server.test:2416",
            &AtomicBool::new(false),
            &ScriptedTimings::default(),
            &reporter,
            || {},
        );
        assert_eq!(session.attempts.load(Ordering::Relaxed), 1);
        let warnings = reporter.warnings.lock().expect("warnings");
        assert!(warnings.iter().any(|m| m.contains("rejected the login")));
        assert_eq!(warnings.len(), 1, "no login fight with a bad password");
    }

    #[test]
    fn a_displaced_session_is_not_retried() {
        let session = ScriptedSession::with([Ok(true)]);
        *session.loss.lock().expect("loss") = Some(SessionLoss::Displaced);
        let reporter = RecordingReporter::default();
        reconnect(
            &session,
            "server.test:2416",
            &AtomicBool::new(false),
            &ScriptedTimings::default(),
            &reporter,
            || {},
        );
        assert_eq!(
            session.attempts.load(Ordering::Relaxed),
            0,
            "reconnecting a displaced name starts a login war"
        );
        let warnings = reporter.warnings.lock().expect("warnings");
        assert!(warnings.iter().any(|m| m.contains("displaced")));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_borrowed_session_is_never_reconnected() {
        struct Borrowed(ScriptedSession);
        impl ReconnectSession for Borrowed {
            fn is_owned(&self) -> bool {
                false
            }
            fn loss(&self) -> Option<SessionLoss> {
                self.0.loss()
            }
            fn relogin(&self, stop: &AtomicBool) -> soulseek_rs::Result<bool> {
                self.0.relogin(stop)
            }
        }
        let session = Borrowed(ScriptedSession::with([Ok(true)]));
        reconnect(
            &session,
            "server.test:2416",
            &AtomicBool::new(false),
            &ScriptedTimings::default(),
            &RecordingReporter::default(),
            || {},
        );
        assert_eq!(session.0.attempts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn waiting_stops_promptly_when_the_daemon_is_told_to() {
        let stop = AtomicBool::new(true);
        let start = Instant::now();
        assert!(!wait_or_stop(Duration::from_secs(60), &stop));
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "the 60s retry sleep must not delay a stop"
        );
    }

    fn persisted(filename: &str, intent: ResumeIntent) -> PersistedDownload {
        PersistedDownload {
            username: "peer".into(),
            filename: filename.into(),
            size: 42,
            download_directory: "/music".into(),
            intent,
        }
    }

    #[test]
    fn a_paused_transfer_is_held_instead_of_retried_after_recovery() {
        let paused = Mutex::new(Vec::new());
        let retries = plan_restore(
            vec![
                persisted("done.mp3", ResumeIntent::Completed),
                persisted("paused.mp3", ResumeIntent::Paused),
                persisted("failed.mp3", ResumeIntent::AutoRetry),
            ],
            &paused,
        );

        assert_eq!(
            retries
                .iter()
                .map(|row| row.filename.as_str())
                .collect::<Vec<_>>(),
            ["failed.mp3"],
            "only the named automatic retry policy re-queues a transfer"
        );
        assert_eq!(
            paused
                .lock()
                .expect("paused")
                .iter()
                .map(|row| row.filename.as_str())
                .collect::<Vec<_>>(),
            ["paused.mp3"],
            "a deliberate pause waits for download.resume"
        );
    }

    fn test_daemon() -> Daemon {
        Daemon {
            session: Arc::new(crate::ui::fixture::TalkativeSession::default()),
            secrets: Arc::new(crate::persist::secret::MemoryStore::default()),
            hub: Arc::new(Hub::new()),
            browses: Arc::new(PendingBrowses::default()),
            downloads: Arc::new(Mutex::new(Vec::new())),
            paused: Arc::new(Mutex::new(Vec::new())),
            download_dir: Mutex::new("/tmp".to_string()),
            use_username_dirs: false,
            server: "server.invalid:2416".to_string(),
            token: "t".to_string(),
            started: Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            open: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            rooms: Mutex::new(Vec::new()),
            searches: Mutex::new(std::collections::HashMap::new()),
        }
    }

    #[test]
    fn failed_daemon_state_writes_are_carried_out() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "").expect("a blocked state directory");
        let daemon = test_daemon();

        let failures = save(
            &StateStore::new(blocked),
            daemon.session.as_ref(),
            &daemon.hub,
            &daemon,
        );
        assert_eq!(failures.0, ["downloads", "messages", "rooms"]);
    }

    #[test]
    fn a_clean_save_reports_no_failures() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let store = StateStore::new(dir.path().join("state"));
        let daemon = test_daemon();
        let failures =
            save(&store, daemon.session.as_ref(), &daemon.hub, &daemon);
        assert!(failures.0.is_empty());
        assert!(dir.path().join("state").join("downloads.json").exists());
    }

    #[test]
    fn save_failure_warnings_are_rate_limited() {
        let reporter = RecordingReporter::default();
        let mut last = None;
        let failures = SaveFailures(vec!["downloads", "rooms"]);
        let now = Instant::now();

        report_save_failures(&reporter, &failures, &mut last, now);
        report_save_failures(
            &reporter,
            &failures,
            &mut last,
            now + Duration::from_secs(5),
        );
        let warnings = reporter.warnings.lock().expect("warnings");
        assert_eq!(warnings.len(), 1, "a full disk must not fill the log");
        assert!(warnings[0].contains("downloads, rooms"));
        drop(warnings);

        report_save_failures(
            &reporter,
            &failures,
            &mut last,
            now + SAVE_WARN_INTERVAL,
        );
        assert_eq!(
            reporter.warnings.lock().expect("warnings").len(),
            2,
            "the next interval surfaces the continuing failure again"
        );
    }

    #[test]
    fn the_paused_hold_rejects_a_duplicate_and_a_full_store() {
        let paused = Mutex::new(Vec::new());
        remember_paused(&paused, persisted("paused.mp3", ResumeIntent::Paused));
        remember_paused(&paused, persisted("paused.mp3", ResumeIntent::Paused));
        assert_eq!(
            paused.lock().expect("paused").len(),
            1,
            "the same decision must not be held twice"
        );

        let mut full = (0..PAUSED_LIMIT)
            .map(|n| persisted(&format!("{n}.mp3"), ResumeIntent::Paused))
            .collect::<Vec<_>>();
        let held = Mutex::new(std::mem::take(&mut full));
        remember_paused(&held, persisted("overflow.mp3", ResumeIntent::Paused));
        assert_eq!(
            held.lock().expect("paused").len(),
            PAUSED_LIMIT,
            "a full store rejects the newcomer instead of growing unbounded"
        );
    }
}
