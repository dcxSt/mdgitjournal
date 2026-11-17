//! Two devices syncing through a real GitHub repository, driven through the shell's own command
//! functions (app/src-tauri/src) the way the UI's `invoke` calls reach them. Needs the network, so
//! it only runs on request:
//!
//! MDGJ_E2E_TOKEN=$(gh auth token) cargo test -p mdgitjournal --test github_e2e -- --ignored
//!
//! MDGJ_E2E_REPO overrides the repository (`owner/repo`, default dcxSt/mdgitjournal-test). The test
//! commits to its default branch and deletes its files again at the end, also after a failure.
//!
//! Runs on macOS (and on Linux with a display); not in CI, which never passes --ignored.
//!
//! How: the commands take a Tauri `AppHandle`, whose event loop macOS only allows on the main
//! thread, which libtest never gives a test; hence `harness = false`. Each device is this binary
//! re-run as a child process with its own HOME (so its own settings.json and credentials.json) and
//! MDGJ_JOURNAL_DIR. The child builds the app state exactly as `setup` does, without a window or
//! menu, and answers one JSON command per line on stdin. The token reaches the child only through
//! its environment, never its arguments. Every keyring entry fails to build, so `KeyringStore`
//! falls back to its 0600 file and the real keychain is never touched.

#![allow(dead_code, unused_imports)]

#[path = "../src/commands.rs"]
mod commands;
#[path = "../src/error.rs"]
mod error;
#[path = "../src/state.rs"]
mod state;
#[path = "../src/view.rs"]
mod view;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const DEFAULT_REPO: &str = "dcxSt/mdgitjournal-test";
const ROLE: &str = "MDGJ_E2E_DEVICE";
const TOKEN: &str = "MDGJ_E2E_TOKEN";
/// Marks reply lines, so anything else a library prints to stdout is skipped.
const REPLY: &str = "@@mdgj-reply ";
/// `app_config_dir()` under the device's HOME.
#[cfg(target_os = "macos")]
const CONFIG: &str = "Library/Application Support/xyz.stephenfay.mdgitjournal";
#[cfg(not(target_os = "macos"))]
const CONFIG: &str = ".config/xyz.stephenfay.mdgitjournal";

fn main() {
    if std::env::var_os(ROLE).is_some() {
        return device::serve();
    }
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        println!("github_e2e: test");
        return;
    }
    if !args
        .iter()
        .any(|a| a == "--ignored" || a == "--include-ignored")
    {
        println!(
            "\nrunning 1 test\ntest github_e2e ... ignored, talks to github.com; needs {TOKEN}\n"
        );
        return;
    }
    let token = match std::env::var(TOKEN) {
        Ok(token) if !token.trim().is_empty() => token.trim().to_owned(),
        _ => {
            eprintln!("set {TOKEN} to a GitHub token with access to the test repository");
            std::process::exit(1);
        }
    };
    let repo = std::env::var("MDGJ_E2E_REPO").unwrap_or_else(|_| DEFAULT_REPO.into());
    println!("\nrunning 1 test (github_e2e against {repo})");
    let root = tempfile::Builder::new()
        .prefix("mdgj-e2e-")
        .tempdir()
        .unwrap();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let tag = format!("e2e{}x{millis}", std::process::id());
    println!("run tag: {tag}  scratch: {}", root.path().display());

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        scenario::run(root.path(), &repo, &tag, &token)
    }));
    // Whatever happened, take the test files off the repository again.
    let cleanup = panic::catch_unwind(AssertUnwindSafe(|| {
        scenario::cleanup(root.path(), &repo, &tag, &token)
    }));
    // The scratch folder holds copies of the token (credentials.json): remove it before exiting,
    // since `process::exit` skips destructors. MDGJ_E2E_KEEP keeps it for debugging.
    if std::env::var_os("MDGJ_E2E_KEEP").is_some() {
        println!("kept {} (it contains the token)", root.keep().display());
    } else {
        root.close().expect("remove the scratch folder");
    }
    match (result, cleanup) {
        (Ok(()), Ok(())) => println!("test github_e2e ... ok\n"),
        (run, clean) => {
            if run.is_err() {
                eprintln!("test github_e2e ... FAILED");
            }
            if clean.is_err() {
                eprintln!("cleanup FAILED: remove files containing {tag} from {repo} by hand");
            }
            std::process::exit(101);
        }
    }
}

/// The child process: one device running the app's backend.
mod device {
    use std::any::Any;
    use std::io::{BufRead, Write};
    use std::sync::{Arc, Mutex};

    use keyring::credential::{Credential, CredentialBuilderApi};
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use serde_json::{json, Value};
    use tauri::async_runtime::block_on;
    use tauri::{AppHandle, Listener, Manager};

