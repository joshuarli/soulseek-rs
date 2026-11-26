use super::*;
use crate::utils::semaphore::bounded_channel;
use std::io::Read;
use std::net::TcpListener;

fn code_of(message: &Message) -> u32 {
    u32::from_le_bytes(message.get_data()[0..4].try_into().unwrap())
}

#[test]
fn a_timed_out_connect_parks_the_actor_in_disconnected() {
    let mut actor = parked_actor(1);
    actor.connection_state = ConnectionState::Connecting {
        since: Instant::now().checked_sub(Duration::from_secs(21)).unwrap(),
    };

    actor.tick();

    assert!(
        matches!(actor.connection_state, ConnectionState::Disconnected),
        "a timed-out connect must leave Connecting"
    );
}

#[test]
fn the_server_socket_keeps_itself_alive() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut actor = parked_actor(listener.local_addr().unwrap().port());

    assert!(actor.initiate_connection());

    assert!(
        crate::utils::keepalive::keepalive_enabled(
            actor.stream.as_ref().unwrap()
        ),
        "an idle NAT mapping must be kept alive and a dead path noticed"
    );
}

fn parked_actor(port: u16) -> ServerActor {
    parked_actor_with_client(port).0
}

fn parked_actor_with_client(
    port: u16,
) -> (ServerActor, std::sync::mpsc::Receiver<ClientOperation>) {
    let (tx, rx) = bounded_channel(64, "test operations");
    let actor = ServerActor::new(
        PeerAddress::new("127.0.0.1".to_string(), port),
        tx,
        0,
        false,
        0,
        0,
    );
    (actor, rx)
}

#[test]
fn dropping_the_connection_forgets_the_dispatcher_and_the_verdict() {
    let mut actor = parked_actor(1);
    actor.initialize_dispatcher();
    actor.connection_state = ConnectionState::Connected;
    actor.handle_login_status(true);

    actor.disconnect_with_error();

    assert_eq!(actor.session.loss(), Some(SessionLoss::Disconnected));
    assert!(
        actor.dispatcher_sender.is_none(),
        "a login queued now must wait in the buffer, not in a channel \
         the next connection replaces"
    );
    assert_eq!(
        actor.context.read().unwrap().logged_in,
        None,
        "the old verdict must not answer the next login"
    );
}

#[test]
fn disconnect_clears_a_partial_frame_so_the_next_session_reframes_clean() {
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).unwrap();
    let (mut server_side, _) = listener.accept().unwrap();
    let partial_frame = [10u8, 0, 0, 0, 1, 2, 3];
    server_side.write_all(&partial_frame).unwrap();
    server_side.flush().unwrap();
    client.set_nonblocking(true).unwrap();

    let mut actor = parked_actor(addr.port());
    actor.stream = Some(client);
    actor.connection_state = ConnectionState::Connected;
    for _ in 0..50 {
        actor.process_read();
        if actor.reader.buffer_len() > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        actor.reader.buffer_len() > 0,
        "an incomplete frame should be waiting for its rest"
    );

    actor.disconnect();

    assert_eq!(
        actor.reader.buffer_len(),
        0,
        "a stale partial frame carried across a reconnect reads the next \
         session's bytes as a length prefix and wedges recovery"
    );
}

#[test]
fn a_login_on_a_parked_actor_dials_again_and_queues_the_login() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut actor = parked_actor(listener.local_addr().unwrap().port());
    actor.session.record(SessionLoss::Disconnected);
    let (response, _verdict) = std::sync::mpsc::channel();

    actor.handle_login(
        "u".into(),
        "p".into(),
        ClientVersion::default(),
        response,
    );

    assert!(
        matches!(actor.connection_state, ConnectionState::Connecting { .. }),
        "a parked actor must dial again on login"
    );
    assert!(
        actor.login_frame.is_some(),
        "the login waits for the connection to come up"
    );
    assert!(
        actor.queued_messages.is_empty(),
        "the replay buffer must not hold the session-opening frame"
    );
}

