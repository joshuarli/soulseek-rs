//! An in-memory index of the files we share with other peers.
//!
//! Scanning a directory produces a read-only [`Shares`] snapshot keyed by the
//! peer-facing *virtual path* (the shared directory's own name followed by the
//! backslash-separated relative path, matching the Soulseek wire convention).
//!
//! A snapshot stores the canonical root path and the device/inode of both the
//! root directory and each file, then reopens the root path for every upload.
//! It does not retain an open root descriptor: see [`Shares::scan`] and
//! [`open_contained`] for the exact containment policy and its limits.

mod audio;

use crate::message::peer::{SharedDirectory, SharedFileEntry};
use crate::message::Message;
use crate::types::MAX_SEARCH_REPLY_FILES;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::io;
use std::ffi::CString;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Component, Path, PathBuf};

/// One shared file: its peer-facing virtual path and where it lives on disk.
///
/// Built by the scan, never by a host, so fields can be added without a
/// breaking release.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SharedFile {
    /// Backslash-separated path exposed to peers, e.g. `music\album\song.mp3`.
    pub virtual_path: String,
    /// `virtual_path` lowercased once at scan time, so answering a search does
    /// not lowercase every shared path again.
    search_key: String,
    /// The real filesystem path, for display and logging only. Bytes are
    /// always reopened through [`open_contained`], never through this string.
    pub real_path: PathBuf,
    /// Canonical share root this file was indexed under. Uploads reopen this
    /// path; no root descriptor is kept, so moving or renaming the root (or an
    /// ancestor) breaks serving until the next scan.
    pub root: PathBuf,
    /// Path relative to `root`, used to walk from a root directory fd at open
    /// time so neither `..` nor a late symlink can escape the share. Renaming
    /// a directory inside the root changes this path and needs a rescan.
    pub relative: PathBuf,
    /// Device/inode of the root directory at scan time. The root reopened for
    /// an upload must still be that directory; a different directory installed
    /// at the recorded path is refused.
    pub root_dev: u64,
    pub root_ino: u64,
    /// Device/inode of the indexed file at scan time, checked against the
    /// descriptor actually read. This proves identity, not path containment
    /// (that comes from the `O_NOFOLLOW` walk) and not that the file's bytes
    /// are unchanged: writing in place keeps the same inode.
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    /// `(code, value)` audio attributes read from the file's headers: bitrate,
    /// duration, VBR, sample rate, bit depth, as far as the format gives them.
    pub attributes: Vec<(u32, u32)>,
}

/// A read-only snapshot of the shared files, cheap to clone behind an `Arc`.
///
/// The directory listing, its compressed browse reply and the folder index are
/// built once when the snapshot is scanned, not when a peer asks for them. The
/// operations thread therefore only clones bytes when serving a browse.
#[derive(Debug)]
pub struct Shares {
    files: Vec<Arc<SharedFile>>,
    by_virtual: HashMap<String, usize>,
    folder_count: usize,
    directories: Arc<Vec<SharedDirectory>>,
    by_folder: HashMap<String, Vec<SharedDirectory>>,
    browse_response: Arc<Message>,
    search_cache: Mutex<HashMap<String, Vec<usize>>>,
}

impl Default for Shares {
    fn default() -> Self {
        Self::from_files(Vec::new(), 0)
    }
}

/// Queries kept in the search index. A full cache is cleared rather than
/// allowed to grow with every distinct string a peer sends us.
const MAX_CACHED_QUERIES: usize = 64;

