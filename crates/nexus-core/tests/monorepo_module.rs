//! Scanning one module of a monorepo must say so.
//!
//! Built as a real project on disk and scanned end to end, because the unit tests for the
//! classifier passed while the feature was completely broken: a CHECK constraint rejected
//! the new resolution value, and nothing below the scan level could see that.
//!
//! The failure this guards against is silent by construction. A scan that misses a module
//! reports a *small* blast radius with *total* confidence, which is worse than reporting
//! nothing — an agent reads "external" as "not my problem" and is right about the library
//! and wrong about the module.

use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-mono-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    root
}

/// One service module, whose code calls both a third-party library and a sibling module of
/// the same monorepo. Only the service is written, so the sibling is genuinely absent —
/// exactly the shape of scanning `sales/` inside a six-service repository.
fn service_module_only(root: &Path) {
    write(
        root,
        "build.gradle",
        "dependencies { implementation 'org.springframework.boot:spring-boot-starter:3.5.0' }\n",
    );
    // Both calls take the same shape — a static method on an imported type — so the only
    // thing separating them is the package. That is precisely the discrimination under
    // test, and it is why an annotation or an inline fully-qualified call will not do:
    // neither produces an edge, so neither can demonstrate anything.
    write(
        root,
        "src/main/java/mn/autoland/sales/VehicleService.java",
        r#"
package mn.autoland.sales;

import mn.autoland.model.BaseEntity;
import org.springframework.util.StringUtils;

public class VehicleService {
    public BaseEntity load(String id) {
        return BaseEntity.find(id);
    }

    public boolean check(String s) {
        return StringUtils.hasText(s);
    }
}
"#,
    );
}

#[test]
fn a_sibling_module_is_reported_separately_from_a_library() {
    let root = fixture("sibling");
    service_module_only(&root);

    let (mut engine, _) = Engine::open_or_init(&root).expect("init");
    let report = engine.scan().expect("scan");

    assert!(
        report.edges_sibling > 0,
        "a call into mn.autoland.model must be recognised as this project's own unscanned \
         code, not as a library; got {} sibling edges",
        report.edges_sibling
    );
    assert!(
        report.edges_external > 0,
        "org.springframework must still resolve as external — ADR-017 is not repealed by \
         this, it is subdivided"
    );
}

#[test]
fn the_two_outcomes_are_stored_distinctly() {
    // The CHECK constraint on symbol_edges.resolution is what broke the first time. This
    // asserts the value actually reaches the database rather than only the counter.
    let root = fixture("stored");
    service_module_only(&root);

    let (mut engine, _) = Engine::open_or_init(&root).expect("init");
    engine.scan().expect("scan");
    let graph = engine.graph().expect("graph");

    let by: std::collections::HashMap<&str, i64> = graph
        .by_resolution
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect();

    assert!(
        by.get("sibling").copied().unwrap_or(0) > 0,
        "no rows stored with resolution='sibling'; got {:?}",
        graph.by_resolution
    );
    assert_eq!(
        graph.edges_sibling,
        by.get("sibling").copied().unwrap_or(0),
        "the summary count and the stored rows must agree"
    );
}

#[test]
fn a_project_that_is_whole_claims_no_siblings() {
    // The signal has to switch itself off, or it becomes a permanent warning that means
    // nothing. Here both packages are present, so nothing is missing.
    let root = fixture("whole");
    service_module_only(&root);
    write(
        &root,
        "src/main/java/mn/autoland/model/BaseEntity.java",
        r#"
package mn.autoland.model;

public class BaseEntity {
    public static BaseEntity find(String id) {
        return new BaseEntity();
    }
}
"#,
    );

    let (mut engine, _) = Engine::open_or_init(&root).expect("init");
    let report = engine.scan().expect("scan");

    assert_eq!(
        report.edges_sibling, 0,
        "with the sibling module present there is nothing unscanned to report"
    );
}

/// Coverage is a pre-edit signal, so its absence has to be stated rather than counted.
/// The data was always in the impact report; nothing announced it.
#[test]
fn code_no_test_reaches_says_so() {
    let root = fixture("uncovered");
    service_module_only(&root);
    // A caller, so the target is depended upon — otherwise "no test covers this" is noise
    // about something nothing uses.
    write(
        &root,
        "src/main/java/mn/autoland/sales/VehicleController.java",
        r#"
package mn.autoland.sales;

public class VehicleController {
    private final VehicleService service;

    public Object show(String id) {
        return service.load(id);
    }
}
"#,
    );

    let (mut engine, _) = Engine::open_or_init(&root).expect("init");
    engine.scan().expect("scan");

    let q = nexus_core::impact::ImpactQuery {
        target: "VehicleService#load".into(),
        direction: nexus_core::impact::Direction::Reverse,
        max_depth: 5,
        min_score: 0.0,
        fan_out_cap: 200,
        body_only: false,
        limit: 50,
    };
    let report = match engine.impact(&q).expect("impact") {
        nexus_core::report::Resolved::One(r) => r,
        other => panic!("expected one match, got {other:?}"),
    };

    assert!(
        !report.items.is_empty(),
        "the controller depends on this method, so something is affected"
    );
    assert!(
        report.uncovered,
        "no test reaches VehicleService#load and something depends on it — that must be \
         stated, not left to be inferred from an empty list"
    );
}

/// An inherited method is declared once and called on every subtype. Without a supertype
/// walk, a `@Data` base class makes every `child.getId()` in the codebase unresolvable.
#[test]
fn a_call_to_an_inherited_method_resolves_to_where_it_is_declared() {
    let root = fixture("inherited");
    write(
        &root,
        "build.gradle",
        "dependencies { implementation 'org.springframework.boot:spring-boot-starter:3.5.0' }\n",
    );
    write(
        &root,
        "src/main/java/mn/autoland/model/BaseEntity.java",
        "package mn.autoland.model;\n\n@Data\npublic class BaseEntity { private String id; }\n",
    );
    write(
        &root,
        "src/main/java/mn/autoland/model/Issue.java",
        "package mn.autoland.model;\n\npublic class Issue extends BaseEntity { private String title; }\n",
    );
    write(
        &root,
        "src/main/java/mn/autoland/sales/IssueReader.java",
        r#"
package mn.autoland.sales;

import mn.autoland.model.Issue;

public class IssueReader {
    public String idOf(Issue issue) {
        return issue.getId();
    }
}
"#,
    );

    let (mut engine, _) = Engine::open_or_init(&root).expect("init");
    engine.scan().expect("scan");

    // getId is declared on BaseEntity via @Data and called on Issue. The blast radius of
    // changing it must include the caller, or an edit to a base class looks free.
    let q = nexus_core::impact::ImpactQuery {
        target: "BaseEntity#getId".into(),
        direction: nexus_core::impact::Direction::Reverse,
        max_depth: 5,
        min_score: 0.0,
        fan_out_cap: 200,
        body_only: false,
        limit: 50,
    };
    let report = match engine.impact(&q).expect("impact") {
        nexus_core::report::Resolved::One(r) => r,
        other => panic!("expected one match, got {other:?}"),
    };

    assert!(
        report.items.iter().any(|i| i.fqn.contains("IssueReader")),
        "a call to the inherited getId() must reach the accessor on BaseEntity; got {:?}",
        report.items.iter().map(|i| &i.fqn).collect::<Vec<_>>()
    );
}
