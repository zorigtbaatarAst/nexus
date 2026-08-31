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
