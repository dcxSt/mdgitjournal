//! The sync state as the UI shows it.

use mdgj_core::RemoteConfig;
use mdgj_git::{FileChange, SyncState};
use serde::Serialize;

use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncView {
    pub connected: bool,
    /// `"owner/repo"`, or origin's URL if the app didn't connect it.
    pub repo: Option<String>,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
    pub last_fetch: Option<i64>,
    /// Why the last sync operation failed, until one succeeds.
    pub error: Option<Error>,
}

impl SyncView {
    /// `origin` is the git remote's URL. `remote` is the repository the settings say the journal
    /// is connected to; git is trusted over it, since `git remote` may have changed it since.
    pub fn new(
        state: SyncState,
        origin: Option<&str>,
        remote: Option<&RemoteConfig>,
        error: Option<Error>,
    ) -> Self {
        let repo = origin.map(|url| match remote {
            Some(remote) if remote.url == url => remote.full_name.clone(),
            _ => url.to_owned(),
        });
        Self {
            connected: state.has_remote,
            repo,
            branch: state.branch,
            ahead: state.ahead,
            behind: state.behind,
            changes: state.changes,
            last_fetch: state.last_fetch,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use mdgj_core::AuthKind;
    use mdgj_git::ChangeKind;
    use serde_json::json;

    use super::*;
    use crate::error::Kind;

    const URL: &str = "https://github.com/dcxSt/journal.git";

    fn state(has_remote: bool) -> SyncState {
        SyncState {
            has_remote,
            branch: "main".into(),
            ahead: 1,
            behind: 2,
            changes: vec![
                FileChange {
                    path: "2026-11-16-1402-run.md".into(),
                    kind: ChangeKind::Renamed,
                    old_path: Some("2026-11-16-1402-walk.md".into()),
                },
                FileChange {
                    path: "assets/2026-11-16-1402-run-1.jpg".into(),
                    kind: ChangeKind::Added,
                    old_path: None,
                },
            ],
            last_fetch: Some(1_763_301_720_000),
        }
    }

    fn remote(url: &str) -> RemoteConfig {
        RemoteConfig {
            full_name: "dcxSt/journal".into(),
            url: url.into(),
            branch: "main".into(),
            auth: AuthKind::GithubApp,
        }
    }

    #[test]
    fn serializes_to_the_contract_shape() {
        let error = Error::new(Kind::Network, "Network error: offline");
        let view = SyncView::new(state(true), Some(URL), Some(&remote(URL)), Some(error));
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            json!({
                "connected": true,
                "repo": "dcxSt/journal",
                "branch": "main",
                "ahead": 1,
                "behind": 2,
                "changes": [
                    { "path": "2026-11-16-1402-run.md", "kind": "renamed",
                      "oldPath": "2026-11-16-1402-walk.md" },
                    { "path": "assets/2026-11-16-1402-run-1.jpg", "kind": "added" },
                ],
                "lastFetch": 1_763_301_720_000_i64,
                "error": { "kind": "network", "message": "Network error: offline" },
            })
        );
    }

    #[test]
    fn not_connected() {
        let view = SyncView::new(
            SyncState {
                last_fetch: None,
                ..state(false)
            },
            None,
            None,
            None,
        );
        assert!(!view.connected);
        assert_eq!((view.repo, view.last_fetch, view.error), (None, None, None));
    }

    #[test]
    fn git_wins_over_stale_settings() {
        // `git remote remove origin` by hand: the settings still name the repository.
        let view = SyncView::new(state(false), None, Some(&remote(URL)), None);
        assert_eq!((view.connected, view.repo), (false, None));

        // `git remote set-url` by hand, or a remote the app didn't add.
        let other = "https://example.com/me/journal.git";
        let view = SyncView::new(state(true), Some(other), Some(&remote(URL)), None);
        assert_eq!(view.repo.as_deref(), Some(other));
        let view = SyncView::new(state(true), Some(other), None, None);
        assert_eq!(view.repo.as_deref(), Some(other));
    }
}