#[test]
fn a_parked_actor_handles_login_rather_than_queueing_it() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut actor = parked_actor(listener.local_addr().unwrap().port());
    actor.session.record(SessionLoss::Disconnected);
    let (response, _verdict) = std::sync::mpsc::channel();

    actor.handle_message(ServerMessage::Login {
        username: "u".into(),
        password: "p".into(),
        version: ClientVersion::default(),
        response,
    });

    assert!(
        matches!(actor.connection_state, ConnectionState::Connecting { .. }),
        "a parked actor must act on a login, not park it in the queue"
    );
    assert!(
        actor
            .queued_messages
            .iter()
            .all(|m| !matches!(m, ServerMessage::Login { .. })),
        "the login must not be sitting unhandled in the queue"
    );
}

#[test]
fn a_login_with_nobody_listening_is_refused_at_once() {
    let mut actor = parked_actor(1);
    let (response, verdict) = std::sync::mpsc::channel();

    actor.handle_login(
        "u".into(),
        "p".into(),
        ClientVersion::default(),
        response,
    );

    assert!(
        matches!(
            verdict.recv_timeout(Duration::from_secs(5)),
            Ok(Err(SoulseekRs::NotConnected))
        ),
        "a refused dial must not make the caller wait out the verdict \
         timeout"
    );
}

#[test]
fn a_cancelled_actor_never_writes_the_login_it_queued() {
    use std::io::Read;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).unwrap();
    let (mut server_side, _) = listener.accept().unwrap();
    client.set_nonblocking(true).unwrap();

    let cancel = CancelHandle::new();
    let mut actor = parked_actor(addr.port());
    actor.set_cancel(cancel.clone());
    actor.stream = Some(client);
    actor.connection_state = ConnectionState::Connecting {
        since: Instant::now(),
    };
    let (response, _verdict) = std::sync::mpsc::channel();
    actor.handle_login(
        "u".into(),
        "p".into(),
        ClientVersion::default(),
        response,
    );
    assert!(actor.login_frame.is_some());
    assert!(actor.queued_messages.is_empty());

    cancel.cancel();
    actor.tick();
    assert!(matches!(actor.connection_state, ConnectionState::Connected));
    actor.on_stop();

    server_side
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut written = Vec::new();
    assert_eq!(server_side.read_to_end(&mut written).unwrap(), 0);
}

#[test]
fn a_successful_login_marks_the_session_live_again() {
    let mut actor = parked_actor(1);
    actor.session.record(SessionLoss::Disconnected);

    actor.handle_login_status(true);

    assert_eq!(actor.session.loss(), None);
}

// A new session starts without a parent: the leaf drops whatever tree it
// hung from and announces its parentless stance from there.
#[test]
fn a_successful_login_resets_the_distributed_leaf() {
    let (mut actor, client) = parked_actor_with_client(1);

    actor.handle_login_status(true);

    assert!(matches!(
        client.try_recv(),
        Ok(ClientOperation::ResetDistributed)
    ));
    // A new session also has to be told what only the session held: our
    // interests, which the server forgets when the old one ended.
    assert!(matches!(
        client.try_recv(),
        Ok(ClientOperation::SessionEstablished)
    ));
    actor.handle_login_status(false);
    assert!(client.try_recv().is_err(), "a failed login resets nothing");
}

#[test]
fn post_login_messages_carry_counts_and_conditional_wait_port() {
    let messages = post_login_messages(true, 4321, 3, 7);
    let codes: Vec<u32> = messages.iter().map(code_of).collect();
    // SharedFolders; SetStatus; SetWaitPort.
    assert_eq!(codes, vec![35, 28, 2]);

    // The SharedFolders message (code 35) carries the real counts.
    let shared = messages[0].get_data();
    assert_eq!(u32::from_le_bytes(shared[4..8].try_into().unwrap()), 3);
    assert_eq!(u32::from_le_bytes(shared[8..12].try_into().unwrap()), 7);

    // Not listening omits SetWaitPort (code 2).
    let no_listen = post_login_messages(false, 4321, 3, 7);
    let codes: Vec<u32> = no_listen.iter().map(code_of).collect();
    assert_eq!(codes, vec![35, 28]);
}

fn frame_codes(bytes: &[u8]) -> Vec<u32> {
    let mut codes = Vec::new();
    let mut position = 0;
    while position + 4 <= bytes.len() {
        let length =
            u32::from_le_bytes(bytes[position..position + 4].try_into().unwrap())
                as usize;
        position += 4;
        if length < 4 || position + length > bytes.len() {
            break;
        }
        codes.push(u32::from_le_bytes(
            bytes[position..position + 4].try_into().unwrap(),
        ));
        position += length;
    }
    codes
}

