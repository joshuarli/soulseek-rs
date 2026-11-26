use crate::actor::{Actor, ActorHandle, ConnectionState};
use crate::client::ClientOperation;
use crate::dispatcher::MessageDispatcher;
use crate::message::peer::UserInfoResponseHandler;
use crate::message::peer::{
    FileSearchResponse, FolderContentsRequest, FolderContentsResponseHandler,
    GetShareFileList, PeerInit, PlaceInQueueRequest, PlaceInQueueResponse,
    QueueUploadHandler, SharedDirectory, SharedFileListResponseHandler,
    TransferRequest, TransferResponse, UploadDeniedHandler,
    UploadFailedHandler, UserInfoRequest,
};
use crate::message::server::MessageFactory;
use crate::message::{Handlers, Message, MessageReader, MessageType};
use crate::peer::Peer;
use crate::types::{Download, SearchResult, Transfer};
use crate::utils::lock::RwLockExt;
use crate::utils::semaphore::BoundedSender;
use crate::{debug, error, trace, warn};

use std::io::{self, Error, Write};
use std::net::TcpStream;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum PeerMessage {
    SendMessage(Message),
    FileSearchResult(SearchResult),
    TransferRequest(Transfer),
    UploadFailed(String, String),
    TransferResponse {
        token: u32,
        allowed: bool,
        reason: Option<String>,
    },
    PlaceInQueueResponse {
        filename: String,
        place: u32,
    },
    SetUsername(String),
    QueueUpload(String),
    RequestTransfer(Download),
    /// A peer queued one of our shared files for download (they sent us code 43).
    IncomingQueueUpload(String),
    /// A peer asked where their queued file sits (they sent us code 51).
    IncomingPlaceInQueueRequest(String),
    /// A peer asked to browse our shared files (they sent us code 4).
    ShareListRequested,
    /// A peer we are browsing sent us their shared-file listing (code 5).
    ShareListReceived(Vec<SharedDirectory>),
    /// A peer answered our `UserInfoRequest` with what it says about itself.
    UserInfoReceived(crate::message::peer::PeerInfo),
    /// A peer answered our `FolderContentsRequest` with one folder's listing.
    FolderContentsReceived {
        token: u32,
        folder: String,
        directories: Vec<SharedDirectory>,
    },
    /// A peer asked what we say about ourselves (they sent us code 15).
    UserInfoRequested,
    /// A peer asked for one folder of our shares (they sent us code 36).
    FolderContentsRequested {
        token: u32,
        folder: String,
    },
    /// Offer the queued file to that peer: send an upload TransferRequest.
    ServeUpload {
        token: u32,
        filename: String,
        size: u64,
    },
    ProcessRead,
    /// A newer connection to this peer took over. Deliver what is already
    /// on the wire, then close.
    Retire,
}

pub struct PeerActor {
    peer: Arc<RwLock<Peer>>,
    stream: Option<TcpStream>,
    connection_state: ConnectionState,
    reader: MessageReader,
    client_channel: BoundedSender<ClientOperation>,
    self_handle: Option<ActorHandle<PeerMessage>>,
    dispatcher: Option<MessageDispatcher<PeerMessage>>,
    dispatcher_receiver: Option<Receiver<PeerMessage>>,
    queued_messages: Vec<PeerMessage>,
    own_username: String,
    /// True when we initiated this connection (no stream supplied at
    /// construction), so we must send a `PeerInit` once connected.
    outbound: bool,
    /// Set once the connection is up. A pre-established failure on an outbound
    /// connection means the peer is unreachable (likely firewalled).
    established: bool,
    /// Set once this actor has reported a terminal outcome (disconnect or
    /// connect-failure). Guards against a second notification — e.g. when a
    /// replaced actor is later stopped — which would otherwise alias and evict
    /// a newer actor registered under the same username.
    disconnect_reported: bool,
    /// Registry-assigned unique id, echoed in terminal notifications so the
    /// client evicts only this actor and never a newer namesake.
    id: u64,
    /// Transfer tokens for uploads we are serving to this peer. A TransferResponse
    /// for one of these is our upload being accepted, not a download offer.
    serving_tokens: std::collections::HashSet<u32>,
    /// Bytes queued for the peer that the socket would not take yet. The stream
    /// is non-blocking (so socket reads never starve the mailbox), so a message
    /// larger than the send buffer — a full share listing is easily megabytes —
    /// only partly fits; the rest drains on later ticks.
    pending_write: Vec<u8>,
    /// When bytes last moved on this connection, in either direction. A peer
    /// that goes quiet is eventually reaped: nothing else ever closes an idle
    /// control connection, and each one holds a descriptor and a thread for
    /// the life of the process.
    last_activity: Instant,
    /// Set once a newer connection to the same peer replaced this one. The
    /// stream stays open until then so a reply already in flight, a browse
    /// listing say, still arrives; past it the actor closes and stops.
    retire_deadline: Option<Instant>,
    /// Code 4 browses sent on this connection and still awaiting a code 5
    /// reply. Code 5 has no token, so the pending count on this connection is
    /// the only correlation available before inflating the compressed body.
    browse_requests: u32,
    /// Code 36 folder requests sent on this connection and still awaiting a
    /// code 37 reply. The reply's token is inside its compressed body, so the
    /// only correlation available before inflating is the pending count.
    folder_requests: u32,
}

