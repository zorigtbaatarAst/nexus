//! Hooks are the deterministic invocation tier, and they ship off by default (ADR-024).
//!
//! The property that decides whether they survive contact with a real developer is
//! fail-open: a tool that occasionally hangs or breaks a session is uninstalled once and
//! never reinstalled. That is asserted here by running the hook's own command string with
//! `nexus` absent from `PATH`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn nexus() -> PathBuf {
    // target/debug/deps/<test binary> -> target/debug/nexus
    let mut p = std::env::current_exe().expect("test binary path");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("nexus")
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-hooks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    root
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(nexus())
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run nexus")
}

fn settings(root: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(root.join(".claude/settings.json")).ok()?;
    Some(serde_json::from_str(&raw).expect("settings.json is valid JSON"))
}

fn session_hooks(v: &Value) -> &Vec<Value> {
    v["hooks"]["SessionStart"]
        .as_array()
        .expect("a SessionStart array")
}

#[test]
fn init_writes_no_hooks_by_default() {
    let root = fixture("default");
    let out = run(&root, &["init"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        settings(&root).is_none(),
        "hooks are opt-in: plain init must write nothing outside .nexus/ (ADR-024)"
    );
}

#[test]
fn init_with_hooks_installs_the_session_start_hook() {
    let root = fixture("install");
    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{out:?}");
    let v = settings(&root).expect("settings.json written");
    let entries = session_hooks(&v);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let cmd = entries[0]["hooks"][0]["command"]
        .as_str()
        .expect("a command string");
    assert!(cmd.contains("context --session"), "{cmd}");
    assert!(
        entries[0]["hooks"][0]["timeout"].is_number(),
        "a hook without a timeout can hang a session: {entries:?}"
    );
}

#[test]
fn installing_twice_changes_nothing() {
    let root = fixture("idempotent");
    run(&root, &["init", "--hooks"]);
    let first = settings(&root).expect("written");
    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{out:?}");
    let second = settings(&root).expect("still there");
    assert_eq!(
        first, second,
        "a second install must not duplicate the hook"
    );
    assert_eq!(session_hooks(&second).len(), 1);
}

#[test]
fn an_existing_settings_file_is_merged_never_clobbered() {
    let root = fixture("merge");
    std::fs::create_dir_all(root.join(".claude")).expect("mkdir");
    std::fs::write(
        root.join(".claude/settings.json"),
        r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo bye"}]}]}}"#,
    )
    .expect("seed");

    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{out:?}");
    let v = settings(&root).expect("written");
    assert_eq!(v["model"], "opus", "an unrelated key was destroyed");
    assert_eq!(
        v["hooks"]["Stop"][0]["hooks"][0]["command"], "echo bye",
        "another hook was destroyed"
    );
    assert_eq!(session_hooks(&v).len(), 1, "ours was still added");
}

#[test]
fn the_hook_command_fails_open_when_nexus_is_not_on_path() {
    // The acceptance criterion for 1.8: removing nexus from PATH mid-session must leave the
    // harness fully working. The hook is a shell string, so this runs the real one.
    let root = fixture("failopen");
    run(&root, &["init", "--hooks"]);
    let v = settings(&root).expect("written");
    let cmd = session_hooks(&v)[0]["hooks"][0]["command"]
        .as_str()
        .expect("command")
        .to_string();

    // The shell is named absolutely: emptying PATH must hide `nexus`, not the interpreter
    // the hook is written in.
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .env("PATH", "/nonexistent")
        .current_dir(&root)
        .output()
        .expect("sh");
    assert!(
        out.status.success(),
        "the hook must exit 0 with nexus absent: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "and print nothing, or the agent reads an error as context: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}
