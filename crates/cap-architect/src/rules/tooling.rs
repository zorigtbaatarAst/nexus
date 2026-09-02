//! A datastore the project uses, with no agent tooling configured for it.
//!
//! The most direct answer to "when vibe coding, what tools does this project need": an agent
//! working in a MongoDB codebase without a MongoDB server attached writes queries blind and
//! cannot check a schema, so it guesses — and a guessed aggregation looks exactly like a
//! correct one until it runs.
//!
//! The rule is symmetrical and checkable on both sides. The datastore is proved by the file
//! and line `detect` recorded; the absence of tooling is proved by reading the project's own
//! MCP configuration. Neither half is an opinion.

use super::{split_evidence, Graph, Rule};
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::project::{ProjectContext, Scoped};
use nexus_types::{FindingType, Severity};

/// Datastore kind, the MCP server that serves it, and what having it buys.
///
/// A table rather than a match arm, so adding a row is a data change. The server names are
/// matched as substrings against the project's MCP configuration, which is why they are the
/// short canonical names rather than full package specifiers — a project may install the
/// same server under several spellings.
const TOOLING: &[(&str, &str, &str)] = &[
    (
        "mongodb",
        "mongodb",
        "read collection schemas and sample documents instead of guessing at field names, \
         and check an aggregation before it ships",
    ),
    (
        "postgresql",
        "postgres",
        "inspect tables and indexes directly instead of inferring them from migrations",
    ),
    (
        "mysql",
        "mysql",
        "inspect tables and indexes directly instead of inferring them from migrations",
    ),
    (
        "redis",
        "redis",
        "see what is actually cached rather than reasoning from the code that writes it",
    ),
    (
        "elasticsearch",
        "elasticsearch",
        "check a mapping before writing a query against it",
    ),
];

/// Where a project declares the servers an agent may use.
const MCP_CONFIG: &[&str] = &[
    ".mcp.json",
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".vscode/mcp.json",
];

pub struct DatastoreWithoutTooling;

impl Rule for DatastoreWithoutTooling {
    fn id(&self) -> &'static str {
        "architect:datastore-tooling"
    }

    fn describe(&self) -> &'static str {
        "a datastore this project uses, with no MCP server configured to reach it"
    }

    fn run(
        &self,
        ctx: &ProjectContext<'_>,
        _scoped: &Scoped<'_>,
        _graph: &Graph<'_>,
    ) -> Vec<Finding> {
        let Some(profile) = ctx.profile else {
            // No profile saved yet. Saying nothing is right: the alternative is claiming a
            // project has no datastore when nobody has looked.
            return Vec::new();
        };

        let configured = configured_servers(ctx);
        let mut out = Vec::new();

        for detected in &profile.databases {
            let Some((_, server, buys)) =
                TOOLING.iter().find(|(kind, _, _)| *kind == detected.kind)
            else {
                continue;
            };
            if configured.contains(server) {
                continue;
            }
            let (file, line) = split_evidence(&detected.evidence);
            out.push(Finding {
                finding_type: FindingType::Tooling,
                title: format!(
                    "{} is used here and no {server} MCP server is configured",
                    detected.kind
                ),
                component: detected.kind.clone(),
                anchor_fqn: None,
                // How much it matters, not how bad it is — ADR-021. Missing tooling costs
                // an agent accuracy; it does not break the build.
                severity: Severity::Low,
                // How sure the rule is that the situation applies. The datastore was proved
                // from a file, and the absence was proved by reading the config, so this is
                // not an estimate — but a project may configure its servers somewhere this
                // rule does not know to look, which is what keeps it below 1.0.
                confidence: 0.9,
                detector: self.id().to_string(),
                structural_key: format!("datastore-tooling:{}", detected.kind),
                slug: format!("tooling-{}", detected.kind),
                evidence: vec![CodeRef {
                    file,
                    line,
                    note: format!(
                        "{} is configured here. With the {server} MCP server attached an \
                         agent can {buys}. Without it, it infers all of that from the code \
                         and cannot check any of it.",
                        detected.kind
                    ),
                }],
                capability_data: Some(serde_json::json!({
                    "kind": "missing_tooling",
                    "datastore": detected.kind,
                    "recommends_mcp_server": server,
                    "searched": MCP_CONFIG,
                })),
            });
        }
        out
    }
}

/// The MCP server names this project already declares, lowercased.
///
/// Read from the project's own files rather than from the machine's global configuration: a
/// server configured only in someone's home directory is not something the repository can
/// promise the next person, and recommending against it would make the finding depend on who
/// is running the scan.
fn configured_servers(ctx: &ProjectContext<'_>) -> String {
    let mut text = String::new();
    for rel in MCP_CONFIG {
        if let Ok(body) = std::fs::read_to_string(ctx.root.join(rel)) {
            text.push_str(&body.to_lowercase());
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::capability::Scope;
    use nexus_core::project::{EdgeFacts, FileFacts, SymbolFacts};
    use nexus_core::report::{Detected, Profile};

    fn profile_with(db: &str, evidence: &str) -> Profile {
        Profile {
            name: "p".into(),
            languages: Vec::new(),
            frameworks: Vec::new(),
            build_system: None,
            package_manager: None,
            databases: vec![Detected {
                kind: db.into(),
                evidence: evidence.into(),
            }],
            containers: Vec::new(),
            vcs: "git".into(),
        }
    }

    fn run_in(root: &std::path::Path, profile: &Profile) -> Vec<Finding> {
        let symbols: Vec<SymbolFacts> = Vec::new();
        let edges: Vec<EdgeFacts> = Vec::new();
        let files: Vec<FileFacts> = Vec::new();
        let ctx = ProjectContext::new(root, &symbols, &edges, &files).with_profile(Some(profile));
        let scoped = ctx.scoped(&Scope::Everything);
        DatastoreWithoutTooling.run(&ctx, &scoped, &Graph::of(&ctx))
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("arc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }

    #[test]
    fn a_datastore_with_no_server_is_reported_where_it_was_proved() {
        let root = tmp("nomcp");
        let found = run_in(&root, &profile_with("mongodb", "docker-compose.yml:12"));
        assert_eq!(found.len(), 1, "{found:#?}");
        let f = &found[0];
        assert_eq!(f.finding_type, FindingType::Tooling);
        // The advisory still anchors on a real line — the one that proved the datastore.
        assert_eq!(f.evidence[0].file, "docker-compose.yml");
        assert_eq!(f.evidence[0].line, 12);
    }

    #[test]
    fn a_configured_server_silences_it() {
        // Both halves are checkable: the datastore from a file, the tooling from the config.
        let root = tmp("withmcp");
        std::fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"MongoDB":{"command":"mongodb-mcp-server"}}}"#,
        )
        .expect("write");
        let found = run_in(&root, &profile_with("mongodb", "docker-compose.yml:12"));
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_datastore_nothing_is_recommended_for_is_left_alone() {
        // Silence beats inventing a server name nobody ships.
        let root = tmp("unknown");
        let found = run_in(&root, &profile_with("clickhouse", "compose.yml:3"));
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn no_profile_means_no_claim() {
        let symbols: Vec<SymbolFacts> = Vec::new();
        let edges: Vec<EdgeFacts> = Vec::new();
        let files: Vec<FileFacts> = Vec::new();
        let root = tmp("noprofile");
        let ctx = ProjectContext::new(&root, &symbols, &edges, &files);
        let scoped = ctx.scoped(&Scope::Everything);
        assert!(DatastoreWithoutTooling
            .run(&ctx, &scoped, &Graph::of(&ctx))
            .is_empty());
    }
}