/// How many folder requests from one client may be in flight to a peer.
const MAX_FOLDER_REQUESTS: u32 = 32;

/// How many browse requests may be outstanding on one connection before a
/// further request stops adding pending replies.
const MAX_BROWSE_REQUESTS: u32 = 8;

/// Cap on queued outbound bytes. A peer that stops reading must not be able to
/// grow this without bound; dropping that connection is the right answer.
const MAX_PENDING_WRITE: usize = 32 * 1024 * 1024;

/// How long a connected peer may stay silent before its control connection is
/// closed. Transfers run on their own sockets, and a peer that wants us again
/// simply reconnects — every serious client reaps idle peers this way. Five
/// minutes comfortably outlives a search's chatter and queue updates.
const IDLE_DISCONNECT: Duration = Duration::from_mins(5);
/// How long a replaced connection keeps listening for replies in flight.
const RETIRE_GRACE: Duration = Duration::from_secs(30);
/// Socket buffers read per tick: 64 KiB, against a 100 ms tick.
const READS_PER_TICK: usize = 64;

impl PeerActor {
    #[must_use]
    pub fn new(
        peer: Peer,
        stream: Option<TcpStream>,
        reader: Option<MessageReader>,
        client_channel: BoundedSender<ClientOperation>,
        own_username: String,
        id: u64,
    ) -> Self {
        let outbound = stream.is_none();
        let connection_state = if stream.is_some() {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        };

        Self {
            peer: Arc::new(RwLock::new(peer)),
            stream,
            connection_state,
            reader: reader.unwrap_or_default(),
            client_channel,
            self_handle: None,
            dispatcher: None,
            dispatcher_receiver: None,
            queued_messages: Vec::new(),
            own_username,
            outbound,
            established: false,
            disconnect_reported: false,
            id,
            serving_tokens: std::collections::HashSet::new(),
            pending_write: Vec::new(),
            last_activity: Instant::now(),
            retire_deadline: None,
            browse_requests: 0,
            folder_requests: 0,
        }
    }

    pub fn set_self_handle(&mut self, handle: ActorHandle<PeerMessage>) {
        self.self_handle = Some(handle);
    }

    fn peer_username(&self) -> String {
        match self.peer.read_safe() {
            Ok(p) => p.username.clone(),
            Err(e) => {
                error!("[peer_actor] peer lock poisoned: {}", e);
                "<unknown>".to_string()
            }
        }
    }

    fn peer_snapshot(&self) -> Option<Peer> {
        match self.peer.read_safe() {
            Ok(p) => Some(p.clone()),
            Err(e) => {
                error!("[peer_actor] peer lock poisoned: {}", e);
                None
            }
        }
    }

    fn initialize_dispatcher(&mut self) {
        let (dispatcher_sender, dispatcher_receiver) =
            std::sync::mpsc::channel::<PeerMessage>();

        self.dispatcher_receiver = Some(dispatcher_receiver);

        let mut handlers = Handlers::new();
        handlers.register_handler(FileSearchResponse);
        handlers.register_handler(TransferRequest);
        handlers.register_handler(TransferResponse);
        handlers.register_handler(GetShareFileList);
        handlers.register_handler(FolderContentsRequest);
        handlers.register_handler(FolderContentsResponseHandler);
        handlers.register_handler(UserInfoRequest);
        handlers.register_handler(UserInfoResponseHandler);
        handlers.register_handler(UploadDeniedHandler);
        handlers.register_handler(UploadFailedHandler);
        handlers.register_handler(PlaceInQueueRequest);
        handlers.register_handler(PlaceInQueueResponse);
        handlers.register_handler(QueueUploadHandler);
        handlers.register_handler(SharedFileListResponseHandler);
        handlers.register_handler(PeerInit);

        self.dispatcher = Some(MessageDispatcher::new(
            "peer".to_string(),
            dispatcher_sender,
            handlers,
        ));
    }

    fn process_dispatcher_messages(&mut self) {
        let messages: Vec<PeerMessage> = self
            .dispatcher_receiver
            .as_ref()
            .map_or_else(Vec::new, |receiver| receiver.try_iter().collect());

        for msg in &messages {
            self.handle_message(msg.clone());
        }
    }

