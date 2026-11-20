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
//! re-run as a child process with its own MDGJ_JOURNAL_DIR and MDGJ_CONFIG_DIR, as
//! scripts/two-devices.sh runs the app (and its own HOME, for the git CLI). The child builds the
//! app state exactly as `setup` does, without a window or menu, and answers one JSON command per
//! line on stdin. The token reaches the child only through its environment, never its arguments.
//! MDGJ_CONFIG_DIR makes the token store file-only; as a second guard every keyring entry fails to
//! build, and the test checks that none was even tried, so the real keychain is never touched.
//!
//! The scenario follows PLAN.md §5: one-sided pulls apply at once (with Undo), a file changed on
//! both sides waits for a review (merged, kept both, kept mine, or resolved with git), and every
//! commit takes the ticked paths only.

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
/// `app_config_dir()` under the device's HOME, which MDGJ_CONFIG_DIR must keep the app out of.
#[cfg(target_os = "macos")]
const APP_CONFIG: &str = "Library/Application Support/xyz.stephenfay.mdgitjournal";
#[cfg(not(target_os = "macos"))]
const APP_CONFIG: &str = ".config/xyz.stephenfay.mdgitjournal";

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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use keyring::credential::{Credential, CredentialBuilderApi};
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use serde_json::{json, Value};
    use tauri::async_runtime::block_on;
    use tauri::{AppHandle, Listener, Manager};

    use crate::{commands, error, state, REPLY, TOKEN};

    /// Keyring entries this process tried to build; MDGJ_CONFIG_DIR means none.
    static KEYRING_BUILDS: AtomicUsize = AtomicUsize::new(0);

    /// Every keyring entry fails to build, so even a `KeyringStore` that tried would have no
    /// keyring and use its file.
    struct NoKeyring;

    impl CredentialBuilderApi for NoKeyring {
        fn build(&self, _: Option<&str>, _: &str, _: &str) -> keyring::Result<Box<Credential>> {
            KEYRING_BUILDS.fetch_add(1, Ordering::SeqCst);
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
            "pull_preview" => reply(block_on(commands::pull_preview(h()))),
            "apply_pull" => reply(block_on(commands::apply_pull(
                h(),
                s("id"),
                arg(args, "resolutions"),
                arg(args, "message"),
            ))),
            "undo_sync" => reply(block_on(commands::undo_sync(h()))),
            "start_external_merge" => reply(block_on(commands::start_external_merge(
                h(),
                s("id"),
                arg(args, "message"),
            ))),
            "abort_merge" => reply(block_on(commands::abort_merge(h()))),
            "commit" => reply(block_on(commands::commit(
                h(),
                s("message"),
                arg(args, "paths"),
            ))),
            "commit_and_push" => reply(block_on(commands::commit_and_push(
                h(),
                s("message"),
                arg(args, "paths"),
            ))),
            "diff" => reply(block_on(commands::diff(h(), s("path")))),
            "keyring_builds" => json!({ "ok": KEYRING_BUILDS.load(Ordering::SeqCst) }),
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
    config: PathBuf,
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
        let config = root.join(format!("{name}-config"));
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
            .env("MDGJ_CONFIG_DIR", &config)
            .env(
                "MDGJ_DEVICE_NAME",
                format!("Device {}", name.to_uppercase()),
            )
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
            config,
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
        self.config.clone()
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

    fn bytes(&self, path: &str) -> Vec<u8> {
        fs::read(self.journal.join(path))
            .unwrap_or_else(|e| panic!("{}: read {path}: {e}", self.name))
    }

    fn write(&mut self, file: &str, content: &str) {
        self.ok("write_entry", json!({ "file": file, "content": content }));
    }

    /// Plain git on the journal, local commands only.
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.journal)
            .args([
                "-c",
                "user.name=e2e terminal",
                "-c",
                "user.email=e2e@example.com",
            ])
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

/// The paths in a list of `FileChange`s.
fn paths(changes: &Value) -> Vec<String> {
    changes
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {changes}"))
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_owned())
        .collect()
}

