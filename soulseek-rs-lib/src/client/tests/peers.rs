//! Uploads, broker connects, and the peers a disconnect may not drop.

use super::*;
use std::io::Read;

static TEST_DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

fn captured_peer(
    username: &str,
    ops: &mpsc::Sender<ClientOperation>,
) -> (PeerRegistry, TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let ours = TcpStream::connect(addr).unwrap();
    ours.set_nonblocking(true).unwrap();
    let far_end = listener.accept().unwrap().0;
    far_end
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let registry = PeerRegistry::new(
        Arc::new(ActorSystem::new()),
        ops.clone(),
        "me".to_string(),
    );
    let peer = Peer::new(
        username.to_string(),
        ConnectionType::P,
        "127.0.0.1".to_string(),
        u32::from(addr.port()),
        None,
        0,
        0,
        0,
    );
    registry.register_peer(peer, Some(ours), None).unwrap();
    (registry, far_end)
}

fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut chunk = [0u8; 512];
    while frame.len() < 4 {
        let read = stream.read(&mut chunk).expect("read frame length");
        assert!(read > 0, "the peer closed before sending a frame");
        frame.extend_from_slice(&chunk[..read]);
    }
    let size = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
    while frame.len() < 4 + size {
        let read = stream.read(&mut chunk).expect("read frame body");
        assert!(read > 0, "the peer closed in the middle of a frame");
        frame.extend_from_slice(&chunk[..read]);
    }
    frame
}

fn assert_no_frame(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    match stream.read(&mut [0u8; 16]) {
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) => {}
        other => panic!("the peer should have heard nothing, got {other:?}"),
    }
}

fn fence(ops: &mpsc::Sender<ClientOperation>, client: &Client, value: u32) {
    ops.send(ClientOperation::OwnPrivileges(value)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.own_privilege_seconds() != Some(value) {
        assert!(Instant::now() < deadline, "ops loop never caught up");
        thread::sleep(Duration::from_millis(5));
    }
}

fn setup_share(client: &Client, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "soulseek-uploads-{}-{}",
        std::process::id(),
        TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), b"data").unwrap();
    let virtual_path =
        format!("{}\\{name}", dir.file_name().unwrap().to_string_lossy());
    let shares = crate::shares::Shares::scan(&dir).unwrap();
    client.context.write().unwrap().shares = Arc::new(shares);
    virtual_path
}

fn synthetic() -> Arc<crate::shares::SharedFile> {
    Arc::new(crate::shares::synthetic_file(std::path::Path::new(
        "/tmp/x.mp3",
    )))
}

fn occupy_slot(ctx: &mut ClientContext, token: u32, downloader: &str) {
    ctx.upload_slots = 1;
    ctx.uploads.insert(
        token,
        UploadJob {
            downloader: downloader.to_string(),
            file: synthetic(),
            virtual_path: format!("@@share\\{downloader}.mp3"),
            size: 4096,
            offered: None,
        },
    );
}

fn add_controlled_serve(
    ctx: &mut ClientContext,
    username: &str,
    token: u32,
    since: Instant,
) {
    ctx.pending_serves
        .entry(username.to_string())
        .or_default()
        .push(PendingServe { token, since });
}

fn pending_serve_context(token: u32, downloader: &str) -> ClientContext {
    let mut ctx = ClientContext::new();
    occupy_slot(&mut ctx, token, downloader);
    ctx.uploads.get_mut(&token).unwrap().virtual_path =
        "@@share\\pending.mp3".to_string();
    ctx
}

fn failed_events(ctx: &ClientContext) -> Vec<crate::types::UploadInfo> {
    let mut events = Vec::new();
    for event in &ctx.upload_events {
        if matches!(event.status, UploadStatus::Failed(_)) {
            events.push(event.clone());
        }
    }
    events
}