    fn handle_message(&mut self, msg: PeerMessage) {
        if matches!(self.connection_state, ConnectionState::Connecting { .. }) {
            match &msg {
                PeerMessage::SetUsername(_) | PeerMessage::ProcessRead => {}
                // Nothing has gone out on a dial still in progress, so there
                // is nothing to wait for.
                PeerMessage::Retire => {
                    self.retire_deadline = Some(Instant::now());
                    self.disconnect();
                    return;
                }
                _ => {
                    self.queued_messages.push(msg);
                    return;
                }
            }
        }

        match msg {
            PeerMessage::SendMessage(message) => {
                self.send_message(message);
            }
            PeerMessage::Retire => {
                self.retire_deadline = Some(Instant::now() + RETIRE_GRACE);
            }
            PeerMessage::FileSearchResult(file_search) => {
                self.handle_file_search_result(file_search);
            }
            PeerMessage::TransferRequest(transfer) => {
                self.handle_transfer_request(transfer);
            }
            PeerMessage::TransferResponse {
                token,
                allowed,
                reason,
            } => {
                self.handle_transfer_response(token, allowed, reason);
            }
            PeerMessage::PlaceInQueueResponse { filename, place } => {
                self.handle_place_in_queue_response(filename, place);
            }
            PeerMessage::SetUsername(username) => {
                self.handle_set_username(username);
            }
            PeerMessage::QueueUpload(filename) => {
                let message =
                    MessageFactory::build_queue_upload_message(&filename);
                self.send_message(message);
            }
            PeerMessage::IncomingQueueUpload(filename) => {
                self.handle_incoming_queue_upload(filename);
            }
            PeerMessage::IncomingPlaceInQueueRequest(filename) => {
                let requester_key = self.peer_username();
                if let Err(e) = self.client_channel.send(
                    ClientOperation::PlaceInQueueRequested {
                        requester_key,
                        filename,
                    },
                ) {
                    error!(
                        "[peer_actor] forward IncomingPlaceInQueueRequest: {}",
                        e
                    );
                }
            }
            PeerMessage::ServeUpload {
                token,
                filename,
                size,
            } => {
                self.handle_serve_upload(token, filename, size);
            }
            PeerMessage::ShareListRequested => {
                self.forward(ClientOperation::ShareListRequested {
                    requester_key: self.peer_username(),
                });
            }
            PeerMessage::ShareListReceived(directories) => {
                self.forward(ClientOperation::BrowseResult {
                    username: self.peer_username(),
                    directories,
                });
            }
            PeerMessage::UserInfoReceived(info) => {
                self.forward(ClientOperation::PeerInfoReceived {
                    username: self.peer_username(),
                    info,
                });
            }
            PeerMessage::FolderContentsReceived {
                token,
                folder,
                directories,
            } => {
                self.forward(ClientOperation::FolderContents {
                    username: self.peer_username(),
                    token,
                    folder,
                    directories,
                });
            }
            PeerMessage::UserInfoRequested => {
                self.forward(ClientOperation::UserInfoRequested {
                    requester_key: self.peer_username(),
                });
            }
            PeerMessage::FolderContentsRequested { token, folder } => {
                self.forward(ClientOperation::FolderContentsRequested {
                    requester_key: self.peer_username(),
                    token,
                    folder,
                });
            }
            PeerMessage::RequestTransfer(download) => {
                let message = MessageFactory::build_transfer_request_message(
                    &download.filename,
                    download.token,
                );
                self.send_message(message);
            }
            PeerMessage::ProcessRead => {
                self.process_read();
            }
            PeerMessage::UploadFailed(_, filename) => {
                self.handle_upload_failed(filename);
            }
        }
    }

    fn handle_file_search_result(&self, file_search: SearchResult) {
        if let Err(e) = self
            .client_channel
            .send(ClientOperation::SearchResult(file_search))
        {
            error!(
                "[peer:{}] failed to forward search result: {}",
                self.peer_username(),
                e
            );
        }
    }

    /// A peer offers us a file. Record the peer's transfer token with the
    /// client, and let *that* step send the TransferResponse.
    ///
    /// The response is what invites the peer's file connection, and that
    /// connection is matched by the peer's token — so replying here would race
    /// the client loop that records the token. Under load the client loop is
    /// the one that loses, the file connection finds no download, and the
    /// transfer is dropped with the download stuck on Queued forever.
    fn handle_transfer_request(&self, transfer: Transfer) {
        let username = self.peer_username();
        debug!("[peer:{}] TransferRequest for {}", username, transfer.token);

        if let Err(e) =
            self.client_channel
                .send(ClientOperation::UpdateDownloadTokens(
                    transfer,
                    username.clone(),
                ))
        {
            error!(
                "[peer:{}] failed to send UpdateDownloadTokens: {}",
                username, e
            );
        }
    }

    fn handle_transfer_response(
        &mut self,
        token: u32,
        allowed: bool,
        reason: Option<String>,
    ) {
        let username = self.peer_username();
        debug!(
            "[peer:{}] transfer response token: {} allowed: {}",
            username, token, allowed
        );

        // If this token is one of our uploads, the peer just accepted
        // our offer — start streaming. This leaves the download path
        // (every other token) byte-for-byte unchanged.
        if self.serving_tokens.remove(&token) {
            if allowed {
                let _ = self
                    .client_channel
                    .send(ClientOperation::StartUpload { token });
            }
            return;
        }

        if allowed {
            debug!(
                "[peer:{}] Transfer allowed, ready to connect with token {:}",
                username, token
            );
            let Some(peer_snapshot) = self.peer_snapshot() else {
                return;
            };
            if let Err(e) =
                self.client_channel.send(ClientOperation::DownloadFromPeer(
                    token,
                    peer_snapshot,
                    allowed,
                ))
            {
                error!(
                    "[peer:{}] failed to send DownloadFromPeer: {}",
                    username, e
                );
            }
        } else if let Some(reason_text) = reason {
            debug!(
                "[peer:{}] Transfer rejected: {} - token {}, waiting for TransferRequest...",
                username, reason_text, token
            );
        }
    }

    fn handle_place_in_queue_response(&self, filename: String, place: u32) {
        let username = self.peer_username();
        debug!(
            "[peer:{}] Place in queue response - file: {}, place: {}",
            username, filename, place
        );
        if let Err(e) =
            self.client_channel
                .send(ClientOperation::PlaceInQueueUpdate {
                    username: username.clone(),
                    filename,
                    place,
                })
        {
            error!(
                "[peer:{}] failed to forward PlaceInQueueUpdate: {}",
                username, e
            );
        }
    }

