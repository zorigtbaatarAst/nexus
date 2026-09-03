//! Reading an external graph for languages Nexus cannot parse (roadmap 2.12).
//!
//! Nexus indexes Java, TypeScript and GraphQL. A real project usually has more than that, and
//! for the rest an impact answer is silently narrower than it looks — which is the failure
//! mode ADR-017 and the sibling-resolution work both exist to prevent. `graphify` already
//! produces a structural graph for anything; this reads it.
//!
//! **Those edges are a weaker kind of evidence and are labelled as such.** Nobody resolved a
//! symbol table to produce them. They get `resolution = 'external-graph'` rather than being
//! laundered into `heuristic`, they carry a confidence ceiling below any parsed edge, and the
//! resolution rate excludes them — a denominator that quietly absorbs weaker evidence stops
//! measuring what it claims to.
//!
//! Off unless `.nexus/config.toml` says `[scan] resolution = "external-graph"`. A scan that
//! silently starts trusting a file someone left in the working tree is not something to ship
//! on by default.

use serde::Deserialize;
use std::path::Path;

/// The ceiling for an imported edge. Below `heuristic` (0.6 in the analyzers), because an
/// edge nobody resolved should never outrank one somebody did.
pub const MAX_CONFIDENCE: f64 = 0.5;

/// Where `graphify` writes, relative to the project root.
const DEFAULT_OUTPUT: &str = "graphify-out/graph.json";

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    scan: ScanSection,
}

#[derive(Debug, Default, Deserialize)]
struct ScanSection {
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    external_graph_path: Option<String>,
}

/// One edge as Nexus can use it: a file pair and a kind. Deliberately minimal — reading more
/// would invent precision the source lacks.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalEdge {
    pub from: String,
    pub to: String,
    pub kind: Option<String>,
    pub confidence: Option<f64>,
}

/// A node an external graph states as *knowledge* rather than structure.
///
/// graphify's second pass reads prose and emits claims — "Hooks fail open", "No stage calls a
/// model", "Baseline-revision run stays in v1". Those are facts about this project that cost
/// a model call to produce, which is exactly the kind of conclusion §2 says should be reached
/// once. Importing them as edges would have thrown them away.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalConcept {
    pub label: String,
    /// `concept` or `rationale`. Decides the fact namespace and nothing else.
    pub kind: String,
    /// The document that states it. Always present, and the anchor of last resort.
    pub source_file: String,
    /// From `source_location` when graphify recorded one, which is rare — 23 of 681 here.
    pub line: u32,
}

/// What one read produced. A missing or malformed file is an empty graph plus a note, never
/// an error: the flag says "use one if it is there", and failing a scan because a side
/// artefact has not been generated would make the option cost more than it gives.
#[derive(Debug, Default)]
pub struct Graph {
    pub edges: Vec<ExternalEdge>,
    pub concepts: Vec<ExternalConcept>,
    pub note: Option<String>,
}

/// graphify writes node-link JSON: `nodes` carrying labels and files, `links` carrying
/// `source`/`target` node ids and a `relation`.
///
/// The importer used to read `{"edges":[{"from","to","kind"}]}`, a shape graphify has never
/// emitted, so it imported nothing from a 2986-node graph and said so only as "has no edges".
#[derive(Debug, Default, Deserialize)]
struct GraphFile {
    #[serde(default)]
    nodes: Vec<NodeRow>,
    #[serde(default)]
    links: Vec<LinkRow>,
}

