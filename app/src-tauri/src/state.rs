//! What the commands share: the journal, its git repository, settings and the GitHub sign-in.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use mdgj_core::{ImageOpts, Journal, RemoteConfig, Settings};
use mdgj_git::{Author, Creds, Repo, SyncState};
use mdgj_github::{
    DeviceCode, GitHub, Http, KeyringStore, ReqwestHttp, StoredAuth, TokenManager, TokenStore, User,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{Error, Kind, Result};
use crate::view::SyncView;

pub const SYNC_STATE: &str = "sync://state";
pub const ENTRIES_CHANGED: &str = "entries://changed";

/// The only host the GitHub token is ever sent to.
const GITHUB: &str = "https://github.com/";

/// The app state, or why it couldn't be set up, which every command then reports.
pub type Backend = Result<AppState>;

pub struct AppState {
    pub journal: Journal,
    /// Held for every git operation, so they never overlap.
    repo: Mutex<Repo>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    pub tokens: TokenManager<KeyringStore, LazyHttp>,
    /// The device-flow sign-in in progress.
    pub device: Mutex<Option<DeviceCode>>,
    /// The signed-in GitHub account, for the commit author.
    user: Mutex<Option<User>>,
    /// Why the last sync operation failed, until one succeeds.
    last_error: Mutex<Option<Error>>,
}

impl AppState {
    pub fn open(app: &AppHandle) -> Result<Self> {
        let journal = Journal::open(journal_dir(app)?)?;
        let repo = Repo::open_or_init(journal.root())?;
        // Only a safeguard: a journal that can't take it still opens.
        let _ = exclude_temp_files(journal.root());
        app.asset_protocol_scope()
            .allow_directory(journal.root().join("assets"), true)
            .map_err(|err| Error::io("could not allow image previews", err))?;
        let config = app
            .path()
            .app_config_dir()
            .map_err(|err| Error::io("could not find the settings folder", err))?;
        let settings_path = config.join("settings.json");
        let store = KeyringStore::new(config.join("credentials.json"));
        Ok(Self {
            journal,
            repo: Mutex::new(repo),
            settings: Mutex::new(Settings::load(&settings_path)),
            settings_path,
            tokens: TokenManager::new(store, GitHub::new(LazyHttp::default())),
            device: Mutex::default(),
            user: Mutex::default(),
            last_error: Mutex::default(),
        })
    }

    pub fn github(&self) -> &GitHub<LazyHttp> {
        self.tokens.github()
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    /// Saves `new`, except for `remote`, which only connecting and disconnecting change.
    pub fn save_settings(&self, new: Settings) -> Result<Settings> {
        self.update_settings(|settings| {
            *settings = Settings {
                remote: settings.remote.take(),
                ..new
            }
        })
    }

    pub fn set_remote(&self, remote: Option<RemoteConfig>) -> Result<()> {
        self.update_settings(|settings| settings.remote = remote)
            .map(drop)
    }

    fn update_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut settings = lock(&self.settings);
        let mut new = settings.clone();
        change(&mut new);
        new.save(&self.settings_path)?;
        *settings = new.clone();
        Ok(new)
    }

    pub fn image_opts(&self) -> ImageOpts {
        let settings = lock(&self.settings);
        ImageOpts {
            max_edge: settings.image_max_edge,
            quality: settings.image_quality,
        }
    }

    pub fn stored_auth(&self) -> Result<Option<StoredAuth>> {
        Ok(self.tokens.store().load()?)
    }

    /// The saved sign-in, or an error asking for one.
    pub fn signed_in(&self) -> Result<StoredAuth> {
        self.stored_auth()?.ok_or_else(not_signed_in)
    }

    /// The access token, refreshed first if it is about to expire.
    pub fn token(&self) -> Result<String> {
        self.tokens.access_token()?.ok_or_else(not_signed_in)
    }

    pub fn sign_in(&self, auth: &StoredAuth, user: Option<User>) -> Result<()> {
        self.tokens.save(auth)?;
        *lock(&self.user) = user;
        *lock(&self.device) = None;
        Ok(())
    }

    pub fn sign_out(&self) -> Result<()> {
        self.tokens.clear()?;
        *lock(&self.user) = None;
        *lock(&self.device) = None;
        Ok(())
    }

    /// The sync state, from local data only.
    pub fn sync_view(&self) -> Result<SyncView> {
        let repo = lock(&self.repo);
        let state = repo.sync_state()?;
        Ok(self.view(&repo, state))
    }

    /// Runs a git operation that changes or checks the sync state, remembers whether it failed,
    /// and sends the UI the new state either way.
    pub fn sync<T>(
        &self,
        app: &AppHandle,
        op: impl FnOnce(&Repo) -> Result<(SyncState, T)>,
    ) -> Result<(SyncView, T)> {
        let repo = lock(&self.repo);
        match op(&repo) {
            Ok((state, value)) => {
                *lock(&self.last_error) = None;
                let view = self.view(&repo, state);
                emit(app, SYNC_STATE, &view);
                Ok((view, value))
            }
            Err(err) => {
                // An empty commit message is for the commit sheet to show, not a sync failure.
                if err.kind != Kind::EmptyMessage {
                    *lock(&self.last_error) = Some(err.clone());
                }
                if let Ok(state) = repo.sync_state() {
                    emit(app, SYNC_STATE, &self.view(&repo, state));
                }
                Err(err)
            }
        }
    }

    fn view(&self, repo: &Repo, state: SyncState) -> SyncView {
        let settings = lock(&self.settings);
        let error = lock(&self.last_error).clone();
        SyncView::new(
            state,
            repo.remote_url().as_deref(),
            settings.remote.as_ref(),
            error,
        )
    }

    /// Credentials for origin: the GitHub token, and only if origin is on github.com.
    pub fn creds(&self, repo: &Repo) -> Result<Option<Creds>> {
        if !repo.remote_url().is_some_and(|url| url.starts_with(GITHUB)) {
            return Ok(None);
        }
        Ok(self.tokens.access_token()?.map(token_creds))
    }

    /// See [`author`]. `online` allows looking up the GitHub account, for operations that go to
    /// the network anyway; if that fails, the commit gets the local placeholder.
    pub fn author(&self, online: bool) -> Author {
        let settings = self.settings();
        let mut user = lock(&self.user);
        let from_settings = non_blank(settings.author_name.as_deref()).is_some()
            && non_blank(settings.author_email.as_deref()).is_some();
        if online && !from_settings && user.is_none() {
            *user = self
                .tokens
                .access_token()
                .ok()
                .flatten()
                .and_then(|token| self.github().user(&token).ok());
        }
        author(&settings, user.as_ref())
    }
}

/// `MDGJ_JOURNAL_DIR`, else `journal/` in the app's data folder; on iOS, in Documents, which the
/// Files app shows.
fn journal_dir(app: &AppHandle) -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("MDGJ_JOURNAL_DIR").filter(|dir| !dir.is_empty()) {
        // Absolute, because the UI turns it into image URLs.
        return std::path::absolute(dir)
            .map_err(|err| Error::io("MDGJ_JOURNAL_DIR is not a usable folder", err));
    }
    #[cfg(target_os = "ios")]
    let base = app.path().document_dir();
    #[cfg(not(target_os = "ios"))]
    let base = app.path().app_data_dir();
    let base = base.map_err(|err| Error::io("could not find the app's data folder", err))?;
    Ok(base.join("journal"))
}

