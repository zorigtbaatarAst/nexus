//! A call to a method must be able to find the method.
//!
//! Every method is stored as `Owner#member`, and an analyzer that sees `self.foo()` or
//! `obj.foo()` in one file cannot name the owner — it reports `foo`, or `#foo`. Neither shape
//! could ever match an `Owner#member` key, so method calls simply did not resolve: on this
//! repository 751 bound call edges landed on free functions and 29 on methods, with 525
//! methods sitting in the index. A JavaScript project resolved nothing at all.
//!
//! Built as a real project on disk, because the interesting failures are in extraction and
//! resolution rather than in the traversal.

use nexus_core::impact::{Direction, ImpactQuery};
use nexus_core::report::Resolved;
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

fn scanned(name: &str, files: &[(&str, &str)]) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-callres-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    for (p, body) in files {
        write(&root, p, body);
    }
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

/// Who reaches `target`, by name.
fn reaches(engine: &Engine, target: &str) -> usize {
    let q = ImpactQuery {
        target: target.into(),
        direction: Direction::Reverse,
        max_depth: 6,
        ..Default::default()
    };
    match engine.impact(&q).expect("impact") {
        Resolved::One(report) => report.items.len(),
        other => panic!("{target} should be unambiguous: {other:?}"),
    }
}

#[test]
fn a_rust_method_call_reaches_the_method() {
    let (_root, engine) = scanned(
        "rust",
        &[(
            "src/lib.rs",
            r#"
pub struct Ledger { total: i64 }

impl Ledger {
    pub fn settle_payment(&mut self, amount: i64) { self.total += amount; }
    pub fn run(&mut self) { self.settle_payment(5); }
}
"#,
        )],
    );
    // The analyzer emits `#settle_payment` — a member name with no owner it could know.
    assert!(
        reaches(&engine, "settle_payment") > 0,
        "a call to a method must bind to it; before this fix Rust method calls never resolved"
    );
}

#[test]
fn a_javascript_method_call_reaches_the_method() {
    let (_root, engine) = scanned(
        "js",
        &[
            (
                "lib/utils.js",
                "export function getMajorVersion(v) { return v.split('.')[0]; }\n",
            ),
            (
                "lib/app.js",
                "export function boot(v) { return getMajorVersion(v); }\n",
            ),
        ],
    );
    assert!(
        reaches(&engine, "getMajorVersion") > 0,
        "Express indexed 532 symbols and zero edges before the analyzer emitted calls"
    );
}

#[test]
fn a_name_shared_by_too_many_symbols_binds_to_none_of_them() {
    // A bare member name is weak evidence. Five candidates means the name is not evidence at
    // all, and five wrong edges are worse than none — ADR-017's argument, one level down.
    let mut files: Vec<(String, String)> = (0..5)
        .map(|i| {
            (
                format!("src/m{i}.rs"),
                format!("pub struct S{i};\nimpl S{i} {{ pub fn handle(&self) {{}} }}\n"),
            )
        })
        .collect();
    files.push((
        "src/lib.rs".into(),
        "pub mod m0;\npub mod m1;\npub mod m2;\npub mod m3;\npub mod m4;\npub fn go(s: &m0::S0) { s.handle(); }\n".into(),
    ));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (_root, engine) = scanned("ambiguous", &refs);

    // Not vacuous: there really are five, and the index really is ambiguous about them.
    let q = ImpactQuery {
        target: "handle".into(),
        ..Default::default()
    };
    match engine.impact(&q).expect("impact") {
        Resolved::Ambiguous(candidates) => assert_eq!(
            candidates.len(),
            5,
            "the fixture must present five candidates: {candidates:?}"
        ),
        other => panic!("expected five candidates, got {other:?}"),
    }

    let g = engine.graph().expect("graph");
    assert_eq!(
        g.edges_resolved, 0,
        "five candidates for `handle` is not evidence, so no edge is bound: {:?}",
        g.by_resolution
    );
}