#[cfg(test)]
pub(crate) static BROWSE_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl Shares {
    /// An empty share set (nothing shared).
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Scan `root` recursively into a share index. Symlinks and unreadable
    /// entries are skipped so a single bad entry never aborts the scan.
    ///
    /// The snapshot records a canonical root path and root directory identity
    /// and is immutable. The serving policy is:
    ///
    /// - **Root replacement:** a different directory at the recorded path is
    ///   refused at open time by root device/inode, even if it holds a file
    ///   with the same relative path.
    /// - **Ancestor rename:** the root is reopened by its recorded path, not
    ///   followed from a retained descriptor. Renaming the root or one of its
    ///   ancestors changes that path, so uploads fail until a fresh scan.
    /// - **Symlink:** the root is resolved once here. A symlink introduced
    ///   later in an ancestor can redirect the reopened path, but the root
    ///   identity check refuses it unless the symlink lands on the same
    ///   directory; symlinks at or below the root are never followed.
    /// - **File replacement:** the leaf device/inode is checked against the
    ///   opened descriptor, so a replacement is refused until a rescan.
    ///   In-place writes to the indexed inode are not detected.
    /// - **Rescan:** only a new snapshot updates roots, relative paths and
    ///   identities; it is the way to accept any of the changes above.
    ///
    /// # Errors
    /// Returns an error only if `root` itself cannot be read.
    pub fn scan(root: &Path) -> io::Result<Self> {
        // Fail fast if the root is unreadable; deeper failures are skipped.
        let _ = std::fs::read_dir(root)?;

        Ok(Self::scan_many(&[root.to_path_buf()]))
    }

    /// Scan several roots into one merged index. Unreadable roots are
    /// skipped (with a log line) instead of failing the whole scan, and
    /// roots that share a directory name get a ` (2)`, ` (3)`… suffix so
    /// their virtual paths cannot collide. Each root captures and serves
    /// under the same identity policy documented on [`Self::scan`].
    #[must_use]
    pub fn scan_many(roots: &[PathBuf]) -> Self {
        let mut files = Vec::new();
        let mut folder_count = 0;
        let mut name_uses: HashMap<String, u32> = HashMap::new();

        for root in roots {
            if std::fs::read_dir(root).is_err() {
                crate::warn!(
                    "Skipping unreadable shared directory {}",
                    root.display()
                );
                continue;
            }
            let base = root_display_name(root);
            let uses = name_uses.entry(base.clone()).or_insert(0);
            *uses += 1;
            let name = if *uses == 1 {
                base
            } else {
                format!("{base} ({uses})")
            };
            // Keep the configured spelling for virtual names, but record a
            // canonical root so the upload path cannot be redirected through
            // a symlink in the root's ancestors.
            let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            let Ok(root_meta) = std::fs::metadata(&canonical) else {
                crate::warn!(
                    "Skipping unreadable shared directory {}",
                    root.display()
                );
                continue;
            };
            if !root_meta.is_dir() {
                crate::warn!(
                    "Skipping shared directory that is not a directory {}",
                    root.display()
                );
                continue;
            }
            scan_root(
                root,
                &canonical,
                (root_meta.dev(), root_meta.ino()),
                &name,
                &mut files,
                &mut folder_count,
            );
        }

        Self::from_files(files, folder_count)
    }

    fn from_files(files: Vec<SharedFile>, folder_count: usize) -> Self {
        let files = files.into_iter().map(Arc::new).collect::<Vec<_>>();
        let by_virtual = files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.virtual_path.clone(), i))
            .collect();

        // Group once, here, instead of on every browse or folder request that
        // reaches the singleton operations loop.
        let mut by_dir: std::collections::BTreeMap<
            String,
            Vec<SharedFileEntry>,
        > = std::collections::BTreeMap::new();
        for file in &files {
            let (dir, base) = file
                .virtual_path
                .rsplit_once('\\')
                .unwrap_or(("", file.virtual_path.as_str()));
            let entry = SharedFileEntry {
                name: base.to_string(),
                size: file.size,
                attributes: file.attributes.clone(),
            };
            if let Some(entries) = by_dir.get_mut(dir) {
                entries.push(entry);
            } else {
                by_dir.insert(dir.to_string(), vec![entry]);
            }
        }
        let directories: Vec<SharedDirectory> = by_dir
            .into_iter()
            .map(|(name, files)| SharedDirectory { name, files })
            .collect();
        let mut by_folder: HashMap<String, Vec<SharedDirectory>> =
            HashMap::new();
        for dir in &directories {
            by_folder
                .entry(dir.name.clone())
                .or_default()
                .push(dir.clone());
        }

        #[cfg(test)]
        BROWSE_BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let browse_response = Arc::new(
            crate::message::peer::build_shared_file_list(&directories),
        );
        Self {
            files,
            by_virtual,
            folder_count,
            directories: Arc::new(directories),
            by_folder,
            browse_response,
            search_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Files matching every term of `query`, case-insensitively.
    ///
    /// An empty query matches nothing. A single search returns at most
    /// [`MAX_SEARCH_REPLY_FILES`] files: it is one reply to one peer, not the
    /// whole result of the query.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<Arc<SharedFile>> {
        let terms: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let key = terms.join("\u{0}");
        if let Ok(cache) = self.search_cache.lock()
            && let Some(indices) = cache.get(&key)
        {
            return indices
                .iter()
                .filter_map(|&i| self.files.get(i).cloned())
                .collect();
        }

        let mut indices = Vec::new();
        for (i, file) in self.files.iter().enumerate() {
            if terms.iter().all(|t| file.search_key.contains(t.as_str())) {
                indices.push(i);
                if indices.len() == MAX_SEARCH_REPLY_FILES {
                    break;
                }
            }
        }
        if let Ok(mut cache) = self.search_cache.lock() {
            if cache.len() == MAX_CACHED_QUERIES {
                cache.clear();
            }
            cache.insert(key, indices.clone());
        }
        indices
            .into_iter()
            .filter_map(|i| self.files.get(i).cloned())
            .collect()
    }

    /// Drop cached query matches. A new [`Shares`] snapshot already starts with
    /// an empty cache; this is the explicit invalidation for a caller that
    /// changes what a query should mean without replacing the snapshot.
    pub fn invalidate_search_cache(&self) {
        if let Ok(mut cache) = self.search_cache.lock() {
            cache.clear();
        }
    }

    /// Look up a shared file by its exact virtual path.
    #[must_use]
    pub fn get(&self, virtual_path: &str) -> Option<Arc<SharedFile>> {
        self.by_virtual
            .get(virtual_path)
            .map(|&i| Arc::clone(&self.files[i]))
    }

    /// All shared files, in scan order.
    #[must_use]
    pub fn files(&self) -> &[Arc<SharedFile>] {
        &self.files
    }

    /// Files grouped by their virtual directory, in browse form.
    ///
    /// Grouping happens at scan time; the clone here is the only work on the
    /// serving thread.
    #[must_use]
    pub fn directories(&self) -> Vec<SharedDirectory> {
        (*self.directories).clone()
    }

    /// The directories named `name`, without grouping the whole share again.
    #[must_use]
    pub fn directory(&self, name: &str) -> Vec<SharedDirectory> {
        self.by_folder.get(name).cloned().unwrap_or_default()
    }

    /// The ready-to-send `SharedFileListResponse`.
    ///
    /// It was compressed when the share was scanned, not inside the operations
    /// loop.
    #[must_use]
    pub fn browse_message(&self) -> Message {
        (*self.browse_response).clone()
    }

    #[must_use]
    pub const fn file_count(&self) -> u32 {
        self.files.len() as u32
    }

    #[must_use]
    pub const fn folder_count(&self) -> u32 {
        self.folder_count as u32
    }
}

