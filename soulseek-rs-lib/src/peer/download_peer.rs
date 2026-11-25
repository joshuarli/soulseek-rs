use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::client::ClientContext;
use crate::download_store::{
    STAGING_IDENTITY_SUFFIX, identity_of, output_path_of,
};
use crate::message::server::MessageFactory;
use crate::trace;
use crate::types::{Download, DownloadStatus};
use crate::utils::path::PART_SUFFIX;

/// Read in 64 KiB slices: an eighth of the syscalls of the 8 KiB reads before,
/// and what a fast peer fills per read. slskd reads 256 KiB, Nicotine+ ~50 KiB.
const READ_BUFFER_SIZE: usize = 64 * 1024;
const CANCEL_POLL: Duration = Duration::from_secs(1);
const STALL_DEADLINE: Duration = Duration::from_secs(30);
/// Bytes between progress reports. Counting bytes rather than reads keeps
/// the report volume flat however big a read is, and gives a slow peer's
/// rate a window wide enough to mean something.
const PROGRESS_INTERVAL_BYTES: usize = 128 * 1024;

#[derive(Debug)]
pub enum DownloadError {
    ConnectionFailed(io::Error),
    InvalidAddress(String),
    HandshakeFailed(io::Error),
    StreamReadError(io::Error),
    StreamWriteError(io::Error),
    TokenNotFound(u32),
    DownloadInfoMissing(u32),
    FileWriteError(io::Error),
    PathResolutionError(String),
    LockPoisoned,
    Cancelled,
    IncompleteDownload { received: u64, expected: u64 },
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed(e) => write!(f, "Connection failed: {e}"),
            Self::InvalidAddress(addr) => {
                write!(f, "Invalid address: {addr}")
            }
            Self::HandshakeFailed(e) => write!(f, "Handshake failed: {e}"),
            Self::StreamReadError(e) => write!(f, "Stream read error: {e}"),
            Self::StreamWriteError(e) => write!(f, "Stream write error: {e}"),
            Self::TokenNotFound(token) => {
                write!(f, "Token not found: {token}")
            }
            Self::DownloadInfoMissing(token) => {
                write!(f, "Download info missing for token: {token}")
            }
            Self::FileWriteError(e) => write!(f, "File write error: {e}"),
            Self::PathResolutionError(msg) => {
                write!(f, "Path resolution error: {msg}")
            }
            Self::LockPoisoned => write!(f, "Lock poisoned"),
            Self::Cancelled => write!(f, "Download cancelled"),
            Self::IncompleteDownload { received, expected } => write!(
                f,
                "Incomplete download: received {received} of {expected} bytes"
            ),
        }
    }
}

impl std::error::Error for DownloadError {}

fn part_path(final_path: &str) -> String {
    format!("{final_path}{PART_SUFFIX}")
}

pub fn part_path_of(download: &Download) -> Option<String> {
    resolve_download_path(download)
        .ok()
        .map(|path| part_path(&path))
}

/// Where a download's bytes end up: its configured directory (falling back
/// to the parent if it names a file) joined with the remote basename.
///
/// A directory that does not exist yet stays the target — `PartFile::open`
/// creates it. Treating it as "not a directory" and dodging to the parent
/// made a daemon whose folder was never created download into `~/Downloads`
/// while reporting `~/Downloads/Soulseek`.
fn resolve_download_path(download: &Download) -> Result<String, DownloadError> {
    let final_path = output_path_of(download).ok_or_else(|| {
        DownloadError::PathResolutionError(format!(
            "Cannot resolve parent directory for: {}",
            download.download_directory
        ))
    })?;

    final_path
        .to_str()
        .ok_or_else(|| {
            DownloadError::PathResolutionError(format!(
                "Path contains invalid UTF-8: {}",
                final_path.display()
            ))
        })
        .map(String::from)
}

/// The `.part` file a transfer streams into, so an interrupted transfer leaves
/// behind what it managed to fetch and the next attempt can pick up from there.
/// A sidecar records the identity of the bytes; without a match the file is
/// migrated aside rather than appended to.
struct PartFile {
    file: File,
    final_path: String,
    part_path: PathBuf,
    identity_path: PathBuf,
    written: u64,
    expected: u64,
}

