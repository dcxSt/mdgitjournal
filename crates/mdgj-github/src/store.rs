//! Token storage: OS keyring with a 0600 file fallback.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::client::Tokens;
use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    GithubApp,
    Pat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAuth {
    pub kind: AuthKind,
    pub tokens: Tokens,
    pub login: Option<String>,
}

pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredAuth>>;
    fn save(&self, auth: &StoredAuth) -> Result<()>;
    /// Removes the saved sign-in; succeeds if there is none.
    fn clear(&self) -> Result<()>;
}

/// JSON in a file only its owner can read (0600 on Unix), replaced atomically on save.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn write(&self, json: &str) -> io::Result<()> {
        let dir = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tmp.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        tmp.write_all(json.as_bytes())?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        // A lost rename would bring back a refresh token that has already been rotated out.
        #[cfg(unix)]
        fs::File::open(dir)?.sync_all()?;
        Ok(())
    }

    fn io_error(&self, err: io::Error) -> Error {
        Error::Store(format!("{}: {err}", self.path.display()))
    }
}

impl TokenStore for FileStore {
    fn load(&self) -> Result<Option<StoredAuth>> {
        match fs::read_to_string(&self.path) {
            Ok(json) => decode(&json).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(self.io_error(e)),
        }
    }

    fn save(&self, auth: &StoredAuth) -> Result<()> {
        self.write(&encode(auth)?).map_err(|e| self.io_error(e))
    }

    fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(self.io_error(e)),
            _ => Ok(()),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
const OS_KEYRING: Option<&str> = Some("keychain");
#[cfg(target_os = "linux")]
const OS_KEYRING: Option<&str> = Some("secret-service");
// keyring would fall back to its non-persistent mock store here.
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux")))]
const OS_KEYRING: Option<&str> = None;

const SERVICE: &str = "xyz.stephenfay.mdgitjournal";
const ACCOUNT: &str = "github";

struct OsKeyring {
    entry: keyring::Entry,
    name: &'static str,
}

/// The OS keyring, falling back to a [`FileStore`] when the keyring fails (for example,
/// no Secret Service is running). After a failure the keyring is left alone for the rest
/// of the process.
///
/// Every successful keyring save deletes the file, so a file that exists holds the newest
/// sign-in. Loading reads it first, and moves it into the keyring once that works again.
pub struct KeyringStore {
    keyring: Option<OsKeyring>,
    file: FileStore,
    /// Whether the keyring failed in this process. Held by every operation, since a load
    /// can write too.
    keyring_failed: Mutex<bool>,
}

impl KeyringStore {
    pub fn new(fallback_file: impl Into<PathBuf>) -> Self {
        let keyring = OS_KEYRING.and_then(|name| {
            let entry = keyring::Entry::new(SERVICE, ACCOUNT).ok()?;
            Some(OsKeyring { entry, name })
        });
        Self::with_keyring(keyring, fallback_file.into())
    }

    /// The file alone: the OS keyring is never built, read or written, and `backend()` is
    /// `"file"`. For `MDGJ_CONFIG_DIR`, so instances with their own folders never share (or
    /// clobber) one keychain item, and test runs never touch the real keychain.
    pub fn file_only(file: impl Into<PathBuf>) -> Self {
        Self::with_keyring(None, file.into())
    }

    fn with_keyring(keyring: Option<OsKeyring>, fallback_file: PathBuf) -> Self {
        Self {
            keyring,
            file: FileStore::new(fallback_file),
            keyring_failed: Mutex::new(false),
        }
    }

    /// Where tokens are kept: `"keychain"`, `"secret-service"` or `"file"`. A keyring
    /// that doesn't work is only found out on first use, so ask after a `load`.
    pub fn backend(&self) -> &'static str {
        self.keyring(*self.lock()).map_or("file", |k| k.name)
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        self.keyring_failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn keyring(&self, failed: bool) -> Option<&OsKeyring> {
        self.keyring.as_ref().filter(|_| !failed)
    }

