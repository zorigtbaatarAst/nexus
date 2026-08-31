//! The architecture's module rules, as tests.
//!
//! docs/architecture.md §4 lists six boundary rules. They are not conventions: this test is
//! how constraints 1, 2, 3 and 12 from the design brief stay true after six months of
//! feature work by people who never read the design documents.

use std::collections::BTreeMap;
use std::process::Command;

fn dependency_graph() -> BTreeMap<String, Vec<String>> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
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
        "bh-core",
        "bh-mcp",
        "MCP is an adapter; the core must not know it exists.",
    );
    assert_forbidden(
        &g,
        "bh-core",
        "bh-cli",
        "The CLI is an adapter; the core must not know it exists.",
    );
}

#[test]
fn mcp_handlers_cannot_reach_past_the_engine() {
    let g = dependency_graph();
    for crate_name in ["bh-store", "bh-verify", "bh-lang"] {
        assert_forbidden(
            &g,
            "bh-mcp",
            crate_name,
            "A handler must reach capabilities only through bh-core, so it cannot grow logic the CLI lacks.",
        );
    }
}

#[test]
fn ai_is_optional_as_a_build_fact() {
    let g = dependency_graph();
    for http in ["reqwest", "hyper", "ureq"] {
        assert_forbidden(
            &g,
            "bh-core",
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
        .filter(|k| k.starts_with("bh-lang-"))
        .cloned()
        .collect::<Vec<_>>()
    {
        assert_forbidden(
            &g,
            &analyzer,
            "bh-store",
            "An analyzer takes source text and returns a ParsedFile.",
        );
        assert_forbidden(
            &g,
            &analyzer,
            "bh-core",
            "An analyzer never learns about scans or baselines.",
        );
    }
}

#[test]
fn only_the_store_touches_sql() {
    let g = dependency_graph();
    for crate_name in g.keys() {
        if crate_name == "bh-store" {
            continue;
        }
        assert_forbidden(
            &g,
            crate_name,
            "rusqlite",
            "Only bh-store may contain SQL, so a schema change has exactly one blast radius.",
        );
    }
}