impl PartFile {
    fn open(download: &Download) -> Result<Self, DownloadError> {
        let final_path = resolve_download_path(download)?;
        let part_path = PathBuf::from(part_path(&final_path));
        let identity_path = PathBuf::from(format!(
            "{}{STAGING_IDENTITY_SUFFIX}",
            part_path.display()
        ));

        if let Some(parent) = part_path.parent() {
            fs::create_dir_all(parent)
                .map_err(DownloadError::FileWriteError)?;
        }

        let identity = identity_of(download);
        let recognised = fs::read_to_string(&identity_path).ok().as_deref()
            == Some(identity.as_str());
        let on_disk = fs::metadata(&part_path).map_or(0, |m| m.len());
        let written = if recognised && on_disk < download.size {
            on_disk
        } else {
            0
        };

        if !recognised && on_disk > 0 {
            Self::migrate_unidentified_part(&part_path, &identity_path)?;
        }

        let file = if written > 0 {
            OpenOptions::new().append(true).open(&part_path)
        } else {
            File::create(&part_path)
        }
        .map_err(DownloadError::FileWriteError)?;
        fs::write(&identity_path, identity).map_err(DownloadError::FileWriteError)?;

        Ok(Self {
            file,
            final_path,
            part_path,
            identity_path,
            written,
            expected: download.size,
        })
    }

    /// Preserve bytes we cannot attribute instead of mixing them into a new
    /// transfer. The `.legacy` name is never opened as a resume point.
    fn migrate_unidentified_part(
        part_path: &Path,
        identity_path: &Path,
    ) -> Result<(), DownloadError> {
        let base = format!("{}.legacy", part_path.display());
        for number in 1..u32::MAX {
            let legacy = if number == 1 {
                PathBuf::from(&base)
            } else {
                PathBuf::from(format!("{base}.{number}"))
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&legacy)
            {
                Ok(placeholder) => {
                    drop(placeholder);
                    if let Err(error) = fs::rename(part_path, &legacy) {
                        let _ = fs::remove_file(&legacy);
                        return Err(DownloadError::FileWriteError(error));
                    }
                    if identity_path.exists() {
                        let legacy_identity = PathBuf::from(format!(
                            "{}{STAGING_IDENTITY_SUFFIX}",
                            legacy.display()
                        ));
                        fs::rename(identity_path, legacy_identity)
                            .map_err(DownloadError::FileWriteError)?;
                    }
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(DownloadError::FileWriteError(e)),
            }
        }
        Err(DownloadError::FileWriteError(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free legacy name",
        )))
    }

    /// Claim a free final name with `create_new`: an existence check followed
    /// by a rename could overwrite a file another process created in between.
    fn claim_install_path(final_path: &Path) -> Result<PathBuf, DownloadError> {
        let stem = final_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let extension = final_path
            .extension()
            .map(|ext| format!(".{}", ext.to_string_lossy()))
            .unwrap_or_default();
        let parent = final_path.parent().unwrap_or_else(|| Path::new(""));
        for number in 1..u32::MAX {
            let candidate = if number == 1 {
                final_path.to_path_buf()
            } else {
                parent.join(format!("{stem} ({number}){extension}"))
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(placeholder) => {
                    drop(placeholder);
                    return Ok(candidate);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(DownloadError::FileWriteError(e)),
            }
        }
        Err(DownloadError::FileWriteError(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free install name",
        )))
    }

    /// Peers can coalesce trailing bytes into the last chunk, so overshoot past
    /// the expected size is dropped rather than written.
    fn write(&mut self, data: &[u8]) -> Result<(), DownloadError> {
        let remaining = self.expected - self.written;
        let write_len = remaining.min(data.len() as u64) as usize;
        let data = &data[..write_len];
        self.file
            .write_all(data)
            .map_err(DownloadError::FileWriteError)?;
        self.written += data.len() as u64;
        Ok(())
    }

    const fn is_complete(&self) -> bool {
        self.written >= self.expected
    }

    fn discard(self) {
        drop(self.file);
        let _ = fs::remove_file(&self.part_path);
        let _ = fs::remove_file(&self.identity_path);
    }

    /// A short transfer is a failure, and the `.part` stays put so the next
    /// attempt can resume it.
    fn finish(self) -> Result<String, DownloadError> {
        if !self.is_complete() {
            return Err(DownloadError::IncompleteDownload {
                received: self.written,
                expected: self.expected,
            });
        }
        drop(self.file);
        let destination = Self::claim_install_path(Path::new(&self.final_path))?;
        if let Err(error) = fs::rename(&self.part_path, &destination) {
            let _ = fs::remove_file(&destination);
            return Err(DownloadError::FileWriteError(error));
        }
        let _ = fs::remove_file(&self.identity_path);
        Ok(destination.to_string_lossy().into_owned())
    }
}

pub struct DownloadPeer {
    username: String,
    host: String,
    port: u32,
    #[allow(dead_code)]
    own_username: String,
    token: u32,
    no_pierce: bool,
}