fn connected_peer_actor() -> (ServerActor, TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    client.set_nonblocking(true).unwrap();
    let (server_side, _) = listener.accept().unwrap();
    let mut actor = parked_actor(address.port());
    actor.stream = Some(client);
    actor.connection_state = ConnectionState::Connecting {
        since: Instant::now(),
    };
    (actor, server_side)
}

#[test]
fn the_login_frame_precedes_operations_queued_while_disconnected() {
    use std::io::Read;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    client.set_nonblocking(true).unwrap();
    let (mut server_side, _) = listener.accept().unwrap();
    server_side.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    let mut actor = parked_actor(address.port());
    actor.stream = Some(client);
    actor.connection_state = ConnectionState::Connecting {
        since: Instant::now(),
    };

    actor.handle_message(ServerMessage::GetPeerAddress("peer".into()));
    let (response, _verdict) = std::sync::mpsc::channel();
    actor.handle_login(
        "u".into(),
        "p".into(),
        ClientVersion::default(),
        response,
    );
    if let Ok(mut context) = actor.context.write() {
        context.logged_in = Some(true);
    }

    for _ in 0..20 {
        actor.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(actor);

    let mut written = Vec::new();
    server_side.read_to_end(&mut written).unwrap();
    let codes = frame_codes(&written);
    assert_eq!(
        codes.first(),
        Some(&1),
        "Login (code 1) must be the first frame of a new session: {codes:?}"
    );
}

#[test]
fn a_failed_reconnect_does_not_erase_operations_waiting_for_login() {
    let mut actor = parked_actor(1);
    let (response, verdict) = std::sync::mpsc::channel();

    actor.handle_message(ServerMessage::GetPeerAddress("peer".into()));
    actor.handle_login(
        "u".into(),
        "p".into(),
        ClientVersion::default(),
        response,
    );

    assert!(matches!(
        verdict.recv_timeout(Duration::from_secs(5)),
        Ok(Err(SoulseekRs::NotConnected))
    ));
    assert!(
        actor
            .queued_messages
            .iter()
            .any(|m| matches!(m, ServerMessage::GetPeerAddress(_))),
        "a refused dial must keep the operation for the next login"
    );
}

#[test]
fn replay_is_held_until_the_login_verdict_and_then_drained() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    client.set_nonblocking(true).unwrap();
    let (_server_side, _) = listener.accept().unwrap();
    let mut actor = parked_actor(address.port());
    actor.stream = Some(client);
    actor.connection_state = ConnectionState::Connecting {
        since: Instant::now(),
    };

    actor.handle_message(ServerMessage::GetPeerAddress("peer".into()));
    actor.tick();

    assert!(
        actor
            .queued_messages
            .iter()
            .any(|m| matches!(m, ServerMessage::GetPeerAddress(_))),
        "connection establishment alone must not replay operations"
    );
    actor.handle_login_status(false);
    assert!(
        actor
            .queued_messages
            .iter()
            .any(|m| matches!(m, ServerMessage::GetPeerAddress(_))),
        "a rejected verdict replays nothing"
    );

    actor.handle_login_status(true);
    assert!(
        actor.queued_messages.is_empty(),
        "a successful login drains the replay buffer in order"
    );
}

#[test]
fn an_operation_sent_before_the_verdict_waits_in_the_replay_buffer() {
    let (mut actor, _server_side) = connected_peer_actor();
    actor.tick();

    actor.file_search(7, "query");

    assert!(
        actor
            .queued_messages
            .iter()
            .any(|m| matches!(m, ServerMessage::SendMessage(_))),
        "a frame queued before login must not reach the socket"
    );
    actor.handle_login_status(true);
    assert!(actor.queued_messages.is_empty());
}

fn cap_buffer(stream: &TcpStream, option: libc::c_int) {
    use std::os::unix::io::AsRawFd;

    let size: libc::c_int = 64 * 1024;
    // SAFETY: `size` outlives the call and its exact size travels with it.
    let rc = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            option,
            (&raw const size).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    assert_eq!(rc, 0, "capping a socket buffer: {}", io::Error::last_os_error());
}