fn has_change(changes: &Value, path: &str, kind: &str) -> bool {
    changes
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == path && c["kind"] == kind)
}

/// The merged text of a suggestion without clashes, as the UI's `joinChunks` makes it.
fn join_clean(suggestion: &Value) -> String {
    assert_eq!(suggestion["clean"], true, "{suggestion}");
    suggestion["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            assert_ne!(c["kind"], "clash", "{suggestion}");
            c["text"].as_str().unwrap()
        })
        .collect()
}

mod scenario {
    use super::*;

    /// Commits `paths` and pushes; blank message and no paths pushes what is waiting.
    fn pushed(device: &mut Device, message: &str, paths: &[&str]) -> Value {
        let out = device.ok(
            "commit_and_push",
            json!({ "message": message, "paths": paths }),
        );
        assert_eq!(out["result"], "pushed", "{}: {out}", device.name);
        let view = &out["view"];
        assert_eq!(
            (view["ahead"].as_u64(), view["behind"].as_u64()),
            (Some(0), Some(0)),
            "{}: {out}",
            device.name
        );
        out
    }

    /// Pulls, expecting GitHub's changes to be applied at once; returns the summary.
    fn applied(device: &mut Device) -> Value {
        let out = device.ok("pull", json!({}));
        assert_eq!(out["status"], "applied", "{}: {out}", device.name);
        assert!(out["view"]["error"].is_null(), "{out}");
        assert_eq!(out["view"]["behind"], 0, "{out}");
        out["summary"].clone()
    }

    /// Pulls, expecting a review; returns the preview.
    fn review(device: &mut Device) -> Value {
        let out = device.ok("pull", json!({}));
        assert_eq!(out["status"], "review", "{}: {out}", device.name);
        out["preview"].clone()
    }

