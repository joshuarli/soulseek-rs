//! Dispatch arms that move bytes: what a peer allows, what it asks of us,
//! and the tokens tying the two together.
//!
//! Lifted out of the dispatch loop whole, so the bodies read exactly as they
//! did inside it — including `own_username`, which is re-owned on the way in
//! rather than have every line that clones it rewritten.

use super::{
    Arc, Client, ClientContext, ClientOperation, Download, DownloadError,
    DownloadPeer, DownloadStatus, MessageFactory, PeerMessage, RwLock,
    RwLockExt, ServerMessage, debug, error, info, trace,
};
use std::sync::atomic::Ordering;

const QUEUE_FULL_REASON: &str = "Upload queue full";

impl Client {
    /// Send a code 41 on the peer's control connection. No context lock is
    /// held while the actor's mailbox is touched.
    fn send_transfer_reply(
        client_context: &Arc<RwLock<ClientContext>>,
        username: &str,
        reply: crate::message::Message,
    ) {
        let registry = match client_context.read_safe() {
            Ok(ctx) => ctx.peer_registry.clone(),
            Err(e) => {
                error!("[client] transfer reply read: {e}");
                return;
            }
        };
        if let Some(registry) = registry {
            let _ = registry
                .send_to_peer(username, PeerMessage::SendMessage(reply));
        }
    }

    /// Decide a direction-0 reply from the live upload state. Separate from
    /// sending so the decision can be exercised without a peer connection.
    fn download_request_reply(
        context: &ClientContext,
        username: &str,
        token: u32,
        filename: &str,
    ) -> crate::message::Message {
        let matched = context.upload_queue.iter().any(|job| {
            job.downloader == username && job.virtual_path == filename
        }) || context.uploads.values().any(|job| {
            job.downloader == username && job.virtual_path == filename
        });
        if matched {
            MessageFactory::build_transfer_response_message(token)
        } else {
            MessageFactory::build_transfer_denial_message(
                token,
                "No matching transfer",
            )
        }
    }

    /// Direction 0: the peer wants one of our files. Only a file that is
    /// actually queued or already offered to that peer may be accepted.
    fn on_download_request(
        client_context: &Arc<RwLock<ClientContext>>,
        username: &str,
        token: u32,
        filename: &str,
    ) {
        let (registry, reply) = match client_context.read_safe() {
            Ok(ctx) => (
                ctx.peer_registry.clone(),
                Self::download_request_reply(&ctx, username, token, filename),
            ),
            Err(e) => {
                error!("[client] download request read: {e}");
                return;
            }
        };
        if let Some(registry) = registry {
            let _ = registry
                .send_to_peer(username, PeerMessage::SendMessage(reply));
        }
    }

    /// Decide a direction-1 reply and re-key a matching download. The caller
    /// holds the write lock so the file connection cannot race the token.
    fn upload_offer_reply(
        context: &mut ClientContext,
        username: &str,
        token: u32,
        filename: &str,
        size: u64,
    ) -> crate::message::Message {
        let download_to_update = context
            .downloads
            .get_by_file(username, filename)
            .map(|d| (d.token, d.clone()));
        let cancelled = download_to_update
            .as_ref()
            .is_some_and(|(_, d)| matches!(d.status, DownloadStatus::Cancelled));
        if !cancelled
            && let Some((old_token, download)) = download_to_update
        {
            trace!("[client] upload offer re-keys download {old_token} to {token}");
            context.add_download(Download {
                token,
                size,
                ..download
            });
            context.remove_download(old_token);
            MessageFactory::build_transfer_response_message(token)
        } else {
            let reason = if cancelled {
                "Cancelled"
            } else {
                "No matching transfer"
            };
            MessageFactory::build_transfer_denial_message(token, reason)
        }
    }

