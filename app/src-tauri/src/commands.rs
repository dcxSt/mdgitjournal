//! The Tauri commands of docs/architecture.md: glue between the UI and the mdgj-* crates.

use std::borrow::Cow;

use chrono::Local;
use mdgj_core::{EntryContent, EntryMeta, ImportedImage, RemoteConfig, SearchHit, Settings};
use mdgj_git::{
    CommitPushOutcome, FileDiff, PullOutcome, PullPreview, PushOutcome, Resolved, SyncSummary,
};
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

/// What a pull (or connecting) did: nothing, applied at once (one-sided changes only), or a
/// review the user decides, with nothing written yet.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PullResult {
    UpToDate {
        view: SyncView,
    },
    Applied {
        view: SyncView,
        summary: SyncSummary,
    },
    Review {
        view: SyncView,
        preview: PullPreview,
    },
}

/// The result of a pull or connect, its state already the view; tells the UI to re-list entries
/// if files changed.
fn pull_result(app: &AppHandle, view: SyncView, outcome: PullOutcome) -> PullResult {
    match outcome {
        PullOutcome::UpToDate { .. } => PullResult::UpToDate { view },
        PullOutcome::Applied { summary, .. } => {
            if !summary.files.is_empty() {
                emit(app, ENTRIES_CHANGED, ());
            }
            PullResult::Applied { view, summary }
        }
        PullOutcome::Review { preview, .. } => PullResult::Review { view, preview },
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

/// Fetches, then applies at once only what changed on one side; anything else is a review.
#[tauri::command]
pub async fn pull(app: AppHandle) -> Result<PullResult> {
    run(app, |app, s| {
        let (view, outcome) = s.sync(app, |repo| {
            let outcome = repo.pull(s.creds(repo)?.as_ref(), &s.author(true))?;
            Ok((outcome.state().clone(), outcome))
        })?;
        Ok(pull_result(app, view, outcome))
    })
    .await
}

/// The review of what was last fetched. Local only, and never applies, even when Pull would:
/// the UI asks for it when a review went stale, where "nothing written" must hold.
#[tauri::command]
pub async fn pull_preview(app: AppHandle) -> Result<PullResult> {
    run(app, |app, s| {
        let (view, preview) = s.sync(app, |repo| {
            let preview = repo.preview_pull()?;
            Ok((repo.sync_state()?, preview))
        })?;
        Ok(match preview {
            None => PullResult::UpToDate { view },
            Some(preview) => PullResult::Review { view, preview },
        })
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct Applied {
    view: SyncView,
    summary: SyncSummary,
}

/// Writes the review's choices. `stale_review` (nothing written) if anything moved since.
#[tauri::command]
pub async fn apply_pull(
    app: AppHandle,
    id: String,
    resolutions: Vec<Resolved>,
    message: Option<String>,
) -> Result<Applied> {
    run(app, move |app, s| {
        let (view, summary) = s.sync(app, |repo| {
            let summary =
                repo.apply_pull(&id, &resolutions, message.as_deref(), &s.author(false))?;
            Ok((repo.sync_state()?, summary))
        })?;
        if !summary.files.is_empty() {
            emit(app, ENTRIES_CHANGED, ());
        }
        Ok(Applied { view, summary })
    })
    .await
}

/// Paths a command put back as they were.
#[derive(Debug, Serialize)]
pub struct Restored {
    view: SyncView,
    restored: Vec<String>,
}

fn restored(app: &AppHandle, view: SyncView, restored: Vec<String>) -> Restored {
    if !restored.is_empty() {
        emit(app, ENTRIES_CHANGED, ());
    }
    Restored { view, restored }
}

/// Reverses the last pull, or refuses (`undo_refused`) rather than overwrite newer text.
#[tauri::command]
pub async fn undo_sync(app: AppHandle) -> Result<Restored> {
    run(app, |app, s| {
        let (view, paths) = s.sync(app, |repo| {
            let paths = repo.undo_last_sync()?;
            Ok((repo.sync_state()?, paths))
        })?;
        Ok(restored(app, view, paths))
    })
    .await
}

/// Resolve with git: leaves a standard merge in progress, with conflict markers, for the git CLI.
/// Uncommitted edits are committed first with `message`.
#[tauri::command]
pub async fn start_external_merge(
    app: AppHandle,
    id: String,
    message: Option<String>,
) -> Result<SyncView> {
    run(app, move |app, s| {
        let (view, ()) = s.sync(app, |repo| {
            repo.start_external_merge(&id, message.as_deref(), &s.author(false))?;
            Ok((repo.sync_state()?, ()))
        })?;
        // It merged GitHub's changes into the working tree.
        emit(app, ENTRIES_CHANGED, ());
        Ok(view)
    })
    .await
}

/// What Abort merge did: the paths put back, and whether what `git merge --autostash` set aside
/// had to stay a git stash.
#[derive(Debug, Serialize)]
pub struct Aborted {
    view: SyncView,
    restored: Vec<String>,
    stashed: bool,
}

/// `git merge --abort`, and the commit Resolve with git made first is undone. `invalid` (nothing
/// changed) if a file git merged was edited since.
#[tauri::command]
pub async fn abort_merge(app: AppHandle) -> Result<Aborted> {
    run(app, |app, s| {
        let (view, aborted) = s.sync(app, |repo| {
            let aborted = repo.abort_merge()?;
            Ok((repo.sync_state()?, aborted))
        })?;
        let Restored { view, restored } = restored(app, view, aborted.restored);
        Ok(Aborted {
            view,
            restored,
            stashed: aborted.stashed,
        })
    })
    .await
}

/// A commit of the ticked `paths`, on this device only, as used when no remote is connected.
#[tauri::command]
pub async fn commit(app: AppHandle, message: String, paths: Vec<String>) -> Result<SyncView> {
    run(app, move |app, s| {
        let (view, ()) = s.sync(app, |repo| {
            repo.commit(&message, &paths, &s.author(false))?;
            Ok((repo.sync_state()?, ()))
        })?;
        Ok(view)
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitPush {
    Pushed,
    NeedsPull,
    Nothing,
}

#[derive(Debug, Serialize)]
pub struct CommitPushResult {
    view: SyncView,
    result: CommitPush,
}

/// Commits the ticked `paths` and pushes. A blank message with no paths only pushes the local
/// commits that are waiting.
#[tauri::command]
pub async fn commit_and_push(
    app: AppHandle,
    message: String,
    paths: Vec<String>,
) -> Result<CommitPushResult> {
    run(app, move |app, s| {
        let (view, result) = s.sync(app, |repo| {
            let creds = s.creds(repo)?;
            if message.trim().is_empty() && paths.is_empty() {
                return Ok(match repo.push(creds.as_ref())? {
                    PushOutcome::Pushed { state } => (state, CommitPush::Pushed),
                    PushOutcome::Rejected { state } => (state, CommitPush::NeedsPull),
                    PushOutcome::NothingToPush { state } => (state, CommitPush::Nothing),
                });
            }
            let outcome =
                repo.commit_and_push(&message, &paths, &s.author(true), creds.as_ref())?;
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

/// One changed file against HEAD, for the commit sheet.
#[tauri::command]
pub async fn diff(app: AppHandle, path: String) -> Result<FileDiff> {
    run(app, move |_, s| s.diff(&path)).await
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
            let state = outcome.state().clone();
            // A review is no failure: the journal is connected, and Pull is lit.
            s.set_remote(Some(RemoteConfig {
                full_name: info.full_name,
                url: info.clone_url,
                branch: state.branch.clone(),
                auth: match kind {
                    AuthKind::GithubApp => mdgj_core::AuthKind::GithubApp,
                    AuthKind::Pat => mdgj_core::AuthKind::Pat,
                },
            }))?;
            Ok((state, outcome))
        })?;
        Ok(pull_result(app, view, outcome))
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

/// The licences of everything compiled into the app (Settings › About). Built into the binary,
/// so every build has them, including the AUR's, which installs only the executable.
#[tauri::command]
pub async fn third_party_notices() -> &'static str {
    include_str!("../../../packaging/THIRD-PARTY-NOTICES.md")
}

#[cfg(test)]
mod tests {
    use mdgj_git::{
        ChangeKind, Choice, CommitInfo, DiffHunk, DiffLine, FileChange, GitMerge, LineKind,
        MergeChunk, MergeSuggestion, Overlap, OverlapKind, PullPlan, Resolution, SyncState,
    };
    use serde_json::{json, Value};
    use tauri::http::HeaderValue;

    use super::*;
    use crate::error::Kind;

    #[test]
    fn third_party_notices_are_built_in() {
        let notices = tauri::async_runtime::block_on(third_party_notices());
        assert!(notices.starts_with("# Third-party notices\n"));
        assert!(notices.contains("based in part on the work of the Independent JPEG Group"));
        assert!(notices.contains("## C libraries"));
    }

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

    fn view() -> SyncView {
        SyncView::new(
            SyncState {
                has_remote: true,
                branch: "main".into(),
                ahead: 0,
                behind: 1,
                changes: vec![],
                last_fetch: None,
                merge: None,
                last_sync: None,
            },
            Some("https://github.com/dcxSt/journal.git"),
            None,
            None,
        )
    }

    fn commit_info() -> CommitInfo {
        CommitInfo {
            id: "b".repeat(40),
            message: "Edit on the phone".into(),
            author: "Stephen".into(),
            time: 1_763_301_000_000,
        }
    }

    fn summary() -> SyncSummary {
        SyncSummary {
            at: 1_763_301_720_000,
            plan: PullPlan::FastForward,
            incoming: vec![commit_info()],
            incoming_count: 1,
            rebased: 0,
            merge_commit: None,
            files: vec![FileChange {
                path: "2026-11-16-1402-run.md".into(),
                kind: ChangeKind::Modified,
                old_path: None,
            }],
            resolved: vec![],
        }
    }

    fn json_of(value: impl Serialize) -> Value {
        serde_json::to_value(value).unwrap()
    }

    #[test]
    fn pull_results_are_tagged_by_status() {
        let up = json_of(PullResult::UpToDate { view: view() });
        assert_eq!(up["status"], "up_to_date");
        assert_eq!(up["view"]["behind"], 1);
        assert_eq!(up.as_object().unwrap().len(), 2, "{up}");

        let applied = json_of(PullResult::Applied {
            view: view(),
            summary: summary(),
        });
        assert_eq!(applied["status"], "applied");
        assert_eq!(
            applied["summary"],
            json!({
                "at": 1_763_301_720_000_i64,
                "plan": "fast_forward",
                "incoming": [{ "id": "b".repeat(40), "message": "Edit on the phone",
                               "author": "Stephen", "time": 1_763_301_000_000_i64 }],
                "incomingCount": 1,
                "rebased": 0,
                "mergeCommit": null,
                "files": [{ "path": "2026-11-16-1402-run.md", "kind": "modified" }],
                "resolved": [],
            })
        );

        let file = "2026-11-16-1402-run.md";
        let preview = PullPreview {
            id: format!(
                "main:{}:{}:{}",
                "a".repeat(40),
                "b".repeat(40),
                "c".repeat(40)
            ),
            branch: "main".into(),
            plan: PullPlan::Merge,
            automatic: false,
            incoming: vec![commit_info()],
            incoming_count: 1,
            local: vec![],
            local_count: 0,
            auto: vec![],
            overlaps: vec![Overlap {
                path: file.into(),
                kind: OverlapKind::BothModified,
                binary: false,
                mine_path: Some(file.into()),
                theirs_path: Some(file.into()),
                committed: true,
                versions: "a:b:c".into(),
                choices: vec![Choice::Mine, Choice::Theirs, Choice::Both, Choice::Merged],
                copy: Some("2026-11-16-1402-run.from-github-15h30.md".into()),
                suggestion: Some(MergeSuggestion {
                    clean: false,
                    chunks: vec![
                        MergeChunk::Same {
                            text: "# Run\n".into(),
                        },
                        MergeChunk::Mine {
                            text: "A\n".into(),
                            base: "a\n".into(),
                        },
                        MergeChunk::Clash {
                            base: "b\n".into(),
                            mine: "b!\n".into(),
                            theirs: "B\n".into(),
                        },
                    ],
                }),
            }],
            git_merge: GitMerge {
                conflicted: vec![file.into()],
                uncommitted: vec![],
            },
        };
        let review = json_of(PullResult::Review {
            view: view(),
            preview,
        });
        assert_eq!(review["status"], "review");
        let preview = &review["preview"];
        for key in [
            "incomingCount",
            "localCount",
            "automatic",
            "auto",
            "overlaps",
            "gitMerge",
        ] {
            assert!(preview.get(key).is_some(), "{key} missing: {preview}");
        }
        assert_eq!(preview["plan"], "merge");
        let overlap = &preview["overlaps"][0];
        assert_eq!(overlap["kind"], "both_modified");
        assert_eq!(
            overlap["choices"],
            json!(["mine", "theirs", "both", "merged"])
        );
        assert_eq!(overlap["copy"], "2026-11-16-1402-run.from-github-15h30.md");
        assert_eq!(overlap["minePath"], file);
        assert_eq!(overlap["theirsPath"], file);
        assert_eq!(overlap["committed"], true);
        assert!(overlap.get("mine").is_none(), "no full texts: {overlap}");
        assert_eq!(
            preview["gitMerge"],
            json!({ "conflicted": [file], "uncommitted": [] })
        );
        assert_eq!(
            overlap["suggestion"],
            json!({ "clean": false, "chunks": [
                { "kind": "same", "text": "# Run\n" },
                { "kind": "mine", "text": "A\n", "base": "a\n" },
                { "kind": "clash", "base": "b\n", "mine": "b!\n", "theirs": "B\n" },
            ] })
        );
    }

    #[test]
    fn apply_undo_and_abort_shapes() {
        let applied = json_of(Applied {
            view: view(),
            summary: summary(),
        });
        assert_eq!(applied["summary"]["plan"], "fast_forward");
        assert_eq!(applied["view"]["connected"], true);
        assert_eq!(applied.as_object().unwrap().len(), 2);

        let restored = json_of(Restored {
            view: view(),
            restored: vec!["2026-11-16-1402-run.md".into()],
        });
        assert_eq!(restored["restored"], json!(["2026-11-16-1402-run.md"]));
        assert_eq!(restored["view"]["merge"], Value::Null);
        assert_eq!(restored["view"]["lastSync"], Value::Null);

        let aborted = json_of(Aborted {
            view: view(),
            restored: vec![],
            stashed: true,
        });
        assert_eq!(aborted["stashed"], true);
        assert_eq!(aborted.as_object().unwrap().len(), 3);
    }

    #[test]
    fn file_diff_shape() {
        let diff = FileDiff {
            path: "2026-11-16-1402-run.md".into(),
            old_path: Some("2026-11-16-1402-walk.md".into()),
            kind: ChangeKind::Renamed,
            binary: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 2,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    DiffLine {
                        kind: LineKind::Context,
                        text: "# Walk".into(),
                    },
                    DiffLine {
                        kind: LineKind::Removed,
                        text: "slow".into(),
                    },
                    DiffLine {
                        kind: LineKind::Added,
                        text: "fast".into(),
                    },
                ],
            }],
        };
        assert_eq!(
            json_of(diff),
            json!({
                "path": "2026-11-16-1402-run.md",
                "oldPath": "2026-11-16-1402-walk.md",
                "kind": "renamed",
                "binary": false,
                "hunks": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2, "lines": [
                    { "kind": "context", "text": "# Walk" },
                    { "kind": "removed", "text": "slow" },
                    { "kind": "added", "text": "fast" },
                ] }],
            })
        );
    }

    #[test]
    fn resolutions_from_the_ui() {
        let resolved: Vec<Resolved> = serde_json::from_value(json!([
            { "path": "a.md", "choice": "merged", "text": "line\r\nno final newline" },
            { "path": "b.md", "choice": "mine" },
            { "path": "c.md", "choice": "theirs" },
            { "path": "assets/x.jpg", "choice": "both" },
        ]))
        .unwrap();
        assert_eq!(resolved[0].path, "a.md");
        assert_eq!(
            resolved[0].resolution,
            Resolution::Merged {
                text: "line\r\nno final newline".into()
            }
        );
        assert_eq!(resolved[1].resolution, Resolution::Mine);
        assert_eq!(resolved[2].resolution, Resolution::Theirs);
        assert_eq!(resolved[3].resolution, Resolution::Both);

        // The UI never sends these; they are refused rather than guessed at.
        for bad in [
            json!({ "path": "a.md", "choice": "merged" }),
            json!({ "path": "a.md", "choice": "ours" }),
            json!({ "choice": "mine" }),
        ] {
            assert!(
                serde_json::from_value::<Resolved>(bad.clone()).is_err(),
                "{bad}"
            );
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
