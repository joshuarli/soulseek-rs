use crate::api::SessionApi;
use crate::error::{Error, Result};
use crate::persist::paths;
use deser::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where passwords live. The real implementation is a private file
/// ([`FileStore`]); tests use an in-memory fake. Passwords are never written
/// to config.toml.
///
/// `Send + Sync` because the daemon holds one behind an `Arc` and answers
/// requests on a thread per connection.
pub trait SecretStore: Send + Sync {
    fn get(&self, username: &str) -> Result<Option<String>>;
    fn set(&self, username: &str, password: &str) -> Result<()>;
    /// Forget this account's password. Succeeds when there was nothing
    /// stored: the caller wants it gone, and it is.
    fn delete(&self, username: &str) -> Result<()>;
}

/// Passwords in `credentials.toml` beside config.toml, plain text but mode
/// 0600 so only the owner can read them.
pub struct FileStore {
    path: Option<PathBuf>,
    lock: Mutex<()>,
}

impl Default for FileStore {
    fn default() -> Self {
        Self {
            path: paths::credentials_file(),
            lock: Mutex::new(()),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[deser(default)]
struct Credentials {
    passwords: BTreeMap<String, String>,
}

impl FileStore {
    #[cfg(test)]
    const fn at(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            lock: Mutex::new(()),
        }
    }

    fn path(&self) -> Result<&Path> {
        self.path.as_deref().ok_or_else(|| {
            Error::msg("no config directory: set SOULSEEK_CONFIG_DIR")
        })
    }

    fn read(&self) -> Result<Credentials> {
        let path = self.path()?;
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Credentials::default());
            }
            Err(e) => {
                return Err(Error::msg(format!(
                    "Cannot read {}: {e}",
                    path.display()
                )));
            }
        };
        deser_toml::from_str(&text).map_err(|e| {
            Error::msg(format!("Malformed {}: {e}", path.display()))
        })
    }

    fn write(&self, credentials: &Credentials) -> Result<()> {
        let path = self.path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(deser_toml::to_string(credentials)?.as_bytes())?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

impl SecretStore for FileStore {
    fn get(&self, username: &str) -> Result<Option<String>> {
        let _guard = self.lock.lock().expect("not poisoned");
        Ok(self.read()?.passwords.remove(username))
    }

    fn set(&self, username: &str, password: &str) -> Result<()> {
        let _guard = self.lock.lock().expect("not poisoned");
        let mut credentials = self.read()?;
        credentials
            .passwords
            .insert(username.to_string(), password.to_string());
        self.write(&credentials)
    }

    fn delete(&self, username: &str) -> Result<()> {
        let _guard = self.lock.lock().expect("not poisoned");
        let mut credentials = self.read()?;
        if credentials.passwords.remove(username).is_none() {
            return Ok(());
        }
        if !credentials.passwords.is_empty() {
            return self.write(&credentials);
        }
        let path = self.path()?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::msg(format!(
                "Cannot remove {}: {e}",
                path.display()
            ))),
        }
    }
}

/// Change the account's password and keep the local copy in step, so the next
/// start still logs in.
///
/// One rule in one place: whoever owns the login owns the stored copy. A
/// window or a command borrowing a daemon's session owns neither, and the
/// daemon writes its own host's store when it serves the request.
///
/// `Ok` means the request reached the server, not that the server took it —
/// Soulseek answers a password change with nothing at all — and carries the
/// store's complaint when the change went out but the local copy did not.
pub fn change_password(
    session: &dyn SessionApi,
    secrets: &dyn SecretStore,
    password: &str,
) -> soulseek_rs::Result<Option<String>> {
    session.change_password(password)?;
    if session.daemon_endpoint().is_some() {
        return Ok(None);
    }
    Ok(secrets
        .set(&session.username(), password)
        .err()
        .map(|e| e.to_string()))
}

/// Resolve the password with precedence: CLI/env > saved password >
/// `password_cmd`.
///
/// `cli_password` already has env merged in by the argument parser. Returns
/// `None` when no source has one — the caller shows the login screen.
///
/// Store errors are downgraded to "not found": an unreadable credentials file
/// must not stop the fallbacks from working.
pub fn resolve_password(
    cli_password: Option<&str>,
    username: Option<&str>,
    password_cmd: Option<&str>,
    store: &dyn SecretStore,
) -> Option<String> {
    if let Some(password) = cli_password {
        return Some(password.to_string());
    }
    if let Some(username) = username
        && let Ok(Some(password)) = store.get(username)
    {
        return Some(password);
    }
    password_cmd.and_then(run_password_cmd)
}

