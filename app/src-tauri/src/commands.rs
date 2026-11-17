//! The Tauri commands of docs/architecture.md: glue between the UI and the mdgj-* crates.

use std::borrow::Cow;

use chrono::Local;
use mdgj_core::{EntryContent, EntryMeta, ImportedImage, RemoteConfig, SearchHit, Settings};
use mdgj_git::{CommitPushOutcome, Conflict, PullOutcome, PushOutcome};
use mdgj_github::{AuthKind, DeviceCode, GitHub, Poll, RepoInfo, StoredAuth, Tokens};
use percent_encoding::percent_decode;
use serde::{Deserialize, Serialize};
use tauri::http::HeaderMap;
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::state::{emit, lock, token_creds, AppState, Backend, ENTRIES_CHANGED};
use crate::view::SyncView;

const SEARCH_LIMIT: usize = 200;

const PLATFORM: &str = if cfg!(target_os = "ios") {
    "ios"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

/// Runs `work` on a blocking thread, since all of it is file, git or network I/O.
async fn run<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &AppState) -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || {
        let backend = app.state::<Backend>();
        let state = backend.as_ref().map_err(Clone::clone)?;
        work(&app, state)
    })
    .await
    .map_err(Error::internal)?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    platform: &'static str,
    journal_dir: String,
    token_backend: &'static str,
    client_id_placeholder: bool,
}

#[tauri::command]
pub async fn app_info(app: AppHandle) -> Result<AppInfo> {
    run(app, |app, s| {
        // Whether the keyring works is only found out on first use.
        let _ = s.stored_auth();
        Ok(AppInfo {
            version: app.package_info().version.to_string(),
            platform: PLATFORM,
            journal_dir: s.journal.root().to_string_lossy().into_owned(),
            token_backend: s.tokens.store().backend(),
            client_id_placeholder: mdgj_github::is_placeholder(),
        })
    })
    .await
}

#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Result<Settings> {
    run(app, |_, s| Ok(s.settings())).await
}

/// `settings.remote` is ignored: only `github_connect` and `github_disconnect` change it.
#[tauri::command]
pub async fn set_settings(app: AppHandle, settings: Settings) -> Result<Settings> {
    run(app, move |_, s| s.save_settings(settings)).await
}

// Entries

#[tauri::command]
pub async fn list_entries(app: AppHandle) -> Result<Vec<EntryMeta>> {
    run(app, |_, s| Ok(s.journal.list()?)).await
}

#[tauri::command]
pub async fn read_entry(app: AppHandle, file: String) -> Result<EntryContent> {
    run(app, move |_, s| Ok(s.journal.read(&file)?)).await
}

#[derive(Serialize)]
pub struct Written {
    hash: String,
}

#[tauri::command]
pub async fn write_entry(app: AppHandle, file: String, content: String) -> Result<Written> {
    run(app, move |_, s| {
        let hash = s.journal.write(&file, &content)?;
        Ok(Written { hash })
    })
    .await
}

#[tauri::command]
pub async fn create_entry(app: AppHandle, title: String) -> Result<EntryMeta> {
    run(app, move |_, s| {
        Ok(s.journal.create(&title, Local::now().naive_local())?)
    })
    .await
}

#[tauri::command]
pub async fn rename_entry(app: AppHandle, file: String, new_title: String) -> Result<EntryMeta> {
    run(app, move |_, s| Ok(s.journal.rename(&file, &new_title)?)).await
}

#[tauri::command]
pub async fn delete_entry(app: AppHandle, file: String) -> Result<()> {
    run(app, move |_, s| Ok(s.journal.delete(&file)?)).await
}

#[tauri::command]
pub async fn search(app: AppHandle, query: String) -> Result<Vec<SearchHit>> {
    run(app, move |_, s| {
        Ok(s.journal.search(&query, SEARCH_LIMIT)?)
    })
    .await
}

/// The image is the raw request body; the entry's file name, percent-encoded, is in the
/// `x-entry-file` header.
#[tauri::command]
pub async fn import_image(app: AppHandle, request: Request<'_>) -> Result<ImportedImage> {
    let file = entry_file(request.headers())?;
    let bytes = image_bytes(request.body())?;
    run(app, move |_, s| {
        Ok(s.journal.import_image(&file, &bytes, s.image_opts())?)
    })
    .await
}

