//! The window driven the way a terminal drives it: keys and mouse events in,
//! a rendered screen out, over a stand-in session.
//!
//! This file is the harness the topic modules below share — the session
//! double, the window constructors, and the press/screen helpers.

mod browse;
mod layout;
mod navigation;
mod rooms;
mod searches;
mod session;
mod settings;
mod transfers;

use super::*;

use crate::daemon::proto::ChatMessageDto;

use crate::models::{FocusedPane, MessageDirection, RoomsView, SearchStatus};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::fixture::{
    TalkativeSession, at_status, completed, queued, uploading,
};

fn key(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
}

fn tui(history: Vec<ChatMessageDto>) -> MainTui {
    with_session(TalkativeSession {
        history,
        shared: true,
        ..TalkativeSession::default()
    })
}

fn shared(searches: Vec<crate::api::SessionSearch>) -> TalkativeSession {
    TalkativeSession {
        searches: std::sync::Mutex::new(searches),
        shared: true,
        ..TalkativeSession::default()
    }
}

fn search_of(query: &str, files: usize, age: u64) -> crate::api::SessionSearch {
    crate::api::SessionSearch {
        query: query.to_string(),
        files,
        started_secs_ago: Some(age),
    }
}

fn with_session(session: TalkativeSession) -> MainTui {
    attach(Arc::new(session))
}

fn attach(session: Arc<TalkativeSession>) -> MainTui {
    MainTui::new(
        session,
        "/tmp".to_string(),
        false,
        Duration::from_secs(1),
        None,
        None,
    )
}

fn files_from_bob(names: &[&str]) -> soulseek_rs::SearchResult {
    soulseek_rs::SearchResult {
        token: 0,
        files: names
            .iter()
            .map(|name| soulseek_rs::types::File {
                username: "bob".to_string(),
                name: (*name).to_string(),
                size: 42,
                attribs: std::collections::HashMap::new(),
            })
            .collect(),
        slots: 1,
        speed: 0,
        username: "bob".to_string(),
    }
}

/// An attached window that selected a two-file search and has already
/// ticked past its one necessary fetch.
fn window_that_pulled_two_files() -> (Arc<TalkativeSession>, MainTui) {
    let session = Arc::new(TalkativeSession {
        results: std::sync::Mutex::new(vec![files_from_bob(&[
            "@@x\\a.mp3",
            "@@x\\b.mp3",
        ])]),
        ..shared(vec![search_of("aphex twin", 2, 9_999)])
    });
    let mut tui = attach(session.clone());
    tui.state.selected_search_index = Some(0);
    tui.update_search_results();
    tui.update_search_results();
    (session, tui)
}

fn results(n: usize) -> Vec<crate::models::FileDisplayData> {
    (0..n)
        .map(|i| crate::models::FileDisplayData {
            filename: format!("@@x\\Music\\{i:02}.mp3"),
            username: "bob".to_string(),
            ..Default::default()
        })
        .collect()
}

fn results_tui(n: usize) -> MainTui {
    let mut tui = with_session(TalkativeSession::default());
    tui.state.results_items = results(n);
    tui.state.focused_pane = FocusedPane::Results;
    // 88 cells wide gives the name column 11 cells and the folder 7.
    tui.state.results_pane_area =
        Some(ratatui::layout::Rect::new(0, 0, 88, 13));
    tui
}

fn press(tui: &mut MainTui, code: KeyCode) {
    tui.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(tui: &mut MainTui, ch: char) {
    tui.handle_key_event(KeyEvent::new(
        KeyCode::Char(ch),
        KeyModifiers::CONTROL,
    ));
}

fn selected(tui: &MainTui) -> Option<usize> {
    tui.state.results_table_state.selected()
}

fn screen_sized(tui: &mut MainTui, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("backend");
    terminal.draw(|frame| tui.render(frame)).expect("draw");
    terminal.backend().to_string()
}

fn screen_of(tui: &mut MainTui) -> String {
    screen_sized(tui, 160, 40)
}

fn mouse(
    tui: &mut MainTui,
    kind: ratatui::crossterm::event::MouseEventKind,
    column: u16,
    row: u16,
) {
    tui.handle_mouse_event(ratatui::crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

fn mouse_drag_row(tui: &mut MainTui, from: (u16, u16), to: (u16, u16)) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    mouse(tui, MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    mouse(tui, MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
    mouse(tui, MouseEventKind::Up(MouseButton::Left), to.0, to.1);
}

/// The screen row carrying a pane's legend, `[2] Results` say, without
/// the quotes the test backend wraps each row in.
fn row_with<'a>(screen: &'a str, needle: &str) -> &'a str {
    let line = screen
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no row with {needle:?} in\n{screen}"));
    let start = line.find('"').map_or(0, |i| i + 1);
    let end = line.rfind('"').unwrap_or(line.len());
    &line[start..end]
}

/// A window with a search, its results, and a transfer, so every pane
/// has something to show.
fn furnished_tui() -> MainTui {
    let mut tui = results_tui(3);
    tui.state.searches.push(crate::models::SearchEntry {
        query: "aphex twin".to_string(),
        status: SearchStatus::Completed,
        results: results(3),
        known_files: 3,
        owned: true,
        start_time: std::time::Instant::now(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    tui.state.selected_search_index = Some(0);
    tui.state.downloads.push(crate::models::DownloadEntry {
        download: queued("bob", "@@x\\Music\\00.mp3"),
        receiver: None,
    });
    tui
}

fn browsed(tui: &mut MainTui, files: usize) {
    tui.state.browse.open("bob");
    let listing = vec![soulseek_rs::SharedDirectory {
        name: "Music".to_string(),
        files: (0..files)
            .map(|i| soulseek_rs::SharedFileEntry {
                name: format!("{i:02}.mp3"),
                size: 1,
                attributes: Vec::new(),
            })
            .collect(),
    }];
    tui.state
        .browse
        .active_tab_mut()
        .expect("tab")
        .load(&listing);
    tui.state.show_browse = true;
    let _ = screen_of(tui);
}

fn in_a_room(tui: &mut MainTui, messages: usize) {
    tui.state.rooms.apply_event(
        soulseek_rs::RoomEvent::Joined {
            room: "jazz".to_string(),
            users: (0..30).map(|i| format!("user{i:02}")).collect(),
        },
        None,
    );
    tui.state.rooms.focus_or_open("jazz");
    for i in 0..messages {
        tui.state.rooms.apply_event(
            soulseek_rs::RoomEvent::Message {
                room: "jazz".to_string(),
                username: "alice".to_string(),
                message: format!("line {i:03}"),
            },
            Some("jazz"),
        );
    }
    tui.state.show_rooms = true;
    let _ = screen_of(tui);
}

/// The queries the window has sent so far, once at least `count` of them
/// are out: a search goes to the session from a worker thread, so a test
/// has to give it a moment.
fn searched(session: &TalkativeSession, count: usize) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let seen = session.searched.lock().expect("not poisoned").clone();
        if seen.len() >= count || std::time::Instant::now() > deadline {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A window over last session's search list: the queries came back from
/// disk, with nothing under them.
fn restored(session: Arc<TalkativeSession>, queries: &[&str]) -> MainTui {
    let mut tui = attach(session);
    let queries: Vec<String> =
        queries.iter().map(ToString::to_string).collect();
    restore_searches(&mut tui.state, &queries);
    tui.state.focused_pane = FocusedPane::Searches;
    tui
}
