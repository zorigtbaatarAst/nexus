//! The contract a second capability will rely on.
//!
//! Two claims are tested here because both are easy to believe and easy to get wrong: that
//! the registry genuinely takes more than one capability, and that a narrow scope genuinely
//! costs less than a full one. A platform that satisfies neither is a platform in name only.

use cap_bughunter::BugHunter;
use nexus_core::capability::{Capability, CapabilityError, Scope};
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::project::ProjectContext;
use nexus_core::Engine;
use nexus_types::{FindingType, Severity};
use std::fs;
use std::path::{Path, PathBuf};

/// A second capability, existing only to prove the first is not special.
///
/// Forty lines, and it needs nothing the platform does not already give BugHunter: it reads
/// the snapshot, returns findings, and gets identity, lifecycle, storage and presentation
/// for free. That is the whole claim of the split.
struct TodoHunter;

impl Capability for TodoHunter {
    fn id(&self) -> &'static str {
        "todo"
    }
    fn finding_prefix(&self) -> &'static str {
        "TODO"
    }
    fn describe(&self) -> &'static str {
        "counts TODO comments"
    }
    fn analyze(
        &self,
        ctx: &ProjectContext<'_>,
        scope: &Scope,
    ) -> Result<Vec<Finding>, CapabilityError> {
        let scoped = ctx.scoped(scope);
        let mut out = Vec::new();
        for f in &scoped.files {
            let Ok(text) = fs::read_to_string(ctx.root.join(&f.path)) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                if !line.contains("TODO") {
                    continue;
                }
                out.push(Finding {
                    finding_type: FindingType::Logic,
                    title: format!("TODO left in {}", f.path),
                    component: f.path.clone(),
                    anchor_fqn: None,
                    severity: Severity::Info,
                    confidence: 1.0,
                    detector: "todo:comment".into(),
                    structural_key: format!("{}:{}", f.path, i),
                    slug: "todo".into(),
                    evidence: vec![CodeRef {
                        file: f.path.clone(),
                        line: i as u32 + 1,
                        note: "a TODO comment".into(),
                    }],
                    capability_data: None,
                });
            }
        }
        Ok(out)
    }
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, body).expect("write");
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-cap-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    write(
        &root,
        "src/a/A.java",
        "package a;\n@Service\npublic class A {\n  @Transactional\n  private void bad() {}\n}\n",
    );
    write(&root, "src/b/B.java", "package b;\n// TODO tidy this up\n@Service\npublic class B {\n  @Transactional\n  private void alsoBad() {}\n}\n");
    root
}

#[test]
fn two_capabilities_coexist_and_number_their_findings_separately() {
    let root = fixture("two");
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine
        .register_capability(Box::new(BugHunter::new()))
        .register_capability(Box::new(TodoHunter));
    engine.scan().expect("scan");

    let bugs = engine
        .analyze("bughunter", Scope::Everything)
        .expect("bughunter");
    let todos = engine.analyze("todo", Scope::Everything).expect("todo");

    assert_eq!(
        bugs.new, 2,
        "two private @Transactional methods: {:?}",
        bugs.findings
    );
    assert_eq!(todos.new, 1, "one TODO: {:?}", todos.findings);

    let all = engine.findings(None, None, None).expect("all");
    assert_eq!(all.len(), 3, "both capabilities' findings live together");
    assert!(all.iter().any(|f| f.uid.starts_with("BUG-")));
    assert!(all.iter().any(|f| f.uid.starts_with("TODO-")), "{all:?}");

    // Filtering by capability is what makes the shared table usable.
    let only_todo = engine
        .findings(Some("todo"), None, None)
        .expect("todo only");
    assert_eq!(only_todo.len(), 1);
    assert_eq!(only_todo[0].capability, "todo");

    // One capability's sweep must not close another's findings.
    engine.rescan().expect("rescan");
    engine
        .analyze("todo", Scope::Everything)
        .expect("todo again");
    let bugs_after = engine
        .findings(Some("bughunter"), None, None)
        .expect("bugs");
    assert!(
        bugs_after.iter().all(|f| f.status != "FIXED"),
        "running one capability must not close another's: {bugs_after:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_narrow_scope_costs_less_than_a_full_one() {
    // The platform's central claim is that it does not re-analyze what it already
    // understands. If a scope does not reduce the symbols examined, that claim is decoration.
    let root = fixture("scope");
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.register_capability(Box::new(BugHunter::new()));
    engine.scan().expect("scan");

    let full = engine
        .analyze("bughunter", Scope::Everything)
        .expect("full");
    let narrow = engine
        .analyze("bughunter", Scope::Files(vec!["src/a/A.java".into()]))
        .expect("narrow");

    assert!(
        narrow.symbols_examined < full.symbols_examined,
        "a file scope examined {} of {} symbols — it must examine fewer",
        narrow.symbols_examined,
        full.symbols_examined
    );
    assert_eq!(
        narrow.found, 1,
        "and finds only what is in scope: {:?}",
        narrow.findings
    );

    // A narrowed run did not look everywhere, so it must not close what it did not examine.
    let b = engine
        .findings(Some("bughunter"), None, None)
        .expect("findings");
    assert!(
        b.iter().all(|f| f.status != "FIXED"),
        "a scoped analysis must not close findings outside its scope: {b:?}"
    );
    let _ = fs::remove_dir_all(&root);
}