fn entry_file(headers: &HeaderMap) -> Result<String> {
    let value = headers
        .get("x-entry-file")
        .ok_or_else(|| Error::invalid("Open an entry before adding images."))?;
    percent_decode(value.as_bytes())
        .decode_utf8()
        .map(Cow::into_owned)
        .map_err(|_| Error::invalid("The entry's file name is not valid UTF-8."))
}

fn image_bytes(body: &InvokeBody) -> Result<Vec<u8>> {
    match body {
        InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        // Tauri's fallback IPC, over postMessage, sends the bytes as a JSON array.
        InvokeBody::Json(json) => {
            Vec::deserialize(json).map_err(|_| Error::invalid("No image data arrived."))
        }
    }
}

// Sync

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullResult {
    view: SyncView,
    updated: Vec<String>,
    merged: Vec<String>,
    conflicts: Vec<Conflict>,
}

/// The result of a pull or connect; tells the UI to re-list entries if files changed.
fn incoming(app: &AppHandle, view: SyncView, outcome: PullOutcome) -> PullResult {
    if !outcome.updated.is_empty() {
        emit(app, ENTRIES_CHANGED, ());
    }
    PullResult {
        view,
        updated: outcome.updated,
        merged: outcome.merged,
        conflicts: outcome.conflicts,
    }
}

#[tauri::command]
pub async fn sync_state(app: AppHandle) -> Result<SyncView> {
    run(app, |_, s| s.sync_view()).await
}

/// Does nothing but report the state when no remote is set.
#[tauri::command]
pub async fn fetch(app: AppHandle) -> Result<SyncView> {
    run(app, |app, s| {
        let (view, ()) = s.sync(app, |repo| {
            let state = match repo.remote_url() {
                Some(_) => repo.fetch(s.creds(repo)?.as_ref())?,
                None => repo.sync_state()?,
            };
            Ok((state, ()))
        })?;
        Ok(view)
    })
    .await
}

#[tauri::command]
pub async fn pull(app: AppHandle) -> Result<PullResult> {
    run(app, |app, s| {
        let (view, outcome) = s.sync(app, |repo| {
            let outcome = repo.pull(s.creds(repo)?.as_ref(), &s.author(true))?;
            Ok((outcome.state.clone(), outcome))
        })?;
        Ok(incoming(app, view, outcome))
    })
    .await
}