/// mdgj-core saves through `.mdgj-tmp-*` files, which a crash can leave behind. Listing them in
/// `.git/info/exclude`, which belongs to this clone and is never committed, keeps them out of
/// commits without changing the journal.
fn exclude_temp_files(root: &Path) -> io::Result<()> {
    const PATTERN: &str = ".mdgj-tmp-*";
    let git = root.join(".git");
    // `.git` is a file in linked worktrees and submodules; leave those alone.
    if !git.is_dir() {
        return Ok(());
    }
    let info = git.join("info");
    let path = info.join("exclude");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    if existing.lines().any(|line| line.trim() == PATTERN) {
        return Ok(());
    }
    let newline = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::create_dir_all(&info)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(format!("{newline}{PATTERN}\n").as_bytes())
}

/// The commit author: the name and email in settings, else the GitHub account's name and no-reply
/// address, else a local placeholder. Each falls back on its own.
pub fn author(settings: &Settings, user: Option<&User>) -> Author {
    let github_name = user.map(|u| non_blank(u.name.as_deref()).unwrap_or(&u.login));
    let github_email = user.map(|u| format!("{}+{}@users.noreply.github.com", u.id, u.login));
    Author {
        name: non_blank(settings.author_name.as_deref())
            .or(github_name)
            .unwrap_or("mdgitjournal")
            .to_owned(),
        email: non_blank(settings.author_email.as_deref())
            .map(str::to_owned)
            .or(github_email)
            .unwrap_or_else(|| "mdgitjournal@localhost".to_owned()),
    }
}

