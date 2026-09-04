//! One binary, two names, and every word it says about itself must use the right one.
//!
//! `argv[0]` decides which product is running — AGENTS.md's single dispatch path. Every
//! user-facing string is supposed to route through `render::product_name()` for that reason.
//! Five did not: the doctor title, and four remedies telling the reader to run a command
//! under the other name. Three of those four live in `nexus-core`, which has no business
//! knowing what the binary is called at all.
//!
//! The advice still worked, because both names are the same file. That is exactly why nobody
//! noticed: a wrong instruction that happens to succeed teaches the reader to stop reading.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().expect("test binary");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join(name)
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-name-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    root
}

fn doctor(exe: &Path, root: &Path) -> String {
    // `doctor` refuses a directory it has never seen, and that refusal is itself one of the
    // messages under test — so initialise first and let the checks with remedies fire.
    Command::new(exe)
        .args(["init", "--project"])
        .arg(root)
        .output()
        .expect("run init");
    let out = Command::new(exe)
        .args(["doctor", "--project"])
        .arg(root)
        .output()
        .expect("run doctor");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn nexus_never_calls_itself_bughunter() {
    // No git repository and no baseline, so the checks that carry remedies all fire: the
    // git advice and the scan advice.
    let root = project("as-nexus");
    let text = doctor(&bin("nexus"), &root);

    assert!(
        text.contains("Nexus doctor"),
        "the title must name the running product:\n{text}"
    );
    assert!(
        !text.to_lowercase().contains("bughunter"),
        "nothing in `nexus doctor` may name the other product:\n{text}"
    );
}

#[test]
fn bughunter_still_calls_itself_bughunter() {
    // The capability's own CLI is a shipped interface, not a legacy alias. Fixing the Nexus
    // side by hardcoding the other name would just move the bug.
    let root = project("as-bughunter");
    let text = doctor(&bin("bughunter"), &root);

    assert!(
        text.contains("BugHunter doctor"),
        "the title must name the running product:\n{text}"
    );
    assert!(
        !text.contains("Nexus doctor"),
        "and must not name the platform:\n{text}"
    );
}

#[test]
fn a_remedy_names_the_binary_the_reader_invoked() {
    // The remedies are the actionable half of the report. `nexus doctor` telling a reader to
    // run `bughunter scan` is advice they must translate before using.
    let root = project("remedy");
    let text = doctor(&bin("nexus"), &root);

    assert!(
        text.contains("nexus scan") || text.contains("nexus rescan"),
        "at least one remedy should name `nexus`:\n{text}"
    );
}