    /// Direction 1: the peer offers us a file we queued. Re-key the download
    /// to their token before inviting the file connection, and refuse an
    /// offer that has no live download behind it.
    fn on_upload_offer(
        client_context: &Arc<RwLock<ClientContext>>,
        username: &str,
        token: u32,
        filename: &str,
        size: u64,
    ) {
        let (registry, reply) = match client_context.write_safe() {
            Ok(mut ctx) => (
                ctx.peer_registry.clone(),
                Self::upload_offer_reply(
                    &mut ctx, username, token, filename, size,
                ),
            ),
            Err(e) => {
                error!("[client] upload offer write: {e}");
                return;
            }
        };
        if let Some(registry) = registry {
            let _ = registry
                .send_to_peer(username, PeerMessage::SendMessage(reply));
        }
    }

    pub(super) fn on_transfers(
        operation: ClientOperation,
        client_context: &Arc<RwLock<ClientContext>>,
        own_username: &str,
    ) {
        let own_username = own_username.to_string();
        match operation {
            ClientOperation::DownloadFromPeer(token, peer, allowed) => {
                let (maybe_download, work) =
                    match client_context.write_safe() {
                        Ok(mut ctx) => (
                            ctx.downloads.claim_for_peer(token, &peer.username),
                            Arc::clone(&ctx.work),
                        ),
                        Err(e) => {
                            error!("[client] DownloadFromPeer write: {}", e);
                            return;
                        }
                    };
                trace!(
                    "[client] DownloadFromPeer token: {} peer: {:?}",
                    token, peer
                );
                let Some(download) = maybe_download else {
                    debug!(
                        "[client] transfer token {} is missing, belongs \
                         to another peer, or is already claimed; ignoring",
                        token,
                    );
                    return;
                };

                let for_worker = Arc::clone(client_context);

                if let Err(e) = work.try_spawn(
                    "soulseek-download",
                    None,
                    move |cancel| {
                        if cancel.load(Ordering::Acquire) {
                            return;
                        }
                        let download_peer = DownloadPeer::new(
                            download.username.clone(),
                            peer.host.clone(),
                            peer.port,
                            token,
                            allowed,
                            own_username,
                        );
                        match download_peer.download_file(
                            for_worker,
                            Some(download.clone()),
                            None,
                        ) {
                            Ok((download, filename)) => {
                                info!(
                                    "Successfully downloaded {} bytes to {}",
                                    download.size, filename
                                );
                            }
                            Err(DownloadError::Cancelled) => {}
                            Err(e) => {
                                error!(
                                    "Failed to download file '{}' from {}:{} (token: {}) - Error: {}",
                                    download.filename,
                                    peer.host,
                                    peer.port,
                                    download.token,
                                    e
                                );
                            }
                        }
                    },
                ) {
                    // Shutdown cancellation is not a transfer failure: the
                    // owner is ending the session, so do not mark the download
                    // Failed or invite a retry on the next start.
                    if matches!(
                        &e,
                        crate::utils::semaphore::SpawnError::Stopped
                    ) {
                        return;
                    }
                    // The claim above succeeded; with no worker to serve it,
                    // report the failure on the download itself instead of
                    // leaving a transfer stuck in progress.
                    error!("[client] cannot start download {token}: {e}");
                    if let Ok(mut ctx) = client_context.write_safe() {
                        ctx.update_download_with_status(
                            token,
                            DownloadStatus::Failed(Some(
                                "no download worker slot".to_string(),
                            )),
                        );
                    }
                }
            }
            ClientOperation::UpdateDownloadTokens(transfer, username) => {
                // Direction decides what a code 40 means; the old code read a
                // size from every frame and funnelled both shapes into this
                // one download-token path.
                match transfer.direction {
                    crate::types::TransferDirection::DownloadRequest => {
                        Self::on_download_request(
                            client_context,
                            &username,
                            transfer.token,
                            &transfer.filename,
                        );
                    }
                    crate::types::TransferDirection::UploadOffer { size } => {
                        Self::on_upload_offer(
                            client_context,
                            &username,
                            transfer.token,
                            &transfer.filename,
                            size,
                        );
                    }
                    crate::types::TransferDirection::Invalid => {
                        Self::send_transfer_reply(
                            client_context,
                            &username,
                            MessageFactory::build_transfer_denial_message(
                                transfer.token,
                                "invalid transfer request",
                            ),
                        );
                    }
                }
            }
            ClientOperation::StartUpload { token } => {
                // The peer accepted our offer: resolve their
                // address (from the code-9 GetPeerAddress) and
                // stream the file, or queue until it resolves.
                let (job_addr, downloader) = match client_context.write_safe() {
                    Ok(mut ctx) => {
                        ctx.mark_offer_answered(token);
                        let Some(job) = ctx.uploads.get(&token) else {
                            return;
                        };
                        (
                            ctx.peer_address(&job.downloader),
                            job.downloader.clone(),
                        )
                    }
                    Err(_) => return,
                };
                if let Some((host, port)) = job_addr {
                    Self::spawn_serve(
                        client_context,
                        &own_username,
                        token,
                        host,
                        port,
                    );
                } else {
                    let dropped = if let Ok(mut ctx) = client_context.write_safe()
                        && !ctx.add_pending_serve(&downloader, token)
                    {
                        debug!(
                            "[client] pending serves full; expiring offer {token}"
                        );
                        ctx.drop_offer(token);
                        true
                    } else {
                        false
                    };
                    if dropped {
                        Self::pump_upload_queue(client_context);
                    }
                    if let Ok(ctx) = client_context.read_safe()
                        && let Some(sender) = ctx.server_sender.clone()
                    {
                        let _ = sender
                            .send(ServerMessage::GetPeerAddress(downloader));
                    }
                }
            }
            ClientOperation::QueueUpload {
                requester_key,
                filename,
            } => {
                // The peer served next may not be this one.
                let (registry, accepted) = match client_context.write_safe() {
                    Ok(mut ctx) => {
                        let Some(file) = ctx.shares.get(&filename) else {
                            debug!(
                                "[client] QueueUpload for unknown file {}",
                                filename
                            );
                            return;
                        };
                        let size = file.size;
                        let accepted = ctx.enqueue_upload(
                            &requester_key,
                            &filename,
                            file,
                            size,
                        );
                        if !accepted {
                            debug!(
                                "[client] upload queue full; refusing {filename} \
                                 from {requester_key}"
                            );
                        }
                        (ctx.peer_registry.clone(), accepted)
                    }
                    Err(e) => {
                        error!("[client] QueueUpload write: {}", e);
                        return;
                    }
                };
                if !accepted {
                    if let Some(registry) = registry {
                        let denial = crate::message::peer::build_upload_denied(
                            &filename,
                            QUEUE_FULL_REASON,
                        );
                        if let Err(e) = registry.send_to_peer(
                            &requester_key,
                            PeerMessage::SendMessage(denial),
                        ) {
                            error!(
                                "[client] cannot deliver queue-full denial to \
                                 {requester_key}: {e}"
                            );
                        }
                    }
                    return;
                }
                Self::pump_upload_queue(client_context);
            }
            // The dispatch loop routes only the variants above to here.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    fn request(direction: u32, token: u32, filename: &str, size: Option<u64>) -> crate::types::Transfer {
        let mut built = Message::new();
        built
            .write_int32(40)
            .write_int32(direction)
            .write_int32(token)
            .write_string(filename);
        if let Some(size) = size {
            built.write_int64(size);
        }
        let mut framed = Message::new_with_data(built.get_buffer());
        framed.set_pointer(8);
        crate::types::Transfer::new_from_message(&mut framed)
    }

    fn assert_reply(reply: &Message, token: u32, allowed: bool) {
        let body = reply.get_data();
        assert_eq!(
            u32::from_le_bytes(body[0..4].try_into().unwrap()),
            41,
            "the reply must be a TransferResponse"
        );
        assert_eq!(
            u32::from_le_bytes(body[4..8].try_into().unwrap()),
            token,
            "the reply must quote the peer's token"
        );
        assert_eq!(body[8], u8::from(allowed));
    }

    #[test]
    fn a_transfer_request_with_no_queued_file_is_denied() {
        // A peer that sends a code 40 for a file we never queued must hear a
        // refusal. Replying "allowed" invites a file connection that matches
        // nothing and leaves the transfer stuck.
        let context = ClientContext::for_user("me");
        let transfer = request(0, 999, "not-queued.mp3", None);
        assert_eq!(
            transfer.direction,
            crate::types::TransferDirection::DownloadRequest
        );
        assert_reply(
            &Client::download_request_reply(
                &context,
                "bob",
                transfer.token,
                &transfer.filename,
            ),
            999,
            false,
        );
    }

    #[test]
    fn an_upload_offer_without_a_queued_download_is_denied() {
        let mut context = ClientContext::for_user("me");
        let transfer = request(1, 999, "not-queued.mp3", Some(4096));
        assert_eq!(
            transfer.direction,
            crate::types::TransferDirection::UploadOffer { size: 4096 }
        );
        assert_reply(
            &Client::upload_offer_reply(
                &mut context,
                "bob",
                transfer.token,
                &transfer.filename,
                4096,
            ),
            999,
            false,
        );
    }

    #[test]
    fn a_download_request_for_a_queued_file_is_allowed() {
        let mut context = ClientContext::for_user("me");
        let file = crate::shares::synthetic_file(std::path::Path::new(
            "song.mp3",
        ));
        assert!(context.enqueue_upload("bob", "song.mp3", Arc::new(file), 4096));
        let reply = Client::download_request_reply(&context, "bob", 999, "song.mp3");
        assert_reply(&reply, 999, true);
    }

    #[test]
    fn a_matching_upload_offer_rekeys_the_download() {
        let mut context = ClientContext::for_user("me");
        context.add_download(Download {
            username: "bob".to_string(),
            filename: "song.mp3".to_string(),
            token: 7,
            size: 100,
            download_directory: "test".to_string(),
            status: DownloadStatus::Queued,
            sender: std::sync::mpsc::channel().0,
            queue_position: None,
            metadata: crate::types::DownloadMetadata::default(),
        });
        let reply =
            Client::upload_offer_reply(&mut context, "bob", 999, "song.mp3", 4096);
        assert_reply(&reply, 999, true);
        assert_eq!(
            context.get_download_by_token(999).map(|d| d.size),
            Some(4096)
        );
        assert!(context.get_download_by_token(7).is_none());
    }

    #[test]
    fn a_download_is_failed_when_the_worker_registry_is_full() {
        use crate::utils::semaphore::WorkRegistry;
        use std::time::Duration;

        let client_context = Arc::new(RwLock::new(ClientContext::new()));
        let work = Arc::new(WorkRegistry::new(1));
        client_context.write().unwrap().work = Arc::clone(&work);
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        work.try_spawn("blocker", None, move |stop| {
            entered_tx.send(()).unwrap();
            while !stop.load(Ordering::Acquire)
                && release_rx.recv_timeout(Duration::from_millis(5)).is_err()
            {}
        })
        .expect("the only slot is free");
        entered_rx.recv_timeout(Duration::from_millis(200)).unwrap();

        client_context.write().unwrap().add_download(Download {
            username: "bob".to_string(),
            filename: "song.mp3".to_string(),
            token: 9,
            size: 100,
            download_directory: "test".to_string(),
            status: DownloadStatus::Queued,
            sender: std::sync::mpsc::channel().0,
            queue_position: None,
            metadata: crate::types::DownloadMetadata::default(),
        });
        let peer = crate::peer::Peer::new(
            "bob".to_string(),
            crate::peer::ConnectionType::P,
            "127.0.0.1".to_string(),
            1,
            None,
            0,
            0,
            0,
        );
        Client::on_transfers(
            ClientOperation::DownloadFromPeer(9, peer, true),
            &client_context,
            "me",
        );

        assert!(
            matches!(
                client_context
                    .read()
                    .unwrap()
                    .get_download_by_token(9)
                    .map(|download| &download.status),
                Some(DownloadStatus::Failed(_))
            ),
            "a refused worker must not leave a download stuck in progress"
        );
        let _ = release_tx.send(());
        work.stop_all();
    }

    #[test]
    fn an_unknown_direction_is_denied() {
        let transfer = request(7, 999, "song.mp3", Some(4096));
        assert_eq!(transfer.direction, crate::types::TransferDirection::Invalid);
        assert_eq!(transfer.token, 999);
        assert_reply(
            &MessageFactory::build_transfer_denial_message(
                transfer.token,
                "invalid transfer request",
            ),
            999,
            false,
        );
    }
}