/// Run `password_cmd` through the platform shell and return trimmed stdout.
fn run_password_cmd(cmd: &str) -> Option<String> {
    let output =
        std::process::Command::new("sh").args(["-c", cmd]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let password = String::from_utf8(output.stdout).ok()?;
    let password = password.trim_end_matches(['\r', '\n']);
    (!password.is_empty()).then(|| password.to_string())
}

/// An in-memory store for tests, so no test touches the machine's real
/// credentials file.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    secrets: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Answer every read as if the file could not be read.
    pub fail: bool,
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn get(&self, username: &str) -> Result<Option<String>> {
        if self.fail {
            return Err(crate::error::Error::msg("no saved password"));
        }
        Ok(self
            .secrets
            .lock()
            .expect("not poisoned")
            .get(username)
            .cloned())
    }

    fn set(&self, username: &str, password: &str) -> Result<()> {
        self.secrets
            .lock()
            .expect("not poisoned")
            .insert(username.into(), password.into());
        Ok(())
    }

    fn delete(&self, username: &str) -> Result<()> {
        self.secrets.lock().expect("not poisoned").remove(username);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type FakeStore = MemoryStore;

    fn file_store() -> (tempfile::TempDir, FileStore) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let store = FileStore::at(dir.path().join("credentials.toml"));
        (dir, store)
    }

    #[test]
    fn cli_password_wins_over_everything() {
        let store = FakeStore::default();
        store.set("alice", "from-store").unwrap();
        let got = resolve_password(
            Some("from-cli"),
            Some("alice"),
            Some("echo from-cmd"),
            &store,
        );
        assert_eq!(got.as_deref(), Some("from-cli"));
    }

    #[test]
    fn saved_password_used_when_no_cli_password() {
        let store = FakeStore::default();
        store.set("alice", "from-store").unwrap();
        let got = resolve_password(None, Some("alice"), None, &store);
        assert_eq!(got.as_deref(), Some("from-store"));
    }

    #[test]
    fn password_cmd_used_when_no_password_is_saved() {
        let store = FakeStore::default();
        let got = resolve_password(
            None,
            Some("alice"),
            Some("echo from-cmd"),
            &store,
        );
        assert_eq!(got.as_deref(), Some("from-cmd"));
    }

    #[test]
    fn store_failure_falls_through_to_password_cmd() {
        let store = FakeStore {
            fail: true,
            ..FakeStore::default()
        };
        let got = resolve_password(
            None,
            Some("alice"),
            Some("echo from-cmd"),
            &store,
        );
        assert_eq!(got.as_deref(), Some("from-cmd"));
    }

    #[test]
    fn none_when_no_source_has_a_password() {
        let store = FakeStore::default();
        assert_eq!(resolve_password(None, Some("alice"), None, &store), None);
    }

    #[test]
    fn a_deleted_password_is_no_longer_resolved() {
        let store = FakeStore::default();
        store.set("alice", "from-store").unwrap();
        store.delete("alice").unwrap();
        assert_eq!(resolve_password(None, Some("alice"), None, &store), None);
        // Deleting again is not an error: the password is gone either way.
        store.delete("alice").unwrap();
    }

    #[test]
    fn no_username_means_no_store_lookup() {
        let store = FakeStore {
            fail: true,
            ..FakeStore::default()
        };
        assert_eq!(resolve_password(None, None, None, &store), None);
    }

    #[test]
    fn failing_password_cmd_yields_none() {
        let store = FakeStore::default();
        assert_eq!(
            resolve_password(None, Some("alice"), Some("false"), &store),
            None
        );
    }

    #[test]
    fn file_store_round_trips_passwords_for_several_accounts() {
        let (_dir, store) = file_store();
        assert_eq!(store.get("alice").unwrap(), None);
        store.set("alice", "first").unwrap();
        store.set("bob", "second").unwrap();
        assert_eq!(store.get("alice").unwrap().as_deref(), Some("first"));
        assert_eq!(store.get("bob").unwrap().as_deref(), Some("second"));
        store.set("alice", "changed").unwrap();
        assert_eq!(store.get("alice").unwrap().as_deref(), Some("changed"));
    }

    #[test]
    fn deleting_the_last_password_removes_the_file() {
        let (dir, store) = file_store();
        store.set("alice", "secret").unwrap();
        let path = dir.path().join("credentials.toml");
        assert!(path.exists());
        store.delete("alice").unwrap();
        assert!(!path.exists());
        assert_eq!(store.get("alice").unwrap(), None);
    }

    #[test]
    fn a_malformed_file_is_an_error_rather_than_an_empty_store() {
        let (dir, store) = file_store();
        std::fs::write(dir.path().join("credentials.toml"), "[unclosed")
            .unwrap();
        assert!(store.get("alice").is_err());
        assert!(store.set("alice", "secret").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_password_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, store) = file_store();
        store.set("alice", "secret").unwrap();
        let mode = std::fs::metadata(dir.path().join("credentials.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