    /// Saves to the keyring unless it has failed; `false` means use the file.
    fn keyring_save(&self, failed: &mut bool, auth: &StoredAuth) -> Result<bool> {
        let Some(k) = self.keyring(*failed) else {
            return Ok(false);
        };
        *failed = k.entry.set_password(&encode(auth)?).is_err();
        Ok(!*failed)
    }
}

impl TokenStore for KeyringStore {
    fn load(&self) -> Result<Option<StoredAuth>> {
        let mut failed = self.lock();
        if let Some(auth) = self.file.load()? {
            if self.keyring_save(&mut failed, &auth)? {
                self.file.clear()?;
            }
            return Ok(Some(auth));
        }
        let Some(k) = self.keyring(*failed) else {
            return Ok(None);
        };
        match k.entry.get_password() {
            Ok(json) => decode(&json).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => {
                *failed = true;
                Ok(None)
            }
        }
    }

    fn save(&self, auth: &StoredAuth) -> Result<()> {
        let mut failed = self.lock();
        if self.keyring_save(&mut failed, auth)? {
            return self.file.clear();
        }
        self.file.save(auth)
    }

    fn clear(&self) -> Result<()> {
        let failed = self.lock();
        self.file.clear()?;
        let Some(k) = &self.keyring else {
            return Ok(());
        };
        // Tried even after a failure: a copy left behind would sign the user back in on a
        // later run.
        match k.entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            // It failed before too, so most likely there is no keyring (no Secret Service)
            // and nothing in it to delete.
            Err(_) if *failed => Ok(()),
            Err(e) => Err(Error::Store(format!("{}: {e}", k.name))),
        }
    }
}

fn encode(auth: &StoredAuth) -> Result<String> {
    serde_json::to_string(auth).map_err(|e| Error::Store(e.to_string()))
}

