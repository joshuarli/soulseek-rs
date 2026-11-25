mod browse;
mod downloads;
mod input;
mod render;
mod rooms;
mod search;
mod settings;

use crate::api::SessionApi;
use crate::models::{AppState, TuiExit};
use crate::persist::{
    snapshot::{Snapshot, restore_messages, restore_searches},
    state::{PersistedMessage, ResumeIntent, SAVE_WARN_INTERVAL, StateStore},
};
use crate::error::Result;
use crate::ui::tui::{
    DefaultTerminal,
    event::{self, Event, KeyEventKind, poll},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub struct MainTui {
    client: Arc<dyn SessionApi>,
    state: AppState,
    download_dir: String,
    use_username_dirs: bool,
    search_timeout: Duration,
    spinner_state: usize,
    store: Option<StateStore>,
    /// Where settings changes are persisted — the config file this run
    /// resolved, so `--config` is honoured.
    config_path: Option<std::path::PathBuf>,
    /// Last snapshot written to disk, to skip no-op saves.
    saved_snapshot: Snapshot,
    /// Last warning about a failed write, so a full disk does not fill the
    /// log once per frame.
    last_save_warning: Option<Instant>,
    /// Which divider the left mouse button is holding, if any.
    resize_drag: Option<input::ResizeDrag>,
}

impl MainTui {
    pub fn new(
        client: Arc<dyn SessionApi>,
        download_dir: String,
        use_username_dirs: bool,
        search_timeout: Duration,
        store: Option<StateStore>,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        let mut tui = Self {
            client,
            state: AppState::new(),
            download_dir,
            search_timeout,
            use_username_dirs,
            spinner_state: 0,
            store,
            config_path,
            saved_snapshot: Snapshot::default(),
            last_save_warning: None,
            resize_drag: None,
        };
        tui.restore_persisted_state();
        tui
    }

    /// Bring back last session's state: search history, chat rooms
    /// (rejoined on the server), and downloads — incomplete ones are
    /// re-enqueued so they resume automatically.
    fn restore_persisted_state(&mut self) {
        // Whatever the session has been collecting comes first: attached to a
        // daemon this is the conversation so far, including everything that
        // arrived while nothing was showing it. Locally it is empty and the
        // snapshot below is the whole record.
        let history: Vec<PersistedMessage> = self
            .client
            .message_history()
            .into_iter()
            .map(|message| PersistedMessage {
                peer: message.peer,
                outgoing: message.outgoing,
                text: message.text,
                at: crate::time::Timestamp::from_unix(message.at),
            })
            .collect();
        restore_messages(&mut self.state, &history);

        let Some(store) = &self.store else {
            self.saved_snapshot = Snapshot::capture(&self.state);
            return;
        };

        restore_searches(&mut self.state, &store.load_search_queries());
        restore_messages(&mut self.state, &store.load_messages());
        // Restore the unread badge so messages left unread carry over. Set
        // before the baseline snapshot below so it isn't seen as a change.
        self.state.unread_messages = store.load_unread();

        for room in store.load_rooms() {
            if self.state.rooms.focus_or_open(&room)
                && let Err(e) = self.client.join_room(&room)
            {
                soulseek_rs::warn!("Could not rejoin {room}: {e}");
            }
        }

        let downloads = store.load_downloads();
        self.saved_snapshot = Snapshot::capture(&self.state);
        // Completed and paused entries are shown as-is; only those with the
        // automatic retry intent re-enqueue and reappear through the channel.
        self.saved_snapshot.downloads.clone_from(&downloads);

        let sender = self.downloads_sender();
        for entry in downloads {
            if entry.intent == ResumeIntent::Paused {
                // The user chose to stop this one. Show it as paused but do not
                // queue it: `download.resume` starts it later.
                self.state.downloads.push(crate::models::DownloadEntry {
                    download: soulseek_rs::types::Download {
                        username: entry.username,
                        filename: entry.filename,
                        token: 0,
                        size: entry.size,
                        download_directory: entry.download_directory,
                        status: soulseek_rs::DownloadStatus::Paused {
                            bytes_downloaded: 0,
                            total_bytes: entry.size,
                        },
                        sender: std::sync::mpsc::channel().0,
                        queue_position: None,
                        metadata: soulseek_rs::types::DownloadMetadata::default(
                        ),
                    },
                    receiver: None,
                });
                continue;
            }
            if entry.intent == ResumeIntent::Completed {
                self.state.downloads.push(crate::models::DownloadEntry {
                    download: soulseek_rs::types::Download {
                        username: entry.username,
                        filename: entry.filename,
                        token: 0,
                        size: entry.size,
                        download_directory: entry.download_directory,
                        status: soulseek_rs::DownloadStatus::Completed,
                        sender: std::sync::mpsc::channel().0,
                        queue_position: None,
                        metadata: soulseek_rs::types::DownloadMetadata::default(
                        ),
                    },
                    receiver: None,
                });
            } else {
                let client = self.client.clone();
                let sender = sender.clone();
                std::thread::spawn(move || {
                    match client.download(
                        entry.filename.clone(),
                        entry.username,
                        entry.size,
                        entry.download_directory,
                    ) {
                        Ok((download, rx)) => {
                            let _ = sender.send((download, rx));
                        }
                        Err(e) => soulseek_rs::warn!(
                            "Could not resume {}: {e}",
                            entry.filename
                        ),
                    }
                });
            }
        }
    }

    /// Write state to disk when it differs from what was last saved.
    fn save_persisted_state(&mut self) {
        let Some(store) = &self.store else { return };
        let snapshot = Snapshot::capture(&self.state);
        if snapshot == self.saved_snapshot {
            return;
        }
        // Advance each field only after its own write succeeds. A failed
        // field stays dirty and is retried on the next poll or on exit.
        let mut saved = self.saved_snapshot.clone();
        let mut failures = Vec::new();
        let mut first_error = None;
        if snapshot.downloads != saved.downloads {
            match store.save_downloads(&snapshot.downloads) {
                Ok(()) => saved.downloads.clone_from(&snapshot.downloads),
                Err(e) => {
                    failures.push("downloads");
                    first_error.get_or_insert(e);
                }
            }
        }
        if snapshot.queries != saved.queries {
            match store.save_search_queries(&snapshot.queries) {
                Ok(()) => saved.queries.clone_from(&snapshot.queries),
                Err(e) => {
                    failures.push("search history");
                    first_error.get_or_insert(e);
                }
            }
        }
        if snapshot.rooms != saved.rooms {
            match store.save_rooms(&snapshot.rooms) {
                Ok(()) => saved.rooms.clone_from(&snapshot.rooms),
                Err(e) => {
                    failures.push("room state");
                    first_error.get_or_insert(e);
                }
            }
        }
        if snapshot.messages != saved.messages {
            match store.save_messages(&snapshot.messages) {
                Ok(()) => saved.messages.clone_from(&snapshot.messages),
                Err(e) => {
                    failures.push("message history");
                    first_error.get_or_insert(e);
                }
            }
        }
        if snapshot.unread_messages != saved.unread_messages {
            match store.save_unread(snapshot.unread_messages) {
                Ok(()) => saved.unread_messages = snapshot.unread_messages,
                Err(e) => {
                    failures.push("unread count");
                    first_error.get_or_insert(e);
                }
            }
        }
        self.saved_snapshot = saved;
        if let Some(error) = first_error {
            let now = Instant::now();
            if self
                .last_save_warning
                .is_none_or(|at| now.duration_since(at) >= SAVE_WARN_INTERVAL)
            {
                self.last_save_warning = Some(now);
                soulseek_rs::warn!(
                    "Could not save {}: {error}",
                    failures.join(", ")
                );
            }
        }
    }

    pub fn run(mut self, mut terminal: DefaultTerminal) -> Result<TuiExit> {
        let result = self.run_event_loop(&mut terminal);
        self.save_persisted_state();

        let _ = terminal.restore();
        soulseek_rs::utils::logger::disable_buffering();

        result.map(|()| self.state.exit.unwrap_or(TuiExit::Quit))
    }

    /// What the window holds, for a test to read back after driving it.
    #[must_use]
    pub const fn state(&self) -> &AppState {
        &self.state
    }

    /// One frame's worth of catching up with the session: everything the
    /// event loop does between drawing and reading the keyboard.
    pub fn poll_session(&mut self) {
        self.update_search_results();
        self.update_downloads();
        self.poll_private_messages();
        self.poll_browse_result();
        self.poll_room_events();
        self.state.uploads = self.client.uploads();
        self.spinner_state = (self.spinner_state + 1) % 10;
        self.save_persisted_state();
    }

    fn run_event_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while self.state.exit.is_none() {
            terminal.draw(|frame| self.render(frame))?;

            self.poll_session();

            // Drain every queued input event before the next draw: key
            // autorepeat outpaces the frame time, and handling one event per
            // frame makes the backlog keep scrolling for seconds after the
            // key is released.
            if poll(Duration::from_millis(100))? {
                loop {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            self.handle_key_event(key);
                        }
                        Event::Mouse(mouse) => {
                            self.handle_mouse_event(mouse);
                        }
                        Event::Key(_) => {}
                    }
                    if self.state.exit.is_some() || !poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

pub fn launch_main_tui(
    terminal: DefaultTerminal,
    client: Arc<dyn SessionApi>,
    download_dir: String,
    use_username_dirs: bool,
    search_timeout: Duration,
    store: Option<StateStore>,
    config_path: Option<std::path::PathBuf>,
) -> Result<TuiExit> {
    let tui = MainTui::new(
        client,
        download_dir,
        use_username_dirs,
        search_timeout,
        store,
        config_path,
    );
    tui.run(terminal)
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::ui::fixture::TalkativeSession;

    fn window(store: StateStore) -> MainTui {
        MainTui::new(
            Arc::new(TalkativeSession::default()),
            "/tmp".to_string(),
            false,
            Duration::from_secs(1),
            Some(store),
            None,
        )
    }

    #[test]
    fn a_failed_save_keeps_the_snapshot_dirty_for_retry() {
        let dir = tempfile::tempdir().expect("a temporary state directory");
        // A file where the state directory should be: reads fall back to
        // empty, every write fails.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "").expect("a blocked state directory");
        let mut tui = window(StateStore::new(blocked));
        restore_searches(&mut tui.state, &["beatles".to_string()]);

        tui.save_persisted_state();
        assert!(
            tui.saved_snapshot.queries.is_empty(),
            "a failed write must not mark the snapshot saved"
        );

        tui.store = Some(StateStore::new(dir.path().join("state")));
        tui.save_persisted_state();
        assert_eq!(tui.saved_snapshot.queries, vec!["beatles".to_string()]);
        assert_eq!(
            StateStore::new(dir.path().join("state")).load_search_queries(),
            vec!["beatles".to_string()]
        );
    }

    #[test]
    fn one_failing_write_does_not_hold_back_the_other_fields() {
        let dir = tempfile::tempdir().expect("a temporary state directory");
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).expect("the state directory");
        // Only downloads.json cannot be replaced; searches.json can.
        std::fs::create_dir(state.join("downloads.json"))
            .expect("an unwritable downloads target");
        let mut tui = window(StateStore::new(state.clone()));
        restore_searches(&mut tui.state, &["beatles".to_string()]);

        tui.save_persisted_state();
        assert_eq!(
            tui.saved_snapshot.queries,
            vec!["beatles".to_string()],
            "the search history was written and must not be retried"
        );
        assert!(
            tui.saved_snapshot.downloads.is_empty(),
            "the failed downloads write stays dirty"
        );
        assert_eq!(
            StateStore::new(state).load_search_queries(),
            vec!["beatles".to_string()]
        );
    }
}

#[cfg(test)]
mod tests;