    fn handle_set_username(&self, username: String) {
        trace!("[peer:{}] SetUsername: {}", self.peer_username(), username);
        match self.peer.write_safe() {
            Ok(mut p) => p.username = username,
            Err(e) => {
                error!("[peer_actor] SetUsername write: {}", e);
            }
        }
    }

    fn handle_incoming_queue_upload(&self, filename: String) {
        // A peer wants to download one of our shared files. Ask the
        // client (which owns the shares) to prepare the upload.
        let requester_key = self.peer_username();
        if let Err(e) = self.client_channel.send(ClientOperation::QueueUpload {
            requester_key,
            filename,
        }) {
            error!("[peer_actor] forward IncomingQueueUpload: {}", e);
        }
    }

    fn handle_serve_upload(&mut self, token: u32, filename: String, size: u64) {
        self.serving_tokens.insert(token);
        let message = MessageFactory::build_upload_transfer_request(
            &filename, token, size,
        );
        self.send_message(message);
    }

    fn forward(&self, operation: ClientOperation) {
        if let Err(e) = self.client_channel.send(operation) {
            error!("[peer_actor] forward to client: {}", e);
        }
    }

    fn handle_upload_failed(&self, filename: String) {
        let username = self.peer_username();
        if let Err(e) = self
            .client_channel
            .send(ClientOperation::UploadFailed(username, filename))
        {
            error!("[peer_actor] failed to forward UploadFailed: {}", e);
        }
    }

    fn process_read(&mut self) {
        if self.reader.buffer_len() > 0 {
            self.extract_and_process_messages();
        }

        let read_error = {
            let Some(stream) = self.stream.as_mut() else {
                return;
            };

            match Self::drain(&mut self.reader, stream) {
                Ok(()) => {
                    self.last_activity = Instant::now();
                    None
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => None,
                Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                    if let Ok(peer_lock) = self.peer.read_safe() {
                        debug!(
                            "Read operation timed out for peer actor {:}:{:}",
                            peer_lock.host, peer_lock.port
                        );
                    }
                    None
                }
                Err(e) => Some(e),
            }
        };

        if self.reader.buffer_len() > 0 {
            self.extract_and_process_messages();
        }

        let Some(e) = read_error else {
            return;
        };
        if self.stream.is_none() {
            return;
        }
        // A peer hanging up a control connection it had already
        // established is ordinary: other clients close idle peer
        // sockets while a transfer runs on its own connection. Report
        // the clean disconnect it is — an error here fails every
        // download queued with that peer, the one streaming included.
        // A connection that never established is the other story: the
        // peer accepted and vanished, which is what the server-brokered
        // fallback exists for, so that one keeps the error path.
        if e.kind() == io::ErrorKind::UnexpectedEof && self.established {
            debug!(
                "[peer:{}] peer closed the connection",
                self.peer_username()
            );
            self.disconnect();
        } else {
            let username = self.peer_username();
            error!(
                "[peer:{}] Error reading from peer: {} (kind: {:?}). Disconnecting.",
                username,
                e,
                e.kind()
            );
            self.disconnect_with_error(e);
        }
    }

