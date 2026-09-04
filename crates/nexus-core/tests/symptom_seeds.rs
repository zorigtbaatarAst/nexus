//! A symptom is written in plain words, and plain words used to seed nothing.
//!
//! `seeds::targets` accepted a word only if it carried a capital, an underscore, or a path
//! separator — a rule that suited "refactor PaymentService" and refused "the cache serves a
//! stale package". Four real defects fixed in this repository, given to the context engine as
//! their symptoms, produced zero hits and three empty packages, while the code they named sat
//! in the index the whole time.

use nexus_core::context::{Purpose, TaskRequest, TASK_BUDGET_TOKENS};
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

fn git(root: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

/// A fixture shaped like the code the symptoms were about: a lowercase module name that
/// occurs exactly once, another that occurs twice, and a stopword that is also a symbol.
fn scanned(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-symptom-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    // The `mod.rs` files are load-bearing, not decoration. The Rust analyzer derives a
    // module's *scope* from the file path but only emits a module *symbol* from a `mod_item`
    // declaration — so without `pub mod cache;` this fixture holds `context::cache::put` and
    // no symbol named `cache` at all, and every test below would fail for a fixture reason
    // rather than a code one.
    for (path, body) in [
        (
            "src/lib.rs",
            "pub mod context;\npub mod store;\npub mod util;\npub mod a;\npub mod b;\n",
        ),
        ("src/context/mod.rs", "pub mod cache;\n"),
        ("src/context/cache.rs", "pub fn put() {}\npub fn get() {}\n"),
        ("src/store/mod.rs", "pub mod ledger;\n"),
        ("src/store/ledger.rs", "pub fn append() {}\n"),
        ("src/a/mod.rs", "pub mod handler;\n"),
        ("src/a/handler.rs", "pub fn handle_it() {}\n"),
        ("src/b/mod.rs", "pub mod handler;\n"),
        ("src/b/handler.rs", "pub fn handle_it() {}\n"),
        ("src/util/mod.rs", "pub mod error;\n"),
        ("src/util/error.rs", "pub fn report() {}\n"),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

fn files_in(engine: &Engine, text: &str) -> Vec<String> {
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = text.into();
    r.purpose = Purpose::Task;
    engine
        .context(&r)
        .expect("context")
        .items
        .iter()
        .map(|i| i.anchor.file.clone())
        .collect()
}

#[test]
fn a_symptom_in_plain_words_reaches_the_code_it_names() {
    let (_root, engine) = scanned("plain");
    let files = files_in(
        &engine,
        "the context cache serves a package from before a fact was recorded",
    );
    assert!(
        files.iter().any(|f| f.contains("context/cache.rs")),
        "`cache` is one indexed symbol and the symptom names it: {files:?}"
    );
}

#[test]
fn a_word_naming_two_symbols_seeds_nothing() {
    let (_root, engine) = scanned("ambiguous");
    let files = files_in(&engine, "the handler drops the second request");
    assert!(
        !files.iter().any(|f| f.contains("handler.rs")),
        "`handler` names two symbols, so it identifies neither: {files:?}"
    );
}

#[test]
fn a_stopword_seeds_nothing_even_when_it_is_a_symbol() {
    // `error` is in the index here. It is also the word every symptom in the world contains.
    let (_root, engine) = scanned("stopword");
    let files = files_in(&engine, "there is an error when the ledger appends");
    assert!(
        !files.iter().any(|f| f.contains("util/error.rs")),
        "a stopword is not a seed however well it matches: {files:?}"
    );
    assert!(
        files.iter().any(|f| f.contains("store/ledger.rs")),
        "the distinctive word in the same sentence still seeds: {files:?}"
    );
}

#[test]
fn a_short_word_seeds_nothing() {
    let (_root, engine) = scanned("short");
    let files = files_in(&engine, "get put now");
    assert!(
        files.is_empty(),
        "three-letter words are not evidence, however many symbols they match: {files:?}"
    );
}

/// A fixture built for one collision: `resolved` the function, three lines under `Resolved`
/// the enum. `find_symbols` matches case-insensitively, so a word that counts arity before
/// filtering by exact `last_segment` sees two hits for `resolved` and refuses both — even
/// though only one of them actually spells it that way.
fn scanned_case_variant() -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-symptom-case-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let p = root.join("src/lib.rs");
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(
        &p,
        "pub enum Resolved { Yes, No }\npub fn resolved() -> Resolved { Resolved::Yes }\n",
    )
    .expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

#[test]
fn a_case_variant_elsewhere_does_not_hide_a_real_unique_match() {
    let (_root, engine) = scanned_case_variant();
    let files = files_in(&engine, "the request is resolved before the retry finishes");
    assert!(
        files.iter().any(|f| f.contains("src/lib.rs")),
        "`resolved` uniquely names the function even though `Resolved` also matches it \
         case-insensitively: {files:?}"
    );
}