    use crate::{commands, error, state, REPLY, TOKEN};

    /// Every keyring entry fails to build, so `KeyringStore` has no keyring and uses its file.
    struct NoKeyring;

    impl CredentialBuilderApi for NoKeyring {
        fn build(&self, _: Option<&str>, _: &str, _: &str) -> keyring::Result<Box<Credential>> {
            Err(keyring::Error::NoStorageAccess(
                "the e2e test never uses the real keychain".into(),
            ))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    pub fn serve() {
        // SAFETY: as `run` does, before any thread uses libgit2.
        unsafe { state::limit_git_waits() };
        keyring::set_default_credential_builder(Box::new(NoKeyring));
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        let app = tauri::Builder::default()
            .enable_macos_default_menu(false)
            .build(context)
            .expect("build the app");
        let handle = app.handle().clone();
        // As `setup` does.
        let backend: state::Backend = state::AppState::open(&handle);
        handle.manage(backend);

        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        for name in [state::SYNC_STATE, state::ENTRIES_CHANGED] {
            let events = events.clone();
            handle.listen_any(name, move |event| {
                let payload: Value = serde_json::from_str(event.payload()).unwrap_or(Value::Null);
                events
                    .lock()
                    .unwrap()
                    .push(json!({ "event": name, "payload": payload }));
            });
        }

        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let line = line.expect("read a command");
            let request: Value = serde_json::from_str(&line).expect("a JSON command");
            let cmd = request["cmd"].as_str().expect("cmd").to_owned();
            let args = request["args"].clone();
            let reply = if cmd == "events" {
                json!({ "ok": std::mem::take(&mut *events.lock().unwrap()) })
            } else {
                dispatch(&handle, &cmd, &args)
            };
            let mut out = std::io::stdout().lock();
            writeln!(out, "{REPLY}{reply}").unwrap();
            out.flush().unwrap();
        }
        // Like quitting the app: nothing else to clean up.
        std::process::exit(0);
    }

    fn reply<T: Serialize>(result: error::Result<T>) -> Value {
        match result {
            Ok(value) => json!({ "ok": value }),
            Err(err) => json!({ "err": err }),
        }
    }

    fn arg<T: DeserializeOwned>(args: &Value, name: &str) -> T {
        serde_json::from_value(args[name].clone()).unwrap_or_else(|e| panic!("arg {name}: {e}"))
    }

    fn dispatch(app: &AppHandle, cmd: &str, args: &Value) -> Value {
        let h = || app.clone();
        let s = |name| arg::<String>(args, name);
        match cmd {
            "app_info" => reply(block_on(commands::app_info(h()))),
            "get_settings" => reply(block_on(commands::get_settings(h()))),
            "set_settings" => reply(block_on(commands::set_settings(h(), arg(args, "settings")))),
            "list_entries" => reply(block_on(commands::list_entries(h()))),
            "read_entry" => reply(block_on(commands::read_entry(h(), s("file")))),
            "write_entry" => reply(block_on(commands::write_entry(
                h(),
                s("file"),
                s("content"),
            ))),
            "create_entry" => reply(block_on(commands::create_entry(h(), s("title")))),
            "rename_entry" => reply(block_on(commands::rename_entry(
                h(),
                s("file"),
                s("newTitle"),
            ))),
            "delete_entry" => reply(block_on(commands::delete_entry(h(), s("file")))),
            "search" => reply(block_on(commands::search(h(), s("query")))),
            // `commands::import_image` takes a `tauri::ipc::Request`, which only Tauri can build;
            // this is what it runs once it has decoded the header and body.
            "import_image" => {
                let file = s("file");
                let bytes = std::fs::read(s("source")).expect("read the source image");
                let app = h();
                reply(
                    block_on(tauri::async_runtime::spawn_blocking(move || {
                        let backend = app.state::<state::Backend>();
                        let st = backend.as_ref().map_err(Clone::clone)?;
                        Ok(st.journal.import_image(&file, &bytes, st.image_opts())?)
                    }))
                    .expect("import_image ran"),
                )
            }
            "sync_state" => reply(block_on(commands::sync_state(h()))),
            "fetch" => reply(block_on(commands::fetch(h()))),
            "pull" => reply(block_on(commands::pull(h()))),
            "commit" => reply(block_on(commands::commit(h(), s("message")))),
            "commit_and_push" => reply(block_on(commands::commit_and_push(h(), s("message")))),
            "auth_status" => reply(block_on(commands::auth_status(h()))),
            "github_set_pat" => {
                // "$TOKEN" stands for the token, which only the environment carries.
                let token = match s("token").as_str() {
                    "$TOKEN" => std::env::var(TOKEN).expect("token in the environment"),
                    other => other.to_owned(),
                };
                reply(block_on(commands::github_set_pat(h(), token)))
            }
            "github_list_repos" => reply(block_on(commands::github_list_repos(h()))),
            "github_connect" => reply(block_on(commands::github_connect(h(), s("fullName")))),
            "github_disconnect" => reply(block_on(commands::github_disconnect(
                h(),
                arg(args, "signOut"),
            ))),
            "github_urls" => reply(block_on(commands::github_urls(h()))),
            other => {
                json!({ "err": { "kind": "test", "message": format!("unknown command {other}") } })
            }
        }
    }
}

/// A device as the orchestrator sees it: a running child process plus its folders.
struct Device {
    name: &'static str,
    home: PathBuf,
    journal: PathBuf,
    token: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Device {
    fn launch(name: &'static str, root: &Path, token: &str) -> Device {
        Device::launch_env(name, root, token, &[])
    }

    fn launch_env(name: &'static str, root: &Path, token: &str, env: &[(&str, &str)]) -> Device {
        let home = root.join(format!("{name}-home"));
        let journal = root.join(format!("{name}-journal"));
        fs::create_dir_all(&home).unwrap();
        let stderr = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join(format!("{name}-stderr.log")))
            .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .env(ROLE, name)
            .env("HOME", &home)
            .env("MDGJ_JOURNAL_DIR", &journal)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .expect("launch a device");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Device {
            name,
            home,
            journal,
            token: token.to_owned(),
            child,
            stdin,
            stdout,
        }
    }

