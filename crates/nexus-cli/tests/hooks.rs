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

#[test]
fn the_prompt_hook_is_installed_alongside_the_session_hook() {
    let root = fixture("prompt");
    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{out:?}");
    let v = settings(&root).expect("written");
    let entries = v["hooks"]["UserPromptSubmit"]
        .as_array()
        .expect("a UserPromptSubmit array");
    assert_eq!(entries.len(), 1, "{entries:?}");
    let cmd = entries[0]["hooks"][0]["command"].as_str().expect("command");
    assert!(cmd.contains("context --task"), "{cmd}");
    assert!(entries[0]["hooks"][0]["timeout"].is_number(), "{entries:?}");
    // Both hooks, and installing again adds neither a second time.
    assert_eq!(session_hooks(&v).len(), 1);
    run(&root, &["init", "--hooks"]);
    let again = settings(&root).expect("written");
    assert_eq!(again, v, "a second install must change nothing");
}

#[test]
fn the_prompt_hook_command_also_fails_open() {
    let root = fixture("promptfailopen");
    run(&root, &["init", "--hooks"]);
    let v = settings(&root).expect("written");
    let cmd = v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
        .as_str()
        .expect("command")
        .to_string();
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .env("PATH", "/nonexistent")
        .current_dir(&root)
        .output()
        .expect("sh");
    assert!(out.status.success(), "{out:?}");
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn every_installed_hook_fails_open() {
    // ADR-024's table. Fail-open is not a property of one of them: a single hook that hangs
    // or errors is enough for someone to disable the lot, and then none of them run.
    //
    // `--verify` is passed so the Stop gate is covered too — it is the hook most likely to
    // fail in the field, because it is the only one that runs a build.
    let root = fixture("failopen");
    assert!(run(&root, &["init", "--hooks", "--verify"])
        .status
        .success());
    let v = settings(&root).expect("written");

    for event in ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop"] {
        let entries = v["hooks"][event]
            .as_array()
            .unwrap_or_else(|| panic!("no {event} array in {v}"));
        assert_eq!(entries.len(), 1, "{event}: {entries:?}");
        let hook = &entries[0]["hooks"][0];
        assert!(hook["timeout"].is_number(), "{event} has no timeout");

        let cmd = hook["command"].as_str().expect("command").to_string();
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(&cmd)
            .env("PATH", "/nonexistent")
            .current_dir(&root)
            .output()
            .expect("sh");
        assert!(out.status.success(), "{event} did not exit 0: {out:?}");
        assert!(
            out.stdout.is_empty(),
            "{event} printed on failure: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    // And installing again adds none of them a second time.
    assert!(run(&root, &["init", "--hooks", "--verify"])
        .status
        .success());
    assert_eq!(settings(&root).expect("written"), v);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_edit_hook_names_the_tools_that_change_files() {
    // Without a matcher, `PostToolUse` fires for every tool the agent calls — every Read,
    // Grep and Bash — so the index is rescanned after work that cannot have changed it, on
    // the developer's critical path. ADR-024's table always said `PostToolUse (Edit|Write)`;
    // the installer just never said it to the harness.
    let root = fixture("matcher");
    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{out:?}");
    let v = settings(&root).expect("settings.json written");

    let post = v["hooks"]["PostToolUse"]
        .as_array()
        .expect("a PostToolUse array");
    let matcher = post[0]["matcher"].as_str().expect("a matcher");
    for tool in ["Edit", "Write"] {
        assert!(
            matcher.contains(tool),
            "{tool} must trigger a rescan: {matcher}"
        );
    }
    assert!(
        !matcher.contains("Read") && !matcher.contains("Grep") && !matcher.contains("Bash"),
        "reading a file cannot change the index: {matcher}"
    );
}

#[test]
fn the_context_hooks_carry_no_matcher() {
    // `SessionStart` and `UserPromptSubmit` have no tool to match on. An empty string is a
    // value the harness is entitled to interpret; absence is not.
    let root = fixture("no-matcher");
    run(&root, &["init", "--hooks"]);
    let v = settings(&root).expect("settings.json written");
    for event in ["SessionStart", "UserPromptSubmit"] {
        let entries = v["hooks"][event].as_array().expect("an array");
        assert!(
            entries[0].get("matcher").is_none(),
            "{event} must carry no matcher: {entries:?}"
        );
    }
}

#[test]
fn the_verification_gate_is_not_installed_by_default() {
    // `verify --changed` accepts the flag and ignores it, so the Stop hook runs a full build
    // of the whole project at the end of every turn. That is worth choosing; it is not worth
    // acquiring as a side effect of turning on a context hook.
    let root = fixture("no-verify");
    run(&root, &["init", "--hooks"]);
    let v = settings(&root).expect("settings.json written");
    assert!(
        v["hooks"].get("Stop").is_none(),
        "the build gate must be opted into separately: {v}"
    );
}

#[test]
fn the_verification_gate_installs_when_asked_for() {
    let root = fixture("verify");
    let out = run(&root, &["init", "--hooks", "--verify"]);
    assert!(out.status.success(), "{out:?}");
    let v = settings(&root).expect("settings.json written");
    let stop = v["hooks"]["Stop"].as_array().expect("a Stop array");
    let cmd = stop[0]["hooks"][0]["command"].as_str().expect("a command");
    assert!(cmd.contains("verify"), "{cmd}");
    // Its budget is a build, not a context lookup.
    let timeout = stop[0]["hooks"][0]["timeout"].as_u64().expect("a timeout");
    assert!(
        timeout > 60,
        "a build gate needs more than a context timeout: {timeout}"
    );
}

#[test]
fn asking_for_the_gate_without_the_hooks_is_refused() {
    // `--verify` alone would silently do nothing. Better to say so than to exit 0 having
    // installed neither.
    let root = fixture("verify-alone");
    let out = run(&root, &["init", "--verify"]);
    assert!(
        !out.status.success(),
        "--verify without --hooks must not succeed"
    );
}

/// `doctor` output as a map of check name to (level, detail).
fn doctor_hooks(root: &Path, path_env: Option<&str>) -> (String, String) {
    let mut cmd = Command::new(nexus());
    cmd.args(["doctor", "--json"]).arg("--project").arg(root);
    if let Some(p) = path_env {
        cmd.env("PATH", p);
    }
    let out = cmd.output().expect("run doctor");
    let doc: Value = serde_json::from_slice(&out.stdout).expect("doctor --json emits one document");
    let checks = doc["result"].as_array().expect("an array of checks");
    let hooks = checks
        .iter()
        .find(|c| c["name"] == "hooks")
        .unwrap_or_else(|| panic!("doctor reported no hooks check: {doc}"));
    (
        hooks["level"].as_str().expect("level").to_string(),
        hooks["detail"].as_str().expect("detail").to_string(),
    )
}

#[test]
fn doctor_reports_uninstalled_hooks_as_fine() {
    // Off is a supported state, not a fault: ADR-024 ships them off by default. Reporting a
    // warning for the default configuration is how a doctor teaches people to ignore it.
    let root = fixture("doc-none");
    run(&root, &["init"]);
    let (level, detail) = doctor_hooks(&root, None);
    assert_eq!(level, "ok", "{detail}");
    assert!(detail.contains("not installed"), "{detail}");
}

#[test]
fn doctor_reports_installed_hooks() {
    let root = fixture("doc-some");
    run(&root, &["init", "--hooks"]);
    let (level, detail) = doctor_hooks(&root, None);
    assert_eq!(level, "ok", "{detail}");
    for event in ["SessionStart", "UserPromptSubmit", "PostToolUse"] {
        assert!(detail.contains(event), "{event} missing from: {detail}");
    }
}

#[test]
fn doctor_catches_the_failure_that_fail_open_hides() {
    // Every hook ends `2>/dev/null || true`, so a hook whose binary is not on PATH looks
    // exactly like a hook that ran and found nothing — forever, with no error anywhere.
    // This is the single failure the check exists for.
    let root = fixture("doc-nopath");
    run(&root, &["init", "--hooks"]);
    // The binary is invoked by absolute path, so it still runs; what it cannot find is
    // itself, which is precisely the situation a hook's shell would be in.
    let (level, detail) = doctor_hooks(&root, Some("/nonexistent"));
    assert_eq!(level, "error", "{detail}");
    assert!(detail.contains("silently does nothing"), "{detail}");
}

#[test]
fn doctor_refuses_to_guess_at_unreadable_settings() {
    let root = fixture("doc-broken");
    run(&root, &["init"]);
    std::fs::create_dir_all(root.join(".claude")).expect("mkdir");
    std::fs::write(root.join(".claude/settings.json"), "{not json").expect("write");
    let (level, detail) = doctor_hooks(&root, None);
    assert_eq!(level, "error", "{detail}");
    assert!(detail.contains("not valid JSON"), "{detail}");
}