#[test]
fn upload_speed_is_reported_only_while_running() {
    let two_seconds_ago = Instant::now()
        .checked_sub(Duration::from_secs(2))
        .expect("clock supports a 2s offset");

    // 1 KiB over ~2s is ~512 B/s.
    let rate = upload_speed(&UploadStatus::InProgress, 1024, two_seconds_ago);
    assert!((rate - 512.0).abs() < 50.0, "unexpected rate {rate}");

    // A finished upload reports no rate, exactly as a finished download
    // does, so the Speed column renders "-" rather than a stale figure.
    for status in [
        UploadStatus::Completed,
        UploadStatus::Cancelled,
        UploadStatus::Failed("nope".to_string()),
    ] {
        let rate = upload_speed(&status, 1024, two_seconds_ago);
        assert!(rate.abs() < f64::EPSILON, "unexpected rate {rate}");
    }

    // A just-started upload must not divide by a zero elapsed time.
    let rate = upload_speed(&UploadStatus::InProgress, 0, Instant::now());
    assert!(
        rate.is_finite() && rate.abs() < f64::EPSILON,
        "unexpected rate {rate}"
    );
}

// A clean disconnect — the idle reaper, or a remote client tidying an idle
// socket while it waits in our queue — must keep the peer's queued uploads;
// only an error disconnect is evidence the peer is gone.

#[test]
fn a_clean_disconnect_keeps_queued_uploads_an_error_drops_them() {
    let client = Client::new("test-user", "test-password");
    let (ops, ops_rx) = mpsc::channel();
    Client::listen_to_client_operations(
        ops_rx,
        client.context.clone(),
        "me".to_string(),
    );

    client.context.write().unwrap().enqueue_upload(
        "amy",
        "@@share\\f.mp3",
        std::sync::Arc::new(crate::shares::synthetic_file(std::path::Path::new(
            "/tmp/f.mp3",
        ))),
        4096,
    );

    ops.send(ClientOperation::PeerDisconnected(
        1,
        "amy".to_string(),
        None,
    ))
    .unwrap();
    // The loop is serial, so once this fence op is visible the disconnect
    // before it has been handled.
    ops.send(ClientOperation::OwnPrivileges(7)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.own_privilege_seconds() != Some(7) {
        assert!(Instant::now() < deadline, "ops loop never caught up");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        client
            .context
            .read()
            .unwrap()
            .place_in_queue("amy", "@@share\\f.mp3"),
        Some(1),
        "a clean disconnect must keep the queue"
    );

    ops.send(ClientOperation::PeerDisconnected(
        1,
        "amy".to_string(),
        Some(crate::error::SoulseekRs::NotConnected),
    ))
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let place = client
            .context
            .read()
            .unwrap()
            .place_in_queue("amy", "@@share\\f.mp3");
        if place.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "an error disconnect must drop the queue"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// The real login handler reports rejection as `Err(AuthenticationFailed)`.
/// This server accepts the first login, drops the session, and rejects the
/// relogin, so the reconnect policy can be tested against the real contract.
#[test]
fn a_rejected_relogin_reaches_the_caller_as_authentication_failed() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn frame(code: u32, body: &[u8]) -> Vec<u8> {
        let mut bytes = (body.len() as u32 + 4).to_le_bytes().to_vec();
        bytes.extend_from_slice(&code.to_le_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut logins = 0;
        while logins < 2 {
            let mut length = [0u8; 4];
            stream.read_exact(&mut length).unwrap();
            let mut body = vec![0u8; u32::from_le_bytes(length) as usize];
            stream.read_exact(&mut body).unwrap();
            if u32::from_le_bytes(body[..4].try_into().unwrap()) != 1 {
                continue; // SetWaitPort and the other post-login messages.
            }
            let accepted = logins == 0;
            logins += 1;
            let reply = if accepted {
                let greeting = b"welcome";
                let mut body = vec![1];
                body.extend_from_slice(&(greeting.len() as u32).to_le_bytes());
                body.extend_from_slice(greeting);
                frame(1, &body)
            } else {
                frame(1, &[0])
            };
            stream.write_all(&reply).unwrap();
        }
        // Keep the socket open until the client has read both verdicts.
        thread::sleep(Duration::from_secs(3));
    });

    let mut client = Client::with_settings(ClientSettings {
        username: "relogin".into(),
        password: "pw".into(),
        server_address: PeerAddress::new("127.0.0.1".into(), address.port()),
        enable_listen: false,
        listen_port: 0,
        shared_directories: Vec::new(),
        accept_children: false,
        version: ClientVersion::default(),
    });
    client.connect().expect("connect");
    assert!(client.login().expect("first login"));

    let result = client.login();
    assert!(
        matches!(result, Err(SoulseekRs::AuthenticationFailed)),
        "rejection must reach the policy as the real error: {result:?}"
    );
    server.join().unwrap();
}

#[test]
fn an_active_login_wait_ends_when_stop_is_set() {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // A local server that accepts the connection and withholds the verdict.
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(3));
    });

    let mut client = Client::with_settings(ClientSettings {
        username: "cancelled".into(),
        password: "pw".into(),
        server_address: PeerAddress::new("127.0.0.1".into(), address.port()),
        enable_listen: false,
        listen_port: 0,
        shared_directories: Vec::new(),
        accept_children: false,
        version: ClientVersion::default(),
    });
    client.connect().expect("connect");
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            stop.store(true, Ordering::Relaxed);
        });
    }
    let started = Instant::now();
    let result = client.login_with_stop(&stop);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "stop must cancel the login wait instead of waiting for the server"
    );
    assert!(matches!(result, Err(SoulseekRs::NotConnected)), "{result:?}");
    server.join().unwrap();
}

