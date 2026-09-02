//! The shipped corpus: every specification under `tests/fixtures/specs/` builds, and builds
//! the same way twice.
//!
//! This is the test that keeps the benchmark honest. A corpus that drifts between runs makes
//! every measurement taken against it a measurement of the corpus.

use nexus_fixtures::spec::Spec;
use nexus_fixtures::{generate, Options};
use std::path::PathBuf;

fn specs_dir() -> PathBuf {
    // crates/nexus-fixtures/ -> repository root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(nexus_fixtures::DEFAULT_SPEC_DIR)
}

#[test]
fn every_shipped_spec_loads_and_validates() {
    let specs = Spec::load_all(&specs_dir()).expect("the corpus loads");
    assert!(!specs.is_empty(), "the corpus is not empty");
    for s in &specs {
        assert!(!s.manifest.commit.is_empty(), "{}: has commits", s.name());
        assert!(
            !s.manifest.fixture.description.is_empty(),
            "{}: says what it is",
            s.name()
        );
    }
}

#[test]
fn every_shipped_spec_generates_identically_twice() {
    let tmp = tempfile::tempdir().expect("tmp");
    for spec in Spec::load_all(&specs_dir()).expect("corpus") {
        let a = generate(
            &spec,
            &tmp.path().join("a"),
            &Options {
                force: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{}: first run: {e}", spec.name()));
        let b = generate(
            &spec,
            &tmp.path().join("b"),
            &Options {
                force: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{}: second run: {e}", spec.name()));

        let sa: Vec<_> = a.manifest.commits.iter().map(|c| (&c.id, &c.sha)).collect();
        let sb: Vec<_> = b.manifest.commits.iter().map(|c| (&c.id, &c.sha)).collect();
        assert_eq!(sa, sb, "{}: two runs disagree about history", spec.name());
        assert_eq!(
            a.manifest.spec_digest,
            b.manifest.spec_digest,
            "{}",
            spec.name()
        );
    }
}

/// Properties the evaluation depends on, asserted against the built repositories rather than
/// against the specifications that claim them.
#[test]
fn the_corpus_carries_what_the_evaluation_needs() {
    let tmp = tempfile::tempdir().expect("tmp");
    let mut families: std::collections::BTreeSet<String> = Default::default();
    let mut saw_deprecated = false;
    let mut saw_dirty = false;
    let mut saw_multi_turn = false;
    let mut saw_branch = false;

    for spec in Spec::load_all(&specs_dir()).expect("corpus") {
        let g = generate(
            &spec,
            tmp.path(),
            &Options {
                force: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e}", spec.name()));
        let m = &g.manifest;

        // A logical id resolves to exactly one commit, or a task can pin the wrong one.
        let ids: Vec<&String> = m.commits.iter().map(|c| &c.id).collect();
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "{}: duplicate commit ids", m.name);

        for t in &m.tasks {
            families.insert(t.family.clone());
            assert!(
                m.commits.iter().any(|c| c.sha == t.commit),
                "{}: task {} pins a sha no commit has",
                m.name,
                t.id
            );
            if t.start_state.starts_with("dirty:") {
                saw_dirty = true;
            }
            if !t.turns.is_empty() {
                saw_multi_turn = true;
            }
        }
        for p in &m.patches {
            assert!(p.verified, "{}: patch {} was not proved", m.name, p.id);
        }
        for d in &m.deprecated_paths {
            saw_deprecated = true;
            assert!(
                !d.live_path.is_empty(),
                "{}: decoy {} must name the live path it is confused with",
                m.name,
                d.id
            );
        }
        if !m.branches.is_empty() {
            saw_branch = true;
        }
    }

    assert!(
        saw_deprecated,
        "Family H needs a deprecated path somewhere in the corpus"
    );
    assert!(saw_dirty, "§13.2 needs at least one dirty-start task");
    assert!(saw_multi_turn, "§14.2 needs at least one multi-turn task");
    assert!(
        saw_branch,
        "the corpus should exercise branch generation at least once"
    );
    for f in ["A", "C", "E", "H", "M", "N"] {
        assert!(families.contains(f), "no task in family {f}");
    }
}

/// The history properties `testing-strategy.md` §3 pins, checked on the built repository.
#[test]
fn spring_payments_has_the_history_the_evaluation_describes() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = Spec::load(&specs_dir().join("spring-payments")).expect("spec");
    let g = generate(
        &spec,
        tmp.path(),
        &Options {
            force: true,
            ..Default::default()
        },
    )
    .expect("generated");
    let m = &g.manifest;
    let at = |id: &str| m.commits.iter().find(|c| c.id == id).expect("commit");

    assert_eq!(m.commits.len(), 7, "seven commits");

    // c3 plants the bug; c6 fixes it; c7 brings it back.
    assert!(at("c3").plants_bug.is_some(), "c3 plants the race");
    assert!(at("c7").plants_bug.is_some(), "c7 re-opens it");
    assert_eq!(
        at("c3").plants_bug.as_ref().map(|b| b.id.as_str()),
        at("c7").plants_bug.as_ref().map(|b| b.id.as_str()),
        "the regression must carry the same bug id, or it is a different finding"
    );
    assert_eq!(
        at("c3")
            .plants_bug
            .as_ref()
            .and_then(|b| b.fixed_by.as_deref()),
        Some("c6")
    );

    // c4 is the reformat: every file moves, the file *set* does not.
    assert_eq!(at("c4").expect.symbol_changes, Some(0));
    assert_eq!(
        at("c3").files,
        at("c4").files,
        "a reformat adds and removes nothing"
    );
    assert_ne!(at("c3").sha, at("c4").sha, "but it is a real commit");

    // c5 is the rename: mn.pay becomes mn.payments, everywhere, in one commit.
    let before = &at("c4").files;
    let after = &at("c5").files;
    assert!(before.iter().any(|f| f.contains("/mn/pay/")));
    assert!(
        !after.iter().any(|f| f.contains("/mn/pay/")),
        "nothing is left behind"
    );
    assert!(after.iter().any(|f| f.contains("/mn/payments/")));
    assert_eq!(
        before.len(),
        after.len(),
        "a rename moves files, it does not add them"
    );

    // The Family H decoy survives to the end, because a decoy nobody can reach is not a decoy.
    assert!(
        at("c7")
            .files
            .iter()
            .any(|f| f.contains("LegacyPaymentCalculator")),
        "the deprecated calculator must still be there at the tip"
    );
}