fn connected_server_actor() -> (ServerActor, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    cap_buffer(&client, libc::SO_SNDBUF);
    client.set_nonblocking(true).unwrap();
    let (far_end, _) = listener.accept().unwrap();
    far_end.set_read_timeout(Some(Duration::from_millis(20))).unwrap();

    let mut actor = parked_actor(address.port());
    actor.stream = Some(client);
    actor.connection_state = ConnectionState::Connected;
    actor.post_login_ready = true;
    (actor, far_end)
}

fn drain_until(actor: &mut ServerActor, far_end: &mut TcpStream, expected: &[u8]) {
    let mut received = Vec::new();
    let mut scratch = vec![0u8; 64 * 1024];
    let deadline = Instant::now() + Duration::from_secs(15);
    while received.len() < expected.len() && Instant::now() < deadline {
        actor.tick();
        match far_end.read(&mut scratch) {
            Ok(0) => break,
            Ok(n) => received.extend_from_slice(&scratch[..n]),
            Err(ref e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(e) => panic!("reading the far end: {e}"),
        }
    }
    assert!(
        received == expected,
        "frames must arrive whole and in order ({} of {} bytes)",
        received.len(),
        expected.len()
    );
}

#[test]
fn a_blocked_server_socket_defers_frames_instead_of_dropping_the_session() {
    let (mut actor, mut far_end) = connected_server_actor();
    let first_query = "a".repeat(256 * 1024);
    let second_query = "b".repeat(256 * 1024);
    let first =
        MessageFactory::build_file_search_message(1, &first_query).get_buffer();
    let second =
        MessageFactory::build_file_search_message(2, &second_query).get_buffer();

    actor.handle_message(ServerMessage::SendMessage(
        MessageFactory::build_file_search_message(1, &first_query),
    ));
    actor.handle_message(ServerMessage::SendMessage(
        MessageFactory::build_file_search_message(2, &second_query),
    ));

    assert!(
        matches!(actor.connection_state, ConnectionState::Connected),
        "WouldBlock on the server socket is backpressure, not a dead session"
    );
    assert!(
        !actor.pending_write.is_empty(),
        "the refused remainder must wait in the pending-write queue"
    );

    let mut expected = Vec::new();
    expected.extend_from_slice(&first);
    expected.extend_from_slice(&second);
    drain_until(&mut actor, &mut far_end, &expected);
    assert!(
        actor.pending_write.is_empty(),
        "a drained queue must be empty, not retain frames for a later send"
    );
}

#[test]
fn a_partial_frame_write_keeps_the_remainder_and_flushes_it_later() {
    let (mut actor, mut far_end) = connected_server_actor();

    let query = "x".repeat(256 * 1024);
    let frame = MessageFactory::build_file_search_message(9, &query).get_buffer();
    actor.handle_message(ServerMessage::SendMessage(
        MessageFactory::build_file_search_message(9, &query),
    ));

    assert!(
        matches!(actor.connection_state, ConnectionState::Connected),
        "a socket that takes only part of a frame has not failed"
    );
    assert!(
        !actor.pending_write.is_empty()
            && actor.pending_write.len() < frame.len(),
        "a partial write must leave the rest queued, not drop or resend it"
    );
    drain_until(&mut actor, &mut far_end, &frame);
    assert!(actor.pending_write.is_empty());
}

#[test]
fn a_server_peer_that_never_drains_hits_a_bounded_queue_and_disconnects() {
    let (mut actor, far_end) = connected_server_actor();
    cap_buffer(&far_end, libc::SO_RCVBUF);

    let query = "x".repeat(256 * 1024);
    actor.handle_message(ServerMessage::SendMessage(
        MessageFactory::build_file_search_message(1, &query),
    ));
    assert!(
        matches!(actor.connection_state, ConnectionState::Connected),
        "one deferred frame is backpressure, not a reason to disconnect"
    );

    for token in 2..40u32 {
        if actor.stream.is_none() {
            break;
        }
        actor.handle_message(ServerMessage::SendMessage(
            MessageFactory::build_file_search_message(token, &query),
        ));
    }

    assert!(
        matches!(actor.connection_state, ConnectionState::Disconnected),
        "a peer that never reads must not grow the queue without bound"
    );
    assert!(
        actor.pending_write.is_empty(),
        "a closed session must not carry queued bytes into the next one"
    );
}
