//! A deterministic session for the PTY fixture binary.
//!
//! It stands in for a daemon that has been running long enough to hold a
//! conversation, a transfer queue, a search cache and a share index. The
//! fixture binary renders the real window over this session, so terminal
//! behaviour can be captured without a network or a server.

use crate::api::{SessionApi, SessionSearch};
use crate::daemon::proto::ChatMessageDto;
use soulseek_rs::types::{
    Download, DownloadMetadata, RoomInfo, RoomUserStats, UploadInfo,
    UploadStatus,
};
use soulseek_rs::{
    DownloadStatus, RoomEvent, SearchResult, SharedDirectory,
    SharedFileEntry, UserInfo, types::File,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub struct FixtureSession {    pub history: Vec<ChatMessageDto>,
    pub downloads: Vec<Download>,
    pub uploads: Vec<UploadInfo>,
    pub searches: Mutex<Vec<SessionSearch>>,
    pub results: Mutex<Vec<SearchResult>>,
    pub searched: Mutex<Vec<String>>,
    pub all_searches_calls: std::sync::atomic::AtomicUsize,
    pub result_fetches: std::sync::atomic::AtomicUsize,
    pub shared: bool,
    pub download_dir_set: Mutex<Option<String>>,
    pub shares_set: Mutex<Option<Vec<String>>>,
    pub password_set: Mutex<Option<String>>,
    pub listing: Mutex<Vec<SharedDirectory>>,
    pub download_cancelled: Mutex<Vec<(String, String)>>,
    pub upload_cancelled: Mutex<Vec<(String, String)>>,
    pub download_removed: Mutex<Vec<(String, String)>>,
    pub room_events: Mutex<Vec<RoomEvent>>,
    pub browsed: Mutex<HashMap<String, Vec<SharedDirectory>>>,
    pub shared_dirs: Vec<String>,
}

#[cfg(test)]
pub use FixtureSession as TalkativeSession;

pub fn at_status(
    username: &str,
    filename: &str,
    status: DownloadStatus,
) -> Download {
    let (sender, _) = std::sync::mpsc::channel();
    let size = match &status {
        DownloadStatus::InProgress { total_bytes, .. }
        | DownloadStatus::Paused { total_bytes, .. } => *total_bytes,
        _ => 8_388_608,
    };
    Download {
        username: username.to_string(),
        filename: filename.to_string(),
        token: 1,
        size,
        download_directory: "/home/tester/Music".to_string(),
        status,
        sender,
        queue_position: None,
        metadata: DownloadMetadata::default(),
    }
}

pub fn queued(username: &str, filename: &str) -> Download {
    at_status(username, filename, DownloadStatus::Queued)
}

pub fn completed(username: &str, filename: &str) -> Download {
    at_status(username, filename, DownloadStatus::Completed)
}

pub fn uploading(username: &str, filename: &str) -> UploadInfo {
    UploadInfo {
        username: username.to_string(),
        filename: filename.to_string(),
        size: 8_000_000,
        bytes_sent: 3_000_000,
        speed_bytes_per_sec: 700_000.0,
        status: UploadStatus::InProgress,
    }
}

fn file(name: &str, size: u64, bitrate: u32, length: u32) -> File {
    let mut attribs = HashMap::new();
    attribs.insert(0, bitrate);
    attribs.insert(1, length);
    File {
        username: "peer".to_string(),
        name: name.to_string(),
        size,
        attribs,
    }
}

fn search_result(user: &str, slots: u8, speed: u32, files: Vec<File>) -> SearchResult {
    SearchResult {
        token: 0,
        files,
        slots,
        speed,
        username: user.to_string(),
    }
}

fn share(name: &str, files: &[(&str, u64)]) -> SharedDirectory {
    SharedDirectory {
        name: name.to_string(),
        files: files
            .iter()
            .map(|(name, size)| SharedFileEntry {
                name: (*name).to_string(),
                size: *size,
                attributes: vec![(0, 320), (1, 245)],
            })
            .collect(),
    }
}

impl FixtureSession {
    /// The state the snapshots are taken from: a session that has already
    /// searched, downloaded, chatted and joined a room.
    #[must_use]
    pub fn sample() -> Self {
        let listing = vec![
            share(
                "Music",
                &[
                    ("Aphex Twin - Xtal.flac", 42_000_000),
                    ("Aphex Twin - Tha.flac", 31_000_000),
                    ("Boards of Canada - Roygbiv.mp3", 8_000_000),
                ],
            ),
            share(
                "Music/Compilations",
                &[("Various Artists - Artificial Intelligence.mp3", 9_500_000)],
            ),
        ];
        let results = vec![
            search_result(
                "idm_peer",
                2,
                4_200_000,
                vec![
                    file(
                        "Music\\Aphex Twin\\Selected Ambient Works 85-92\\01 - Xtal.flac",
                        42_000_000,
                        1411,
                        294,
                    ),
                    file(
                        "Music\\Aphex Twin\\Selected Ambient Works 85-92\\02 - Tha.flac",
                        31_000_000,
                        1411,
                        211,
                    ),
                    file(
                        "Music\\Aphex Twin\\Selected Ambient Works 85-92\\03 - Pulsewidth.flac",
                        35_000_000,
                        1411,
                        262,
                    ),
                ],
            ),
            search_result(
                "braindance",
                1,
                1_800_000,
                vec![
                    file(
                        "lossless\\Aphex Twin\\Xtal (Remastered).wav",
                        68_000_000,
                        1411,
                        294,
                    ),
                    file(
                        "lossless\\Aphex Twin\\Tha (Remastered).wav",
                        52_000_000,
                        1411,
                        211,
                    ),
                    file(
                        "lossless\\Aphex Twin\\Pulsewidth (Remastered).wav",
                        58_000_000,
                        1411,
                        262,
                    ),
                    file(
                        "lossless\\Aphex Twin\\Ageispolis (Remastered).wav",
                        61_000_000,
                        1411,
                        321,
                    ),
                ],
            ),
            search_result(
                "crate_digger",
                0,
                900_000,
                vec![
                    file(
                        "unsorted\\afx\\xtal_320.mp3",
                        14_000_000,
                        320,
                        294,
                    ),
                    file(
                        "unsorted\\afx\\tha_320.mp3",
                        10_000_000,
                        320,
                        211,
                    ),
                    file(
                        "unsorted\\long folder name with spaces and a very long tail\\aphex twin - xtal - 1992 - remastered - 320kbps - vinyl rip.mp3",
                        18_000_000,
                        320,
                        294,
                    ),
                ],
            ),
        ];
        let downloads = vec![
            at_status(
                "idm_peer",
                "Music\\Aphex Twin\\Selected Ambient Works 85-92\\01 - Xtal.flac",
                DownloadStatus::InProgress {
                    bytes_downloaded: 27_000_000,
                    total_bytes: 42_000_000,
                    speed_bytes_per_sec: 3_400_000.0,
                },
            ),
            at_status(
                "braindance",
                "lossless\\Aphex Twin\\Tha (Remastered).wav",
                DownloadStatus::Paused {
                    bytes_downloaded: 12_000_000,
                    total_bytes: 52_000_000,
                },
            ),
            queued("crate_digger", "unsorted\\afx\\xtal_320.mp3"),
            completed(
                "idm_peer",
                "Music\\Aphex Twin\\Selected Ambient Works 85-92\\02 - Tha.flac",
            ),
            at_status(
                "idm_peer",
                "Music\\Aphex Twin\\Selected Ambient Works 85-92\\03 - Pulsewidth.flac",
                DownloadStatus::Failed(Some("peer went offline".to_string())),
            ),
        ];
        let uploads = vec![
            uploading(
                "listener_one",
                "Music\\Boards of Canada - Roygbiv.mp3",
            ),
            UploadInfo {
                username: "listener_two".to_string(),
                filename: "Music\\Aphex Twin - Xtal.flac".to_string(),
                size: 42_000_000,
                bytes_sent: 0,
                status: UploadStatus::Queued(2),
                speed_bytes_per_sec: 0.0,
            },
            UploadInfo {
                username: "listener_three".to_string(),
                filename: "Music\\Aphex Twin - Tha.flac".to_string(),
                size: 31_000_000,
                bytes_sent: 31_000_000,
                status: UploadStatus::Completed,
                speed_bytes_per_sec: 0.0,
            },
        ];
        let history = vec![
            ChatMessageDto {
                peer: "idm_peer".to_string(),
                outgoing: false,
                text: "that Xtal rip sounds clean".to_string(),
                at: 1_700_000_000,
            },
            ChatMessageDto {
                peer: "idm_peer".to_string(),
                outgoing: true,
                text: "thanks, grabbed it from your share".to_string(),
                at: 1_700_000_060,
            },
            ChatMessageDto {
                peer: "braindance".to_string(),
                outgoing: false,
                text: "do you have the remastered Tha too?".to_string(),
                at: 1_700_000_120,
            },
        ];
        Self {
            history,
            downloads,
            uploads,
            searches: Mutex::new(vec![SessionSearch {
                query: "aphex twin".to_string(),
                files: results.iter().map(|r| r.files.len()).sum(),
                started_secs_ago: Some(9_999),
            }]),
            results: Mutex::new(results),
            shared: true,
            listing: Mutex::new(listing),
            shared_dirs: vec!["/home/tester/Music".to_string()],
            browsed: Mutex::new(HashMap::from([(
                "idm_peer".to_string(),
                vec![
                    share(
                        "Music\\Aphex Twin\\Selected Ambient Works 85-92",
                        &[
                            ("01 - Xtal.flac", 42_000_000),
                            ("02 - Tha.flac", 31_000_000),
                            ("03 - Pulsewidth.flac", 35_000_000),
                        ],
                    ),
                    share(
                        "Music\\Aphex Twin\\Richard D. James Album",
                        &[
                            ("01 - 4.flac", 22_000_000),
                            ("02 - Cornish Acid.flac", 18_000_000),
                        ],
                    ),
                ],
            )])),
            ..Self::default()
        }
    }

    fn room_list() -> Vec<RoomInfo> {
        vec![
            RoomInfo {
                name: "The Ambient Room".to_string(),
                user_count: 214,
            },
            RoomInfo {
                name: "IDM".to_string(),
                user_count: 87,
            },
            RoomInfo {
                name: "Vinyl Rips".to_string(),
                user_count: 42,
            },
        ]
    }
}

impl SessionApi for FixtureSession {
    fn message_history(&self) -> Vec<ChatMessageDto> {
        self.history.clone()
    }
    fn daemon_endpoint(&self) -> Option<String> {
        self.shared.then(|| "127.0.0.1:5030".to_string())
    }
    fn username(&self) -> String {
        "tester".to_string()
    }
    fn listen_port(&self) -> Option<u16> {
        Some(50300)
    }
    fn session_loss(&self) -> Option<soulseek_rs::SessionLoss> {
        None
    }
    fn search_with_cancel(
        &self,
        query: &str,
        _timeout: Duration,
        _cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> soulseek_rs::Result<Vec<SearchResult>> {
        self.searched
            .lock()
            .expect("not poisoned")
            .push(query.to_string());
        Ok(Vec::new())
    }
    fn get_search_results(&self, _key: &str) -> Vec<SearchResult> {
        Vec::new()
    }
    fn all_searches(&self) -> Vec<SessionSearch> {
        self.all_searches_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.searches.lock().expect("not poisoned").clone()
    }
    fn forget_search(&self, query: &str) -> bool {
        let mut searches = self.searches.lock().expect("not poisoned");
        let before = searches.len();
        searches.retain(|search| search.query != query);
        searches.len() != before
    }
    fn get_search_results_count(&self, _key: &str) -> usize {
        0
    }
    fn try_get_search_results(&self, _key: &str) -> Option<Vec<SearchResult>> {
        self.result_fetches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(self.results.lock().expect("not poisoned").clone())
    }
    fn start_wishlist_search(&self, _query: &str) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn wishlist_interval(&self) -> Duration {
        Duration::from_mins(12)
    }
    fn download(
        &self,
        _filename: String,
        _username: String,
        _size: u64,
        _directory: String,
    ) -> soulseek_rs::Result<(Download, std::sync::mpsc::Receiver<DownloadStatus>)>
    {
        Err(soulseek_rs::SoulseekRs::NotConnected)
    }
    fn download_with_metadata(
        &self,
        _filename: String,
        _username: String,
        _size: u64,
        _directory: String,
        _metadata: DownloadMetadata,
    ) -> soulseek_rs::Result<(Download, std::sync::mpsc::Receiver<DownloadStatus>)>
    {
        Err(soulseek_rs::SoulseekRs::NotConnected)
    }
    fn get_all_downloads(&self) -> Vec<Download> {
        self.downloads.clone()
    }
    fn pause_download(&self, _u: &str, _f: &str) -> bool {
        false
    }
    fn resume_download(&self, _u: &str, _f: &str) -> bool {
        false
    }
    fn remove_queued_download(&self, _u: &str, _f: &str) -> bool {
        false
    }
    fn remove_download(&self, u: &str, f: &str) -> bool {
        self.download_removed
            .lock()
            .expect("not poisoned")
            .push((u.to_string(), f.to_string()));
        true
    }
    fn uploads(&self) -> Vec<UploadInfo> {
        self.uploads.clone()
    }
    fn take_upload_events(&self) -> Vec<UploadInfo> {
        Vec::new()
    }
    fn cancel_upload(&self, u: &str, f: &str) -> bool {
        self.upload_cancelled
            .lock()
            .expect("not poisoned")
            .push((u.to_string(), f.to_string()));
        true
    }
    fn cancel_download(&self, u: &str, f: &str) -> bool {
        self.download_cancelled
            .lock()
            .expect("not poisoned")
            .push((u.to_string(), f.to_string()));
        true
    }
    fn set_upload_slots(&self, _slots: usize) {}
    fn check_privileges(&self) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn own_privilege_seconds(&self) -> Option<u32> {
        Some(1_000_000)
    }
    fn request_room_list(&self) -> soulseek_rs::Result<()> {
        self.room_events
            .lock()
            .expect("not poisoned")
            .push(RoomEvent::List(Self::room_list()));
        Ok(())
    }
    fn join_room(&self, room: &str) -> soulseek_rs::Result<()> {
        let users = vec![
            "idm_peer".to_string(),
            "braindance".to_string(),
            "crate_digger".to_string(),
        ];
        let mut events = self.room_events.lock().expect("not poisoned");
        events.push(RoomEvent::Joined {
            room: room.to_string(),
            users,
        });
        events.push(RoomEvent::Message {
            room: room.to_string(),
            username: "idm_peer".to_string(),
            message: "anyone else on the new Autechre?".to_string(),
        });
        events.push(RoomEvent::Message {
            room: room.to_string(),
            username: "braindance".to_string(),
            message: "been on repeat all week".to_string(),
        });
        Ok(())
    }
    fn leave_room(&self, _room: &str) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn say_in_room(&self, _room: &str, _message: &str) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn room_members(&self, _room: &str) -> Vec<String> {
        vec![
            "idm_peer".to_string(),
            "braindance".to_string(),
            "crate_digger".to_string(),
        ]
    }
    fn room_member_stats(&self, _room: &str) -> Vec<RoomUserStats> {
        Vec::new()
    }
    fn take_room_events(&self) -> Vec<RoomEvent> {
        std::mem::take(&mut *self.room_events.lock().expect("not poisoned"))
    }
    fn send_private_message(&self, _u: &str, _m: &str) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn take_private_messages(&self) -> Vec<soulseek_rs::UserMessage> {
        Vec::new()
    }
    fn browse_user(&self, username: &str) -> soulseek_rs::Result<()> {
        if !self
            .browsed
            .lock()
            .expect("not poisoned")
            .contains_key(username)
        {
            self.browsed
                .lock()
                .expect("not poisoned")
                .insert(username.to_string(), Vec::new());
        }
        Ok(())
    }
    fn take_browse_result(&self, username: &str) -> Option<Vec<SharedDirectory>> {
        self.browsed
            .lock()
            .expect("not poisoned")
            .get(username)
            .cloned()
    }
    fn request_user_info(&self, _username: &str) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn watch_user(&self, _username: &str) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn unwatch_user(&self, _username: &str) -> soulseek_rs::Result<()> {
        Ok(())
    }
    fn watched_users(&self) -> Vec<String> {
        vec!["idm_peer".to_string(), "braindance".to_string()]
    }
    fn user_info(&self, _username: &str) -> Option<UserInfo> {
        None
    }
    fn shared_counts(&self) -> (u32, u32) {
        (2, 4)
    }
    fn change_password(&self, password: &str) -> soulseek_rs::Result<()> {
        *self.password_set.lock().expect("not poisoned") =
            Some(password.to_string());
        Ok(())
    }
    fn shared_listing(&self) -> Vec<SharedDirectory> {
        self.listing.lock().expect("not poisoned").clone()
    }
    fn shared_directories(&self) -> Vec<String> {
        self.shared_dirs.clone()
    }
    fn set_shared_directories(
        &self,
        directories: Vec<String>,
    ) -> soulseek_rs::Result<()> {
        *self.shares_set.lock().expect("not poisoned") = Some(directories);
        Ok(())
    }
    fn set_download_directory(
        &self,
        directory: String,
    ) -> soulseek_rs::Result<()> {
        *self.download_dir_set.lock().expect("not poisoned") = Some(directory);
        Ok(())
    }
}