#[derive(Debug, Deserialize)]
struct NodeRow {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    file_type: String,
    #[serde(default)]
    source_file: Option<String>,
    #[serde(default)]
    source_location: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LinkRow {
    source: String,
    target: String,
    #[serde(default)]
    relation: Option<String>,
    #[serde(default)]
    confidence_score: Option<f64>,
}

/// `L42` -> 42. Anything else is line 1: an anchor on the file is still an anchor, and
/// inventing a line number would be worse than admitting there is none.
fn line_of(loc: Option<&str>) -> u32 {
    loc.and_then(|l| l.trim().trim_start_matches(['L', 'l']).parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

/// What a scan should do with an external graph, decided from config alone.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// The flag is absent or set to anything else. Nothing is read.
    Off,
    /// Read this file if it exists.
    On(std::path::PathBuf),
}

pub fn mode(root: &Path) -> Mode {
    let raw = match std::fs::read_to_string(root.join(crate::NEXUS_DIR).join("config.toml")) {
        Ok(raw) => raw,
        Err(_) => return Mode::Off,
    };
    let Ok(cfg) = toml::from_str::<ConfigFile>(&raw) else {
        return Mode::Off;
    };
    if cfg.scan.resolution.as_deref() != Some("external-graph") {
        return Mode::Off;
    }
    Mode::On(
        root.join(
            cfg.scan
                .external_graph_path
                .unwrap_or_else(|| DEFAULT_OUTPUT.to_string()),
        ),
    )
}

/// Read the graph. A missing file is an empty graph, not an error: the flag says "use one if
/// it is there", and failing a scan because a side artefact has not been generated would make
/// the option cost more than it gives.
pub fn read(path: &Path) -> Graph {
    let note = |m: String| Graph {
        note: Some(m),
        ..Default::default()
    };
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            return note(format!(
                "external-graph resolution is on but {} does not exist — run graphify, or \
                 unset [scan] resolution",
                path.display()
            ))
        }
    };
    let file = match serde_json::from_str::<GraphFile>(&raw) {
        Ok(f) => f,
        Err(e) => return note(format!("{} could not be read ({e})", path.display())),
    };

    let mut concepts = Vec::new();
    let mut file_of: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for n in &file.nodes {
        let src = n.source_file.as_deref().unwrap_or("").trim();
        if !src.is_empty() {
            file_of.insert(n.id.as_str(), src);
        }
        if matches!(n.file_type.as_str(), "concept" | "rationale") && !n.label.trim().is_empty() {
            // A claim with nowhere to point cannot become a fact: §12 refuses an item with no
            // `file:line`, and a fact that can never be shown is a row nobody reads.
            if src.is_empty() {
                continue;
            }
            concepts.push(ExternalConcept {
                label: n.label.trim().to_string(),
                kind: n.file_type.clone(),
                source_file: src.to_string(),
                line: line_of(n.source_location.as_deref()),
            });
        }
    }

    // Nexus resolves an imported edge against files, so a node-to-node link becomes a
    // file-to-file one. A link inside a single file says nothing the parser did not already
    // see, and a link whose endpoints graphify could not attribute says nothing at all.
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    for l in &file.links {
        let (Some(&from), Some(&to)) = (
            file_of.get(l.source.as_str()),
            file_of.get(l.target.as_str()),
        ) else {
            continue;
        };
        if from == to {
            continue;
        }
        let kind = l.relation.clone().unwrap_or_else(|| "imports".to_string());
        if !seen.insert((from.to_string(), to.to_string(), kind.clone())) {
            continue;
        }
        edges.push(ExternalEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: Some(kind),
            confidence: l.confidence_score,
        });
    }

    let note = (edges.is_empty() && concepts.is_empty())
        .then(|| format!("{} has no usable nodes or links", path.display()));
    Graph {
        edges,
        concepts,
        note,
    }
}