fn root_display_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || "shared".to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Recursively walk `root`, appending its files (under the virtual root name
/// `root_name`) to `files` and counting folders that contain files.
fn scan_root(
    root: &Path,
    canonical_root: &Path,
    root_identity: (u64, u64),
    root_name: &str,
    files: &mut Vec<SharedFile>,
    folder_count: &mut usize,
) {
    let mut folders_with_files: HashSet<PathBuf> = HashSet::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            // `entry.metadata()` does not traverse the symlink itself.
            if meta.file_type().is_symlink() {
                continue;
            }
            let path = entry.path();
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                // The download directory is usually shared too; an in-progress
                // `.part` file is not something to offer other peers.
                if path
                    .to_string_lossy()
                    .ends_with(crate::utils::path::PART_SUFFIX)
                {
                    continue;
                }
                let virtual_path = virtual_path_for(root_name, root, &path);
                let relative =
                    path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                files.push(SharedFile {
                    search_key: virtual_path.to_lowercase(),
                    virtual_path,
                    attributes: probe_indexed(&path, meta.dev(), meta.ino()),
                    real_path: path.clone(),
                    root: canonical_root.to_path_buf(),
                    relative,
                    root_dev: root_identity.0,
                    root_ino: root_identity.1,
                    dev: meta.dev(),
                    ino: meta.ino(),
                    size: meta.len(),
                });
                folders_with_files.insert(dir.clone());
            }
        }
    }
    *folder_count += folders_with_files.len();
}

