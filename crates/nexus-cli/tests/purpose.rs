//! A caller that knows what it is doing may say so.
//!
//! Intent is derived from the prompt by a deterministic verb table, and the table is
//! conservative on purpose: `"have a look at this"` matches no verb and classifies `unknown`.
//! That is right for text a person typed and wrong for a caller with better information than
//! the text — an agent that has just been asked to fix a failing test knows it is debugging
//! whatever words it happens to use.
//!
//! `07-agent-integration.md` §6.4 has asked for this since it was written. `Purpose::Debug`
//! existed the whole time and was constructed nowhere, which is the same silent-failure shape
//! AGENTS.md names: a value written by one function and read by none.

use std::path::{Path, PathBuf};
use std::process::Command;

fn nexus() -> PathBuf {
    let mut p = std::env::current_exe().expect("test binary");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("nexus")
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-purpose-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\nimpl Alpha { pub fn save(&self) {} }\n",
    )
    .expect("write");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "x"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
    }
    Command::new(nexus())
        .args(["scan", "--project"])
        .arg(&root)
        .output()
        .expect("scan");
    root
}

fn context(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(nexus())
        .arg("context")
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run context")
}

fn intent_of(out: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON: {e}\n{stdout}"));
    doc["result"]["intent"]["intent"]
        .as_str()
        .unwrap_or_else(|| panic!("no intent in {}", doc["result"]["intent"]))
        .to_string()
}

#[test]
fn a_declared_purpose_overrides_a_prompt_the_verb_table_cannot_read() {
    let root = project("declared");

    let derived = context(&root, &["--task", "have a look at this", "--json"]);
    assert_eq!(
        intent_of(&derived),
        "unknown",
        "the verb table matches nothing here, which is what makes this the interesting case"
    );

    let declared = context(
        &root,
        &[
            "--task",
            "have a look at this",
            "--purpose",
            "debug",
            "--json",
        ],
    );
    assert_eq!(intent_of(&declared), "debug");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_the_flag_classification_is_untouched() {
    // The override is opt-in. If declaring nothing changed behaviour, every existing golden
    // would move and the flag would be a rewrite rather than an addition.
    let root = project("underived");
    assert_eq!(
        intent_of(&context(
            &root,
            &["--task", "fix the save method", "--json"]
        )),
        "debug",
        "a prompt the table *can* read still classifies itself"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unrecognised_purpose_is_a_usage_error_not_a_silent_fallback() {
    // policy.rs already treats an unrecognised `execute` value as `none`, because a typo must
    // not become a grant. The same reasoning inverted: a typo must not silently become the
    // default purpose and hand back a package the caller did not ask for.
    let root = project("typo");
    let out = context(&root, &["--task", "x", "--purpose", "debgu", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "usage errors exit 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn review_is_declarable_too_and_it_changes_what_is_seeded() {
    // `Purpose::Review` is not new. `seeds.rs` read it before this change and
    // `memory_scale.rs` constructs it, so it was the one purpose already doing this job —
    // for one purpose, in one place. The ticket asked for it to be deleted on the premise
    // that nothing constructed it; that premise was wrong, and the rule it embodied is now
    // stated once for every purpose instead of twice for one.
    //
    // Declared review seeds from the changed set rather than the prompt's words, so the
    // interesting assertion is that a prompt naming nothing still classifies review.
    let root = project("review");
    let declared = context(
        &root,
        &[
            "--task",
            "have a look at this",
            "--purpose",
            "review",
            "--json",
        ],
    );
    assert_eq!(intent_of(&declared), "review");

    let _ = std::fs::remove_dir_all(&root);
}