    fn overlap<'a>(preview: &'a Value, path: &str) -> &'a Value {
        preview["overlaps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["path"] == path)
            .unwrap_or_else(|| panic!("no overlap at {path}: {preview}"))
    }

    fn head(device: &Device) -> String {
        device.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn author_of(device: &Device, rev: &str) -> String {
        device
            .git(&["log", "-1", "--format=%an <%ae>|%cn <%ce>", rev])
            .trim()
            .to_owned()
    }

    fn entry(device: &mut Device, title: String, content: &str) -> String {
        let meta = device.ok("create_entry", json!({ "title": title }));
        let file = meta["file"].as_str().unwrap().to_owned();
        device.write(&file, content);
        file
    }

    pub fn run(root: &Path, repo: &str, tag: &str, token: &str) {
        let mut a = Device::launch("a", root, token);
        let mut b = Device::launch("b", root, token);

        // Fresh devices: file token store in MDGJ_CONFIG_DIR, empty journal, signed out.
        for d in [&mut a, &mut b] {
            let info = d.ok("app_info", json!({}));
            assert_eq!(info["tokenBackend"], "file", "{info}");
            assert_eq!(
                Path::new(info["journalDir"].as_str().unwrap()),
                d.journal.as_path()
            );
            let auth = d.ok("auth_status", json!({}));
            assert_eq!(auth["signedIn"], false);
            let view = d.ok("sync_state", json!({}));
            assert!(
                view["merge"].is_null() && view["lastSync"].is_null(),
                "{view}"
            );
        }
        println!("ok  fresh devices use the file token store");

        // A token GitHub rejects gives a readable auth error that doesn't repeat it.
        let bogus = format!("ghp_{}", "0".repeat(36));
        let err = a.err("github_set_pat", json!({ "token": bogus }));
        assert_eq!(err["kind"], "auth", "{err}");
        assert!(!err["message"].as_str().unwrap().contains(&bogus), "{err}");
        assert_eq!(a.ok("auth_status", json!({}))["signedIn"], false);
        println!("ok  bad token: {}", err["message"]);

        // Real token on both devices, each in its own MDGJ_CONFIG_DIR.
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
            assert!(
                !d.home.join(APP_CONFIG).exists(),
                "{}: the app's own config folder was used",
                d.name
            );
        }
        println!("ok  signed in with a PAT as {} on both devices", user.0);

        // A repository that doesn't exist: readable error, not connected, no red dot.
        let missing = format!("{}/{}-does-not-exist", user.0, tag);
        let err = a.err("github_connect", json!({ "fullName": missing }));
        println!("ok  missing repo: {err}");
        let view = a.ok("sync_state", json!({}));
        assert_eq!(view["connected"], false, "{view}");
        assert!(view["error"].is_null(), "{view}");
        assert!(a.ok("get_settings", json!({}))["remote"].is_null());

        // Connect both: an empty journal takes what is there at once.
        for d in [&mut a, &mut b] {
            let out = d.ok("github_connect", json!({ "fullName": repo }));
            assert_ne!(out["status"], "review", "{out}");
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
            assert!(d.config().join("settings.json").is_file());
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

        // A writes an entry and a draft, and ticks only the entry.
        let first = entry(&mut a, format!("{tag} first"), "hello from a\n");
        assert!(first.contains(tag), "{first}");
        let draft = entry(&mut a, format!("{tag} draft"), "not ready\n");
        let diff = a.ok("diff", json!({ "path": first }));
        assert_eq!(
            diff,
            json!({ "path": first, "oldPath": null, "kind": "added", "binary": false, "hunks": [
                { "oldStart": 0, "oldLines": 0, "newStart": 1, "newLines": 1,
                  "lines": [{ "kind": "added", "text": "hello from a" }] },
            ] }),
        );
        let out = pushed(&mut a, "e2e: add entry", &[&first]);
        assert_eq!(paths(&out["view"]["changes"]), [draft.as_str()], "{out}");
        let expected_author = format!(
            "{} <{}+{}@users.noreply.github.com>",
            user.2, user.1, user.0
        );
        assert_eq!(
            author_of(&a, "HEAD"),
            format!("{expected_author}|{expected_author}"),
            "commit author from the GitHub account"
        );
        assert_eq!(
            a.git(&["show", "--name-only", "--format=", "HEAD"]).trim(),
            first,
            "only the ticked file is in the commit"
        );
        let events = a.events();
        assert!(
            events.iter().any(|e| e["event"] == state::SYNC_STATE),
            "{events:?}"
        );
        a.ok("delete_entry", json!({ "file": draft }));
        println!("ok  A pushed {first} only (its draft stayed out) as {expected_author}");

        // B sees it as behind = 1, then pulls: applied at once, with a summary.
        let view = b.ok("fetch", json!({}));
        assert_eq!(
            (view["ahead"].as_u64(), view["behind"].as_u64()),
            (Some(0), Some(1)),
            "{view}"
        );
        assert!(view["lastFetch"].is_i64(), "{view}");
        let _ = b.events();
        let summary = applied(&mut b);
        assert_eq!(summary["plan"], "fast_forward", "{summary}");
        assert_eq!(summary["incomingCount"], 1, "{summary}");
        assert_eq!(summary["incoming"][0]["message"], "e2e: add entry");
        assert!(has_change(&summary["files"], &first, "added"), "{summary}");
        assert_eq!(b.text(&first), "hello from a\n");
        let events = b.events();
        assert!(
            events.iter().any(|e| e["event"] == state::ENTRIES_CHANGED),
            "{events:?}"
        );
        let view = b.ok("sync_state", json!({}));
        assert_eq!(view["lastSync"]["incomingCount"], 1, "{view}");

        // Undo puts B back exactly; pulling again brings it in again.
        let out = b.ok("undo_sync", json!({}));
        assert_eq!(out["restored"], json!([first]), "{out}");
        assert!(!b.journal.join(&first).exists());
        assert_eq!(out["view"]["behind"], 1, "{out}");
        assert!(out["view"]["lastSync"].is_null(), "{out}");
        let err = b.err("undo_sync", json!({}));
        assert_eq!(err["kind"], "invalid", "{err}");
        applied(&mut b);
        assert_eq!(b.text(&first), "hello from a\n");
        println!("ok  B pull applied at once, Undo restored, pulled again");

        // B's own commit can't be pushed until it pulls; then it goes after GitHub's.
        a.write(&first, "hello from a\nand a second line\n");
        pushed(&mut a, "e2e: second line", &[&first]);
        let second = entry(&mut b, format!("{tag} from b"), "hello from b\n");
        let out = b.ok(
            "commit_and_push",
            json!({ "message": "e2e: entry from b", "paths": [second] }),
        );
        assert_eq!(out["result"], "needs_pull", "{out}");
        let view = &out["view"];
        assert_eq!(
            (view["ahead"].as_u64(), view["behind"].as_u64()),
            (Some(1), Some(1)),
            "{out}"
        );
        assert_eq!(view["changes"], json!([]), "{out}");
        // The push-only path (blank message, nothing ticked) is refused the same way.
        let out = b.ok("commit_and_push", json!({ "message": "", "paths": [] }));
        assert_eq!(out["result"], "needs_pull", "{out}");
        let summary = applied(&mut b);
        assert_eq!(summary["plan"], "rebase", "{summary}");
        assert_eq!(summary["rebased"], 1, "{summary}");
        assert_eq!(b.text(&first), "hello from a\nand a second line\n");
        assert_eq!(b.text(&second), "hello from b\n");
        // Only this test's commits: earlier runs left merge commits further back in the history.
        assert_eq!(
            b.git(&["log", "-2", "--format=%s"]).trim(),
            "e2e: entry from b\ne2e: second line"
        );
        assert_eq!(
            b.git(&["rev-list", "--merges", "--count", "HEAD~2..HEAD"])
                .trim(),
            "0",
            "history stays a line"
        );
        pushed(&mut b, "", &[]);
        applied(&mut a);
        assert_eq!(a.text(&second), "hello from b\n");
        assert_eq!(head(&a), head(&b));
        println!("ok  B's unpushed commit was placed after A's; push; A pulled it");

        // Different paragraphs of one entry on both devices: a review, with the merge prepared.
        let two = "para one\n\npara two\n";
        a.write(&first, two);
        pushed(&mut a, "e2e: two paragraphs", &[&first]);
        applied(&mut b);
        a.write(&first, "para one, edited on a\n\npara two\n");
        pushed(&mut a, "e2e: edit para one on a", &[&first]);
        let mine = "para one\n\npara two, edited on b\n";
        b.write(&first, mine);
        let before = head(&b);
        let preview = review(&mut b);
        assert_eq!(b.text(&first), mine, "the review wrote to the file");
        assert_eq!(head(&b), before);
        assert_eq!(preview["plan"], "fast_forward", "{preview}");
        assert_eq!(preview["automatic"], false, "{preview}");
        let o = overlap(&preview, &first);
        assert_eq!(o["kind"], "both_modified", "{o}");
        assert_eq!(o["choices"], json!(["mine", "theirs", "both", "merged"]));
        assert_eq!(o["mine"], mine);
        assert_eq!(o["theirs"], "para one, edited on a\n\npara two\n");
        let merged = join_clean(&o["suggestion"]);
        assert_eq!(merged, "para one, edited on a\n\npara two, edited on b\n");
        let again = b.ok("pull_preview", json!({}));
        assert_eq!(again["status"], "review");
        assert_eq!(
            again["preview"]["id"], preview["id"],
            "the review is stable"
        );
        let out = b.ok(
            "apply_pull",
            json!({ "id": preview["id"], "message": null,
                    "resolutions": [{ "path": first, "choice": "merged", "text": merged }] }),
        );
        assert_eq!(out["summary"]["plan"], "fast_forward", "{out}");
        assert_eq!(
            out["summary"]["resolved"],
            json!([{ "path": first, "choice": "merged", "copy": null }])
        );
        assert_eq!(b.text(&first), merged);
        // The resolution is left uncommitted, as the edit was.
        assert!(
            has_change(&out["view"]["changes"], &first, "modified"),
            "{out}"
        );
        assert_eq!(out["view"]["behind"], 0, "{out}");
        pushed(&mut b, "e2e: merged paragraphs on b", &[&first]);
        applied(&mut a);
        assert_eq!(a.text(&first), merged);
        let out = a.ok("undo_sync", json!({}));
        assert_eq!(out["restored"], json!([first]), "{out}");
        assert_eq!(a.text(&first), "para one, edited on a\n\npara two\n");
        applied(&mut a);
        assert_eq!(a.text(&first), merged);
        println!("ok  different paragraphs: review, merged on B, pushed; A applied, undone, again");

        // The same line on both: a clash. An edit after the review opened makes it stale.
        a.write(&first, "para one, from a again\n\npara two, edited on b\n");
        pushed(&mut a, "e2e: para one again on a", &[&first]);
        b.write(&first, "para one, from b\n\npara two, edited on b\n");
        let preview = review(&mut b);
        let o = overlap(&preview, &first);
        assert_eq!(o["suggestion"]["clean"], false, "{o}");
        let newer = "para one, from b (newer)\n\npara two, edited on b\n";
        b.write(&first, newer);
        let err = b.err(
            "apply_pull",
            json!({ "id": preview["id"], "message": null,
                    "resolutions": [{ "path": first, "choice": "mine" }] }),
        );
        assert_eq!(err["kind"], "stale_review", "{err}");
        assert_eq!(b.text(&first), newer);
        assert!(b.ok("sync_state", json!({}))["error"].is_null());
        let fresh = b.ok("pull_preview", json!({}));
        assert_eq!(fresh["status"], "review", "{fresh}");
        let preview = fresh["preview"].clone();
        let copy = overlap(&preview, &first)["copy"]
            .as_str()
            .expect("a Keep both name")
            .to_owned();
        let stem = first.trim_end_matches(".md");
        let stamp = copy
            .strip_prefix(&format!("{stem}.from-github-"))
            .and_then(|rest| rest.strip_suffix(".md"))
            .unwrap_or_else(|| panic!("copy name {copy}"));
        assert!(
            stamp.len() == 4 && stamp.bytes().all(|c| c.is_ascii_digit()),
            "{copy}"
        );
        let out = b.ok(
            "apply_pull",
            json!({ "id": preview["id"], "message": null,
                    "resolutions": [{ "path": first, "choice": "both" }] }),
        );
        assert_eq!(
            out["summary"]["resolved"],
            json!([{ "path": first, "choice": "both", "copy": copy }])
        );
        assert_eq!(b.text(&first), newer);
        assert_eq!(
            b.text(&copy),
            "para one, from a again\n\npara two, edited on b\n"
        );
        pushed(&mut b, "e2e: kept both", &[&first, &copy]);
        let summary = applied(&mut a);
        assert!(has_change(&summary["files"], &copy, "added"), "{summary}");
        assert_eq!(a.text(&first), newer);
        println!("ok  same line: stale review refused, fresh review, kept both ({copy})");

        // Resolve with git: B's uncommitted edit is committed first, Abort brings it back.
        a.write(&second, "hello from b, edited on a\n");
        pushed(&mut a, "e2e: second edited on a", &[&second]);
        let b_edit = "hello from b, edited on b\n";
        b.write(&second, b_edit);
        let before = head(&b);
        let preview = review(&mut b);
        let id = preview["id"].clone();
        let err = b.err("start_external_merge", json!({ "id": id, "message": null }));
        assert_eq!(err["kind"], "empty_message", "{err}");
        let _ = b.events();
        let view = b.ok(
            "start_external_merge",
            json!({ "id": id, "message": "e2e: b's edit, before merging" }),
        );
        assert_eq!(
            view["merge"],
            json!({ "conflicted": [second], "startedByApp": true }),
            "{view}"
        );
        let marked = b.text(&second);
        for marker in ["<<<<<<< HEAD\n", "=======\n", ">>>>>>> origin/main\n"] {
            assert!(marked.contains(marker), "{marked}");
        }
        assert!(b.journal.join(".git/MERGE_HEAD").is_file());
        assert!(
            b.git(&["status", "--porcelain"])
                .contains(&format!("UU {second}")),
            "git sees the conflict"
        );
        assert!(b
            .events()
            .iter()
            .any(|e| e["event"] == state::ENTRIES_CHANGED));
        for (cmd, args) in [
            ("pull", json!({})),
            (
                "commit_and_push",
                json!({ "message": "nope", "paths": [second] }),
            ),
            ("commit", json!({ "message": "nope", "paths": [second] })),
        ] {
            let err = b.err(cmd, args);
            assert_eq!(err["kind"], "merge_in_progress", "{cmd}: {err}");
        }
        // Resolve with git also ended Undo's record; either answer leaves the merge alone.
        let err = b.err("undo_sync", json!({}));
        assert!(
            err["kind"] == "merge_in_progress" || err["kind"] == "invalid",
            "{err}"
        );
        let view = b.ok("fetch", json!({}));
        assert!(view["error"].is_null(), "{view}");
        assert!(!view["merge"].is_null(), "{view}");
        let out = b.ok("abort_merge", json!({}));
        assert!(
            out["restored"].as_array().unwrap().contains(&json!(second)),
            "{out}"
        );
        assert!(out["view"]["merge"].is_null(), "{out}");
        assert_eq!(b.text(&second), b_edit);
        assert_eq!(head(&b), before, "the commit made first is undone");
        assert!(
            has_change(&out["view"]["changes"], &second, "modified"),
            "{out}"
        );
        assert!(!b.journal.join(".git/MERGE_HEAD").exists());
        println!("ok  Resolve with git: markers, buttons refused, Abort restored B's edit");

        // Again, and this time finished in a terminal.
        let preview = review(&mut b);
        b.ok(
            "start_external_merge",
            json!({ "id": preview["id"], "message": "e2e: b's edit, before merging" }),
        );
        let resolved = "hello from b, edited on a and on b\n";
        fs::write(b.journal.join(&second), resolved).unwrap();
        b.git(&["add", "-A"]);
        b.git(&["commit", "--no-edit"]);
        let view = b.ok("sync_state", json!({}));
        assert!(view["merge"].is_null(), "{view}");
        assert_eq!(view["changes"], json!([]), "{view}");
        pushed(&mut b, "", &[]);
        applied(&mut a);
        assert_eq!(a.text(&second), resolved);
        println!("ok  merge finished with the git CLI; pushed; A pulled it at once");

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
        let on_a = a.bytes(&image_path);
        assert_eq!(on_a.len() as u64, image["bytes"].as_u64().unwrap());
        assert_eq!(&on_a[..3], &[0xff, 0xd8, 0xff], "a JPEG");
        let diff = a.ok("diff", json!({ "path": image_path }));
        assert_eq!(
            (diff["binary"].clone(), diff["hunks"].clone()),
            (json!(true), json!([]))
        );
        let with_image = format!("{newer}\n![]({image_path})\n");
        a.write(&first, &with_image);
        pushed(&mut a, "e2e: image", &[&first, &image_path]);
        let summary = applied(&mut b);
        assert!(
            has_change(&summary["files"], &image_path, "added"),
            "{summary}"
        );
        assert!(on_a == b.bytes(&image_path), "the image differs on B");
        assert_eq!(b.text(&first), with_image);
        println!(
            "ok  image {image_path} ({} bytes) identical on B",
            on_a.len()
        );

        // Rename on A while B edited the entry: the edit waits for B, the new name arrives.
        let renamed = a.ok(
            "rename_entry",
            json!({ "file": first, "newTitle": format!("{tag} renamed") }),
        );
        let renamed = renamed["file"].as_str().unwrap().to_owned();
        assert_ne!(renamed, first);
        let view = a.ok("sync_state", json!({}));
        assert_eq!(view["changes"][0]["kind"], "renamed", "{view}");
        assert_eq!(view["changes"][0]["oldPath"], first.as_str(), "{view}");
        let diff = a.ok("diff", json!({ "path": renamed }));
        assert_eq!(
            (
                diff["kind"].clone(),
                diff["oldPath"].clone(),
                diff["hunks"].clone()
            ),
            (json!("renamed"), json!(first), json!([]))
        );
        pushed(&mut a, "e2e: rename", &[&renamed]);
        assert_eq!(
            a.git(&["ls-tree", "--name-only", "HEAD", &first]).trim(),
            "",
            "ticking a rename removes the old name"
        );
        let edited_on_b = format!("{with_image}a line written on b\n");
        b.write(&first, &edited_on_b);
        let preview = review(&mut b);
        let o = overlap(&preview, &first);
        assert_eq!(o["kind"], "modified_deleted", "{o}");
        assert_eq!(o["choices"], json!(["mine", "theirs"]));
        assert!(has_change(&preview["auto"], &renamed, "added"), "{preview}");
        b.ok(
            "apply_pull",
            json!({ "id": preview["id"], "message": null,
                    "resolutions": [{ "path": first, "choice": "mine" }] }),
        );
        assert_eq!(b.text(&first), edited_on_b);
        assert_eq!(b.text(&renamed), with_image);
        pushed(&mut b, "e2e: b kept its edit after the rename", &[&first]);
        applied(&mut a);
        assert_eq!(a.text(&first), edited_on_b);
        println!("ok  rename on A + edit on B: B kept its text, the new name arrived");

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
        let offline = entry(
            &mut a,
            format!("{tag} offline"),
            "written while disconnected\n",
        );
        let view = a.ok(
            "commit",
            json!({ "message": "e2e: offline commit", "paths": [offline] }),
        );
        assert_eq!(view["changes"], json!([]), "{view}");
        println!(
            "ok  disconnected; local commit author: {}",
            author_of(&a, "HEAD")
        );
        let out = a.ok("github_connect", json!({ "fullName": repo }));
        assert_eq!(out["status"], "up_to_date", "{out}");
        assert_eq!(out["view"]["ahead"], 1, "{out}");
        assert_eq!(out["view"]["behind"], 0, "{out}");
        pushed(&mut a, "", &[]);
        let summary = applied(&mut b);
        assert!(
            has_change(&summary["files"], &offline, "added"),
            "{summary}"
        );
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
        a.write(
            &offline,
            "written while disconnected\nand after a restart\n",
        );
        pushed(&mut a, "e2e: after restart", &[&offline]);
        assert_eq!(
            author_of(&a, "HEAD"),
            format!("{expected_author}|{expected_author}"),
            "author after a restart"
        );
        applied(&mut b);
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
        a.write(
            &offline,
            "written while disconnected\nand after a restart\nwith a settings author\n",
        );
        pushed(&mut a, "e2e: settings author", &[&offline]);
        assert_eq!(
            author_of(&a, "HEAD"),
            "Settings Author <author@example.com>|Settings Author <author@example.com>"
        );
        settings["authorName"] = Value::Null;
        settings["authorEmail"] = Value::Null;
        a.ok("set_settings", json!({ "settings": settings }));
        applied(&mut b);
        println!("ok  settings author used; set_settings kept the remote");

        // A third device with its own history, a draft and a different file at a name GitHub has:
        // connecting opens a review for that file only; its commit goes after GitHub's.
        let mut c = Device::launch("c", root, token);
        let own = entry(&mut c, format!("{tag} c own"), "c's history\n");
        c.ok(
            "commit",
            json!({ "message": "e2e: c local commit", "paths": [own] }),
        );
        let c_draft = entry(&mut c, format!("{tag} c draft"), "c's draft\n");
        fs::write(c.journal.join(&second), "c's version\n").unwrap();
        c.ok("github_set_pat", json!({ "token": "$TOKEN" }));
        let out = c.ok("github_connect", json!({ "fullName": repo }));
        assert_eq!(out["status"], "review", "{out}");
        assert_eq!(out["view"]["connected"], true, "{out}");
        let preview = out["preview"].clone();
        assert_eq!(paths(&preview["overlaps"]), [second.as_str()], "{preview}");
        let o = overlap(&preview, &second);
        assert_eq!(o["kind"], "both_added", "{o}");
        assert_eq!(o["base"], Value::Null);
        assert_eq!(preview["plan"], "rebase", "{preview}");
        assert_eq!(c.text(&second), "c's version\n", "connecting wrote to it");
        let out = c.ok(
            "apply_pull",
            json!({ "id": preview["id"], "message": null,
                    "resolutions": [{ "path": second, "choice": "both" }] }),
        );
        let c_copy = out["summary"]["resolved"][0]["copy"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(out["summary"]["rebased"], 1, "{out}");
        assert_eq!(c.text(&second), "c's version\n");
        assert_eq!(c.text(&c_copy), a.text(&second));
        assert_eq!(c.text(&c_draft), "c's draft\n");
        assert_eq!(
            c.git(&["log", "-1", "--format=%s"]).trim(),
            "e2e: c local commit"
        );
        pushed(&mut c, "", &[]);
        let summary = applied(&mut a);
        assert!(has_change(&summary["files"], &own, "added"), "{summary}");
        assert!(
            !a.journal.join(&c_draft).exists(),
            "C's draft wasn't ticked"
        );
        println!("ok  device with its own history joined: one review, rebased, draft kept");

        // The token is nowhere on disk but in the token stores, and no keyring was touched.
        let allowed = [&a, &b, &c].map(|d| d.config().join("credentials.json"));
        assert_not_on_disk(root, token, &allowed);
        for d in [&mut a, &mut b, &mut c] {
            let config = fs::read_to_string(d.journal.join(".git/config")).unwrap();
            assert!(!config.contains(token));
            let settings = fs::read_to_string(d.config().join("settings.json")).unwrap();
            assert!(!settings.contains(token));
            assert_eq!(d.ok("keyring_builds", json!({})), 0, "{}", d.name);
        }
        println!("ok  token only in credentials.json (also not in logs or .git); no keyring used");
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
        // An empty journal: never a review.
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
            let changes = paths(&c.ok("sync_state", json!({}))["changes"]);
            let out = c.ok(
                "commit_and_push",
                json!({ "message": format!("e2e: remove {tag}"), "paths": changes }),
            );
            match out["result"].as_str() {
                Some("pushed") => {
                    let left = c.git(&["ls-tree", "-r", "--name-only", "origin/main"]);
                    assert!(!left.contains(tag), "left on the remote: {left}");
                    println!("cleanup: removed {} files from {repo}", removed.len());
                    return;
                }
                Some("needs_pull") => {
                    let out = c.ok("pull", json!({}));
                    if out["status"] == "review" {
                        // Someone changed the files meanwhile: the deletions win.
                        let preview = &out["preview"];
                        let resolutions: Vec<Value> = paths(&preview["overlaps"])
                            .into_iter()
                            .map(|path| json!({ "path": path, "choice": "mine" }))
                            .collect();
                        c.ok(
                            "apply_pull",
                            json!({ "id": preview["id"], "resolutions": resolutions,
                                    "message": format!("e2e: merge before removing {tag}") }),
                        );
                    }
                }
                _ => panic!("cleanup: {out}"),
            }
        }
        panic!("cleanup: could not push the deletions");
    }
}