/// Read audio attributes from the descriptor that was just checked, so a swap
/// after indexing cannot redirect the probe to another path or block it on a
/// FIFO. The descriptor stays owned by this call and is closed before return.
fn probe_indexed(path: &Path, dev: u64, ino: u64) -> Vec<(u32, u32)> {
    const FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;

    let Ok(name) = CString::new(path.as_os_str().as_encoded_bytes()) else {
        return Vec::new();
    };
    // SAFETY: `name` is a live C string and this call owns the fresh fd.
    let fd = unsafe { libc::open(name.as_ptr(), FLAGS) };
    if fd < 0 {
        return Vec::new();
    }
    // SAFETY: `open` returned a fresh, owned file descriptor.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let Ok(meta) = file.metadata() else {
        return Vec::new();
    };
    if !meta.file_type().is_file()
        || meta.dev() != dev
        || meta.ino() != ino
    {
        return Vec::new();
    }
    if clear_nonblock(fd).is_err() {
        return Vec::new();
    }
    audio::probe_open(&mut file, path)
}

/// Restore blocking reads on a regular file that was opened nonblocking only
/// so it could be inspected safely.
fn clear_nonblock(fd: libc::c_int) -> io::Result<()> {
    // SAFETY: `fd` is owned by the caller and valid for this call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above; only the nonblocking flag on this open description is
    // changed.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Open an indexed file for upload, confined to the share root it was scanned
/// under.
///
/// The root is reopened from the path captured at scan time; this function
/// does not have a descriptor from scan time to follow. The reopened directory
/// must still have the root device/inode from the index, so a different
/// directory installed at that path is refused. Because the path is reopened,
/// renaming the root or an ancestor breaks serving until a rescan.
///
/// Walking with `O_NOFOLLOW` at each step means neither `..` nor a symlink in
/// or below the root can escape it. The leaf is opened with `O_NONBLOCK` so a
/// regular file replaced by a FIFO cannot block here; the descriptor is
/// inspected before it is returned, and rejected descriptors are released.
/// The leaf device/inode is checked on that same descriptor, so a replacement
/// file is refused until the next scan. That check proves identity only: it
/// says nothing about path containment, in-place changes to the same inode,
/// or content read after the descriptor is returned.
///
/// # Errors
/// Returns the first I/O error, or `InvalidData` when the reopened root is no
/// longer the indexed directory, the entry would leave the root, it is not a
/// regular file, or it is not the indexed file.
pub fn open_contained(file: &SharedFile) -> io::Result<std::fs::File> {
    const DIR_FLAGS: libc::c_int = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_DIRECTORY;
    const LEAF_FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;

    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(DIR_FLAGS)
        .open(&file.root)?;
    let root_meta = directory.metadata()?;
    if root_meta.dev() != file.root_dev || root_meta.ino() != file.root_ino {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the share root changed since it was indexed",
        ));
    }
    let components: Vec<_> = file.relative.components().collect();
    let Some((last, parents)) = components.split_last() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a shared file must have a name relative to its root",
        ));
    };
    for component in parents {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a shared path may not traverse its root",
            ));
        };
        let name = CString::new(name.as_encoded_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a shared path component contains a NUL byte",
            )
        })?;
        // SAFETY: `directory` is owned by this call and stays open until the
        // new descriptor replaces it; `name` is a live C string for the call.
        let fd = unsafe {
            libc::openat(directory.as_raw_fd(), name.as_ptr(), DIR_FLAGS)
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a fresh, owned file descriptor.
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    let Component::Normal(name) = last else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a shared path may not traverse its root",
        ));
    };
    // SAFETY: as above, both descriptors are valid for the duration of the
    // call and `name` is a borrowed encoded path component.
    let name = CString::new(name.as_encoded_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a shared path component contains a NUL byte",
        )
    })?;
    let fd = unsafe {
        libc::openat(directory.as_raw_fd(), name.as_ptr(), LEAF_FLAGS)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a fresh, owned file descriptor.
    let opened = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = opened.metadata()?;
    if !meta.file_type().is_file()
        || meta.dev() != file.dev
        || meta.ino() != file.ino
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the shared file changed since it was indexed",
        ));
    }
    clear_nonblock(opened.as_raw_fd())?;
    Ok(opened)
}