/// Clamp an imported confidence to the ceiling. An external graph asserting 0.99 is still an
/// external graph.
pub fn confidence(stated: Option<f64>) -> f64 {
    stated.unwrap_or(MAX_CONFIDENCE).clamp(0.0, MAX_CONFIDENCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One directory per call site. Naming it after the config's *length* made two tests
    /// that pass the same config share a directory, and they delete each other's files —
    /// which passed locally and failed on a clean checkout, the worst way to find out.
    fn project(name: &str, config: Option<&str>) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("nexus-graphify-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(crate::NEXUS_DIR)).expect("mkdir");
        if let Some(c) = config {
            std::fs::write(root.join(crate::NEXUS_DIR).join("config.toml"), c).expect("write");
        }
        root
    }

    #[test]
    fn the_importer_is_off_unless_the_config_asks_for_it() {
        assert_eq!(mode(&project("none", None)), Mode::Off);
        assert_eq!(
            mode(&project("noflag", Some("[scan]\nexclude = []\n"))),
            Mode::Off
        );
        assert_eq!(
            mode(&project(
                "otherflag",
                Some("[scan]\nresolution = \"exact\"\n")
            )),
            Mode::Off,
            "a scan must not silently start trusting a file left in the tree"
        );
    }

    #[test]
    fn the_flag_turns_it_on_with_a_default_path() {
        let root = project("on", Some("[scan]\nresolution = \"external-graph\"\n"));
        match mode(&root) {
            Mode::On(p) => assert!(p.ends_with("graphify-out/graph.json"), "{p:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_graph_is_a_warning_not_a_failed_scan() {
        let g = read(std::path::Path::new("/nonexistent/graph.json"));
        assert!(g.edges.is_empty() && g.concepts.is_empty());
        assert!(g.note.is_some_and(|n| n.contains("does not exist")));
    }

    #[test]
    fn an_imported_confidence_cannot_outrank_a_parsed_edge() {
        // An external graph asserting certainty is still an external graph.
        assert_eq!(confidence(Some(0.99)), MAX_CONFIDENCE);
        assert_eq!(confidence(None), MAX_CONFIDENCE);
        assert_eq!(confidence(Some(0.2)), 0.2);
    }

    /// The shape graphify actually writes. Pinned by a test because the importer previously
    /// read a shape nobody emits and reported "no edges" on a 2986-node graph.
    const NODE_LINK: &str = r#"{
      "nodes": [
        {"id": "a", "label": "Alpha", "file_type": "code", "source_file": "a.py"},
        {"id": "b", "label": "Beta",  "file_type": "code", "source_file": "b.py"},
        {"id": "c", "label": "Gamma", "file_type": "code", "source_file": "a.py"},
        {"id": "k", "label": "Hooks fail open", "file_type": "rationale",
         "source_file": "docs/hooks.md", "source_location": "L42"},
        {"id": "m", "label": "No stage calls a model", "file_type": "concept",
         "source_file": "docs/context.md", "source_location": null},
        {"id": "z", "label": "Homeless", "file_type": "concept", "source_file": ""}
      ],
      "links": [
        {"source": "a", "target": "b", "relation": "calls", "confidence_score": 0.9},
        {"source": "a", "target": "b", "relation": "calls", "confidence_score": 0.9},
        {"source": "a", "target": "c", "relation": "calls"},
        {"source": "a", "target": "nowhere", "relation": "calls"}
      ]
    }"#;

    #[test]
    fn links_become_file_edges_and_duplicates_collapse() {
        let root = project("edges", None);
        let p = root.join("g.json");
        std::fs::write(&p, NODE_LINK).expect("write");
        let g = read(&p);
        assert_eq!(
            g.edges,
            vec![ExternalEdge {
                from: "a.py".into(),
                to: "b.py".into(),
                kind: Some("calls".into()),
                confidence: Some(0.9),
            }],
            "one edge: the repeat collapses, a→c is inside one file, and a→nowhere has no file"
        );
        assert!(g.note.is_none());
    }

    #[test]
    fn prose_nodes_become_concepts_and_carry_their_line() {
        let root = project("concepts", None);
        let p = root.join("g.json");
        std::fs::write(&p, NODE_LINK).expect("write");
        let g = read(&p);
        let labels: Vec<&str> = g.concepts.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Hooks fail open", "No stage calls a model"]);
        assert_eq!(g.concepts[0].line, 42, "L42 is a line number");
        assert_eq!(
            g.concepts[1].line, 1,
            "no location is the file, not a guess"
        );
        assert!(
            !labels.contains(&"Homeless"),
            "a claim with no file cannot be anchored, so it is not imported"
        );
    }

    #[test]
    fn the_old_edge_shape_is_not_silently_accepted() {
        let root = project("oldshape", None);
        let p = root.join("g.json");
        std::fs::write(&p, r#"{"edges":[{"from":"a.py","to":"b.py"}]}"#).expect("write");
        let g = read(&p);
        assert!(g.edges.is_empty() && g.concepts.is_empty());
        assert!(g.note.is_some_and(|n| n.contains("no usable")));
    }
}