impl DownloadPeer {
    #[must_use]
    pub const fn new(
        username: String,
        host: String,
        port: u32,
        token: u32,
        no_pierce: bool,
        own_username: String,
    ) -> Self {
        Self {
            username,
            host,
            port,
            own_username,
            token,
            no_pierce,
        }
    }

    fn establish_connection(&self) -> Result<TcpStream, DownloadError> {
        let socket_address = format!("{}:{}", self.host, self.port)
            .to_socket_addrs()
            .map_err(DownloadError::ConnectionFailed)?
            .next()
            .ok_or_else(|| {
                DownloadError::InvalidAddress(format!(
                    "{}:{}",
                    self.host, self.port
                ))
            })?;

        let stream = TcpStream::connect_timeout(
            &socket_address,
            Duration::from_secs(20),
        )
        .map_err(DownloadError::ConnectionFailed)?;

        Self::apply_transfer_deadlines(&stream)?;

        Ok(stream)
    }

    /// Bound every stall on a transfer socket. This must cover streams the
    /// listener accepted as well as ones we dialled: an accepted F connection
    /// used to arrive here with no read timeout at all, so a peer that sent a
    /// token and went silent parked the transfer thread — holding the socket
    /// and the open `.part` file — for the rest of the session.
    fn apply_transfer_deadlines(
        stream: &TcpStream,
    ) -> Result<(), DownloadError> {
        stream
            .set_read_timeout(Some(STALL_DEADLINE))
            .map_err(DownloadError::ConnectionFailed)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(DownloadError::ConnectionFailed)?;
        stream
            .set_nodelay(true)
            .map_err(DownloadError::ConnectionFailed)?;
        Ok(())
    }

    fn perform_handshake(
        &self,
        stream: &mut TcpStream,
    ) -> Result<(), DownloadError> {
        trace!(
            "[download_peer:{}] performing handshake no_pierce: {}",
            self.username, self.no_pierce
        );

        if !self.no_pierce {
            let message =
                MessageFactory::build_pierce_firewall_message(self.token);
            stream
                .write_all(&message.get_buffer())
                .map_err(DownloadError::HandshakeFailed)?;
            trace!(
                "[download_peer:{}] sending pierce firewall message token: {}: {:?}",
                self.username,
                self.token,
                &message.get_buffer()
            );
            stream.flush().map_err(DownloadError::HandshakeFailed)?;
        }
        Ok(())
    }

    /// Soulseek's START_DOWNLOAD is an 8-byte little-endian offset, so anything
    /// already in the `.part` file is skipped by asking the peer to start past it.
    fn start_transfer(
        download: Download,
        stream: &mut TcpStream,
        client_context: &Arc<RwLock<ClientContext>>,
    ) -> Result<(Download, PartFile), DownloadError> {
        Self::wait_while_paused(client_context, &download)?;
        let part = PartFile::open(&download)?;
        stream
            .write_all(&part.written.to_le_bytes())
            .map_err(DownloadError::StreamWriteError)?;
        Self::report_progress(client_context, &download, part.written, 0.0);
        Ok((download, part))
    }