/// A commit on this device only, as used when no remote is connected.
#[tauri::command]
pub async fn commit(app: AppHandle, message: String) -> Result<SyncView> {
    run(app, move |app, s| {
        let (view, ()) = s.sync(app, |repo| {
            repo.commit_all(&message, &s.author(false))?;
            Ok((repo.sync_state()?, ()))
        })?;
        Ok(view)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitPush {
    Pushed,
    NeedsPull,
    Nothing,
}

#[derive(Serialize)]
pub struct CommitPushResult {
    view: SyncView,
    result: CommitPush,
}

/// With a blank message and nothing to commit, pushes the local commits that are waiting.
#[tauri::command]
pub async fn commit_and_push(app: AppHandle, message: String) -> Result<CommitPushResult> {
    run(app, move |app, s| {
        let (view, result) = s.sync(app, |repo| {
            let creds = s.creds(repo)?;
            if message.trim().is_empty() && repo.status()?.is_empty() {
                return Ok(match repo.push(creds.as_ref())? {
                    PushOutcome::Pushed { state } => (state, CommitPush::Pushed),
                    PushOutcome::Rejected { state } => (state, CommitPush::NeedsPull),
                    PushOutcome::NothingToPush { state } => (state, CommitPush::Nothing),
                });
            }
            let outcome = repo.commit_and_push(&message, &s.author(true), creds.as_ref())?;
            Ok(match outcome {
                CommitPushOutcome::Pushed { state } => (state, CommitPush::Pushed),
                CommitPushOutcome::NeedsPull { state } => (state, CommitPush::NeedsPull),
                CommitPushOutcome::NothingToCommit { state } => (state, CommitPush::Nothing),
            })
        })?;
        Ok(CommitPushResult { view, result })
    })
    .await
}

// GitHub

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    signed_in: bool,
    kind: Option<AuthKind>,
    login: Option<String>,
}

#[tauri::command]
pub async fn auth_status(app: AppHandle) -> Result<AuthStatus> {
    run(app, |_, s| {
        Ok(match s.stored_auth() {
            Ok(Some(auth)) => AuthStatus {
                signed_in: true,
                kind: Some(auth.kind),
                login: auth.login,
            },
            // An unreadable sign-in must not keep the journal from opening; signing in again
            // replaces it.
            Ok(None) | Err(_) => AuthStatus {
                signed_in: false,
                kind: None,
                login: None,
            },
        })
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[tauri::command]
pub async fn github_device_start(app: AppHandle) -> Result<DeviceStart> {
    run(app, |_, s| {
        let code = s.github().start_device_flow()?;
        let start = DeviceStart {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
            expires_in: code.expires_in,
            interval: code.interval,
        };
        *lock(&s.device) = Some(code);
        Ok(start)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Pending,
    SlowDown,
    Complete,
    Expired,
    Denied,
}

#[derive(Serialize)]
pub struct DevicePoll {
    status: DeviceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    login: Option<String>,
}

#[tauri::command]
pub async fn github_device_poll(app: AppHandle) -> Result<DevicePoll> {
    run(app, |_, s| {
        let code = lock(&s.device)
            .clone()
            .ok_or_else(|| Error::invalid("Start signing in with GitHub first."))?;
        let same = |pending: &DeviceCode| pending.device_code == code.device_code;
        let status = match s.github().poll_device_flow(&code)? {
            Poll::Pending => DeviceStatus::Pending,
            Poll::SlowDown { interval } => {
                if let Some(pending) = lock(&s.device).as_mut().filter(|p| same(p)) {
                    pending.interval = interval;
                }
                DeviceStatus::SlowDown
            }
            Poll::Expired => {
                lock(&s.device).take_if(|p| same(p));
                DeviceStatus::Expired
            }
            Poll::Denied => {
                lock(&s.device).take_if(|p| same(p));
                DeviceStatus::Denied
            }
            Poll::Complete(tokens) => {
                // The code is single-use, so a failed lookup of the name must not lose the sign-in.
                let user = s.github().user(&tokens.access_token).ok();
                let login = user.as_ref().map(|u| u.login.clone());
                let auth = StoredAuth {
                    kind: AuthKind::GithubApp,
                    tokens,
                    login: login.clone(),
                };
                s.sign_in(&auth, user)?;
                return Ok(DevicePoll {
                    status: DeviceStatus::Complete,
                    login,
                });
            }
        };
        Ok(DevicePoll {
            status,
            login: None,
        })
    })
    .await
}

#[derive(Serialize)]
pub struct Login {
    login: String,
}

/// Signs in with a personal access token, after checking it with GitHub.
#[tauri::command]
pub async fn github_set_pat(app: AppHandle, token: String) -> Result<Login> {
    run(app, move |_, s| {
        let token = token.trim();
        if token.is_empty() {
            return Err(Error::invalid("Paste a token first."));
        }
        let user = s.github().user(token)?;
        let login = user.login.clone();
        let auth = StoredAuth {
            kind: AuthKind::Pat,
            tokens: Tokens {
                access_token: token.to_owned(),
                expires_at: None,
                refresh_token: None,
                refresh_expires_at: None,
            },
            login: Some(login.clone()),
        };
        s.sign_in(&auth, Some(user))?;
        Ok(Login { login })
    })
    .await
}

/// The repositories the GitHub App is installed on; none for a token, whose user types the name.
#[tauri::command]
pub async fn github_list_repos(app: AppHandle) -> Result<Vec<RepoInfo>> {
    run(app, |_, s| match s.signed_in()?.kind {
        AuthKind::Pat => Ok(Vec::new()),
        AuthKind::GithubApp => Ok(s.github().installation_repos(&s.token()?)?),
    })
    .await
}

#[tauri::command]
pub async fn github_connect(app: AppHandle, full_name: String) -> Result<PullResult> {
    run(app, move |app, s| {
        let (view, outcome) = s.sync(app, |repo| {
            let kind = s.signed_in()?.kind;
            let token = s.token()?;
            let full_name = full_name.trim();
            let info = match kind {
                AuthKind::GithubApp => s
                    .github()
                    .installation_repos(&token)?
                    .into_iter()
                    .find(|r| r.full_name.eq_ignore_ascii_case(full_name))
                    .ok_or_else(|| {
                        Error::invalid(format!(
                            "mdgitjournal isn't installed on {full_name}. Install it on that \
                             repository, then try again."
                        ))
                    })?,
                AuthKind::Pat => s.github().repo(&token, full_name)?,
            };
            let outcome =
                repo.connect(&info.clone_url, Some(&token_creds(token)), &s.author(true))?;
            s.set_remote(Some(RemoteConfig {
                full_name: info.full_name,
                url: info.clone_url,
                branch: outcome.state.branch.clone(),
                auth: match kind {
                    AuthKind::GithubApp => mdgj_core::AuthKind::GithubApp,
                    AuthKind::Pat => mdgj_core::AuthKind::Pat,
                },
            }))?;
            Ok((outcome.state.clone(), outcome))
        })?;
        Ok(incoming(app, view, outcome))
    })
    .await
}

/// Removes the remote; the journal and its history stay. `sign_out` also forgets the sign-in.
#[tauri::command]
pub async fn github_disconnect(app: AppHandle, sign_out: bool) -> Result<SyncView> {
    run(app, move |app, s| {
        let (view, ()) = s.sync(app, |repo| {
            repo.remove_remote()?;
            s.set_remote(None)?;
            if sign_out {
                s.sign_out()?;
            }
            Ok((repo.sync_state()?, ()))
        })?;
        Ok(view)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubUrls {
    new_repo: String,
    install: String,
}

#[tauri::command]
pub async fn github_urls(app: AppHandle) -> Result<GithubUrls> {
    run(app, |_, s| {
        Ok(GithubUrls {
            new_repo: GitHub::new_repo_url("journal"),
            install: s.github().install_url(),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tauri::http::HeaderValue;

    use super::*;
    use crate::error::Kind;

    #[test]
    fn entry_file_is_percent_decoded() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-entry-file",
            HeaderValue::from_static("My%20notes%20%E2%80%94%20caf%C3%A9.md"),
        );
        assert_eq!(entry_file(&headers).unwrap(), "My notes — café.md");

        headers.insert(
            "x-entry-file",
            HeaderValue::from_static("2026-11-16-1402-run.md"),
        );
        assert_eq!(entry_file(&headers).unwrap(), "2026-11-16-1402-run.md");
    }

    #[test]
    fn entry_file_is_required_and_utf8() {
        assert_eq!(
            entry_file(&HeaderMap::new()).unwrap_err().kind,
            Kind::Invalid
        );
        let mut headers = HeaderMap::new();
        headers.insert("x-entry-file", HeaderValue::from_static("%FF.md"));
        assert_eq!(entry_file(&headers).unwrap_err().kind, Kind::Invalid);
    }

    #[test]
    fn image_bytes_raw_or_json_array() {
        let bytes = vec![0xff, 0xd8, 0xff, 0x00];
        assert_eq!(image_bytes(&InvokeBody::Raw(bytes.clone())).unwrap(), bytes);
        assert_eq!(
            image_bytes(&InvokeBody::Json(json!([255, 216, 255, 0]))).unwrap(),
            bytes
        );
        for json in [json!(null), json!({ "bytes": [1] }), json!([256])] {
            let err = image_bytes(&InvokeBody::Json(json)).unwrap_err();
            assert_eq!(err.kind, Kind::Invalid);
        }
    }

    #[test]
    fn response_shapes() {
        let result = serde_json::to_value(CommitPush::NeedsPull).unwrap();
        assert_eq!(result, json!("needs_pull"));
        let poll = DevicePoll {
            status: DeviceStatus::SlowDown,
            login: None,
        };
        assert_eq!(
            serde_json::to_value(poll).unwrap(),
            json!({ "status": "slow_down" })
        );
        let status = AuthStatus {
            signed_in: true,
            kind: Some(AuthKind::GithubApp),
            login: Some("dcxSt".into()),
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({ "signedIn": true, "kind": "github_app", "login": "dcxSt" })
        );
    }
}