fn decode(json: &str) -> Result<StoredAuth> {
    serde_json::from_str(json)
        .map_err(|e| Error::Store(format!("it is unreadable ({e}). Sign in again.")))
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use keyring::credential::CredentialApi;

    use super::*;

    fn auth(kind: AuthKind, token: &str) -> StoredAuth {
        StoredAuth {
            kind,
            tokens: Tokens {
                access_token: token.into(),
                expires_at: Some(1_800_028_800),
                refresh_token: Some("ghr_r".into()),
                refresh_expires_at: Some(1_815_897_600),
            },
            login: Some("dcxSt".into()),
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// An in-memory keyring entry that can be made to fail like an unreachable keyring.
    #[derive(Default)]
    struct FakeKeyring {
        secret: Mutex<Option<String>>,
        broken: AtomicBool,
        calls: AtomicUsize,
    }

    impl FakeKeyring {
        fn broken() -> Arc<Self> {
            let fake = Arc::new(Self::default());
            fake.set_broken(true);
            fake
        }

        fn set_broken(&self, broken: bool) {
            self.broken.store(broken, Ordering::Relaxed);
        }

        fn holding(auth: &StoredAuth) -> Arc<Self> {
            let fake = Arc::new(Self::default());
            *fake.secret.lock().unwrap() = Some(encode(auth).unwrap());
            fake
        }

        fn saved(&self) -> Option<StoredAuth> {
            let secret = self.secret.lock().unwrap();
            secret.as_deref().map(|json| decode(json).unwrap())
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }

        fn check(&self) -> keyring::Result<()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.broken.load(Ordering::Relaxed) {
                return Err(keyring::Error::NoStorageAccess("keyring is locked".into()));
            }
            Ok(())
        }
    }

    struct FakeCredential(Arc<FakeKeyring>);

    impl CredentialApi for FakeCredential {
        fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
            self.0.check()?;
            *self.0.secret.lock().unwrap() = Some(String::from_utf8(secret.to_vec()).unwrap());
            Ok(())
        }

        fn get_secret(&self) -> keyring::Result<Vec<u8>> {
            self.0.check()?;
            let secret = self.0.secret.lock().unwrap().clone();
            secret
                .map(String::into_bytes)
                .ok_or(keyring::Error::NoEntry)
        }

        fn delete_credential(&self) -> keyring::Result<()> {
            self.0.check()?;
            match self.0.secret.lock().unwrap().take() {
                Some(_) => Ok(()),
                None => Err(keyring::Error::NoEntry),
            }
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn keyring_store(fake: &Arc<FakeKeyring>, file: &Path) -> KeyringStore {
        let entry = keyring::Entry::new_with_credential(Box::new(FakeCredential(fake.clone())));
        KeyringStore::with_keyring(
            Some(OsKeyring {
                entry,
                name: "keychain",
            }),
            file.to_owned(),
        )
    }

    fn credentials_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credentials.json");
        (dir, file)
    }

    #[test]
    fn stored_auth_json_uses_contract_names() {
        let json = serde_json::to_value(auth(AuthKind::GithubApp, "ghu_a")).unwrap();
        assert_eq!(json["kind"], "github_app");
        assert_eq!(json["tokens"]["access_token"], "ghu_a");
        assert_eq!(serde_json::to_value(AuthKind::Pat).unwrap(), "pat");
    }

    #[test]
    fn file_store_round_trips() {
        let (dir, file) = credentials_file();
        let store = FileStore::new(&file);
        assert_eq!(store.load().unwrap(), None);

        store.save(&auth(AuthKind::Pat, "ghp_a")).unwrap();
        assert_eq!(store.load().unwrap(), Some(auth(AuthKind::Pat, "ghp_a")));
        store.save(&auth(AuthKind::Pat, "ghp_b")).unwrap();
        assert_eq!(store.load().unwrap(), Some(auth(AuthKind::Pat, "ghp_b")));

        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
        // Only the credentials file was ever there; no temp files are left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn file_store_writes_0600_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/mdgitjournal/credentials.json");
        FileStore::new(&path)
            .save(&auth(AuthKind::Pat, "ghp_a"))
            .unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn file_store_replaces_a_readable_file_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, file) = credentials_file();
        fs::write(&file, "{}").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        FileStore::new(&file)
            .save(&auth(AuthKind::Pat, "ghp_a"))
            .unwrap();
        assert_eq!(mode(&file), 0o600);
    }

    #[test]
    fn file_store_reports_unreadable_contents() {
        let (_dir, file) = credentials_file();
        fs::write(&file, "not json").unwrap();
        let err = FileStore::new(&file).load().unwrap_err();
        assert!(matches!(err, Error::Store(_)), "{err:?}");
        assert!(err
            .to_string()
            .starts_with("Couldn't access the saved GitHub sign-in: "));
    }

    #[test]
    fn keyring_store_uses_the_keyring_when_it_works() {
        let (_dir, file) = credentials_file();
        let fake = Arc::new(FakeKeyring::default());
        let store = keyring_store(&fake, &file);
        assert_eq!(store.backend(), "keychain");
        assert_eq!(store.load().unwrap(), None);

        store.save(&auth(AuthKind::GithubApp, "ghu_a")).unwrap();
        assert!(!file.exists());
        assert_eq!(fake.saved(), Some(auth(AuthKind::GithubApp, "ghu_a")));
        assert_eq!(
            store.load().unwrap(),
            Some(auth(AuthKind::GithubApp, "ghu_a"))
        );

        store.clear().unwrap();
        assert_eq!(fake.saved(), None);
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
        assert_eq!(store.backend(), "keychain");
    }

    #[cfg(unix)]
    #[test]
    fn keyring_store_falls_back_to_a_0600_file() {
        let (_dir, file) = credentials_file();
        let fake = FakeKeyring::broken();
        let store = keyring_store(&fake, &file);
        assert_eq!(store.backend(), "keychain");

        store.save(&auth(AuthKind::GithubApp, "ghu_a")).unwrap();
        assert_eq!(store.backend(), "file");
        assert_eq!(mode(&file), 0o600);
        store.save(&auth(AuthKind::GithubApp, "ghu_b")).unwrap();
        assert_eq!(
            store.load().unwrap(),
            Some(auth(AuthKind::GithubApp, "ghu_b"))
        );
        // One failed attempt, then the keyring is left alone.
        assert_eq!(fake.calls(), 1);

        store.clear().unwrap();
        assert!(!file.exists());
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn keyring_store_load_falls_back_to_the_file() {
        let (_dir, file) = credentials_file();
        FileStore::new(&file)
            .save(&auth(AuthKind::Pat, "ghp_a"))
            .unwrap();
        let store = keyring_store(&FakeKeyring::broken(), &file);
        assert_eq!(store.load().unwrap(), Some(auth(AuthKind::Pat, "ghp_a")));
        assert_eq!(store.backend(), "file");
        assert!(file.exists());
    }

    #[test]
    fn unreadable_keyring_counts_as_signed_out() {
        let (_dir, file) = credentials_file();
        let store = keyring_store(&FakeKeyring::broken(), &file);
        assert_eq!(store.load().unwrap(), None);
        assert_eq!(store.backend(), "file");
    }

    #[test]
    fn keyring_store_moves_the_file_into_the_keyring_once_it_works() {
        let (_dir, file) = credentials_file();
        FileStore::new(&file)
            .save(&auth(AuthKind::Pat, "ghp_a"))
            .unwrap();
        let fake = Arc::new(FakeKeyring::default());
        let store = keyring_store(&fake, &file);

        assert_eq!(store.load().unwrap(), Some(auth(AuthKind::Pat, "ghp_a")));
        assert!(!file.exists());
        assert_eq!(fake.saved(), Some(auth(AuthKind::Pat, "ghp_a")));
        assert_eq!(store.backend(), "keychain");
    }

    #[test]
    fn a_fallback_file_wins_over_the_older_keyring_copy() {
        // An earlier run couldn't save rotated tokens to the keyring, so they went to the
        // file; the keyring copy has a refresh token that no longer works.
        let (_dir, file) = credentials_file();
        FileStore::new(&file)
            .save(&auth(AuthKind::GithubApp, "ghu_new"))
            .unwrap();
        let fake = FakeKeyring::holding(&auth(AuthKind::GithubApp, "ghu_old"));
        let store = keyring_store(&fake, &file);

        assert_eq!(
            store.load().unwrap(),
            Some(auth(AuthKind::GithubApp, "ghu_new"))
        );
        assert_eq!(fake.saved(), Some(auth(AuthKind::GithubApp, "ghu_new")));
        assert!(!file.exists());
    }

    #[test]
    fn keyring_store_clear_removes_both_copies() {
        let (_dir, file) = credentials_file();
        FileStore::new(&file)
            .save(&auth(AuthKind::Pat, "ghp_file"))
            .unwrap();
        let fake = FakeKeyring::holding(&auth(AuthKind::Pat, "ghp_kr"));

        keyring_store(&fake, &file).clear().unwrap();
        assert!(!file.exists());
        assert_eq!(fake.saved(), None);
    }

    #[test]
    fn clear_after_a_keyring_failure_still_removes_the_keyring_copy() {
        let (_dir, file) = credentials_file();
        let fake = FakeKeyring::holding(&auth(AuthKind::GithubApp, "ghu_a"));
        let store = keyring_store(&fake, &file);
        fake.set_broken(true);
        store.save(&auth(AuthKind::GithubApp, "ghu_b")).unwrap();
        assert_eq!(store.backend(), "file");

        fake.set_broken(false);
        store.clear().unwrap();
        assert!(!file.exists());
        assert_eq!(fake.saved(), None);
    }

    #[test]
    fn clear_ignores_a_keyring_that_never_worked() {
        let (_dir, file) = credentials_file();
        let store = keyring_store(&FakeKeyring::broken(), &file);
        store.save(&auth(AuthKind::Pat, "ghp_a")).unwrap();
        store.clear().unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn clear_reports_a_keyring_copy_it_cant_remove() {
        let (_dir, file) = credentials_file();
        FileStore::new(&file)
            .save(&auth(AuthKind::Pat, "ghp_a"))
            .unwrap();
        let fake = FakeKeyring::holding(&auth(AuthKind::Pat, "ghp_a"));
        fake.set_broken(true);

        let err = keyring_store(&fake, &file).clear().unwrap_err();
        assert!(matches!(err, Error::Store(_)), "{err:?}");
        assert!(err.to_string().contains("keychain"), "{err}");
        assert!(!file.exists());
    }

    #[test]
    fn file_only_never_builds_the_keyring() {
        let (_dir, file) = credentials_file();
        let store = KeyringStore::file_only(&file);
        assert!(store.keyring.is_none());
        // "file" from the start, not only after a keyring failure.
        assert_eq!(store.backend(), "file");
        assert_eq!(store.load().unwrap(), None);
        store.save(&auth(AuthKind::Pat, "ghp_a")).unwrap();
        assert_eq!(store.load().unwrap(), Some(auth(AuthKind::Pat, "ghp_a")));
        #[cfg(unix)]
        assert_eq!(mode(&file), 0o600);
        assert_eq!(store.backend(), "file");
        store.clear().unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn keyring_store_without_a_keyring_uses_the_file() {
        let (_dir, file) = credentials_file();
        let store = KeyringStore::with_keyring(None, file.clone());
        assert_eq!(store.backend(), "file");
        store.save(&auth(AuthKind::Pat, "ghp_a")).unwrap();
        assert!(file.exists());
        assert_eq!(store.load().unwrap(), Some(auth(AuthKind::Pat, "ghp_a")));
        store.clear().unwrap();
        assert!(!file.exists());
    }
}
