//! The token only goes to `Creds::url_prefix`, whatever URL git ends up contacting.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use common::{author, Device};
use mdgj_git::{Creds, Error};

const GITHUB_REPO: &str = "https://github.com/example/journal.git";

fn creds(url_prefix: &str) -> Creds {
    Creds {
        username: "x-access-token".into(),
        password: "ghu_FAKE_TOKEN_FOR_TEST".into(),
        url_prefix: url_prefix.into(),
    }
}

/// A loopback HTTP server that asks for Basic auth on every request and records whether any
/// request carried an Authorization header. Returns its base URL.
fn auth_listener() -> (String, Arc<Mutex<bool>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let saw_auth = Arc::new(Mutex::new(false));
    let seen = Arc::clone(&saw_auth);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                if line.to_ascii_lowercase().starts_with("authorization:") {
                    *seen.lock().unwrap() = true;
                }
                line.clear();
            }
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (base, saw_auth)
}

fn committed_device() -> Device {
    let d = Device::new();
    d.write("a.md", "a\n");
    d.commit("A");
    d
}

#[test]
fn the_token_is_not_sent_to_a_pushurl_elsewhere() {
    let (base, saw_auth) = auth_listener();
    let d = committed_device();
    d.repo.set_remote(GITHUB_REPO).unwrap();
    let mut config = d.git().config().unwrap();
    config
        .set_str("remote.origin.pushurl", &format!("{base}/me/journal.git"))
        .unwrap();

    let result = d.repo.push(Some(&creds("https://github.com/")));
    assert!(
        matches!(&result, Err(Error::Auth(m)) if m.contains(&base)),
        "{result:?}"
    );
    assert!(!*saw_auth.lock().unwrap(), "the token reached {base}");

    // The listener does see a token meant for it, so the check above is meaningful.
    let result = d.repo.push(Some(&creds(&format!("{base}/"))));
    assert!(matches!(result, Err(Error::Auth(_))), "{result:?}");
    assert!(*saw_auth.lock().unwrap());
}

#[test]
fn the_token_is_not_sent_where_insteadof_points_on_connect() {
    let (base, saw_auth) = auth_listener();
    let d = committed_device();
    let mut config = d.git().config().unwrap();
    config
        .set_str(&format!("url.{base}/.insteadOf"), "https://github.com/")
        .unwrap();

    let result = d
        .repo
        .connect(GITHUB_REPO, Some(&creds("https://github.com/")), &author());
    assert!(
        matches!(&result, Err(Error::Auth(m)) if m.contains(&base)),
        "{result:?}"
    );
    assert!(!*saw_auth.lock().unwrap(), "the token reached {base}");
    assert_eq!(d.repo.remote_url(), None);
}
