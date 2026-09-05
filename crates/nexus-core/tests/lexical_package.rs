//! The control arm must differ from the product in exactly one respect: the ranking function.
//!
//! Same budget, same serialisation, same item shape, same entry point. If anything else
//! differs, A1-vs-A5 in the Tier 2 benchmark compares two harnesses rather than two rankers,
//! and the comparison is void in a way no reader could detect from the numbers.

use nexus_core::{Engine, Purpose, RankMode, TaskRequest};
use std::path::{Path, PathBuf};
use std::process::Command;

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-lex-{name}-{}", std::process::id()));
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
    std::fs::write(
        root.join("src/notes.rs"),
        "// nothing here mentions the other file at all\npub fn unrelated() {}\n",
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
    root
}

fn engine(root: &Path) -> Engine {
    let (mut e, _) = Engine::init(root, nexus_lang_pack::default_registry()).expect("init");
    e.scan().expect("scan");
    e
}

fn request(text: &str, rank: RankMode) -> TaskRequest {
    TaskRequest {
        text: text.into(),
        files: Vec::new(),
        symbols: Vec::new(),
        budget_tokens: 4000,
        purpose: Purpose::Task,
        rank,
        explain: false,
        carry_seeds: Vec::new(),
        recent: None,
    }
}

#[test]
fn the_lexical_arm_selects_by_text_and_respects_the_budget() {
    let root = project("selects");
    let e = engine(&root);

    let pkg = e
        .context(&request("the save method on Alpha", RankMode::Lexical))
        .expect("package");

    assert!(
        !pkg.items.is_empty(),
        "a lexical package must select something"
    );
    assert!(
        pkg.tokens_estimated <= pkg.budget_tokens,
        "{} over {}",
        pkg.tokens_estimated,
        pkg.budget_tokens
    );
    assert!(
        pkg.items.iter().any(|i| i.anchor.file.ends_with("lib.rs")),
        "the file containing the query terms must be selected: {:?}",
        pkg.items.iter().map(|i| &i.anchor.file).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_two_arms_produce_the_same_shape_and_differ_only_in_content() {
    // Same fields populated, same budget honoured. A reader of the JSON must not be able to
    // tell which arm produced it except by looking at which items were chosen.
    let root = project("shape");
    let e = engine(&root);

    let engine_pkg = e
        .context(&request("the save method on Alpha", RankMode::Engine))
        .expect("a");
    let lexical_pkg = e
        .context(&request("the save method on Alpha", RankMode::Lexical))
        .expect("b");

    assert_eq!(engine_pkg.budget_tokens, lexical_pkg.budget_tokens);
    assert_eq!(engine_pkg.purpose, lexical_pkg.purpose);
    assert!(
        lexical_pkg.tokens_estimated > 0,
        "the control arm must actually inject context"
    );
    for item in &lexical_pkg.items {
        assert!(
            !item.anchor.file.is_empty(),
            "every item needs an anchor, as in the engine arm"
        );
        assert!(
            !item.why.is_empty(),
            "every item says why it is here, in both arms"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_default_rank_mode_changes_nothing() {
    // Every existing caller constructs TaskRequest without naming `rank`. If the default were
    // anything but Engine, every golden in the repo would move.
    assert_eq!(RankMode::default(), RankMode::Engine);
}