#[test]
fn broker_admission_refusal_fails_queued_downloads_with_a_retryable_cause() {
    let client = Client::new("me", "p");
    let (ops, ops_rx) = mpsc::channel();
    Client::listen_to_client_operations(
        ops_rx,
        client.context.clone(),
        "me".into(),
    );
    let (sender, receiver) = mpsc::channel();
    {
        let mut ctx = client.context.write().unwrap();
        for token in 0..super::super::MAX_PENDING_CONNECTS {
            assert!(ctx.add_pending_connect(token as u32, "ghost".into()));
        }
        ctx.add_download(download(
            "peer",
            "f.mp3",
            7,
            DownloadStatus::Queued,
            sender,
        ));
    }

    ops.send(ClientOperation::PeerConnectFailed(
        1,
        "peer".into(),
        None,
    ))
    .unwrap();
    ops.send(ClientOperation::OwnPrivileges(7)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.own_privilege_seconds() != Some(7) {
        assert!(Instant::now() < deadline, "ops loop never caught up");
        thread::sleep(Duration::from_millis(10));
    }

    match receiver.try_recv() {
        Ok(DownloadStatus::Failed(Some(reason))) => {
            assert!(reason.contains("retry"), "{reason}");
        }
        other => panic!("queued download was stranded: {other:?}"),
    }
    assert!(matches!(
        client
            .context
            .read()
            .unwrap()
            .get_download_by_token(7)
            .expect("download still recorded")
            .status,
        DownloadStatus::Failed(_)
    ));

    // The refusal consumed no token: freeing one slot lets the next attempt
    // be admitted, so its queued download is no longer failed on sight.
    assert!(client.context.write().unwrap().take_pending_connect(0).is_some());
    let (retry_sender, retry_receiver) = mpsc::channel();
    client.context.write().unwrap().add_download(download(
        "peer",
        "f2.mp3",
        8,
        DownloadStatus::Queued,
        retry_sender,
    ));
    ops.send(ClientOperation::PeerConnectFailed(
        2,
        "peer".into(),
        None,
    ))
    .unwrap();
    ops.send(ClientOperation::OwnPrivileges(8)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.own_privilege_seconds() != Some(8) {
        assert!(Instant::now() < deadline, "ops loop never caught up");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        retry_receiver.recv_timeout(Duration::from_millis(200)).is_err(),
        "an admitted retry must not be failed by the saturation branch"
    );
}

#[test]
fn a_pending_connect_expires_after_the_broker_timeout() {
    let mut context = ClientContext::new();
    context.add_pending_connect(7, "ghost".to_string());

    assert!(context.take_expired_connects(Instant::now()).is_empty());

    let after_deadline =
        Instant::now() + BROKER_CONNECT_TIMEOUT + Duration::from_secs(1);
    assert_eq!(
        context.take_expired_connects(after_deadline),
        vec!["ghost".to_string()]
    );
    assert!(context.take_pending_connect(7).is_none());
}

#[test]
fn pending_serves_expire_and_accept_no_flood() {
    use std::time::{Duration, Instant};

    let mut context = ClientContext::new();
    assert!(context.add_pending_serve("waiting", 1));
    let now = Instant::now() + Duration::from_secs(120);
    context.sweep_expired_serves(now);
    assert!(
        context.pending_serves.is_empty(),
        "an unanswered address request must not hold an upload slot forever"
    );

    for token in 0..super::super::MAX_PENDING_SERVES_PER_PEER as u32 {
        assert!(context.add_pending_serve("flooder", token));
    }
    assert!(
        !context.add_pending_serve("flooder", 9999),
        "one peer must not fill the pending-serve map"
    );
}

#[test]
fn protected_peers_covers_downloads_uploads_and_pending_serves() {
    let mut context = ClientContext::new();
    context.add_download(download(
        "downloader",
        "song.mp3",
        1,
        DownloadStatus::Queued,
        mpsc::channel().0,
    ));
    context.add_download(download(
        "done",
        "old.mp3",
        2,
        DownloadStatus::Completed,
        mpsc::channel().0,
    ));
    assert!(context.add_pending_serve("waiting", 9));
    context.mark_browse_pending("browsed");

    let protected = context.protected_peers();
    assert!(protected.contains("downloader"));
    assert!(protected.contains("waiting"));
    assert!(protected.contains("browsed"));
    assert!(!protected.contains("done"));

    context.store_browse_result("browsed".to_string(), Vec::new());
    assert!(!context.protected_peers().contains("browsed"));
}

#[test]
fn a_peer_message_nobody_could_deliver_expires() {
    let mut ctx = ClientContext::for_user("me");
    ctx.queue_peer_message("ghost", crate::message::Message::new());
    ctx.expire_pending_peer_messages(Instant::now());
    assert_eq!(ctx.take_peer_messages("ghost").len(), 1, "still fresh");

    ctx.queue_peer_message("ghost", crate::message::Message::new());
    ctx.expire_pending_peer_messages(Instant::now() + PENDING_PEER_TTL);
    assert!(ctx.take_peer_messages("ghost").is_empty(), "expired");
}

#[test]
fn the_server_is_told_our_child_capacity_only_when_it_changes() {
    // AcceptChildren is a standing state on the server, not a heartbeat.
    let mut ctx = ClientContext::new();
    assert_eq!(
        ctx.accept_children_change(),
        Some(false),
        "the first answer is always worth sending"
    );
    assert_eq!(ctx.accept_children_change(), None, "unchanged, so silent");
}

#[test]
fn a_full_broker_table_fails_the_upload_without_holding_the_context_lock() {
    let client = Client::new("u", "p");
    let (ops, ops_rx) = mpsc::channel();
    Client::listen_to_client_operations(
        ops_rx,
        client.context.clone(),
        "u".to_string(),
    );

    let token = 9001;
    let file = synthetic();
    {
        let mut ctx = client.context.write().unwrap();
        for index in 0..super::super::MAX_BROKERED_UPLOADS {
            ctx.brokered_uploads.insert(
                u32::try_from(index).unwrap() + 1,
                BrokeredUpload {
                    token: 7000 + u32::try_from(index).unwrap(),
                    file: Arc::clone(&file),
                    deadline: Instant::now() + Duration::from_secs(3600),
                    claimed: Arc::new(AtomicBool::new(false)),
                },
            );
        }
        occupy_slot(&mut ctx, token, "amy");
        ctx.enqueue_upload("bob", "next.mp3", Arc::clone(&file), 4096);
    }

    Client::spawn_serve(
        &client.context,
        "u",
        token,
        "127.0.0.1".to_string(),
        0,
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        let status = client
            .context
            .read()
            .unwrap()
            .active_uploads
            .get(&token)
            .map(|upload| upload.status.clone());
        if matches!(status, Some(UploadStatus::Failed(_))) {
            break status.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "the brokered-upload cap deadlocked: {status:?}"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(
        status,
        UploadStatus::Failed("too many brokered uploads".to_string())
    );

    {
        let ctx = client.context.read().unwrap();
        assert!(
            ctx.upload_queue.is_empty(),
            "the rejected upload must hand its slot to the next in line"
        );
        let offered: Vec<_> =
            ctx.uploads.values().map(|job| job.downloader.as_str()).collect();
        assert_eq!(offered, ["bob"], "the next queued upload should be served");
    }

    fence(&ops, &client, 11);
    assert!(
        client.context.try_write().is_ok(),
        "no context guard should be left held after the rejection"
    );
    drop(ops);
    assert!(
        client.context.try_read().is_ok(),
        "shutting the operations loop must not leave a guard behind"
    );
}

#[test]
fn a_full_peer_queue_denies_the_request_and_a_retry_takes_the_freed_place() {
    let client = Client::new("u", "p");
    let (ops, ops_rx) = mpsc::channel();
    Client::listen_to_client_operations(
        ops_rx,
        client.context.clone(),
        "u".to_string(),
    );
    let (registry, mut captured) = captured_peer("amy", &ops);
    let filename = setup_share(&client, "f.mp3");
    {
        let mut ctx = client.context.write().unwrap();
        ctx.peer_registry = Some(registry);
        occupy_slot(&mut ctx, 1, "busy");
        for index in 0..super::super::upload_queue::MAX_QUEUED_UPLOADS_PER_PEER {
            ctx.enqueue_upload(
                "amy",
                &format!("old-{index}.mp3"),
                synthetic(),
                4096,
            );
        }
    }

    ops.send(ClientOperation::QueueUpload {
        requester_key: "amy".to_string(),
        filename: filename.clone(),
    })
    .unwrap();

    let frame = read_frame(&mut captured);
    assert_eq!(&frame[4..8], &50u32.to_le_bytes());
    let mut message = crate::message::Message::new_with_data(frame);
    message.set_pointer(8);
    let sent_filename = message.read_string();
    let reason = message.read_string();
    assert_eq!(sent_filename, filename);
    assert_ne!(reason, "Queued", "Queued means the request was accepted");
    assert_eq!(reason, "Upload queue full");

    fence(&ops, &client, 11);
    {
        let ctx = client.context.read().unwrap();
        assert_eq!(ctx.upload_queue.len(), super::super::upload_queue::MAX_QUEUED_UPLOADS_PER_PEER);
        assert!(ctx.place_in_queue("amy", &filename).is_none());
        assert_eq!(ctx.uploads.len(), 1, "a denied file must not take a slot");
    }

    client.context.write().unwrap().upload_queue.remove(0);
    ops.send(ClientOperation::QueueUpload {
        requester_key: "amy".to_string(),
        filename: filename.clone(),
    })
    .unwrap();
    fence(&ops, &client, 12);
    assert_no_frame(&mut captured);

    let ctx = client.context.read().unwrap();
    assert_eq!(ctx.upload_queue.len(), super::super::upload_queue::MAX_QUEUED_UPLOADS_PER_PEER);
    assert_eq!(
        ctx.upload_queue
            .iter()
            .filter(|job| job.downloader == "amy" && job.virtual_path == filename)
            .count(),
        1,
        "the retry must take the freed place exactly once"
    );
    assert_eq!(ctx.uploads.len(), 1);
}

#[test]
fn a_globally_full_queue_denies_a_new_peer_and_recovers_when_a_place_frees() {
    let client = Client::new("u", "p");
    let (ops, ops_rx) = mpsc::channel();
    Client::listen_to_client_operations(
        ops_rx,
        client.context.clone(),
        "u".to_string(),
    );
    let (registry, mut captured) = captured_peer("amy", &ops);
    let filename = setup_share(&client, "global.mp3");
    {
        let mut ctx = client.context.write().unwrap();
        ctx.peer_registry = Some(registry);
        occupy_slot(&mut ctx, 1, "busy");
        for index in 0..super::super::upload_queue::MAX_QUEUED_UPLOADS {
            ctx.enqueue_upload(
                &format!("peer-{index}"),
                "f.mp3",
                synthetic(),
                4096,
            );
        }
    }

    ops.send(ClientOperation::QueueUpload {
        requester_key: "amy".to_string(),
        filename: filename.clone(),
    })
    .unwrap();
    let frame = read_frame(&mut captured);
    let mut message = crate::message::Message::new_with_data(frame);
    message.set_pointer(8);
    assert_eq!(message.read_string(), filename);
    assert_eq!(message.read_string(), "Upload queue full");

    fence(&ops, &client, 11);
    {
        let ctx = client.context.read().unwrap();
        assert_eq!(ctx.upload_queue.len(), super::super::upload_queue::MAX_QUEUED_UPLOADS);
        assert!(ctx.place_in_queue("amy", &filename).is_none());
        assert_eq!(ctx.uploads.len(), 1);
    }

    client.context.write().unwrap().upload_queue.remove(0);
    ops.send(ClientOperation::QueueUpload {
        requester_key: "amy".to_string(),
        filename: filename.clone(),
    })
    .unwrap();
    fence(&ops, &client, 12);
    assert_no_frame(&mut captured);

    let ctx = client.context.read().unwrap();
    assert_eq!(ctx.upload_queue.len(), super::super::upload_queue::MAX_QUEUED_UPLOADS);
    assert_eq!(
        ctx.upload_queue
            .iter()
            .filter(|job| job.downloader == "amy" && job.virtual_path == filename)
            .count(),
        1
    );
    assert_eq!(ctx.uploads.len(), 1);
}

#[test]
fn an_address_before_expiry_keeps_the_job_and_frees_the_next_one() {
    let mut ctx = pending_serve_context(1, "amy");
    ctx.enqueue_upload("bob", "next.mp3", synthetic(), 4096);
    let since = Instant::now();
    add_controlled_serve(&mut ctx, "amy", 1, since);

    let live = ctx.take_pending_serves(
        "amy",
        (since + super::super::PENDING_SERVE_TTL)
            .checked_sub(Duration::from_nanos(1))
            .unwrap(),
    );
    assert_eq!(live, [1]);
    assert!(ctx.uploads.contains_key(&1));
    assert!(ctx.take_upload_events().is_empty());

    ctx.uploads.remove(&1);
    let mut token = 100;
    let offers = ctx.pump_uploads(|| {
        token += 1;
        token
    }).1;
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].requester_key, "bob");
}

#[test]
fn an_address_at_or_after_expiry_matches_the_timer_sweep() {
    let since = Instant::now();
    for now in [
        since + super::super::PENDING_SERVE_TTL,
        since + super::super::PENDING_SERVE_TTL + Duration::from_secs(1),
    ] {
        let late_answer = {
            let mut ctx = pending_serve_context(1, "amy");
            ctx.enqueue_upload("bob", "next.mp3", synthetic(), 4096);
            add_controlled_serve(&mut ctx, "amy", 1, since);
            assert!(ctx.take_pending_serves("amy", now).is_empty());
            ctx
        };
        let timer_sweep = {
            let mut ctx = pending_serve_context(1, "amy");
            ctx.enqueue_upload("bob", "next.mp3", synthetic(), 4096);
            add_controlled_serve(&mut ctx, "amy", 1, since);
            ctx.sweep_expired_serves(now);
            ctx
        };

        for mut ctx in [late_answer, timer_sweep] {
            assert!(ctx.pending_serves.is_empty());
            assert!(!ctx.uploads.contains_key(&1), "the slot must be released");
            let events = failed_events(&ctx);
            assert_eq!(events.len(), 1, "the terminal record must survive");
            assert_eq!(events[0].username, "amy");
            assert_eq!(events[0].filename, "@@share\\pending.mp3");
            assert_eq!(
                events[0].status,
                UploadStatus::Failed("peer address never arrived".to_string())
            );
            let mut token = 200;
            let offers = ctx.pump_uploads(|| {
                token += 1;
                token
            }).1;
            assert_eq!(offers.len(), 1);
            assert_eq!(offers[0].requester_key, "bob");
        }
    }
}

#[test]
fn a_late_answer_and_the_sweep_leave_one_record_whichever_runs_first() {
    let since = Instant::now();
    let now = since + super::super::PENDING_SERVE_TTL + Duration::from_secs(1);

    let mut ctx = pending_serve_context(1, "amy");
    add_controlled_serve(&mut ctx, "amy", 1, since);
    assert!(ctx.take_pending_serves("amy", now).is_empty());
    ctx.sweep_expired_serves(now);
    assert!(ctx.take_pending_serves("amy", now).is_empty());
    assert_eq!(failed_events(&ctx).len(), 1);

    let mut ctx = pending_serve_context(1, "amy");
    add_controlled_serve(&mut ctx, "amy", 1, since);
    ctx.sweep_expired_serves(now);
    assert!(ctx.take_pending_serves("amy", now).is_empty());
    assert!(ctx.take_pending_serves("amy", now).is_empty());
    let events = failed_events(&ctx);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].status,
        UploadStatus::Failed("peer address never arrived".to_string())
    );
}

