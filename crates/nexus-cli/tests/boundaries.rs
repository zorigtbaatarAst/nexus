//! The architecture's module rules, as tests.
//!
//! docs/architecture.md §4 lists six boundary rules. They are not conventions: this test is
//! how constraints 1, 2, 3 and 12 from the design brief stay true after six months of
//! feature work by people who never read the design documents.

use std::collections::BTreeMap;
use std::process::Command;

fn dependency_graph() -> BTreeMap<String, Vec<String>> {
    // No `current_dir`: cargo runs a test with the package root as its working directory,
    // and `env!("CARGO_MANIFEST_DIR")` is baked in at compile time — so a stale test binary
    // from a checkout that has since moved points at a directory that no longer exists, and
    // every boundary rule fails with "No such file or directory" rather than a real verdict.
    let out = Command::new(option_env!("CARGO").unwrap_or("cargo"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("cargo metadata should run");
    assert!(out.status.success(), "cargo metadata failed");

    let text = String::from_utf8(out.stdout).expect("metadata is utf-8");
    let json: serde_json::Value = serde_json::from_str(&text).expect("metadata is json");

    let mut graph = BTreeMap::new();
    for pkg in json["packages"].as_array().into_iter().flatten() {
        let name = pkg["name"].as_str().unwrap_or_default().to_string();
        let deps = pkg["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["kind"].is_null()) // normal dependencies only, not dev or build
            .filter_map(|d| d["name"].as_str().map(str::to_string))
            .collect();
        graph.insert(name, deps);
    }
    graph
}

fn assert_forbidden(graph: &BTreeMap<String, Vec<String>>, from: &str, to: &str, why: &str) {
    if let Some(deps) = graph.get(from) {
        assert!(
            !deps.iter().any(|d| d == to),
            "boundary violated: {from} must not depend on {to}.\n  {why}"
        );
    }
}

#[test]
fn mcp_is_an_adapter_not_the_core() {
    let g = dependency_graph();
    assert_forbidden(
        &g,
        "nexus-core",
        "nexus-mcp",
        "MCP is an adapter; the core must not know it exists.",
    );
    assert_forbidden(
        &g,
        "nexus-core",
        "nexus-cli",
        "The CLI is an adapter; the core must not know it exists.",
    );
}

#[test]
fn mcp_handlers_cannot_reach_past_the_engine() {
    let g = dependency_graph();
    for crate_name in ["nexus-store", "nexus-verify", "nexus-lang"] {
        assert_forbidden(
            &g,
            "nexus-mcp",
            crate_name,
            "A handler must reach capabilities only through nexus-core, so it cannot grow logic the CLI lacks.",
        );
    }
}

/// Language is an extension point, not a list compiled into the platform (roadmap 5.1).
///
/// `LanguageAnalyzer` was always the trait, but the *choice* of analyzers used to live in
/// `nexus-core`, which made every new language a core edit — the same inversion the
/// capability split already refused for rules. The composition roots choose now, through
/// `nexus-lang-pack`, and this is what keeps it that way.
#[test]
fn the_core_does_not_know_its_languages() {
    let g = dependency_graph();
    for analyzer in g
        .keys()
        .filter(|k| k.starts_with("nexus-lang-") && k.as_str() != "nexus-lang-pack")
        .cloned()
        .collect::<Vec<_>>()
    {
        assert_forbidden(
            &g,
            "nexus-core",
            &analyzer,
            "An analyzer is registered into the engine by the composition root. Adding a \
             language must be a new crate and one line at the root, never an edit to the core.",
        );
    }
    assert_forbidden(
        &g,
        "nexus-core",
        "nexus-lang-pack",
        "The pack is the composition root's list. The core depending on it would name every \
         language again, one indirection further away.",
    );
}

/// Verification executes processes; the store answers queries. Those are different risk
/// surfaces, and ADR-025 keeps them in different crates for that reason rather than for
/// tidiness. A `nexus-verify` that could reach the database could write its own verdicts.
#[test]
fn verification_cannot_reach_the_database() {
    let g = dependency_graph();
    assert_forbidden(
        &g,
        "nexus-verify",
        "nexus-store",
        "nexus-verify runs processes and returns a verdict; nexus-core writes what it decided.",
    );
    assert_forbidden(
        &g,
        "nexus-verify",
        "nexus-core",
        "It takes a plan and returns a result. Reaching back into the core would make the \
         judgement depend on the index it is judging.",
    );
}

#[test]
fn ai_is_optional_as_a_build_fact() {
    let g = dependency_graph();
    for http in ["reqwest", "hyper", "ureq"] {
        assert_forbidden(
            &g,
            "nexus-core",
            http,
            "The deterministic build must carry no HTTP client — checkable with `cargo tree`.",
        );
    }
}

#[test]
fn language_analyzers_know_nothing_about_storage() {
    let g = dependency_graph();
    for analyzer in g
        .keys()
        .filter(|k| k.starts_with("nexus-lang-"))
        .cloned()
        .collect::<Vec<_>>()
    {
        assert_forbidden(
            &g,
            &analyzer,
            "nexus-store",
            "An analyzer takes source text and returns a ParsedFile.",
        );
        assert_forbidden(
            &g,
            &analyzer,
            "nexus-core",
            "An analyzer never learns about scans or baselines.",
        );
    }
}

#[test]
fn a_capability_is_not_coupled_to_a_nexus_ui() {
    // "BugHunter should be usable independently while also exposed through Nexus" is only
    // true if the capability links neither adapter. A capability that could reach the CLI
    // or the MCP server would drag a UI with it wherever it went.
    let g = dependency_graph();
    for capability in g
        .keys()
        .filter(|k| k.starts_with("cap-"))
        .cloned()
        .collect::<Vec<_>>()
    {
        for adapter in ["nexus-cli", "nexus-mcp"] {
            assert_forbidden(
                &g,
                &capability,
                adapter,
                "a capability must not depend on an adapter — that is what makes it separable.",
            );
        }
        assert_forbidden(
            &g,
            &capability,
            "nexus-store",
            "a capability reads a prepared snapshot, never the database.",
        );
    }
}

#[test]
fn the_core_does_not_know_its_capabilities() {
    // Capabilities are registered into the core by the composition root, never compiled
    // into it. The reverse dependency would make "add Code Review later" a core change.
    let g = dependency_graph();
    for capability in g
        .keys()
        .filter(|k| k.starts_with("cap-"))
        .cloned()
        .collect::<Vec<_>>()
    {
        assert_forbidden(
            &g,
            "nexus-core",
            &capability,
            "the platform must not depend on a capability.",
        );
    }
}

#[test]
fn only_the_store_touches_sql() {
    let g = dependency_graph();
    for crate_name in g.keys() {
        if crate_name == "nexus-store" {
            continue;
        }
        assert_forbidden(
            &g,
            crate_name,
            "rusqlite",
            "Only nexus-store may contain SQL, so a schema change has exactly one blast radius.",
        );
    }
}

#[test]
fn the_fixture_generator_cannot_index_what_it_builds() {
    let g = dependency_graph();
    let why = "nexus-fixtures generates benchmark repositories. If it could also index them it \
               would be marking its own work — the `expect` fields it records exist precisely \
               so that something else does the checking. It needs git2 and nothing of Nexus.";
    for forbidden in [
        "nexus-core",
        "nexus-store",
        "nexus-mcp",
        "nexus-lang",
        "nexus-lang-java",
        "nexus-lang-ts",
        "nexus-lang-graphql",
        "cap-bughunter",
        "cap-architect",
        "cap-review",
        "nexus-cli",
    ] {
        assert_forbidden(&g, "nexus-fixtures", forbidden, why);
    }
}

#[test]
fn nothing_but_the_composition_root_depends_on_the_fixture_generator() {
    let g = dependency_graph();
    for crate_name in g.keys() {
        if crate_name == "nexus-cli" || crate_name == "nexus-fixtures" {
            continue;
        }
        assert_forbidden(
            &g,
            crate_name,
            "nexus-fixtures",
            "Test infrastructure reaches the product through the composition root and nowhere \
             else. A capability or an analyzer that could build a repository would be one that \
             could tailor a fixture to itself.",
        );
    }
}

/// The CLI and the MCP server are both composition roots, by design — AGENTS.md constraint 0.
/// Two roots means two lists, and a capability added to one and forgotten in the other is
/// invisible: the CLI would run it and an agent would be told it does not exist.
#[test]
fn both_composition_roots_register_the_same_capabilities() {
    // Cargo runs a test with the package root as its working directory, which is what the
    // relative paths below rely on — the same assumption `dependency_graph` documents.
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();

    // Compare the sets rather than the text: nexus-mcp imports
    // `cap_bughunter::BugHunter as BugHunterCapability`, so the lines differ but the
    // capabilities are the same.
    let names = |src: &str| -> std::collections::BTreeSet<String> {
        src.lines()
            .filter(|l| l.contains("register_capability"))
            .filter_map(|l| l.split("Box::new(").nth(1))
            .filter_map(|l| l.split("::new()").next())
            .map(|n| n.trim_end_matches("Capability").to_string())
            .collect()
    };

    let cli = names(&read("src/main.rs"));
    let mcp = names(&read("../nexus-mcp/src/lib.rs"));

    assert!(!cli.is_empty(), "the CLI must register capabilities");
    assert!(!mcp.is_empty(), "the MCP server must register capabilities");
    assert_eq!(
        cli, mcp,
        "the two composition roots disagree. A capability the CLI runs but MCP does not is \
         one an agent cannot reach, and nothing else in the build would catch it."
    );
}

/// Every rule names a `from` crate that must exist.
///
/// `assert_forbidden` skips when `from` is absent from the graph, which is right for a `to`
/// that has not been built yet — `nexus-verify` is named as a forbidden target on purpose.
/// It is wrong for a `from`: rename a crate and every rule about it stops checking anything,
/// with a green build. This test is what makes that impossible.
#[test]
fn every_guarded_crate_is_actually_in_the_workspace() {
    let g = dependency_graph();
    for from in [
        "nexus-core",
        "nexus-mcp",
        "nexus-cli",
        "nexus-store",
        "nexus-fixtures",
        "cap-bughunter",
        "cap-architect",
        "cap-review",
        "nexus-verify",
    ] {
        assert!(
            g.contains_key(from),
            "`{from}` is named as the subject of a boundary rule but is not in the workspace. \
             Either it was renamed — in which case the rules about it are silently inert — or \
             the rule is stale and should be deleted."
        );
    }
}