    /// Read what the socket holds, up to `READS_PER_TICK` buffers, so a big
    /// listing lands in a few ticks rather than a few minutes; the cap keeps
    /// a peer that never stops sending from starving the mailbox. WouldBlock
    /// and TimedOut are normal backpressure and end a later read; any other
    /// error — a capacity failure after bytes were consumed, or EOF after a
    /// reply — is returned so the caller can parse the reply before it
    /// resolves the connection.
    fn drain(
        reader: &mut MessageReader,
        stream: &mut TcpStream,
    ) -> io::Result<()> {
        reader.read_from_socket(stream)?;
        for _ in 1..READS_PER_TICK {
            match reader.read_from_socket(stream) {
                Ok(()) => {}
                Err(ref e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn extract_and_process_messages(&mut self) {
        let username = self.peer_username();
        let mut extracted_count = 0;
        loop {
            match self.reader.extract_message() {
                Ok(Some(mut message)) => {
                    if !self.accepts_compressed_response(&message) {
                        continue;
                    }
                    extracted_count += 1;
                    trace!(
                        "[peer:{}] ← Message #{}: {:?}",
                        username,
                        extracted_count,
                        message
                            .get_message_name(
                                MessageType::Peer,
                                message.get_message_code()
                            )
                            .map_err(|e| e.to_string())
                    );
                    if let Some(ref dispatcher) = self.dispatcher {
                        dispatcher.dispatch(&mut message);
                    } else {
                        warn!("[peer:{}] No dispatcher available!", username);
                    }
                }
                Err(e) => {
                    warn!(
                        "[peer:{}] Error extracting message: {}. Disconnecting peer.",
                        username, e
                    );
                    // An illegal frame after valid ones must not swallow the
                    // operations those valid frames already dispatched.
                    self.process_dispatcher_messages();
                    self.disconnect_with_error(e);
                    return;
                }
                Ok(None) => {
                    break;
                }
            }
        }

        self.process_dispatcher_messages();
    }

    /// Record the requests we send so only replies to them are inflated. Both
    /// code 5 and code 37 carry their identifying fields inside the compressed
    /// body, so the pending request itself is the correlation available before
    /// spending an inflation budget.
    fn note_outgoing_request(&mut self, message: &Message) {
        let data = message.get_data();
        // Outgoing messages are built, so their code leads the payload; the
        // `get_message_code` helper is for frames read off a socket, which
        // carry a four-byte length in front of it.
        let Some(code_bytes) = data.get(0..4) else {
            return;
        };
        match u32::from_le_bytes(code_bytes.try_into().unwrap_or_default()) {
            4 => {
                // Explicit full policy: past the cap a further browse is sent
                // but cannot add another reply worth inflating.
                if self.browse_requests < MAX_BROWSE_REQUESTS {
                    self.browse_requests += 1;
                }
            }
            // Past the cap a request is still sent, but cannot add another
            // pending reply worth inflating.
            36 if self.folder_requests < MAX_FOLDER_REQUESTS => {
                self.folder_requests += 1;
            }
            _ => {}
        }
    }

    /// Whether a compressed reply corresponds to something we asked this peer
    /// for. A retired connection is the exception: it was kept open precisely
    /// so a reply already on the wire can still be delivered.
    fn accepts_compressed_response(&mut self, message: &Message) -> bool {
        let username = self.peer_username();
        match message.get_message_code() {
            5 => {
                if self.browse_requests > 0 {
                    self.browse_requests -= 1;
                    return true;
                }
                if self.retire_deadline.is_some() {
                    return true;
                }
                warn!(
                    "[peer:{username}] ignoring a browse listing with no request"
                );
                false
            }
            37 => {
                if self.folder_requests > 0 {
                    self.folder_requests -= 1;
                    return true;
                }
                if self.retire_deadline.is_some() {
                    return true;
                }
                warn!(
                    "[peer:{username}] ignoring a folder reply with no request"
                );
                false
            }
            _ => true,
        }
    }

    fn send_message(&mut self, message: Message) {
        self.note_outgoing_request(&message);
        let username = self.peer_username();
        if self.stream.is_none() {
            error!("Cannot send message: stream is None");
            return;
        }

        trace!(
            "[peer:{}] ➡ {:?}",
            username,
            message
                .get_message_name(
                    MessageType::Peer,
                    u32::from_le_bytes(
                        message.get_slice(0, 4).try_into().unwrap_or_default()
                    )
                )
                .map_err(|e| e.to_string())
        );

        // Queue then drain, so messages always leave in order even when an
        // earlier one is still partly buffered.
        self.pending_write.extend_from_slice(&message.get_buffer());
        self.flush_pending_write();
    }

    /// Push as much of `pending_write` into the socket as it will take. A
    /// non-blocking socket refuses the rest with `WouldBlock`; that is normal
    /// backpressure, not a failure, so the remainder stays queued for the next
    /// tick. Only a real I/O error disconnects.
    fn flush_pending_write(&mut self) {
        if self.pending_write.is_empty() {
            return;
        }

        let username = self.peer_username();
        let Some(stream) = self.stream.as_mut() else {
            return;
        };

        let mut written = 0;
        let outcome = loop {
            match stream.write(&self.pending_write[written..]) {
                Ok(0) => {
                    break Err(Error::new(
                        io::ErrorKind::WriteZero,
                        "peer stopped accepting data",
                    ));
                }
                Ok(n) => {
                    written += n;
                    if written == self.pending_write.len() {
                        break Ok(());
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break Ok(());
                }
                Err(e) => break Err(e),
            }
        };
        self.pending_write.drain(..written);
        if written > 0 {
            self.last_activity = Instant::now();
        }

        if let Err(e) = outcome {
            error!(
                "[peer:{}] Error writing message: {}. Disconnecting.",
                username, e
            );
            self.disconnect_with_error(e);
            return;
        }

        // Backpressure is normal, but what a peer has still not taken after we
        // pushed everything we could is a peer that is not reading at all.
        if self.pending_write.len() > MAX_PENDING_WRITE {
            error!(
                "[peer:{}] Outbound queue stuck above {} bytes. Disconnecting.",
                username, MAX_PENDING_WRITE
            );
            self.disconnect_with_error(Error::new(
                io::ErrorKind::WriteZero,
                "peer is not draining our outbound queue",
            ));
        }
    }

    fn disconnect_with_error(&mut self, error: Error) {
        // A retired connection is expected to go: the peer closing it is not
        // evidence the peer is gone, and must not fail its transfers on the
        // connection that replaced this one.
        if self.retire_deadline.is_some() {
            return self.disconnect();
        }
        let username = self.peer_username();
        // Include the cause: a failed outbound dial reports nothing else, so
        // without it an unreachable peer is indistinguishable from a clean
        // disconnect in the logs.
        debug!("[peer:{}] disconnect: {}", username, error);

        self.stream.take();
        self.connection_state = ConnectionState::Disconnected;
        self.browse_requests = 0;
        self.folder_requests = 0;

        if self.disconnect_reported {
            return;
        }
        self.disconnect_reported = true;

        // A direct outbound connection that never established means the peer is
        // unreachable (likely firewalled): signal a connect failure so the
        // client can fall back to server-brokered connect. Anything else is a
        // normal disconnect.
        // A dial the server asked for carries the token the far peer quoted;
        // it is what a CantConnectToPeer must echo back, so the peer stops
        // waiting for a connection that is never coming.
        let brokered_token = self
            .peer
            .read_safe()
            .ok()
            .and_then(|p| if p.brokered { p.token } else { None });
        let op = if self.outbound && !self.established {
            ClientOperation::PeerConnectFailed(
                self.id,
                username,
                brokered_token,
            )
        } else {
            ClientOperation::PeerDisconnected(
                self.id,
                username,
                Some(error.into()),
            )
        };

        if let Err(e) = self.client_channel.send(op) {
            error!("Failed to send disconnect notification: {}", e);
        }
    }
    fn disconnect(&mut self) {
        let username = self.peer_username();
        debug!("[peer:{}] disconnect", username);

        self.stream.take();
        self.connection_state = ConnectionState::Disconnected;
        self.browse_requests = 0;
        self.folder_requests = 0;

        if self.disconnect_reported {
            return;
        }
        self.disconnect_reported = true;

        if let Err(e) = self
            .client_channel
            .send(ClientOperation::PeerDisconnected(self.id, username, None))
        {
            error!("Failed to send disconnect notification: {}", e);
        }
    }

    fn initiate_connection(&mut self) {
        let (username, host, port) = match self.peer.read_safe() {
            Ok(peer) => (peer.username.clone(), peer.host.clone(), peer.port),
            Err(e) => {
                error!("[peer_actor] initiate_connection peer lock: {}", e);
                return;
            }
        };

        let socket_addr =
            format!("{host}:{port}").parse::<std::net::SocketAddr>();

        let addr = match socket_addr {
            Ok(addr) => addr,
            Err(e) => {
                error!(
                    "[peer:{}] Invalid socket address {}:{} - {}",
                    username, host, port, e
                );
                self.disconnect_with_error(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    e,
                ));
                return;
            }
        };

        // Use connect_timeout to prevent blocking the thread for too long
        let timeout = Duration::from_secs(5);
        let stream = match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => stream,
            Err(e) => {
                self.disconnect_with_error(e);
                return;
            }
        };

        if let Err(e) = stream.set_nonblocking(true) {
            error!("[peer:{}] Failed to set non-blocking: {}", username, e);
            self.disconnect_with_error(e);
            return;
        }
        stream.set_nodelay(true).ok();
        self.stream = Some(stream);
        self.connection_state = ConnectionState::Connecting {
            since: Instant::now(),
        };
    }

    fn check_connection_status(&mut self) {
        let ConnectionState::Connecting { since } = self.connection_state
        else {
            return;
        };

        let username = self.peer_username();

        if since.elapsed() > Duration::from_secs(20) {
            error!("[peer:{}] Connection timeout after 20 seconds", username);
            self.disconnect_with_error(io::Error::new(
                io::ErrorKind::TimedOut,
                "Connection timeout",
            ));
            return;
        }

        let Some(ref stream) = self.stream else {
            return;
        };

        match stream.peer_addr() {
            Ok(_) => {
                self.connection_state = ConnectionState::Connected;
                self.on_connection_established();
            }
            Err(ref e) if e.kind() == io::ErrorKind::NotConnected => {}
            Err(e) => {
                error!("[peer:{}] Connection failed: {}", username, e);
                self.disconnect_with_error(e);
            }
        }
    }

    fn on_connection_established(&mut self) {
        let (username, connection_type, token) = match self.peer.read_safe() {
            Ok(peer) => (
                peer.username.clone(),
                peer.connection_type.clone(),
                peer.token,
            ),
            Err(e) => {
                error!(
                    "[peer_actor] on_connection_established peer lock: {}",
                    e
                );
                return;
            }
        };

        self.established = true;

        let Some(ref mut stream) = self.stream else {
            return;
        };

        // Connections we initiated must announce themselves; inbound peers
        // already sent us theirs, so we stay silent for them. A direct dial
        // (resolved via GetPeerAddress, token None) sends a PeerInit; a dial we
        // make because the server asked us to (a brokered ConnectToPeer carries
        // the server's token) must instead PierceFirewall with that token so
        // the remote peer can correlate its own request. `get_buffer` prepends
        // the length prefix the peer's MessageReader expects.
        if self.outbound {
            let handshake = match token {
                Some(server_token) => {
                    MessageFactory::build_pierce_firewall_message(server_token)
                }
                None => MessageFactory::build_peer_init_message(
                    &self.own_username,
                    connection_type,
                    0,
                ),
            };
            if let Err(e) = stream.write_all(&handshake.get_buffer()) {
                error!(
                    "[peer:{}] Failed to send outbound handshake: {}",
                    username, e
                );
                self.disconnect_with_error(e);
                return;
            }
        }

        self.initialize_dispatcher();

        let queued = std::mem::take(&mut self.queued_messages);
        for msg in queued {
            self.handle_message(msg);
        }

        // Tell the client the control connection is live so any downloads
        // queued for this peer can now be requested over a handshaken stream.
        if self.outbound {
            let _ = self
                .client_channel
                .send(ClientOperation::PeerConnected(username));
        }

        if let Some(ref handle) = self.self_handle {
            handle.send(PeerMessage::ProcessRead).ok();
        }

        self.process_read();
    }
}

impl Actor for PeerActor {
    type Message = PeerMessage;

    fn handle(&mut self, msg: Self::Message) {
        self.handle_message(msg);
    }

    fn on_start(&mut self) {
        if self.stream.is_none() {
            self.initiate_connection();
        } else {
            self.connection_state = ConnectionState::Connected;
            self.on_connection_established();
        }
    }

    fn on_stop(&mut self) {
        let username = self.peer_username();
        trace!("[peer:{}] actor stopping", username);
        self.disconnect();
    }

    fn tick(&mut self) {
        match self.connection_state {
            ConnectionState::Connecting { .. } => {
                self.check_connection_status();
            }
            ConnectionState::Connected => {
                if self.stream.is_some() {
                    // a backlogged message drains one socket buffer
                    // per 100ms tick. Enough for a share listing; if a future
                    // message stream needs more throughput, wake the actor as
                    // soon as the socket is writable instead of on the tick.
                    self.flush_pending_write();
                    self.process_read();
                    self.flush_pending_write();

                    if self.stream.is_some()
                        && self.last_activity.elapsed() > IDLE_DISCONNECT
                    {
                        debug!(
                            "[peer:{}] idle for {}s, closing",
                            self.peer_username(),
                            IDLE_DISCONNECT.as_secs()
                        );
                        self.disconnect();
                    }
                    if self.stream.is_some()
                        && self
                            .retire_deadline
                            .is_some_and(|d| d <= Instant::now())
                    {
                        self.disconnect();
                    }
                }
            }
            // Nothing in the registry points at a retired actor any more, so
            // it is the one to stop itself.
            ConnectionState::Disconnected => {
                if self.retire_deadline.is_some()
                    && let Some(handle) = &self.self_handle
                {
                    let _ = handle.stop();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod framing_tests {
    use super::*;
    use crate::peer::ConnectionType;
    use crate::utils::semaphore::bounded_channel;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    const TEST_LIMIT: usize = 32;

    fn socket_pair(read_timeout: Duration) -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let ours = TcpStream::connect(addr).unwrap();
        ours.set_read_timeout(Some(read_timeout)).unwrap();
        let (theirs, _) = listener.accept().unwrap();
        theirs.set_nodelay(true).unwrap();
        (ours, theirs)
    }

    fn actor_on(
        stream: TcpStream,
    ) -> (PeerActor, Receiver<ClientOperation>) {
        let (tx, rx) = bounded_channel(64, "test operations");
        let peer = Peer::new(
            "bob".to_string(),
            ConnectionType::P,
            "127.0.0.1".to_string(),
            0,
            None,
            0,
            0,
            0,
        );
        let reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        let mut actor = PeerActor::new(
            peer,
            Some(stream),
            Some(reader),
            tx,
            "me".to_string(),
            7,
        );
        actor.connection_state = ConnectionState::Connected;
        actor.established = true;
        actor.initialize_dispatcher();
        (actor, rx)
    }

    fn browse_request(payload_len: usize) -> Vec<u8> {
        let mut payload = 4u32.to_le_bytes().to_vec();
        payload.resize(payload_len, 0x61);
        let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&payload);
        bytes
    }

    fn expect_browse_request(rx: &Receiver<ClientOperation>) {
        match rx.try_recv() {
            Ok(ClientOperation::ShareListRequested { requester_key }) => {
                assert_eq!(requester_key, "bob");
            }
            other => panic!("expected a browse request, got {other:?}"),
        }
    }

    #[test]
    fn a_fragmented_frame_is_emitted_only_when_the_last_byte_arrives() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);
        let bytes = browse_request(8);

        theirs.write_all(&bytes[..2]).unwrap();
        actor.process_read();
        assert!(rx.try_recv().is_err());
        assert_eq!(actor.reader.buffer_len(), 2);

        theirs.write_all(&bytes[2..]).unwrap();
        actor.process_read();
        expect_browse_request(&rx);
        assert_eq!(actor.reader.buffer_len(), 0);
    }

    #[test]
    fn coalesced_frames_all_reach_the_client() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);
        let wire = [browse_request(4), browse_request(6)].concat();
        theirs.write_all(&wire).unwrap();

        actor.process_read();
        expect_browse_request(&rx);
        expect_browse_request(&rx);
        assert!(rx.try_recv().is_err());
        assert_eq!(actor.reader.buffer_len(), 0);
    }

    #[test]
    fn a_near_limit_frame_then_another_frame_are_both_emitted() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);

        theirs.write_all(&browse_request(TEST_LIMIT - 2)).unwrap();
        actor.process_read();
        expect_browse_request(&rx);
        assert_eq!(actor.reader.buffer_len(), 0);

        theirs.write_all(&browse_request(4)).unwrap();
        actor.process_read();
        expect_browse_request(&rx);
        assert_eq!(actor.reader.buffer_len(), 0);
    }