    /// Quits and starts the app again on the same device.
    fn restart(self, root: &Path) -> Device {
        let Device {
            name,
            token,
            mut child,
            stdin,
            ..
        } = self;
        drop(stdin);
        child.wait().unwrap();
        Device::launch(name, root, &token)
    }

    fn config(&self) -> PathBuf {
        self.home.join(CONFIG)
    }

    fn redact(&self, text: &str) -> String {
        text.replace(&self.token, "<TOKEN>")
    }

    /// Calls a command; every reply is checked for the token first.
    fn call(&mut self, cmd: &str, args: Value) -> Result<Value, Value> {
        let request = json!({ "cmd": cmd, "args": args });
        writeln!(self.stdin, "{request}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).unwrap();
            assert!(n > 0, "{}: the app exited during {cmd}", self.name);
            assert!(
                !line.contains(&self.token),
                "{}: the token appeared in the output of {cmd}: {}",
                self.name,
                self.redact(&line)
            );
            if let Some(json) = line.strip_prefix(REPLY) {
                let mut reply: Value = serde_json::from_str(json).unwrap();
                if let Some(err) = reply.get_mut("err") {
                    return Err(err.take());
                }
                return Ok(reply["ok"].take());
            }
        }
    }

    fn ok(&mut self, cmd: &str, args: Value) -> Value {
        match self.call(cmd, args) {
            Ok(value) => value,
            Err(err) => panic!("{}: {cmd} failed: {err}", self.name),
        }
    }

    fn err(&mut self, cmd: &str, args: Value) -> Value {
        match self.call(cmd, args) {
            Ok(value) => panic!("{}: {cmd} should have failed, got {value}", self.name),
            Err(err) => err,
        }
    }

    fn events(&mut self) -> Vec<Value> {
        let events = self.ok("events", json!({}));
        events.as_array().unwrap().clone()
    }

    fn text(&self, path: &str) -> String {
        fs::read_to_string(self.journal.join(path))
            .unwrap_or_else(|e| panic!("{}: read {path}: {e}", self.name))
    }

    /// Plain git on the journal, local commands only.
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.journal)
            .args(args)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}: git {args:?}: {}",
            self.name,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

/// Fails if any file under `dir`, other than `allowed`, contains `secret`. Never prints it.
fn assert_not_on_disk(dir: &Path, secret: &str, allowed: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            assert_not_on_disk(&path, secret, allowed);
        } else if meta.is_file() && !allowed.contains(&path) {
            let content = fs::read(&path).unwrap();
            let found = content
                .windows(secret.len())
                .any(|window| window == secret.as_bytes());
            assert!(!found, "the token was written to {}", path.display());
        }
    }
}

fn contains(list: &Value, item: &str) -> bool {
    list.as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some(item))
}

mod scenario {
    use super::*;

    fn pushed(device: &mut Device, message: &str) -> Value {
        let out = device.ok("commit_and_push", json!({ "message": message }));
        assert_eq!(out["result"], "pushed", "{}: {out}", device.name);
        let view = &out["view"];
        assert_eq!(
            (view["ahead"].as_u64(), view["behind"].as_u64()),
            (Some(0), Some(0))
        );
        out
    }

