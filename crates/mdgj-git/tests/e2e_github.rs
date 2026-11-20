//! Two devices syncing through a real GitHub repository over HTTPS with a token, the way the app
//! does. Needs the network, so it only runs on request:
//!
//! MDGJ_E2E_TOKEN=$(gh auth token) cargo test -p mdgj-git --test e2e_github -- --ignored --nocapture
//!
//! MDGJ_E2E_REPO overrides the repository URL. The test commits one entry to its default branch
//! and deletes it again at the end, also when an assertion fails.

mod common;

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use common::{author, merged, overlap, Device};
use mdgj_git::{CommitPushOutcome, Creds, PullOutcome, PullPlan};

const DEFAULT_REPO: &str = "https://github.com/dcxSt/mdgitjournal-test.git";

#[test]
#[ignore = "talks to github.com; needs MDGJ_E2E_TOKEN"]
fn two_devices_sync_through_github() {
    let token = std::env::var("MDGJ_E2E_TOKEN").expect("set MDGJ_E2E_TOKEN to a GitHub token");
    let url = std::env::var("MDGJ_E2E_REPO").unwrap_or_else(|_| DEFAULT_REPO.into());
    let creds = Creds {
        username: "x-access-token".into(),
        password: token,
        url_prefix: "https://github.com/".into(),
    };
    let (a, b) = (Device::new(), Device::new());
    for device in [&a, &b] {
        let outcome = device.repo.connect(&url, Some(&creds), &author()).unwrap();
        assert!(
            matches!(
                outcome,
                PullOutcome::Applied { .. } | PullOutcome::UpToDate { .. }
            ),
            "{outcome:?}"
        );
        assert_eq!(device.repo.remote_url(), Some(url.clone()));
    }

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let name = format!("e2e-{}-{millis}.md", std::process::id());
    println!("test entry: {name}");
    let _cleanup = Cleanup {
        url: &url,
        creds: &creds,
        name: &name,
    };

    // A adds the entry; B's pull brings it at once.
    let base = "line 1\nline 2\nline 3\nline 4\nline 5\n";
    a.write(&name, base);
    push(&a, "e2e: add entry", &creds);
    let PullOutcome::Applied { summary, .. } = b.repo.pull(Some(&creds), &author()).unwrap() else {
        panic!("expected B's pull to apply");
    };
    assert!(summary.files.iter().any(|f| f.path == name), "{summary:?}");
    assert_eq!(b.text(&name), base);

    // A edits the first line and pushes; B, with an uncommitted edit to the last line, gets a
    // review with a clean suggestion, applies the merged text and pushes it.
    let theirs = base.replace("line 1", "line 1, edited on a");
    a.write(&name, &theirs);
    push(&a, "e2e: edit on a", &creds);
    b.write(&name, base.replace("line 5", "line 5, edited on b"));
    let before = b.read(&name);
    let PullOutcome::Review { preview, .. } = b.repo.pull(Some(&creds), &author()).unwrap() else {
        panic!("expected a review");
    };
    assert_eq!(b.read(&name), before, "the review wrote nothing");
    assert_eq!(preview.plan, PullPlan::FastForward);
    let found = overlap(&preview, &name);
    let suggestion = found.suggestion.as_ref().unwrap();
    assert!(suggestion.clean);
    let text = suggestion.text().unwrap();
    let expected = "line 1, edited on a\nline 2\nline 3\nline 4\nline 5, edited on b\n";
    assert_eq!(text, expected);
    b.repo
        .apply_pull(&preview.id, &[merged(&name, &text)], None, &author())
        .unwrap();
    assert_eq!(b.text(&name), expected);
    push(&b, "e2e: merged on b", &creds);

    // A's pull is clean, and can be undone.
    let a_head = a.head();
    let PullOutcome::Applied { summary, .. } = a.repo.pull(Some(&creds), &author()).unwrap() else {
        panic!("expected A's pull to apply");
    };
    assert_eq!(summary.incoming[0].message, "e2e: merged on b");
    assert_eq!(a.text(&name), expected);
    a.repo.undo_last_sync().unwrap();
    assert_eq!(a.head(), a_head);
    assert_eq!(a.text(&name), theirs);
    assert!(matches!(
        a.repo.pull(Some(&creds), &author()).unwrap(),
        PullOutcome::Applied { .. }
    ));
    assert_eq!(a.head(), b.head());

    // Clean up through the app's own path: delete, commit and push, then see it on the other side.
    b.remove(&name);
    push(&b, "e2e: remove test entry", &creds);
    let PullOutcome::Applied { summary, .. } = a.repo.pull(Some(&creds), &author()).unwrap() else {
        panic!("expected A's pull to apply");
    };
    assert!(summary.files.iter().any(|f| f.path == name));
    assert!(!a.exists(&name));
    assert_eq!(a.head(), b.head());

    for device in [&a, &b] {
        assert_not_on_disk(device.root(), &creds.password);
    }
}

fn push(device: &Device, message: &str, creds: &Creds) {
    let paths: Vec<String> = device
        .repo
        .status()
        .unwrap()
        .into_iter()
        .map(|change| change.path)
        .collect();
    match device
        .repo
        .commit_and_push(message, &paths, &author(), Some(creds))
        .unwrap()
    {
        CommitPushOutcome::Pushed { .. } => {}
        other => panic!("expected a push: {other:?}"),
    }
}

/// Removes the test entry from the repository if it is still there, from a fresh device, so a
/// failed run leaves nothing behind.
struct Cleanup<'a> {
    url: &'a str,
    creds: &'a Creds,
    name: &'a str,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let device = Device::new();
        if device
            .repo
            .connect(self.url, Some(self.creds), &author())
            .is_err()
            || !device.exists(self.name)
        {
            return;
        }
        device.remove(self.name);
        let result = device.repo.commit_and_push(
            "e2e: remove test entry (cleanup)",
            &[self.name.to_owned()],
            &author(),
            Some(self.creds),
        );
        println!("cleanup of {}: {result:?}", self.name);
    }
}

/// Fails if any file under `dir`, the git directory included, contains `secret`. Never prints it.
fn assert_not_on_disk(dir: &Path, secret: &str) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_not_on_disk(&path, secret);
        } else {
            let content = fs::read(&path).unwrap();
            let found = content
                .windows(secret.len())
                .any(|window| window == secret.as_bytes());
            assert!(!found, "the token was written to {}", path.display());
        }
    }
}
