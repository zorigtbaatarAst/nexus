//! The control arm's flag works, and stays invisible.
//!
//! `09-tooling.md` refuses benchmark-only surfaces in the shipped binary; the spec accepts
//! one anyway, because a separate implementation would have to reproduce the package format
//! and any drift would silently favour one side of the comparison. The price of that
//! exception is that the flag is undocumented, and this test is what keeps it that way.

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
    let root = std::env::temp_dir().join(format!("nexus-arm-{name}-{}", std::process::id()));
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

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(nexus())
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run nexus")
}

#[test]
fn the_lexical_arm_produces_a_package() {
    let root = project("works");
    let out = run(
        &root,
        &["context", "--task", "the save method", "--rank", "lexical"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("bm25"),
        "items should say how they were ranked:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The benchmark's analysis parses this renderer's output, and only this renderer's.
///
/// `scripts/eval/analyse.py` counts how many packages came from the lexical fallback so a sweep
/// cannot look healthy while the graph anchors nothing. The only evidence it has is what the
/// arms' hooks capture: `nexus context --task … --brief`, whose items carry `· bm25 <score>`.
/// `basis.selection` — which names the fallback outright — is a `--explain` line and never
/// reaches `injected.log` at all.
///
/// So a purely cosmetic edit to the item line (swap `{why}` before `{file}:{line}`, drop the
/// ` · `) leaves every other test in this file green and silently makes every fallback package
/// read as a graph package: the counter goes to zero, which is exactly the false good news it
/// was built to prevent. `the_lexical_arm_produces_a_package` above does not catch it — a bare
/// `contains("bm25")` still holds.
///
/// This feeds a real `--brief` render to `analyse.py`'s own pattern and classifier rather than
/// asserting a second copy of the format here. One definition, one test that both sides agree
/// on it.
#[test]
fn the_brief_render_is_what_analyse_py_parses() {
    let root = project("parse");
    let out = run(
        &root,
        &[
            "context",
            "--task",
            "the save method",
            "--rank",
            "lexical",
            "--brief",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !text.trim().is_empty(),
        "a lexical package rendered nothing"
    );

    let eval_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/eval");
    let check = r#"
import sys
sys.dont_write_bytecode = True          # no __pycache__ in a source tree
sys.path.insert(0, sys.argv[1])
from analyse import ITEM_WHY_RE, package_kind
text = sys.argv[2]
whys = [m.group(1) for m in (ITEM_WHY_RE.match(l) for l in text.splitlines()) if m]
assert whys, "analyse.py's item pattern matched nothing in a --brief package:\n" + text
kind = package_kind(len(text), whys)
assert kind == "lexical", "analyse.py read a lexical package as %r (whys: %r)" % (kind, whys)
"#;
    let py = Command::new("python3")
        .args(["-c", check])
        .arg(eval_dir)
        .arg(&text)
        .output()
        .expect("python3 — the benchmark scripts require it");
    assert!(
        py.status.success(),
        "{}\n--- the render analyse.py was given ---\n{text}",
        String::from_utf8_lossy(&py.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_flag_is_absent_from_help() {
    // Undocumented is the deal. If this ever fails, either hide the flag again or update
    // cli-spec.md and 09-tooling.md to admit the surface exists — but do not do it silently.
    let out = Command::new(nexus())
        .args(["context", "--help"])
        .output()
        .expect("help");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("--rank"),
        "the control arm's flag must stay hidden:\n{text}"
    );
}

#[test]
fn an_unknown_rank_is_a_usage_error() {
    // Exit code 2 alone doesn't prove the flag was validated: clap also exits 2 when
    // `--rank` isn't a registered argument at all, which is exactly the regression this
    // test exists to catch. The stderr text is what tells the two apart.
    let root = project("typo");
    let out = run(&root, &["context", "--task", "x", "--rank", "bm25"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("unknown --rank"), "{stderr}");
    let _ = std::fs::remove_dir_all(&root);
}