    fn author_of(device: &Device, rev: &str) -> String {
        device
            .git(&["log", "-1", "--format=%an <%ae>|%cn <%ce>", rev])
            .trim()
            .to_owned()
    }

    pub fn run(root: &Path, repo: &str, tag: &str, token: &str) {
        let mut a = Device::launch("a", root, token);
        let mut b = Device::launch("b", root, token);

        // Fresh devices: file token store, empty journal, signed out.
        for d in [&mut a, &mut b] {
            let info = d.ok("app_info", json!({}));
            assert_eq!(info["tokenBackend"], "file", "{info}");
            assert_eq!(
                Path::new(info["journalDir"].as_str().unwrap()),
                d.journal.as_path()
            );
            let auth = d.ok("auth_status", json!({}));
            assert_eq!(auth["signedIn"], false);
        }
        println!("ok  fresh devices use the file token store");

        // A token GitHub rejects gives a readable auth error that doesn't repeat it.
        let bogus = format!("ghp_{}", "0".repeat(36));
        let err = a.err("github_set_pat", json!({ "token": bogus }));
        assert_eq!(err["kind"], "auth", "{err}");
        assert!(!err["message"].as_str().unwrap().contains(&bogus), "{err}");
        assert_eq!(a.ok("auth_status", json!({}))["signedIn"], false);
        println!("ok  bad token: {}", err["message"]);

        // Real token on both devices.
        let user = github_user(&mut a);
        for d in [&mut a, &mut b] {
            let login = d.ok("github_set_pat", json!({ "token": "$TOKEN" }));
            assert_eq!(login["login"], user.0.as_str());
            let auth = d.ok("auth_status", json!({}));
            assert_eq!(
                (
                    auth["signedIn"].clone(),
                    auth["kind"].clone(),
                    auth["login"].clone()
                ),
                (json!(true), json!("pat"), json!(user.0))
            );
            assert_eq!(d.ok("github_list_repos", json!({})), json!([]));
            let creds = d.config().join("credentials.json");
            assert!(creds.is_file(), "{}: no credentials.json", d.name);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&creds).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{}: credentials.json mode {mode:o}", d.name);
            }
        }
        println!("ok  signed in with a PAT as {} on both devices", user.0);

        // A repository that doesn't exist: readable error, not connected afterwards.
        let missing = format!("{}/{}-does-not-exist", user.0, tag);
        let err = a.err("github_connect", json!({ "fullName": missing }));
        println!("ok  missing repo: {err}");
        let view = a.ok("sync_state", json!({}));
        assert_eq!(view["connected"], false, "{view}");
        assert!(a.ok("get_settings", json!({}))["remote"].is_null());

        // Connect both devices.
        for d in [&mut a, &mut b] {
            let out = d.ok("github_connect", json!({ "fullName": repo }));
            let view = &out["view"];
            assert_eq!(view["connected"], true, "{out}");
            assert_eq!(view["repo"], repo, "{out}");
            assert_eq!(view["branch"], "main", "{out}");
            assert_eq!(
                (view["ahead"].as_u64(), view["behind"].as_u64()),
                (Some(0), Some(0))
            );
            assert!(view["error"].is_null(), "{out}");
            let remote = &d.ok("get_settings", json!({}))["remote"];
            assert_eq!(remote["fullName"], repo);
            assert_eq!(remote["url"], format!("https://github.com/{repo}.git"));
            assert_eq!(remote["auth"], "pat");
            let config = fs::read_to_string(d.journal.join(".git/config")).unwrap();
            assert!(
                config.contains(&format!("url = https://github.com/{repo}.git")),
                "{config}"
            );
            assert!(
                !config.contains('@'),
                "credentials in the remote URL:\n{config}"
            );
            let _ = d.events();
        }
        println!("ok  connected both devices");

        // A creates an entry and pushes it.
        let first = a.ok("create_entry", json!({ "title": format!("{tag} first") }));
        let first = first["file"].as_str().unwrap().to_owned();
        assert!(first.contains(tag), "{first}");
        a.ok(
            "write_entry",
            json!({ "file": first, "content": "hello from a\n" }),
        );
        let view = a.ok("sync_state", json!({}));
        assert_eq!(view["changes"][0]["path"], first.as_str(), "{view}");
        pushed(&mut a, "e2e: add entry");
        let expected_author = format!(
            "{} <{}+{}@users.noreply.github.com>",
            user.2, user.1, user.0
        );
        assert_eq!(
            author_of(&a, "HEAD"),
            format!("{expected_author}|{expected_author}"),
            "commit author from the GitHub account"
        );
        let events = a.events();
        assert!(
            events.iter().any(|e| e["event"] == state::SYNC_STATE),
            "{events:?}"
        );
        println!("ok  A created {first} and pushed it as {expected_author}");

        // B sees it as behind = 1.
        let view = b.ok("fetch", json!({}));
        assert_eq!(
            (view["ahead"].as_u64(), view["behind"].as_u64()),
            (Some(0), Some(1)),
            "{view}"
        );
        assert!(view["lastFetch"].is_i64(), "{view}");
        println!("ok  B fetch: behind 1");

        // B's own commit can't be pushed until it pulls; it stays committed locally.
        let second = b.ok("create_entry", json!({ "title": format!("{tag} from b") }));
        let second = second["file"].as_str().unwrap().to_owned();
        b.ok(
            "write_entry",
            json!({ "file": second, "content": "hello from b\n" }),
        );
        let out = b.ok("commit_and_push", json!({ "message": "e2e: entry from b" }));
        assert_eq!(out["result"], "needs_pull", "{out}");
        let view = &out["view"];
        assert_eq!(
            (view["ahead"].as_u64(), view["behind"].as_u64()),
            (Some(1), Some(1)),
            "{out}"
        );
        assert_eq!(view["changes"], json!([]), "{out}");
        // The push-only path (blank message, nothing to commit) is refused the same way.
        let out = b.ok("commit_and_push", json!({ "message": "" }));
        assert_eq!(out["result"], "needs_pull", "{out}");
        assert_eq!(b.text(&second), "hello from b\n");
        println!("ok  B commit & push while behind: needs_pull, commit kept");

        // B pulls: gets A's entry, its own commit is rebased on top.
        let _ = b.events();
        let out = b.ok("pull", json!({}));
        assert!(contains(&out["updated"], &first), "{out}");
        assert_eq!(out["conflicts"], json!([]), "{out}");
        let view = &out["view"];
        assert_eq!(
            (view["ahead"].as_u64(), view["behind"].as_u64()),
            (Some(1), Some(0)),
            "{out}"
        );
        assert_eq!(b.text(&first), "hello from a\n");
        assert_eq!(b.text(&second), "hello from b\n");
        let events = b.events();
        assert!(
            events.iter().any(|e| e["event"] == state::ENTRIES_CHANGED),
            "{events:?}"
        );
        assert_eq!(
            b.git(&["log", "-1", "--format=%s"]).trim(),
            "e2e: entry from b"
        );
        // Blank message, nothing to commit: pushes the waiting commit.
        pushed(&mut b, "");
        let out = a.ok("pull", json!({}));
        assert!(contains(&out["updated"], &second), "{out}");
        assert_eq!(a.text(&second), "hello from b\n");
        assert_eq!(a.git(&["rev-parse", "HEAD"]), b.git(&["rev-parse", "HEAD"]));
        println!("ok  B pull rebased its commit; push; A pull got B's entry");

        // Same line edited on both devices; B's edit is uncommitted when it pulls.
        a.ok(
            "write_entry",
            json!({ "file": first, "content": "hello from a, edited on a\n" }),
        );
        pushed(&mut a, "e2e: edit on a");
        b.ok(
            "write_entry",
            json!({ "file": first, "content": "hello from a, edited on b\n" }),
        );
        let out = b.ok("pull", json!({}));
        let conflict = out["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == first.as_str())
            .unwrap_or_else(|| panic!("no conflict for {first}: {out}"))
            .clone();
        let copy = conflict["copy"]
            .as_str()
            .expect("a keep-both copy")
            .to_owned();
        assert!(
            copy.contains(".conflict-") && copy.ends_with(".md"),
            "{copy}"
        );
        assert_eq!(b.text(&first), "hello from a, edited on a\n");
        assert_eq!(b.text(&copy), "hello from a, edited on b\n");
        let listed = b.ok("list_entries", json!({}));
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["file"] == copy.as_str()),
            "conflict copy listed: {listed}"
        );
        pushed(&mut b, "e2e: keep both");
        let out = a.ok("pull", json!({}));
        assert!(contains(&out["updated"], &copy), "{out}");
        assert_eq!(a.text(&copy), "hello from a, edited on b\n");
        println!("ok  uncommitted concurrent edit: kept both ({copy})");

        // Same line edited and committed on both devices: the rebase keeps both.
        a.ok(
            "write_entry",
            json!({ "file": second, "content": "hello from b, edited on a\n" }),
        );
        pushed(&mut a, "e2e: second edit on a");
        b.ok(
            "write_entry",
            json!({ "file": second, "content": "hello from b, edited on b\n" }),
        );
        let out = b.ok(
            "commit_and_push",
            json!({ "message": "e2e: second edit on b" }),
        );
        assert_eq!(out["result"], "needs_pull", "{out}");
        let out = b.ok("pull", json!({}));
        let conflict = out["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == second.as_str())
            .unwrap_or_else(|| panic!("no conflict for {second}: {out}"))
            .clone();
        let copy2 = conflict["copy"]
            .as_str()
            .expect("a keep-both copy")
            .to_owned();
        assert_eq!(b.text(&second), "hello from b, edited on a\n");
        assert_eq!(b.text(&copy2), "hello from b, edited on b\n");
        assert_eq!(
            out["view"]["changes"],
            json!([]),
            "copy committed by the rebase: {out}"
        );
        pushed(&mut b, "");
        let out = a.ok("pull", json!({}));
        assert!(contains(&out["updated"], &copy2), "{out}");
        println!("ok  committed concurrent edit: rebase kept both ({copy2})");

        // An image on A arrives byte for byte on B.
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/128x128.png");
        let image = a.ok("import_image", json!({ "file": first, "source": source }));
        let image_path = image["path"].as_str().unwrap().to_owned();
        // assets/<entry stem>-<first 8 hex digits of the JPEG's SHA-256>.jpg
        let hash = image_path
            .strip_prefix(&format!("assets/{}-", first.trim_end_matches(".md")))
            .and_then(|rest| rest.strip_suffix(".jpg"))
            .unwrap_or_else(|| panic!("not named after the entry: {image}"));
        assert!(
            hash.len() == 8 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{image}"
        );
        let on_a = fs::read(a.journal.join(&image_path)).unwrap();
        assert_eq!(on_a.len() as u64, image["bytes"].as_u64().unwrap());
        assert_eq!(&on_a[..3], &[0xff, 0xd8, 0xff], "a JPEG");
        let with_image = format!("hello from a, edited on a\n\n![]({image_path})\n");
        a.ok(
            "write_entry",
            json!({ "file": first, "content": with_image }),
        );
        pushed(&mut a, "e2e: image");
        let out = b.ok("pull", json!({}));
        assert!(contains(&out["updated"], &image_path), "{out}");
        let on_b = fs::read(b.journal.join(&image_path)).unwrap();
        assert!(on_a == on_b, "the image differs on B");
        assert_eq!(b.text(&first), with_image);
        println!(
            "ok  image {image_path} ({} bytes) identical on B",
            on_a.len()
        );

        // Rename on A, pulled by B.
        let renamed = a.ok(
            "rename_entry",
            json!({ "file": first, "newTitle": format!("{tag} renamed") }),
        );
        let renamed = renamed["file"].as_str().unwrap().to_owned();
        assert_ne!(renamed, first);
        let view = a.ok("sync_state", json!({}));
        assert_eq!(view["changes"][0]["kind"], "renamed", "{view}");
        assert_eq!(view["changes"][0]["oldPath"], first.as_str(), "{view}");
        pushed(&mut a, "e2e: rename");
        let out = b.ok("pull", json!({}));
        assert!(contains(&out["updated"], &renamed), "{out}");
        assert!(contains(&out["updated"], &first), "{out}");
        assert!(!b.journal.join(&first).exists(), "old name still on B");
        assert_eq!(b.text(&renamed), with_image);
        assert_eq!(out["view"]["changes"], json!([]), "{out}");
        println!("ok  rename {first} -> {renamed} pulled by B");

        // Rename on A while B has edited the entry (saved, not committed): B's text survives.
        let renamed2 = a.ok(
            "rename_entry",
            json!({ "file": renamed, "newTitle": format!("{tag} renamed again") }),
        );
        let renamed2 = renamed2["file"].as_str().unwrap().to_owned();
        pushed(&mut a, "e2e: rename again");
        let edited_on_b = format!("{with_image}a line written on b\n");
        b.ok(
            "write_entry",
            json!({ "file": renamed, "content": edited_on_b }),
        );
        let out = b.ok("pull", json!({}));
        let all_text: String = fs::read_dir(&b.journal)
            .unwrap()
            .filter_map(|e| fs::read_to_string(e.unwrap().path()).ok())
            .collect();
        assert!(
            all_text.contains("a line written on b"),
            "B's edit was lost: {out}"
        );
        println!(
            "ok  rename on A + edit on B: B's text kept (updated {}, conflicts {})",
            out["updated"], out["conflicts"]
        );
        assert!(b.text(&renamed2).contains("a line written on b"), "{out}");
        assert!(!b.journal.join(&renamed).exists(), "{out}");
        pushed(&mut b, "e2e: b edit after rename");
        a.ok("pull", json!({}));

        // Disconnect keeps the journal and the sign-in; reconnect pushes a commit made meanwhile.
        let view = a.ok("github_disconnect", json!({ "signOut": false }));
        assert_eq!(
            (view["connected"].clone(), view["repo"].clone()),
            (json!(false), Value::Null),
            "{view}"
        );
        assert!(a.ok("get_settings", json!({}))["remote"].is_null());
        // Plain git agrees there is no remote (libgit2, like `git remote remove`, may leave an
        // empty section header behind).
        assert_eq!(a.git(&["remote"]), "", "a remote is left");
        let config = fs::read_to_string(a.journal.join(".git/config")).unwrap();
        assert!(
            !config.contains("url ="),
            "remote URL left in .git/config:\n{config}"
        );
        assert_eq!(a.ok("auth_status", json!({}))["signedIn"], true);
        assert!(a.journal.join(&second).is_file());
        let offline = a.ok("create_entry", json!({ "title": format!("{tag} offline") }));
        let offline = offline["file"].as_str().unwrap().to_owned();
        a.ok(
            "write_entry",
            json!({ "file": offline, "content": "written while disconnected\n" }),
        );
        let view = a.ok("commit", json!({ "message": "e2e: offline commit" }));
        assert_eq!(view["changes"], json!([]), "{view}");
        println!(
            "ok  disconnected; local commit author: {}",
            author_of(&a, "HEAD")
        );
        let out = a.ok("github_connect", json!({ "fullName": repo }));
        assert_eq!(out["view"]["ahead"], 1, "{out}");
        assert_eq!(out["view"]["behind"], 0, "{out}");
        pushed(&mut a, "");
        let out = b.ok("pull", json!({}));
        assert!(contains(&out["updated"], &offline), "{out}");
        println!("ok  reconnect pushed the offline commit; B pulled it");

        // Disconnect and sign out: the token file goes; connecting asks for a sign-in.
        a.ok("github_disconnect", json!({ "signOut": true }));
        assert!(
            !a.config().join("credentials.json").exists(),
            "credentials.json left behind"
        );
        assert_eq!(a.ok("auth_status", json!({}))["signedIn"], false);
        let err = a.err("github_connect", json!({ "fullName": repo }));
        assert_eq!(err["kind"], "auth", "{err}");
        a.ok("github_set_pat", json!({ "token": "$TOKEN" }));
        let out = a.ok("github_connect", json!({ "fullName": repo }));
        assert_eq!(
            (
                out["view"]["ahead"].as_u64(),
                out["view"]["behind"].as_u64()
            ),
            (Some(0), Some(0)),
            "{out}"
        );
        println!("ok  sign out removed the token; sign in + reconnect");

        // After a restart, commit & push looks the account up again for the author.
        let mut a = a.restart(root);
        let restarted = a.ok("sync_state", json!({}));
        assert_eq!(restarted["connected"], true, "{restarted}");
        assert_eq!(restarted["repo"], repo, "{restarted}");
        a.ok("write_entry", json!({ "file": offline, "content": "written while disconnected\nand after a restart\n" }));
        pushed(&mut a, "e2e: after restart");
        assert_eq!(
            author_of(&a, "HEAD"),
            format!("{expected_author}|{expected_author}"),
            "author after a restart"
        );
        b.ok("pull", json!({}));
        println!("ok  restart: still connected, author resolved again");

        // A token GitHub no longer accepts (revoked): fetch fails readably, then recovers.
        let creds = a.config().join("credentials.json");
        let saved = fs::read_to_string(&creds).unwrap();
        let revoked = format!("ghp_{}", "1".repeat(36));
        fs::write(&creds, saved.replace(token, &revoked)).unwrap();
        let mut a = a.restart(root);
        let err = a.err("fetch", json!({}));
        assert!(
            !err.to_string().contains(&revoked),
            "the revoked token is in the error: {err}"
        );
        let view = a.ok("sync_state", json!({}));
        assert_eq!(view["error"], err, "{view}");
        println!("ok  revoked token: {err}");
        a.ok("github_set_pat", json!({ "token": "$TOKEN" }));
        let view = a.ok("fetch", json!({}));
        assert!(view["error"].is_null(), "{view}");
        println!("ok  signed in again: fetch works, error cleared");

        // Author and email from settings win; set_settings can't change the remote.
        let mut settings = a.ok("get_settings", json!({}));
        settings["authorName"] = json!("Settings Author");
        settings["authorEmail"] = json!("author@example.com");
        settings["remote"] = Value::Null;
        let saved = a.ok("set_settings", json!({ "settings": settings.clone() }));
        assert_eq!(
            saved["remote"]["fullName"], repo,
            "set_settings dropped the remote"
        );
        a.ok("write_entry", json!({ "file": offline, "content": "written while disconnected\nand after a restart\nwith a settings author\n" }));
        pushed(&mut a, "e2e: settings author");
        assert_eq!(
            author_of(&a, "HEAD"),
            "Settings Author <author@example.com>|Settings Author <author@example.com>"
        );
        settings["authorName"] = Value::Null;
        settings["authorEmail"] = Value::Null;
        a.ok("set_settings", json!({ "settings": settings }));
        b.ok("pull", json!({}));
        println!("ok  settings author used; set_settings kept the remote");

        // A third device with its own history, a draft and a same-named entry joins the repository:
        // its commit is rebased on top, the draft stays, the clash keeps both.
        let mut c = Device::launch("c", root, token);
        let own = c.ok("create_entry", json!({ "title": format!("{tag} c own") }));
        let own = own["file"].as_str().unwrap().to_owned();
        c.ok(
            "write_entry",
            json!({ "file": own, "content": "c's history\n" }),
        );
        c.ok("commit", json!({ "message": "e2e: c local commit" }));
        let draft = c.ok("create_entry", json!({ "title": format!("{tag} c draft") }));
        let draft = draft["file"].as_str().unwrap().to_owned();
        c.ok(
            "write_entry",
            json!({ "file": draft, "content": "c's draft\n" }),
        );
        fs::write(c.journal.join(&second), "c's version\n").unwrap();
        c.ok("github_set_pat", json!({ "token": "$TOKEN" }));
        let out = c.ok("github_connect", json!({ "fullName": repo }));
        assert_eq!(
            (
                out["view"]["ahead"].as_u64(),
                out["view"]["behind"].as_u64()
            ),
            (Some(1), Some(0)),
            "{out}"
        );
        let conflict = out["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == second.as_str())
            .unwrap_or_else(|| panic!("no conflict for {second}: {out}"))
            .clone();
        let copy3 = conflict["copy"]
            .as_str()
            .expect("a keep-both copy")
            .to_owned();
        assert_eq!(c.text(&second), a.text(&second));
        assert_eq!(c.text(&copy3), "c's version\n");
        assert_eq!(c.text(&draft), "c's draft\n");
        assert_eq!(
            c.git(&["log", "-1", "--format=%s"]).trim(),
            "e2e: c local commit"
        );
        pushed(&mut c, "e2e: c joins");
        let out = a.ok("pull", json!({}));
        assert!(
            contains(&out["updated"], &own) && contains(&out["updated"], &draft),
            "{out}"
        );
        println!("ok  device with its own history joined: rebased, draft kept, clash kept both");

        // The token is nowhere on disk but in the token stores.
        let allowed = [&a, &b, &c].map(|d| d.config().join("credentials.json"));
        assert_not_on_disk(root, token, &allowed);
        for d in [&a, &b, &c] {
            let config = fs::read_to_string(d.journal.join(".git/config")).unwrap();
            assert!(!config.contains(token));
            let settings = fs::read_to_string(d.config().join("settings.json")).unwrap();
            assert!(!settings.contains(token));
        }
        println!("ok  token only in credentials.json (also not in logs or .git)");
    }

    /// (login, id, display name) of the token's account, as the app sees it.
    fn github_user(d: &mut Device) -> (String, u64, String) {
        let user = mdgj_github::GitHub::new(mdgj_github::ReqwestHttp::new().unwrap())
            .user(&d.token)
            .unwrap();
        let name = user
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(user.login.clone());
        (user.login, user.id, name)
    }

    /// Removes every file with `tag` in its name from the repository, through the app.
    pub fn cleanup(root: &Path, repo: &str, tag: &str, token: &str) {
        let mut c = Device::launch("cleanup", root, token);
        c.ok("github_set_pat", json!({ "token": "$TOKEN" }));
        c.ok("github_connect", json!({ "fullName": repo }));
        let mut removed = Vec::new();
        for dir in [c.journal.clone(), c.journal.join("assets")] {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if name.contains(tag) {
                    fs::remove_file(&path).unwrap();
                    removed.push(name);
                }
            }
        }
        if removed.is_empty() {
            println!("cleanup: nothing of {tag} on {repo}");
            return;
        }
        for _ in 0..3 {
            let out = c.ok(
                "commit_and_push",
                json!({ "message": format!("e2e: remove {tag}") }),
            );
            match out["result"].as_str() {
                Some("pushed") => {
                    let left = c.git(&["ls-tree", "-r", "--name-only", "origin/main"]);
                    assert!(!left.contains(tag), "left on the remote: {left}");
                    println!("cleanup: removed {} files from {repo}", removed.len());
                    return;
                }
                Some("needs_pull") => {
                    c.ok("pull", json!({}));
                }
                _ => panic!("cleanup: {out}"),
            }
        }
        panic!("cleanup: could not push the deletions");
    }
}