fn non_blank(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

pub fn token_creds(token: String) -> Creds {
    Creds {
        username: "x-access-token".into(),
        password: token,
    }
}

fn not_signed_in() -> Error {
    Error::new(Kind::Auth, "Sign in to GitHub first.")
}

/// Events are best effort: sending fails only when the window is gone.
pub fn emit(app: &AppHandle, event: &str, payload: impl Serialize + Clone) {
    let _ = app.emit(event, payload);
}

/// The values behind these locks stay usable after a panic elsewhere, so poisoning is ignored.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// [`ReqwestHttp`], built on first use: setting up TLS must not keep the journal from opening,
/// and this way it happens on the blocking thread of the command that needs it.
#[derive(Default)]
pub struct LazyHttp(OnceLock<ReqwestHttp>);

impl LazyHttp {
    fn client(&self) -> mdgj_github::Result<&ReqwestHttp> {
        if let Some(http) = self.0.get() {
            return Ok(http);
        }
        let http = ReqwestHttp::new()?;
        Ok(self.0.get_or_init(|| http))
    }
}

impl Http for LazyHttp {
    fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> mdgj_github::Result<(u16, String)> {
        self.client()?.post_form(url, form, headers)
    }

    fn get(&self, url: &str, headers: &[(&str, &str)]) -> mdgj_github::Result<(u16, String)> {
        self.client()?.get(url, headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(name: Option<&str>) -> User {
        User {
            login: "dcxSt".into(),
            id: 4242,
            name: name.map(str::to_owned),
            email: Some("public@example.com".into()),
        }
    }

    fn settings(name: Option<&str>, email: Option<&str>) -> Settings {
        Settings {
            author_name: name.map(str::to_owned),
            author_email: email.map(str::to_owned),
            ..Settings::default()
        }
    }

    fn pair(author: Author) -> (String, String) {
        (author.name, author.email)
    }

    #[test]
    fn author_from_settings_wins() {
        let s = settings(Some(" Stephen Fay "), Some("me@stephenfay.xyz"));
        assert_eq!(
            pair(author(&s, Some(&user(Some("Someone"))))),
            ("Stephen Fay".into(), "me@stephenfay.xyz".into())
        );
    }

    #[test]
    fn author_from_github_uses_the_noreply_address() {
        let s = settings(None, None);
        assert_eq!(
            pair(author(&s, Some(&user(Some("Stephen Fay"))))),
            (
                "Stephen Fay".into(),
                "4242+dcxSt@users.noreply.github.com".into()
            )
        );
        // No display name on GitHub: the login stands in.
        assert_eq!(author(&s, Some(&user(Some("  ")))).name, "dcxSt");
        assert_eq!(author(&s, Some(&user(None))).name, "dcxSt");
    }

    #[test]
    fn author_fields_fall_back_separately() {
        let s = settings(Some("Stephen"), Some(" "));
        assert_eq!(
            pair(author(&s, Some(&user(None)))),
            (
                "Stephen".into(),
                "4242+dcxSt@users.noreply.github.com".into()
            )
        );
        let s = settings(None, Some("me@stephenfay.xyz"));
        assert_eq!(
            pair(author(&s, None)),
            ("mdgitjournal".into(), "me@stephenfay.xyz".into())
        );
    }

    #[test]
    fn author_placeholder_when_offline_and_unset() {
        assert_eq!(
            pair(author(&settings(Some(""), None), None)),
            ("mdgitjournal".into(), "mdgitjournal@localhost".into())
        );
    }

    #[test]
    fn token_creds_use_the_github_convention() {
        let creds = token_creds("ghu_secret".into());
        assert_eq!(
            (creds.username.as_str(), creds.password.as_str()),
            ("x-access-token", "ghu_secret")
        );
    }

    #[test]
    fn temp_files_are_excluded_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Repo::open_or_init(root).unwrap();
        let exclude = root.join(".git/info/exclude");
        fs::write(&exclude, "# user rules\n*.swp").unwrap();
        exclude_temp_files(root).unwrap();
        exclude_temp_files(root).unwrap();
        assert_eq!(
            fs::read_to_string(&exclude).unwrap(),
            "# user rules\n*.swp\n.mdgj-tmp-*\n"
        );

        // A temp file left by a crash, at the top and in assets/, is never staged.
        fs::write(root.join(".mdgj-tmp-a1b2c3"), "half a save").unwrap();
        fs::create_dir(root.join("assets")).unwrap();
        fs::write(root.join("assets/.mdgj-tmp-d4e5f6"), "half an image").unwrap();
        fs::write(root.join("2026-11-16-1402-run.md"), "ran\n").unwrap();
        let repo = Repo::open_or_init(root).unwrap();
        let changed: Vec<_> = repo.status().unwrap().into_iter().map(|c| c.path).collect();
        assert_eq!(changed, ["2026-11-16-1402-run.md"]);
    }

    #[test]
    fn temp_files_exclude_is_created_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Repo::open_or_init(root).unwrap();
        fs::remove_dir_all(root.join(".git/info")).unwrap();
        exclude_temp_files(root).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(".git/info/exclude")).unwrap(),
            ".mdgj-tmp-*\n"
        );
    }

    #[test]
    fn linked_worktrees_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
        exclude_temp_files(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(".git")).unwrap(),
            "gitdir: /elsewhere\n"
        );
    }
}
