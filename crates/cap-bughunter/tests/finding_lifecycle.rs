//! The bug lifecycle, end to end.
//!
//! `FIXED` requires evidence and `REGRESSED` is only reachable from `FIXED`. Getting either
//! wrong silently closes real bugs or loses the strongest thing this product can say, and
//! neither failure is visible without driving a real project through a real edit.

use cap_bughunter::BugHunter;
use nexus_core::capability::Scope;
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

const SELF_INVOCATION: &str = r#"
package mn.pay;

@Service
public class PaymentService {
    private final PaymentRepository repo;

    public Payment create(String key) {
        return this.persist(key);
    }

    @Transactional
    public Payment persist(String key) { return repo.save(key); }
}
"#;

const FIXED: &str = r#"
package mn.pay;

@Service
public class PaymentService {
    private final PaymentRepository repo;

    public Payment create(String key) {
        return repo.save(key);
    }

    @Transactional
    public Payment persist(String key) { return repo.save(key); }
}
"#;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-life-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    write(&root, "src/mn/pay/PaymentService.java", SELF_INVOCATION);
    write(&root, "src/mn/pay/PaymentRepository.java",
        "package mn.pay;\n@Repository\npublic class PaymentRepository { public Payment save(String k) { return null; } }\n");
    root
}

fn analyze(engine: &mut Engine) -> nexus_core::AnalyzeReport {
    engine
        .analyze("bughunter", Scope::Everything)
        .expect("analyze")
}

fn status_of(engine: &Engine, uid: &str) -> String {
    engine
        .finding(uid)
        .expect("bug")
        .map(|d| d.summary.status)
        .unwrap_or_else(|| "MISSING".into())
}

#[test]
fn a_bug_is_found_fixed_and_then_regressed() {
    let root = fixture("cycle");
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.register_capability(Box::new(BugHunter::new()));
    engine.scan().expect("scan");

    let first = analyze(&mut engine);
    assert_eq!(
        first.new, 1,
        "the self-invocation should be the only finding: {:?}",
        first.findings
    );
    let uid = first.findings[0].uid.clone();
    assert_eq!(
        first.findings[0].status, "UNVERIFIED",
        "a deterministic finding carries evidence"
    );
    assert_eq!(first.rejected, 0);

    // Seen again, unchanged: recognized, not duplicated.
    engine.rescan().expect("rescan");
    let again = analyze(&mut engine);
    assert_eq!(again.new, 0, "a known bug must not be re-reported as new");
    assert_eq!(again.recurring, 1);
    assert_eq!(
        engine.findings(None, None, None).expect("bugs").len(),
        1,
        "one row, not two"
    );

    // Fixed.
    write(&root, "src/mn/pay/PaymentService.java", FIXED);
    engine.rescan().expect("rescan");
    let fixed = analyze(&mut engine);
    assert_eq!(fixed.fixed, 1, "the rule ran again and did not fire");
    assert_eq!(status_of(&engine, &uid), "FIXED");

    // Reintroduced. This is the claim the whole immutable ledger exists to support.
    write(&root, "src/mn/pay/PaymentService.java", SELF_INVOCATION);
    engine.rescan().expect("rescan");
    let regressed = analyze(&mut engine);
    assert_eq!(
        regressed.regressed, 1,
        "a bug returning after a fix is a regression"
    );
    assert_eq!(regressed.new, 0, "and not a new finding");
    assert_eq!(status_of(&engine, &uid), "REGRESSED");

    let detail = engine.finding(&uid).expect("finding").expect("present");
    assert!(
        detail.history.len() >= 2,
        "the history is the evidence: {:?}",
        detail.history
    );
    assert!(
        !detail.evidence.is_empty(),
        "and every finding cites source"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_dismissed_bug_stays_dismissed() {
    let root = fixture("ignore");
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.register_capability(Box::new(BugHunter::new()));
    engine.scan().expect("scan");
    let uid = analyze(&mut engine).findings[0].uid.clone();

    assert!(engine.ignore_finding(&uid).expect("ignore"));
    engine.rescan().expect("rescan");
    analyze(&mut engine);
    assert_eq!(
        status_of(&engine, &uid),
        "IGNORED",
        "a human decision is not overturned by the next scan"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn identity_survives_a_package_move() {
    // The whole point of the fingerprint: moving the class must not invent a second bug.
    let root = fixture("move");
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.register_capability(Box::new(BugHunter::new()));
    engine.scan().expect("scan");
    let before = analyze(&mut engine);
    let uid = before.findings[0].uid.clone();

    fs::remove_file(root.join("src/mn/pay/PaymentService.java")).expect("rm");
    fs::remove_file(root.join("src/mn/pay/PaymentRepository.java")).expect("rm");
    write(
        &root,
        "src/mn/billing/PaymentService.java",
        &SELF_INVOCATION.replace("package mn.pay;", "package mn.billing;"),
    );
    write(&root, "src/mn/billing/PaymentRepository.java",
        "package mn.billing;\n@Repository\npublic class PaymentRepository { public Payment save(String k) { return null; } }\n");

    engine.rescan().expect("rescan");
    let after = analyze(&mut engine);

    // The class kept its name, so `component` and the anchor shape's type are unchanged;
    // only the package moved. That must not be a new bug.
    assert_eq!(
        after.new, 0,
        "a package move is not a new bug: {:?}",
        after.findings
    );
    assert_eq!(engine.findings(None, None, None).expect("bugs").len(), 1);
    assert_eq!(status_of(&engine, &uid), "UNVERIFIED");
    let _ = fs::remove_dir_all(&root);
}