/// Build the peer-facing virtual path for `path` under `root`: the root's own
/// name followed by the backslash-separated components relative to it.
fn virtual_path_for(root_name: &str, root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut parts = vec![root_name.to_string()];
    for component in rel.components() {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("\\")
}

#[cfg(test)]
pub(crate) fn indexed_file(path: &Path) -> SharedFile {
    let root = path.canonicalize().expect("a real fixture path");
    let name = root.file_name().expect("a file name").to_os_string();
    let parent = root.parent().expect("a parent").to_path_buf();
    let root_meta = std::fs::metadata(&parent).expect("fixture root metadata");
    let meta = std::fs::metadata(&root).expect("fixture metadata");
    SharedFile {
        virtual_path: name.to_string_lossy().into_owned(),
        search_key: name.to_string_lossy().to_lowercase(),
        real_path: root,
        root: parent,
        relative: PathBuf::from(name),
        root_dev: root_meta.dev(),
        root_ino: root_meta.ino(),
        dev: meta.dev(),
        ino: meta.ino(),
        size: meta.len(),
        attributes: Vec::new(),
    }
}

#[cfg(test)]
pub(crate) fn synthetic_file(path: &Path) -> SharedFile {
    let root = path.parent().unwrap_or_else(|| Path::new("/"));
    let name = path.file_name().unwrap_or_default().to_os_string();
    SharedFile {
        virtual_path: name.to_string_lossy().into_owned(),
        search_key: name.to_string_lossy().to_lowercase(),
        real_path: path.to_path_buf(),
        root: root.to_path_buf(),
        relative: PathBuf::from(name),
        root_dev: 0,
        root_ino: 0,
        dev: 0,
        ino: 0,
        size: 0,
        attributes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{BROWSE_BUILDS, Shares, open_contained};
    use std::io::Read;

    fn temp_tree() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir()
            .join(format!("soulseek-shares-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("album")).unwrap();
        std::fs::write(root.join("top.mp3"), b"aaaa").unwrap();
        std::fs::write(root.join("album").join("song one.flac"), b"bbbbbb")
            .unwrap();
        std::fs::write(root.join("album").join("song two.flac"), b"cc")
            .unwrap();
        root
    }

    #[test]
    fn a_scanned_mp3_carries_its_bitrate() {
        let root = std::env::temp_dir()
            .join(format!("soulseek-shares-audio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // 100 frames at 128 kbps is 2.6 seconds.
        std::fs::write(
            root.join("track.mp3"),
            super::audio::tests::cbr_mp3(100),
        )
        .unwrap();
        std::fs::write(root.join("cover.jpg"), b"jpeg").unwrap();

        let shares = Shares::scan(&root).unwrap();
        let track = shares
            .get(
                "soulseek-shares-audio-{pid}\\track.mp3"
                    .replace("{pid}", &std::process::id().to_string())
                    .as_str(),
            )
            .unwrap();
        assert!(track.attributes.contains(&(0, 128)));
        assert!(track.attributes.contains(&(1, 2)));
        let cover = shares
            .files()
            .iter()
            .find(|f| f.virtual_path.ends_with("cover.jpg"))
            .unwrap();
        assert!(cover.attributes.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_counts_files_and_folders() {
        let root = temp_tree();
        let shares = Shares::scan(&root).unwrap();
        assert_eq!(shares.file_count(), 3);
        // Two folders contain files: the root and `album`.
        assert_eq!(shares.folder_count(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn in_progress_downloads_are_not_shared() {
        let root = temp_tree();
        std::fs::write(root.join("half.mp3.part"), b"dd").unwrap();

        let shares = Shares::scan(&root).unwrap();
        assert_eq!(shares.file_count(), 3, ".part files stay out of the index");
        let base = root.file_name().unwrap().to_string_lossy().into_owned();
        assert!(shares.get(&format!("{base}\\half.mp3.part")).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nested_files_use_backslash_virtual_paths() {
        let root = temp_tree();
        let base = root.file_name().unwrap().to_string_lossy().into_owned();
        let shares = Shares::scan(&root).unwrap();

        let vpath = format!("{base}\\album\\song one.flac");
        let file = shares.get(&vpath).expect("nested file indexed");
        assert_eq!(file.size, 6);
        assert!(shares.get("does\\not\\exist").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_mixed_case_path_matches_a_lowercase_query() {
        let root = std::env::temp_dir()
            .join(format!("soulseek-shares-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Live Set")).unwrap();
        std::fs::write(root.join("Live Set").join("Opening TRACK.flac"), b"x")
            .unwrap();
        let shares = Shares::scan(&root).unwrap();

        assert_eq!(shares.search("live opening track").len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn search_is_case_insensitive_and_multi_term_and() {
        let root = temp_tree();
        let shares = Shares::scan(&root).unwrap();

        // Both album files match "SONG"; "song one" narrows to one.
        assert_eq!(shares.search("SONG").len(), 2);
        assert_eq!(shares.search("song ONE").len(), 1);
        assert!(shares.search("nonexistent").is_empty());
        assert!(shares.search("").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_many_merges_roots() {
        let root_a = temp_tree();
        let root_b = temp_tree();
        let shares = Shares::scan_many(&[root_a.clone(), root_b.clone()]);
        assert_eq!(shares.file_count(), 6);
        assert_eq!(shares.folder_count(), 4);
        // Files from both roots are reachable under their root's name.
        let name_a = root_a.file_name().unwrap().to_string_lossy();
        let name_b = root_b.file_name().unwrap().to_string_lossy();
        assert!(shares.get(&format!("{name_a}\\top.mp3")).is_some());
        assert!(shares.get(&format!("{name_b}\\top.mp3")).is_some());
        let _ = std::fs::remove_dir_all(root_a);
        let _ = std::fs::remove_dir_all(root_b);
    }

    #[test]
    fn scan_many_disambiguates_equal_root_names() {
        // Two different directories both named "music".
        let parent_a = temp_tree();
        let parent_b = temp_tree();
        let music_a = parent_a.join("music");
        let music_b = parent_b.join("music");
        std::fs::create_dir_all(&music_a).unwrap();
        std::fs::create_dir_all(&music_b).unwrap();
        std::fs::write(music_a.join("a.mp3"), b"a").unwrap();
        std::fs::write(music_b.join("b.mp3"), b"b").unwrap();

        let shares = Shares::scan_many(&[music_a, music_b]);
        assert!(shares.get("music\\a.mp3").is_some());
        assert!(
            shares.get("music (2)\\b.mp3").is_some(),
            "second root with the same name gets a numbered suffix"
        );
        let _ = std::fs::remove_dir_all(parent_a);
        let _ = std::fs::remove_dir_all(parent_b);
    }

    #[test]
    fn scan_many_skips_unreadable_roots() {
        let root = temp_tree();
        let shares = Shares::scan_many(&[
            std::path::PathBuf::from("/does/not/exist"),
            root.clone(),
        ]);
        assert_eq!(shares.file_count(), 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_search_collects_at_most_one_reply_of_matches() {
        // A broad query against a large share must not collect every match
        // before the response cap is applied: the scan runs on the singleton
        // operations thread, so the collection itself is the cost.
        let files = (0..500)
            .map(|i| super::synthetic_file(std::path::Path::new(&format!(
                "match-{i:04}.mp3"
            ))))
            .collect();
        let shares = super::Shares::from_files(files, 0);
        assert_eq!(shares.files().len(), 500);
        assert!(
            shares.search("match").len() <= crate::types::MAX_SEARCH_REPLY_FILES,
            "a single scan must stop at one reply's worth of matches"
        );
    }

    #[test]
    fn repeated_queries_reuse_the_index_until_it_is_invalidated() {
        let root = temp_tree();
        let shares = Shares::scan(&root).unwrap();
        assert_eq!(shares.search_cache.lock().unwrap().len(), 0);
        let first: Vec<_> = shares
            .search("song")
            .iter()
            .map(|f| f.virtual_path.clone())
            .collect();
        assert_eq!(shares.search_cache.lock().unwrap().len(), 1);
        let again: Vec<_> = shares
            .search("song")
            .iter()
            .map(|f| f.virtual_path.clone())
            .collect();
        assert_eq!(again, first);
        assert_eq!(shares.search_cache.lock().unwrap().len(), 1);
        shares.invalidate_search_cache();
        assert_eq!(shares.search_cache.lock().unwrap().len(), 0);
        let after: Vec<_> = shares
            .search("song")
            .iter()
            .map(|f| f.virtual_path.clone())
            .collect();
        assert_eq!(after, first);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn serving_a_browse_does_not_rebuild_or_recompress_the_listing() {
        let root = temp_tree();
        let shares = Shares::scan(&root).unwrap();
        let built_at_scan = BROWSE_BUILDS.load(std::sync::atomic::Ordering::Relaxed);
        let expected =
            crate::message::peer::build_shared_file_list(&shares.directories());

        let first = shares.browse_message();
        let second = shares.browse_message();
        assert_eq!(
            BROWSE_BUILDS.load(std::sync::atomic::Ordering::Relaxed),
            built_at_scan,
            "serving a browse must not rebuild or recompress the listing"
        );
        assert_eq!(first.get_data(), expected.get_data());
        assert_eq!(second.get_data(), first.get_data());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn empty_shares_have_no_files_or_folders() {
        let shares = Shares::empty();
        assert_eq!(shares.file_count(), 0);
        assert_eq!(shares.folder_count(), 0);
        assert!(shares.search("anything").is_empty());
    }

    #[test]
    fn a_scanned_file_is_opened_from_its_root_not_from_its_display_path() {
        let root = temp_tree();
        let shares = Shares::scan(&root).unwrap();
        let file = shares
            .get(&format!(
                "{}\\album\\song one.flac",
                root.file_name().unwrap().to_string_lossy()
            ))
            .unwrap();
        let opened = open_contained(&file).expect("the indexed file opens");
        let mut bytes = Vec::new();
        (&opened).read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"bbbbbb");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_symlink_swapped_in_after_indexing_cannot_serve_outside_the_root() {
        let root = temp_tree();
        let outside = std::env::temp_dir().join(format!(
            "soulseek-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&outside, b"secret").unwrap();
        let file = shares_file(&root, "top.mp3");

        std::fs::remove_file(root.join("top.mp3")).unwrap();
        symlink(&outside, &root.join("top.mp3"));
        let error = open_contained(&file).unwrap_err();
        assert!(
            error.kind() == std::io::ErrorKind::InvalidData
                || error.raw_os_error() == Some(libc::ELOOP),
            "a symlink must not be followed: {error}"
        );
        let _ = std::fs::remove_file(outside);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_replaced_file_is_refused_until_the_next_scan() {
        let root = temp_tree();
        let file = shares_file(&root, "top.mp3");
        // Replace the directory entry, not just its bytes: an in-place write
        // keeps the inode and is still the indexed file.
        let replacement = root.join("replacement.tmp");
        std::fs::write(&replacement, b"different").unwrap();
        std::fs::rename(&replacement, root.join("top.mp3")).unwrap();
        assert_eq!(
            open_contained(&file).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let shares = Shares::scan(&root).unwrap();
        assert!(open_contained(shares.files().first().unwrap()).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_parent_directory_cannot_be_traversed() {
        let root = temp_tree();
        let mut file = shares_file(&root, "top.mp3");
        file.relative = std::path::PathBuf::from("../outside.mp3");
        assert_eq!(
            open_contained(&file).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn mkfifo(path: &std::path::Path) {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .expect("a fixture path without NUL bytes");
        // SAFETY: `name` is a live C string for this call.
        assert_eq!(
            unsafe { libc::mkfifo(name.as_ptr(), 0o600) },
            0,
            "mkfifo failed"
        );
    }

    #[cfg(unix)]
    fn open_fd_count() -> usize {
        std::fs::read_dir("/dev/fd").map_or(0, Iterator::count)
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_swapped_in_after_indexing_is_rejected_before_a_deadline() {
        use std::thread;
        use std::time::Duration;

        let root = temp_tree();
        let file = shares_file(&root, "top.mp3");
        std::fs::remove_file(root.join("top.mp3")).unwrap();
        mkfifo(&root.join("top.mp3"));

        let target = file.clone();
        let worker = thread::spawn(move || open_contained(&target).map(|_| ()));
        // Wait for the worker to finish before joining it: a join on a worker
        // wedged in a blocking open would turn a regression into a hang.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !worker.is_finished() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            worker.is_finished(),
            "opening a FIFO wedged past the deadline"
        );
        let outcome = worker.join().expect("the opening thread did not panic");
        assert_eq!(
            outcome.expect_err("a FIFO is not the indexed file").kind(),
            std::io::ErrorKind::InvalidData
        );

        let before = open_fd_count();
        for _ in 0..64 {
            assert_eq!(
                open_contained(&file).unwrap_err().kind(),
                std::io::ErrorKind::InvalidData
            );
        }
        if before > 0 {
            assert!(
                open_fd_count() <= before + 4,
                "rejected opens leaked descriptors"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_swapped_root_directory_is_refused_until_the_next_scan() {
        let root = temp_tree();
        let file = shares_file(&root, "top.mp3");
        let name = root.file_name().unwrap().to_string_lossy().into_owned();
        let moved = root.with_file_name(format!("{name}-moved"));
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("top.mp3"), b"evil").unwrap();

        assert_eq!(
            open_contained(&file).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let shares = Shares::scan(&root).unwrap();
        let refreshed = shares
            .get(&format!("{name}\\top.mp3"))
            .expect("the replacement root is indexed");
        let mut bytes = Vec::new();
        (&open_contained(&refreshed).expect("the replacement root serves"))
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"evil");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(moved);
    }

    #[test]
    fn moving_an_ancestor_invalidates_the_root_path_until_a_rescan() {
        let parent = temp_tree();
        let root = parent.join("music");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("top.mp3"), b"aaaa").unwrap();
        let file = shares_file(&root, "top.mp3");

        let moved_parent = parent.with_file_name(format!(
            "{}-moved",
            parent.file_name().unwrap().to_string_lossy()
        ));
        std::fs::rename(&parent, &moved_parent).unwrap();
        assert!(open_contained(&file).is_err(), "the recorded path is gone");

        let moved_root = moved_parent.join("music");
        let shares = Shares::scan(&moved_root).unwrap();
        let refreshed = shares
            .get("music\\top.mp3")
            .expect("the moved root is indexed");
        let mut bytes = Vec::new();
        (&open_contained(&refreshed).expect("the moved root serves"))
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"aaaa");
        let _ = std::fs::remove_dir_all(moved_parent);
    }

    fn shares_file(root: &std::path::Path, name: &str) -> super::SharedFile {
        let shares = Shares::scan(root).unwrap();
        let file = shares
            .files()
            .iter()
            .find(|f| f.virtual_path.ends_with(name))
            .unwrap();
        (**file).clone()
    }

    fn symlink(target: &std::path::Path, link: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }
}
