//! scripts/two-devices.sh: syntax, help, and dry runs with stand-in binaries in a scratch HOME, so
//! the real ~/mdgitjournal-test-devices is never touched.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/two-devices.sh")
}

/// The script under `sh`, with `home` as HOME and no inherited MDGJ_* or CARGO_TARGET_DIR.
fn two_devices(home: &Path, bin: Option<&Path>, args: &[&str]) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg(script())
        .args(args)
        .env("HOME", home)
        .env_remove("MDGJ_BIN")
        .env_remove("MDGJ_JOURNAL_DIR")
        .env_remove("MDGJ_CONFIG_DIR")
        .env_remove("MDGJ_DEVICE_NAME")
        .env_remove("CARGO_TARGET_DIR");
    if let Some(bin) = bin {
        cmd.env("MDGJ_BIN", bin);
    }
    cmd
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("run sh")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// An executable shell script standing in for the app.
fn fake_app(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-mdgitjournal");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn parses_as_posix_sh() {
    for shell in ["sh", "dash"] {
        let Ok(out) = Command::new(shell).arg("-n").arg(script()).output() else {
            continue; // dash isn't everywhere
        };
        assert!(out.status.success(), "{shell} -n: {}", text(&out.stderr));
    }
    match Command::new("shellcheck").arg(script()).output() {
        Ok(out) => assert!(
            out.status.success(),
            "shellcheck:\n{}{}",
            text(&out.stdout),
            text(&out.stderr)
        ),
        Err(_) => eprintln!("shellcheck not installed; skipped"),
    }
}

#[test]
fn help_and_bad_options() {
    let home = tempfile::tempdir().unwrap();
    for flag in ["--help", "-h"] {
        let out = run(&mut two_devices(home.path(), None, &[flag]));
        assert!(out.status.success());
        let help = text(&out.stdout);
        assert!(help.starts_with("Usage: scripts/two-devices.sh [--dev] [--reset]"));
        assert!(
            help.contains("--reset") && help.contains("MDGJ_BIN"),
            "{help}"
        );
    }
    let out = run(&mut two_devices(home.path(), None, &["--bogus"]));
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).starts_with("Usage:"));
    // Neither creates anything.
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn dry_run_creates_both_devices_and_gives_each_its_own_folders() {
    let home = tempfile::tempdir().unwrap();
    let out = run(&mut two_devices(
        home.path(),
        Some(Path::new("/usr/bin/true")),
        &[],
    ));
    assert!(out.status.success(), "{}", text(&out.stderr));
    let devices = home.path().join("mdgitjournal-test-devices");
    for d in ["a", "b"] {
        for sub in ["journal", "config"] {
            assert!(devices.join(d).join(sub).is_dir(), "{d}/{sub} missing");
        }
        assert!(devices.join(d).join("app.log").is_file());
    }
    let stdout = text(&out.stdout);
    assert!(stdout.contains("Device A: journal ") && stdout.contains("Device B: journal "));
    assert!(stdout.contains(&devices.join("a/config").display().to_string()));

    // What each instance gets.
    let scratch = tempfile::tempdir().unwrap();
    let app = fake_app(
        scratch.path(),
        r#"printf '%s|%s|%s\n' "$MDGJ_JOURNAL_DIR" "$MDGJ_CONFIG_DIR" "$MDGJ_DEVICE_NAME""#,
    );
    let out = run(&mut two_devices(home.path(), Some(&app), &[]));
    assert!(out.status.success(), "{}", text(&out.stderr));
    for (d, name) in [("a", "Device A"), ("b", "Device B")] {
        let log = fs::read_to_string(devices.join(d).join("app.log")).unwrap();
        let dir = devices.join(d);
        let expected = format!(
            "{}|{}|{name}\n",
            dir.join("journal").display(),
            dir.join("config").display()
        );
        assert_eq!(log, expected, "device {d}");
    }
}

#[test]
fn reset_deletes_only_the_test_devices() {
    let home = tempfile::tempdir().unwrap();
    let devices = home.path().join("mdgitjournal-test-devices");
    fs::create_dir_all(devices.join("a/journal")).unwrap();
    fs::write(devices.join("a/journal/old.md"), "old\n").unwrap();
    let keep = home.path().join("journal-i-care-about.md");
    fs::write(&keep, "mine\n").unwrap();

    let true_bin = Path::new("/usr/bin/true");
    // Without --reset the folders are kept.
    assert!(run(&mut two_devices(home.path(), Some(true_bin), &[]))
        .status
        .success());
    assert!(devices.join("a/journal/old.md").is_file());

    let out = run(&mut two_devices(home.path(), Some(true_bin), &["--reset"]));
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Deleting "));
    assert!(!devices.join("a/journal/old.md").exists());
    assert!(devices.join("a/journal").is_dir(), "recreated");
    assert_eq!(fs::read_to_string(&keep).unwrap(), "mine\n");
}

#[test]
fn a_missing_binary_says_how_to_build_it() {
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("nowhere/mdgitjournal");
    let out = run(&mut two_devices(home.path(), Some(&missing), &[]));
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no mdgitjournal at"));

    // The default location, in a target folder that has no build.
    let target = tempfile::tempdir().unwrap();
    let out = run(two_devices(home.path(), None, &[]).env("CARGO_TARGET_DIR", target.path()));
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("pnpm -C app tauri build"), "{err}");
    let out =
        run(two_devices(home.path(), None, &["--dev"]).env("CARGO_TARGET_DIR", target.path()));
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("target/debug") || text(&out.stderr).contains("debug"));
    assert!(!home.path().join("mdgitjournal-test-devices").exists());
}

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn interrupting_the_script_ends_both_instances() {
    let home = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    // Records its pid, then runs until killed.
    let app = fake_app(
        scratch.path(),
        r#"echo $$ > "$MDGJ_CONFIG_DIR/pid"; exec sleep 60"#,
    );
    let mut script = two_devices(home.path(), Some(&app), &[])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let devices = home.path().join("mdgitjournal-test-devices");
    let pid_file = |d: &str| devices.join(d).join("config/pid");
    let started = Instant::now();
    let pids: Vec<String> = loop {
        let pids: Vec<String> = ["a", "b"]
            .iter()
            .filter_map(|d| fs::read_to_string(pid_file(d)).ok())
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect();
        if pids.len() == 2 {
            break pids;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "not started");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(pids.iter().all(|p| alive(p)));

    // As Ctrl-C does to the script (the instances, started in the background, ignore SIGINT).
    let status = Command::new("kill")
        .args(["-INT", &script.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let code = script.wait().unwrap();
    assert_eq!(code.code(), Some(130), "{code:?}");
    let started = Instant::now();
    while pids.iter().any(|p| alive(p)) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "an instance outlived the script"
        );
        thread::sleep(Duration::from_millis(50));
    }
}
