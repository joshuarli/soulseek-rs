//! Resolution and validation of the configured download / shared directories.

use soulseek_rs::utils::path::expand_tilde;
use std::path::PathBuf;

/// Validate and normalize an optional shared-directory setting.
///
/// Returns `Ok(None)` when nothing is shared (unset or blank), `Ok(Some(path))`
/// for a `~`-expanded, existing directory, or `Err` with a human-readable
/// message when the configured path is not a usable directory.
pub fn resolve_shared_directory(
    raw: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let path = expand_tilde(trimmed);
    if !path.exists() {
        return Err(format!(
            "shared directory does not exist: {}",
            path.display()
        ));
    }
    if !path.is_dir() {
        return Err(format!(
            "shared path is not a directory: {}",
            path.display()
        ));
    }
    Ok(Some(path))
}

/// Prepare the folders a local session needs, then resolve what it shares.
///
/// First-run defaults point the download folder and the default share at the
/// same fresh folder, so it has to exist before share validation. Only the
/// download folder is created: an explicitly configured root is never created
/// behind the user's back, and an explicit opt-out stays empty.
pub fn prepare_local_session(
    download_dir: &str,
    shared_dirs: &[String],
    mut warn: impl FnMut(&str),
) -> Vec<String> {
    if let Err(e) = std::fs::create_dir_all(download_dir) {
        warn(&format!(
            "could not create download directory {download_dir}: {e}"
        ));
    }
    let mut resolved = Vec::new();
    for dir in shared_dirs {
        match resolve_shared_directory(Some(dir)) {
            Ok(Some(path)) => resolved.push(path.display().to_string()),
            Ok(None) => {}
            Err(e) => warn(&format!("ignoring shared directory: {e}")),
        }
    }
    resolved
}

/// Validate several configured share paths; invalid ones are skipped with a
/// warning instead of aborting (the client just shares less).
#[must_use]
pub fn resolve_shared_directories(dirs: &[String]) -> Vec<String> {
    let mut resolved = Vec::new();
    for dir in dirs {
        match resolve_shared_directory(Some(dir)) {
            Ok(Some(path)) => resolved.push(path.display().to_string()),
            Ok(None) => {}
            Err(e) => eprintln!("⚠️  Ignoring shared directory: {e}"),
        }
    }
    resolved
}

/// The folder a download from `username` lands in: `download_dir` itself, or
/// a per-user subfolder of it when `per_user` is set, as SoulseekQt does.
#[must_use]
pub fn download_dir_for(
    download_dir: &str,
    username: &str,
    per_user: bool,
) -> String {
    if !per_user {
        return download_dir.to_string();
    }
    std::path::Path::new(download_dir)
        .join(sanitize_path_component(username))
        .display()
        .to_string()
}

/// A peer-chosen name made safe as one path component: separators and control
/// characters become `_`, so `../..` cannot climb out.
fn sanitize_path_component(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '/' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if out.is_empty() || out.ends_with('.') {
        out.push('_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        download_dir_for, prepare_local_session, resolve_shared_directory,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir() -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("soulseek-rs-dirtest-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn unused_child(root: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = root.join(name);
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn a_previously_missing_default_folder_is_created_and_shared() {
        let root = unique_temp_dir();
        let download = unused_child(&root, "downloads");
        let mut warnings = Vec::new();
        let shared = prepare_local_session(
            download.to_str().unwrap(),
            &[download.display().to_string()],
            |message| warnings.push(message.to_string()),
        );
        assert!(download.is_dir(), "a fresh download folder must be created");
        assert_eq!(shared, vec![download.display().to_string()]);
        assert!(warnings.is_empty(), "{warnings:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_explicit_opt_out_shares_nothing() {
        let root = unique_temp_dir();
        let download = unused_child(&root, "downloads");
        let shared = prepare_local_session(
            download.to_str().unwrap(),
            &[],
            |_| panic!("an empty share list must not warn"),
        );
        assert!(shared.is_empty());
        assert!(
            download.is_dir(),
            "downloads still need a folder even with sharing off"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_roots_are_shared_without_creating_missing_ones() {
        let root = unique_temp_dir();
        let download = unused_child(&root, "downloads");
        let custom = unique_temp_dir();
        let missing = unused_child(&root, "never-create-me");
        let mut warnings = Vec::new();
        let shared = prepare_local_session(
            download.to_str().unwrap(),
            &[
                custom.display().to_string(),
                missing.display().to_string(),
            ],
            |message| warnings.push(message.to_string()),
        );
        assert_eq!(shared, vec![custom.display().to_string()]);
        assert!(!missing.exists(), "a configured root is not created for us");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ignoring shared directory"));
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(custom);
    }

    #[test]
    fn unset_or_blank_shares_nothing() {
        assert_eq!(resolve_shared_directory(None), Ok(None));
        assert_eq!(resolve_shared_directory(Some("")), Ok(None));
        assert_eq!(resolve_shared_directory(Some("   ")), Ok(None));
    }

    #[test]
    fn existing_directory_is_accepted() {
        let dir = unique_temp_dir();
        let resolved =
            resolve_shared_directory(Some(dir.to_str().unwrap())).unwrap();
        assert_eq!(resolved, Some(dir.clone()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_directory_is_rejected() {
        let missing = std::env::temp_dir().join("soulseek-rs-does-not-exist-x");
        let _ = std::fs::remove_dir_all(&missing);
        let result = resolve_shared_directory(Some(missing.to_str().unwrap()));
        assert!(result.is_err());
    }

    #[test]
    fn a_file_is_not_a_valid_shared_directory() {
        let dir = unique_temp_dir();
        let file = dir.join("not-a-dir.txt");
        std::fs::write(&file, b"x").unwrap();
        let result = resolve_shared_directory(Some(file.to_str().unwrap()));
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn dl(name: &str) -> String {
        std::path::Path::new("/dl").join(name).display().to_string()
    }

    #[test]
    fn per_user_folder_is_a_child_of_the_download_dir() {
        assert_eq!(download_dir_for("/dl", "alice", false), "/dl");
        assert_eq!(download_dir_for("/dl", "alice", true), dl("alice"));
    }

    #[test]
    fn a_hostile_username_cannot_leave_the_download_dir() {
        assert_eq!(download_dir_for("/dl", "../..", true), dl(".._.._"));
        assert_eq!(download_dir_for("/dl", "/tmp/x", true), dl("_tmp_x"));
        assert_eq!(download_dir_for("/dl", "..", true), dl(".._"));
        assert_eq!(download_dir_for("/dl", "", true), dl("_"));
    }
}