    fn read_download_stream(
        &self,
        stream: &mut TcpStream,
        client_context: &Arc<RwLock<ClientContext>>,
        download: Option<Download>,
        matched_download: &mut Option<Download>,
    ) -> Result<(Download, String), DownloadError> {
        let mut read_buffer = vec![0u8; READ_BUFFER_SIZE];
        let mut bytes_since_last_update = 0usize;
        let mut last_update_time = Instant::now();

        trace!(
            "[download_peer:{}] Starting to read data from peer",
            self.username
        );

        let (download, mut part) = match download {
            Some(download) => {
                Self::start_transfer(download, stream, client_context)?
            }
            None if !self.no_pierce => {
                let mut token = [0u8; 4];
                stream
                    .read_exact(&mut token)
                    .map_err(DownloadError::StreamReadError)?;
                let token = u32::from_le_bytes(token);
                trace!(
                    "[download_peer:{}] got token: {} from data chunk",
                    self.username, token
                );
                let download = client_context
                    .write()
                    .map_err(|_| DownloadError::LockPoisoned)?
                    .downloads
                    .claim_for_peer(token, &self.username)
                    .ok_or(DownloadError::TokenNotFound(token))?;
                *matched_download = Some(download.clone());
                Self::start_transfer(download, stream, client_context)?
            }
            None => {
                return Err(DownloadError::DownloadInfoMissing(self.token));
            }
        };
        stream
            .set_read_timeout(Some(CANCEL_POLL))
            .map_err(DownloadError::ConnectionFailed)?;
        let speed_limit = client_context
            .read()
            .map_err(|_| DownloadError::LockPoisoned)?
            .download_speed_limit
            .clone();
        let mut last_data = Instant::now();

        loop {
            match Self::wait_while_paused(client_context, &download) {
                Ok(true) => last_data = Instant::now(),
                Ok(false) => {}
                Err(error) => {
                    if matches!(error, DownloadError::Cancelled) {
                        part.discard();
                    }
                    return Err(error);
                }
            }

            // A zero-byte transfer needs no payload bytes. There is no offset
            // acknowledgement in Soulseek's transfer protocol, so the uploader
            // waits for us to close and we must finalize without another read.
            if part.is_complete() {
                break;
            }

            let allowance = speed_limit.take(READ_BUFFER_SIZE);
            let read = stream.read(&mut read_buffer[..allowance]);
            speed_limit.give_back(allowance - read.as_ref().map_or(0, |&n| n));
            match read {
                Ok(0) => {
                    trace!(
                        "[download_peer:{}] connection closed by peer",
                        self.username
                    );
                    break;
                }
                Ok(bytes_read) => {
                    last_data = Instant::now();
                    part.write(&read_buffer[..bytes_read])?;
                    bytes_since_last_update += bytes_read;

                    if bytes_since_last_update >= PROGRESS_INTERVAL_BYTES {
                        let elapsed = last_update_time.elapsed().as_secs_f64();
                        let speed = if elapsed > 0.0 {
                            bytes_since_last_update as f64 / elapsed
                        } else {
                            0.0
                        };
                        Self::report_progress(
                            client_context,
                            &download,
                            part.written,
                            speed,
                        );
                        bytes_since_last_update = 0;
                        last_update_time = Instant::now();
                    }

                    if part.is_complete() {
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) && last_data.elapsed() < STALL_DEADLINE => {}
                Err(e) => {
                    return Err(DownloadError::StreamReadError(e));
                }
            }
        }

        trace!(
            "[download_peer:{}] finished reading data from peer",
            self.username
        );

        let final_path =
            Self::finish_transfer(client_context, &download, part)?;
        Ok((download, final_path))
    }

    fn report_progress(
        client_context: &Arc<RwLock<ClientContext>>,
        download: &Download,
        bytes_downloaded: u64,
        speed_bytes_per_sec: f64,
    ) {
        Self::send_download_status(
            client_context,
            download,
            DownloadStatus::InProgress {
                bytes_downloaded,
                total_bytes: download.size,
                speed_bytes_per_sec,
            },
        );
    }

    pub(crate) fn send_download_status(
        client_context: &Arc<RwLock<ClientContext>>,
        download: &Download,
        status: DownloadStatus,
    ) {
        let applied = client_context.write().is_ok_and(|mut context| {
            context
                .downloads
                .update_status(download.token, status.clone())
        });
        if applied {
            let _ = download.sender.send(status);
        }
    }

    /// Promote a complete partial and publish `Completed` as one operation with
    /// respect to cancellation. Holding the store write lock across the rename
    /// gives both possible races deterministic outcomes: cancellation marks the
    /// row first and discards the partial, or completion lands first and a later
    /// cancellation is refused.
    fn finish_transfer(
        client_context: &Arc<RwLock<ClientContext>>,
        download: &Download,
        part: PartFile,
    ) -> Result<String, DownloadError> {
        loop {
            let mut context = client_context
                .write()
                .map_err(|_| DownloadError::LockPoisoned)?;
            let status = context
                .get_download_by_token(download.token)
                .map(|download| download.status.clone())
                .ok_or(DownloadError::TokenNotFound(download.token))?;

            if matches!(status, DownloadStatus::Cancelled) {
                drop(context);
                part.discard();
                return Err(DownloadError::Cancelled);
            }
            if matches!(status, DownloadStatus::Paused { .. }) {
                drop(context);
                if let Err(error) =
                    Self::wait_while_paused(client_context, download)
                {
                    if matches!(error, DownloadError::Cancelled) {
                        part.discard();
                    }
                    return Err(error);
                }
                continue;
            }

            let final_path = part.finish()?;
            if !context
                .downloads
                .update_status(download.token, DownloadStatus::Completed)
            {
                return Err(DownloadError::TokenNotFound(download.token));
            }
            drop(context);
            let _ = download.sender.send(DownloadStatus::Completed);
            return Ok(final_path);
        }
    }

    fn wait_while_paused(
        client_context: &Arc<RwLock<ClientContext>>,
        download: &Download,
    ) -> Result<bool, DownloadError> {
        let mut waited = false;
        loop {
            let status = client_context
                .read()
                .map_err(|_| DownloadError::LockPoisoned)?
                .get_download_by_token(download.token)
                .map(|download| download.status.clone())
                .ok_or(DownloadError::TokenNotFound(download.token))?;

            if matches!(status, DownloadStatus::Cancelled) {
                return Err(DownloadError::Cancelled);
            }
            if !matches!(status, DownloadStatus::Paused { .. }) {
                return Ok(waited);
            }

            waited = true;
            thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn download_file(
        self,
        client_context: Arc<RwLock<ClientContext>>,
        download: Option<Download>,
        stream: Option<TcpStream>,
    ) -> Result<(Download, String), DownloadError> {
        let mut matched_download = download.clone();
        let result: Result<_, DownloadError> = (|| {
            trace!(
                "[download_peer:{}] download_file: download is present?: {:?}, stream is present?: {:?}, no_pierce: {}",
                self.username,
                download.is_some(),
                stream.is_some(),
                self.no_pierce
            );

            let mut stream = match stream {
                Some(s) => {
                    Self::apply_transfer_deadlines(&s)?;
                    s
                }
                None => self.establish_connection()?,
            };

            trace!("[download_peer:{}] connected", self.username);

            self.perform_handshake(&mut stream)?;
            trace!("[download_peer:{}] handshake completed", self.username);

            let (download, final_path) = self.read_download_stream(
                &mut stream,
                &client_context,
                download,
                &mut matched_download,
            )?;

            trace!(
                "[download_peer:{}] download completed successfully: {} bytes, saved to: {}",
                self.username, download.size, final_path
            );

            Ok((download, final_path))
        })();
        if let Err(error) = &result
            && !matches!(error, DownloadError::Cancelled)
            && let Some(download) = matched_download
        {
            Self::send_download_status(
                &client_context,
                &download,
                DownloadStatus::Failed(Some(error.to_string())),
            );
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{DownloadError, DownloadPeer, PartFile};
    use crate::client::ClientContext;
    use crate::download_store::extract_filename_from_path;
    use crate::types::{Download, DownloadMetadata, DownloadStatus};
    use std::sync::{Arc, RwLock, mpsc};

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("soulseek-part-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn download_into(dir: &std::path::Path, size: u64) -> Download {
        Download {
            username: "peer".to_string(),
            filename: "file.bin".to_string(),
            token: 1,
            size,
            download_directory: dir.display().to_string(),
            status: DownloadStatus::Queued,
            sender: mpsc::channel().0,
            queue_position: None,
            metadata: DownloadMetadata::default(),
        }
    }

    #[test]
    fn a_discarded_part_file_is_removed_from_disk() {
        let dir = scratch_dir("discard");
        let mut part = PartFile::open(&download_into(&dir, 8)).unwrap();
        part.write(b"data").unwrap();
        let path = dir.join("file.bin.part");
        assert!(path.exists());

        part.discard();

        assert!(!path.exists());
        assert!(!dir.join("file.bin").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pause_is_reported_so_the_stall_budget_restarts() {
        let dir = scratch_dir("paused");
        let mut download = download_into(&dir, 8);
        download.status = DownloadStatus::Paused {
            bytes_downloaded: 0,
            total_bytes: 8,
        };
        let context = Arc::new(RwLock::new(ClientContext::new()));
        context.write().unwrap().add_download(download.clone());
        let resumer = {
            let context = context.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                assert!(
                    context
                        .write()
                        .unwrap()
                        .downloads
                        .resume_by_file("peer", "file.bin")
                );
            })
        };

        assert!(matches!(
            DownloadPeer::wait_while_paused(&context, &download),
            Ok(true)
        ));
        assert!(matches!(
            DownloadPeer::wait_while_paused(&context, &download),
            Ok(false)
        ));
        let _ = resumer.join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_download_stops_the_transfer_loop() {
        let dir = scratch_dir("cancelled");
        let mut download = download_into(&dir, 8);
        download.status = DownloadStatus::Cancelled;
        let context = Arc::new(RwLock::new(ClientContext::new()));
        context.write().unwrap().add_download(download.clone());

        assert!(matches!(
            DownloadPeer::wait_while_paused(&context, &download),
            Err(DownloadError::Cancelled)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_status_after_cancel_is_not_relayed_on_the_channel() {
        let dir = scratch_dir("relay");
        let (tx, rx) = mpsc::channel();
        let mut download = download_into(&dir, 8);
        download.sender = tx;
        download.status = DownloadStatus::Cancelled;
        let context = Arc::new(RwLock::new(ClientContext::new()));
        context.write().unwrap().add_download(download.clone());

        DownloadPeer::send_download_status(
            &context,
            &download,
            DownloadStatus::InProgress {
                bytes_downloaded: 4,
                total_bytes: 8,
                speed_bytes_per_sec: 1.0,
            },
        );

        assert!(rx.try_recv().is_err(), "a cancelled row hears nothing more");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pierced_short_transfer_reports_failure() {
        let dir = scratch_dir("pierced-short-transfer");
        let (sender, receiver) = mpsc::channel();
        let mut download = download_into(&dir, 10);
        download.sender = sender;
        let context = Arc::new(RwLock::new(ClientContext::new()));
        context.write().unwrap().add_download(download.clone());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client_stream =
            std::net::TcpStream::connect(listener.local_addr().unwrap())
                .unwrap();
        let (mut uploader, _) = listener.accept().unwrap();
        uploader
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let peer = DownloadPeer::new(
            download.username.clone(),
            "127.0.0.1".into(),
            0,
            99,
            false,
            "me".into(),
        );
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            peer.download_file(worker_context, None, Some(client_stream))
        });

        // Drain the framed PierceFirewall request before sending the raw token.
        let mut frame_len = [0u8; 4];
        std::io::Read::read_exact(&mut uploader, &mut frame_len).unwrap();
        let mut frame = vec![0; u32::from_le_bytes(frame_len) as usize];
        std::io::Read::read_exact(&mut uploader, &mut frame).unwrap();

        std::io::Write::write_all(&mut uploader, &download.token.to_le_bytes())
            .unwrap();

        let mut offset = [0u8; 8];
        std::io::Read::read_exact(&mut uploader, &mut offset).unwrap();
        assert_eq!(u64::from_le_bytes(offset), 0);
        std::io::Write::write_all(&mut uploader, b"0123").unwrap();
        std::net::TcpStream::shutdown(&uploader, std::net::Shutdown::Write)
            .unwrap();
        drop(uploader);

        assert!(matches!(
            worker.join().unwrap(),
            Err(DownloadError::IncompleteDownload {
                received: 4,
                expected: 10
            })
        ));
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(DownloadStatus::InProgress { .. })
        ));
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(DownloadStatus::Failed(_))
        ));
        assert!(matches!(
            context
                .read()
                .unwrap()
                .get_download_by_token(download.token)
                .unwrap()
                .status,
            DownloadStatus::Failed(_)
        ));
        assert_eq!(std::fs::read(dir.join("file.bin.part")).unwrap(), b"0123");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_pierced_token_cannot_claim_another_peers_download() {
        let dir = scratch_dir("token-peer-mismatch");
        let mut download = download_into(&dir, 10);
        download.username = "victim".into();
        let context = Arc::new(RwLock::new(ClientContext::new()));
        context.write().unwrap().add_download(download.clone());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client_stream =
            std::net::TcpStream::connect(listener.local_addr().unwrap())
                .unwrap();
        let (mut attacker, _) = listener.accept().unwrap();
        let peer = DownloadPeer::new(
            "attacker".into(),
            "127.0.0.1".into(),
            0,
            100,
            false,
            "me".into(),
        );
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            peer.download_file(worker_context, None, Some(client_stream))
        });

        let mut frame_len = [0u8; 4];
        std::io::Read::read_exact(&mut attacker, &mut frame_len).unwrap();
        let mut frame = vec![0; u32::from_le_bytes(frame_len) as usize];
        std::io::Read::read_exact(&mut attacker, &mut frame).unwrap();
        std::io::Write::write_all(&mut attacker, &download.token.to_le_bytes())
            .unwrap();

        assert!(matches!(
            worker.join().unwrap(),
            Err(DownloadError::TokenNotFound(1))
        ));
        assert!(!dir.join("file.bin.part").exists());
        assert!(matches!(
            context
                .read()
                .unwrap()
                .get_download_by_token(download.token)
                .unwrap()
                .status,
            DownloadStatus::Queued
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn start_transfer_refuses_a_cancelled_download() {
        let dir = scratch_dir("refuse");
        let mut download = download_into(&dir, 8);
        download.status = DownloadStatus::Cancelled;
        let context = Arc::new(RwLock::new(ClientContext::new()));
        context.write().unwrap().add_download(download.clone());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut stream =
            std::net::TcpStream::connect(listener.local_addr().unwrap())
                .unwrap();
        let _peer = listener.accept().unwrap();

        let outcome =
            DownloadPeer::start_transfer(download, &mut stream, &context);

        assert!(matches!(outcome, Err(DownloadError::Cancelled)));
        assert!(!dir.join("file.bin.part").exists(), "no .part is opened");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_that_does_not_exist_yet_is_created_not_dodged() {
        let dir = scratch_dir("fresh").join("Soulseek");

        let mut part = PartFile::open(&download_into(&dir, 4)).unwrap();
        part.write(b"data").unwrap();
        let final_path = part.finish().unwrap();

        assert_eq!(
            std::path::Path::new(&final_path).parent(),
            Some(dir.as_path()),
            "the bytes belong in the configured directory, not its parent"
        );
        assert_eq!(std::fs::read(&final_path).unwrap(), b"data");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn a_partial_file_sets_the_resume_offset() {
        let dir = scratch_dir("resume");
        {
            let mut part = PartFile::open(&download_into(&dir, 10)).unwrap();
            part.write(b"0123").unwrap();
        }

        let mut part = PartFile::open(&download_into(&dir, 10)).unwrap();
        assert_eq!(part.written, 4, "resume from this transfer's own bytes");

        part.write(b"456789").unwrap();
        let final_path = part.finish().unwrap();

        assert_eq!(std::fs::read(&final_path).unwrap(), b"0123456789");
        assert!(
            !dir.join("file.bin.part").exists(),
            "the .part is renamed, not left behind"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unidentified_exact_size_partial_restarts_from_zero() {
        // A part with no identity sidecar is never a valid resume point,
        // whatever its length happens to be.
        let dir = scratch_dir("complete");
        std::fs::write(dir.join("file.bin.part"), b"0123456789").unwrap();

        let mut part = PartFile::open(&download_into(&dir, 10)).unwrap();
        assert_eq!(part.written, 0);
        assert!(!part.is_complete());

        part.write(b"abcdefghij").unwrap();
        let final_path = part.finish().unwrap();
        assert_eq!(std::fs::read(final_path).unwrap(), b"abcdefghij");
        assert!(!dir.join("file.bin.part").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancellation_while_a_complete_transfer_is_paused_discards_its_part() {
        let dir = scratch_dir("cancel-paused-finish");
        let mut download = download_into(&dir, 10);
        let mut part = PartFile::open(&download).unwrap();
        part.write(b"0123456789").unwrap();
        download.status = DownloadStatus::Paused {
            bytes_downloaded: 10,
            total_bytes: 10,
        };
        let context = Arc::new(RwLock::new(ClientContext::new()));
        context.write().unwrap().add_download(download.clone());
        let worker_context = context.clone();
        let worker_download = download.clone();

        let worker = std::thread::spawn(move || {
            DownloadPeer::finish_transfer(
                &worker_context,
                &worker_download,
                part,
            )
        });

        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            context
                .write()
                .unwrap()
                .downloads
                .cancel_by_file(&download.username, &download.filename)
                .is_some()
        );

        assert!(matches!(
            worker.join().unwrap(),
            Err(DownloadError::Cancelled)
        ));
        assert!(!dir.join("file.bin.part").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_oversized_partial_file_restarts_from_zero() {
        // Left over from a different transfer — not a valid resume point.
        let dir = scratch_dir("stale");
        std::fs::write(dir.join("file.bin.part"), vec![9u8; 40]).unwrap();

        let mut part = PartFile::open(&download_into(&dir, 10)).unwrap();
        assert_eq!(part.written, 0);

        part.write(&(0..10).collect::<Vec<u8>>()).unwrap();
        let final_path = part.finish().unwrap();
        assert_eq!(
            std::fs::read(final_path).unwrap(),
            (0..10).collect::<Vec<u8>>()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_truncated_transfer_fails_and_keeps_the_partial_file() {
        let dir = scratch_dir("truncated");
        let mut part = PartFile::open(&download_into(&dir, 10)).unwrap();
        part.write(b"01234").unwrap();

        assert!(matches!(
            part.finish(),
            Err(DownloadError::IncompleteDownload {
                received: 5,
                expected: 10
            })
        ));
        assert_eq!(
            std::fs::read(dir.join("file.bin.part")).unwrap(),
            b"01234",
            "the partial stays on disk for the next attempt"
        );
        assert!(!dir.join("file.bin").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn overshoot_past_the_expected_size_is_dropped() {
        let dir = scratch_dir("overshoot");
        let mut part = PartFile::open(&download_into(&dir, 10)).unwrap();
        part.write(&(0..12).collect::<Vec<u8>>()).unwrap();

        assert!(part.is_complete());
        assert_eq!(
            std::fs::read(part.finish().unwrap()).unwrap(),
            (0..10).collect::<Vec<u8>>()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // Streams the listener accepted arrive with no deadlines; without these a
    // peer that sends a token and goes silent parks the transfer thread —
    // socket and open .part file included — for the rest of the session.
    #[test]
    fn supplied_streams_get_transfer_deadlines() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream =
            std::net::TcpStream::connect(listener.local_addr().unwrap())
                .unwrap();

        DownloadPeer::apply_transfer_deadlines(&stream).unwrap();

        let secs = std::time::Duration::from_secs;
        assert_eq!(stream.read_timeout().unwrap(), Some(secs(30)));
        assert_eq!(stream.write_timeout().unwrap(), Some(secs(5)));
    }

    #[test]
    fn test_establish_connection_invalid_address() {
        let download_peer = DownloadPeer::new(
            "user".to_string(),
            "invalid-host".to_string(),
            9999,
            123,
            false,
            "own_user".to_string(),
        );
        let result = download_peer.establish_connection();
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_filename_from_path() {
        assert_eq!(extract_filename_from_path("/path/to/file.mp3"), "file.mp3");
        assert_eq!(
            extract_filename_from_path("C:\\path\\to\\file.mp3"),
            "file.mp3"
        );
        assert_eq!(
            extract_filename_from_path(
                "@@bhfrv\\Soulseek Downloads\\complete\\Beatport Top Deep House (2021)\\michel test file.mp3"
            ),
            "michel test file.mp3"
        );
        assert_eq!(extract_filename_from_path("file.mp3"), "file.mp3");
    }

    #[test]
    fn a_partial_recorded_for_another_transfer_is_never_appended() {
        // The witness: album-a\01.mp3 leaves a four-byte prefix, then
        // album-b\01.mp3 advertises a larger size. The old length check
        // accepted the prefix and mixed two unrelated files' bytes.
        let dir = scratch_dir("cross-path");
        let mut first = download_into(&dir, 10);
        first.username = "alice".into();
        first.filename = "album-a\\01.mp3".into();
        let mut part = PartFile::open(&first).unwrap();
        part.write(b"abcd").unwrap();
        drop(part);

        let mut second = download_into(&dir, 20);
        second.username = "bob".into();
        second.filename = "album-b\\01.mp3".into();
        let resumed = PartFile::open(&second).unwrap();

        assert_eq!(
            resumed.written, 0,
            "a different username and virtual path must not resume these bytes"
        );
        assert_eq!(
            std::fs::read(dir.join("01.mp3.part.legacy")).unwrap(),
            b"abcd",
            "the stale bytes are migrated aside, not overwritten"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unidentified_partial_is_migrated_instead_of_resumed() {
        let dir = scratch_dir("legacy");
        std::fs::write(dir.join("file.bin.part"), b"abcd").unwrap();

        let part = PartFile::open(&download_into(&dir, 20)).unwrap();

        assert_eq!(part.written, 0, "an unidentified part is not a resume point");
        assert_eq!(
            std::fs::read(dir.join("file.bin.part.legacy")).unwrap(),
            b"abcd",
            "the bytes survive under a name that cannot be resumed by mistake"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_unidentified_partials_keep_every_migrated_copy() {
        let dir = scratch_dir("legacy-twice");
        std::fs::write(dir.join("file.bin.part"), b"first").unwrap();
        drop(PartFile::open(&download_into(&dir, 20)).unwrap());

        let _ = std::fs::remove_file(dir.join("file.bin.part.identity"));
        std::fs::write(dir.join("file.bin.part"), b"second").unwrap();
        drop(PartFile::open(&download_into(&dir, 20)).unwrap());

        let saved: Vec<Vec<u8>> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_string_lossy().contains(".legacy")
            })
            .map(|entry| std::fs::read(entry.path()).unwrap())
            .collect();
        assert!(
            saved.contains(&b"first".to_vec()),
            "the first migrated copy must survive a later migration"
        );
        assert!(saved.contains(&b"second".to_vec()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_never_clobbers_an_existing_final_file() {
        let dir = scratch_dir("no-clobber");
        std::fs::write(dir.join("file.bin"), b"keep").unwrap();
        let mut part = PartFile::open(&download_into(&dir, 4)).unwrap();
        part.write(b"data").unwrap();

        let installed = part.finish().unwrap();

        assert_eq!(std::fs::read(dir.join("file.bin")).unwrap(), b"keep");
        assert_ne!(
            installed,
            dir.join("file.bin").to_str().unwrap(),
            "the new download gets a unique name"
        );
        assert_eq!(std::fs::read(&installed).unwrap(), b"data");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