    #[test]
    fn a_slow_oversized_frame_is_rejected_without_a_payload() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);
        theirs
            .write_all(&((TEST_LIMIT + 1) as u32).to_le_bytes())
            .unwrap();

        actor.process_read();

        let operations: Vec<_> = rx.try_iter().collect();
        assert!(
            !operations.iter().any(|op| matches!(
                op,
                ClientOperation::ShareListRequested { .. }
            )),
            "no frame may be emitted"
        );
        assert!(actor.stream.is_none());
        assert!(
            operations.iter().any(|op| matches!(
                op,
                ClientOperation::PeerDisconnected(7, _, Some(_))
            )),
            "expected an error disconnect, got {operations:?}"
        );
    }

    #[test]
    fn eof_after_a_complete_response_delivers_it_and_closes_cleanly() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(500));
        let (mut actor, rx) = actor_on(ours);
        theirs.write_all(&browse_request(4)).unwrap();
        drop(theirs);

        actor.process_read();

        expect_browse_request(&rx);
        assert!(actor.stream.is_none());
        match rx.try_recv() {
            Ok(ClientOperation::PeerDisconnected(7, _, None)) => {}
            other => panic!("expected a clean disconnect, got {other:?}"),
        }
    }

    #[test]
    fn drain_caps_a_later_chunk_instead_of_disconnecting() {
        let (mut ours, mut theirs) = socket_pair(Duration::from_millis(500));
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        let response = browse_request(4);
        theirs.write_all(&response).unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            theirs.write_all(&[0x7f; 128]).unwrap();
        });

        PeerActor::drain(&mut reader, &mut ours).unwrap();
        assert_eq!(
            reader.extract_message().unwrap().unwrap().get_data(),
            response
        );
        writer.join().unwrap();

        // The oversized chunk is capped at the buffer budget, not treated as
        // a reason to drop the connection before the next parse.
        PeerActor::drain(&mut reader, &mut ours).unwrap();
        assert!(reader.buffer_len() <= TEST_LIMIT + 4);
        assert_eq!(
            reader.extract_message().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    fn a_small_browse_listing() -> Vec<u8> {
        let dir = crate::message::peer::SharedDirectory {
            name: "music".to_string(),
            files: Vec::new(),
        };
        crate::message::peer::build_shared_file_list(std::slice::from_ref(&dir))
            .get_buffer()
    }

    #[test]
    fn an_unsolicited_browse_listing_is_not_inflated_or_delivered() {
        // Code 5 has no token, but it is only ever a reply to our code 4.
        // A peer that sends one unasked must not get a decompression budget
        // spent on it, nor a place in the client's browse results.
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);
        theirs.write_all(&a_small_browse_listing()).unwrap();

        actor.process_read();

        let operations: Vec<_> = rx.try_iter().collect();
        assert!(
            !operations.iter().any(|op| matches!(
                op,
                ClientOperation::BrowseResult { .. }
            )),
            "an unrequested listing must not reach the client: {operations:?}"
        );
        assert!(actor.stream.is_some(), "dropping a reply is not a disconnect");
    }

    #[test]
    fn a_browse_listing_after_our_request_is_still_delivered() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);
        actor.handle_message(PeerMessage::SendMessage(
            MessageFactory::build_get_share_file_list(),
        ));
        theirs.write_all(&a_small_browse_listing()).unwrap();

        actor.process_read();

        assert!(
            rx.try_iter().any(|op| matches!(
                op,
                ClientOperation::BrowseResult { .. }
            )),
            "the reply to a real browse must still be delivered"
        );
    }

    #[test]
    fn a_folder_reply_to_our_request_is_still_delivered() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);
        actor.handle_message(PeerMessage::SendMessage(
            crate::message::peer::build_folder_contents_request(77, "music"),
        ));
        let dir = crate::message::peer::SharedDirectory {
            name: "music".to_string(),
            files: Vec::new(),
        };
        let reply = crate::message::peer::build_folder_contents(
            77,
            "music",
            std::slice::from_ref(&dir),
        )
        .get_buffer();
        theirs.write_all(&reply).unwrap();

        actor.process_read();

        assert!(
            rx.try_iter().any(|op| matches!(
                op,
                ClientOperation::FolderContents { token: 77, .. }
            )),
            "the reply to a real folder request must still be delivered"
        );
    }

    #[test]
    fn an_unsolicited_folder_listing_is_not_inflated_or_delivered() {
        // Code 37 carries its token inside the compressed body, so the only
        // pre-inflation correlation is a code 36 still in flight.
        let (ours, mut theirs) = socket_pair(Duration::from_millis(50));
        let (mut actor, rx) = actor_on(ours);
        let dir = crate::message::peer::SharedDirectory {
            name: "music".to_string(),
            files: Vec::new(),
        };
        let reply = crate::message::peer::build_folder_contents(
            77,
            "music",
            std::slice::from_ref(&dir),
        )
        .get_buffer();
        theirs.write_all(&reply).unwrap();

        actor.process_read();

        let operations: Vec<_> = rx.try_iter().collect();
        assert!(
            !operations.iter().any(|op| matches!(
                op,
                ClientOperation::FolderContents { .. }
            )),
            "an unrequested folder reply must not reach the client: {operations:?}"
        );
    }

    #[test]
    fn a_later_illegal_frame_closes_after_delivering_earlier_frames() {
        let (ours, mut theirs) = socket_pair(Duration::from_millis(500));
        let (mut actor, rx) = actor_on(ours);
        let response = browse_request(4);
        theirs.write_all(&response).unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            theirs.write_all(&[0x7f; 128]).unwrap();
        });

        actor.process_read();
        writer.join().unwrap();
        actor.process_read();

        let messages: Vec<_> = rx.try_iter().collect();
        assert!(
            messages.iter().any(|m| matches!(
                m,
                ClientOperation::ShareListRequested { requester_key }
                    if requester_key == "bob"
            )),
            "the earlier frame must still be delivered: {messages:?}"
        );
        assert!(
            messages.iter().any(|m| matches!(
                m,
                ClientOperation::PeerDisconnected(7, _, Some(_))
            )),
            "the illegal frame must close the connection: {messages:?}"
        );
        assert!(actor.stream.is_none());
    }
}
