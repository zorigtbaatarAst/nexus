//! `--brief`: stop paying for the same three lines on every prompt.
//!
//! `SessionStart` sends the project profile once. `UserPromptSubmit` sends it again on every
//! prompt, whatever the prompt says. Measured on the Nexus repository: 234–256 tokens of
//! profile header per prompt, which is half of a small package and the entirety of an empty
//! one. Across a fourteen-prompt conversation that is ~3,500 tokens of pure duplication.
//!
//! The flag is opt-in and the hook opts in. A person running `nexus context --task` at a
//! terminal keeps the header, because for them it is the useful part.

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
    let root = std::env::temp_dir().join(format!("nexus-brief-{name}-{}", std::process::id()));
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

fn context(root: &Path, args: &[&str]) -> String {
    let out = Command::new(nexus())
        .arg("context")
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run context");
    assert!(
        out.status.success(),
        "context failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn a_package_with_nothing_in_it_prints_nothing() {
    // The whole point. A prompt naming no symbol seeds nothing, and today still costs a
    // profile header the session already sent.
    let root = project("empty");
    let plain = context(&root, &["--task", "yes, that works"]);
    assert!(
        plain.contains("Project:"),
        "without the flag the header is still there:\n{plain}"
    );

    let brief = context(&root, &["--task", "yes, that works", "--brief"]);
    assert_eq!(brief, "", "an empty package under --brief is zero bytes");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_package_with_items_keeps_them_and_drops_only_the_profile() {
    let root = project("items");
    let brief = context(&root, &["--task", "the save method is broken", "--brief"]);

    assert!(
        !brief.is_empty(),
        "this prompt seeds, so something must come back"
    );
    assert!(
        !brief.contains("Project:") && !brief.contains("languages"),
        "the profile is what --brief removes:\n{brief}"
    );
    assert!(
        brief.contains("save"),
        "the items are what it keeps:\n{brief}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_the_flag_nothing_moves() {
    // Every existing golden and every existing caller depends on this.
    let root = project("unchanged");
    let plain = context(&root, &["--task", "the save method is broken"]);
    assert!(
        plain.contains("Project:") && plain.contains("save"),
        "{plain}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn brief_composes_with_json_and_still_emits_one_document() {
    // json_contract.rs pins one document per command. `--brief` must not make that zero, or
    // a `--json | jq` pipeline breaks on empty input rather than on a real error.
    let root = project("json");
    let out = context(&root, &["--task", "yes, that works", "--brief", "--json"]);
    let n = serde_json::Deserializer::from_str(&out)
        .into_iter::<serde_json::Value>()
        .count();
    assert_eq!(n, 1, "--json is a document, not a rendering:\n{out}");
    let _ = std::fs::remove_dir_all(&root);
}