#[test]
fn a_live_address_response_serves_the_offer_and_fills_the_freed_slot() {
    let client = Client::new("u", "p");
    let (ops, ops_rx) = mpsc::channel();
    Client::listen_to_client_operations(
        ops_rx,
        client.context.clone(),
        "u".to_string(),
    );
    {
        let mut ctx = client.context.write().unwrap();
        occupy_slot(&mut ctx, 77, "amy");
        ctx.enqueue_upload("bob", "next.mp3", synthetic(), 4096);
        assert!(ctx.add_pending_serve("amy", 77));
    }

    ops.send(ClientOperation::GetPeerAddressResponse {
        username: "amy".to_string(),
        host: "127.0.0.1".to_string(),
        port: 0,
        obfuscation_type: 0,
        obfuscated_port: 0,
    })
    .unwrap();
    fence(&ops, &client, 11);

    let ctx = client.context.read().unwrap();
    assert!(ctx.active_uploads.contains_key(&77));
    assert!(ctx.upload_queue.is_empty());
    let served: Vec<_> =
        ctx.uploads.values().map(|job| job.downloader.as_str()).collect();
    assert_eq!(served, ["bob"]);
}

#[test]
fn a_pending_serve_that_cannot_be_tracked_reports_a_failure_and_frees_the_slot() {
    let client = Client::new("u", "p");
    let (ops, ops_rx) = mpsc::channel();
    Client::listen_to_client_operations(
        ops_rx,
        client.context.clone(),
        "u".to_string(),
    );
    {
        let mut ctx = client.context.write().unwrap();
        occupy_slot(&mut ctx, 500, "amy");
        ctx.enqueue_upload("bob", "next.mp3", synthetic(), 4096);
        for token in 0..super::MAX_PENDING_SERVES_PER_PEER as u32 {
            add_controlled_serve(&mut ctx, "amy", token, Instant::now());
        }
    }

    ops.send(ClientOperation::StartUpload { token: 500 }).unwrap();
    fence(&ops, &client, 11);

    let ctx = client.context.read().unwrap();
    assert!(!ctx.uploads.contains_key(&500));
    let events = failed_events(&ctx);
    assert_eq!(events.len(), 1, "the dropped offer's only record must survive");
    assert_eq!(events[0].username, "amy");
    assert_eq!(
        events[0].status,
        UploadStatus::Failed("pending address requests full".to_string())
    );
    let offered: Vec<_> =
        ctx.uploads.values().map(|job| job.downloader.as_str()).collect();
    assert_eq!(offered, ["bob"], "the freed slot goes to the next upload");
}
