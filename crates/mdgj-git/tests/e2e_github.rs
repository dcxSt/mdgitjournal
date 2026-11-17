//! Two devices syncing through a real GitHub repository over HTTPS with a token, the way the app
//! does. Needs the network, so it only runs on request:
//!
//! MDGJ_E2E_TOKEN=$(gh auth token) cargo test -p mdgj-git --test e2e_github -- --ignored --nocapture
//!
//! MDGJ_E2E_REPO overrides the repository URL. The test commits to its default branch, and
//! deletes its files again at the end.

mod common;

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use common::{author, Device};
use mdgj_git::{CommitPushOutcome, Creds};

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
        device.repo.connect(&url, Some(&creds), &author()).unwrap();
        assert_eq!(device.repo.remote_url(), Some(url.clone()));
    }

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let name = format!("e2e-{}-{millis}.md", std::process::id());
    println!("test entry: {name}");

    a.write(&name, "line 1\nline 2\nline 3\n");
    push(&a, "e2e: add entry", &creds);
    let out = b.repo.pull(Some(&creds), &author()).unwrap();
    assert!(out.updated.contains(&name), "{:?}", out.updated);
    assert_eq!(b.text(&name), "line 1\nline 2\nline 3\n");

    // Both devices change the same line; B's edit is uncommitted when it pulls.
    a.write(&name, "line 1, edited on a\nline 2\nline 3\n");
    push(&a, "e2e: edit on a", &creds);
    b.write(&name, "line 1, edited on b\nline 2\nline 3\n");
    let out = b.repo.pull(Some(&creds), &author()).unwrap();
    let copy = out
        .conflicts
        .iter()
        .find(|conflict| conflict.path == name)
        .and_then(|conflict| conflict.copy.clone())
        .expect("a keep-both conflict copy");
    println!("conflict copy: {copy}");
    assert_eq!(b.text(&name), "line 1, edited on a\nline 2\nline 3\n");
    assert_eq!(b.text(&copy), "line 1, edited on b\nline 2\nline 3\n");

    // Clean up through the app's own path: delete, commit and push, then see it on the other side.
    b.remove(&copy);
    b.remove(&name);
    push(&b, "e2e: remove test entry", &creds);
    let out = a.repo.pull(Some(&creds), &author()).unwrap();
    assert!(out.updated.contains(&name));
    assert!(!a.exists(&name));
    assert_eq!(a.head(), b.head());

    for device in [&a, &b] {
        assert_not_on_disk(device.root(), &creds.password);
    }
}

fn push(device: &Device, message: &str, creds: &Creds) {
    match device
        .repo
        .commit_and_push(message, &author(), Some(creds))
        .unwrap()
    {
        CommitPushOutcome::Pushed { .. } => {}
        other => panic!("expected a push: {other:?}"),
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
