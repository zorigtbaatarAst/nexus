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

/// One edge as an external graph states it. Deliberately minimal: a path pair and a kind is
/// all Nexus can honestly use, and reading more would invent precision the source lacks.
#[derive(Debug, Clone, Deserialize)]
pub struct ExternalEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct GraphFile {
    #[serde(default)]
    edges: Vec<ExternalEdge>,
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
pub fn read(path: &Path) -> (Vec<ExternalEdge>, Option<String>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            return (
                Vec::new(),
                Some(format!(
                    "external-graph resolution is on but {} does not exist — run graphify, or \
                     unset [scan] resolution",
                    path.display()
                )),
            )
        }
    };
    match serde_json::from_str::<GraphFile>(&raw) {
        Ok(g) => {
            let n = g.edges.len();
            (
                g.edges,
                (n == 0).then(|| format!("{} has no edges", path.display())),
            )
        }
        Err(e) => (
            Vec::new(),
            Some(format!("{} could not be read ({e})", path.display())),
        ),
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

    fn project(config: Option<&str>) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "nexus-graphify-{}-{}",
            std::process::id(),
            config.map_or(0, str::len)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(crate::NEXUS_DIR)).expect("mkdir");
        if let Some(c) = config {
            std::fs::write(root.join(crate::NEXUS_DIR).join("config.toml"), c).expect("write");
        }
        root
    }

    #[test]
    fn the_importer_is_off_unless_the_config_asks_for_it() {
        assert_eq!(mode(&project(None)), Mode::Off);
        assert_eq!(mode(&project(Some("[scan]\nexclude = []\n"))), Mode::Off);
        assert_eq!(
            mode(&project(Some("[scan]\nresolution = \"exact\"\n"))),
            Mode::Off,
            "a scan must not silently start trusting a file left in the tree"
        );
    }

    #[test]
    fn the_flag_turns_it_on_with_a_default_path() {
        let root = project(Some("[scan]\nresolution = \"external-graph\"\n"));
        match mode(&root) {
            Mode::On(p) => assert!(p.ends_with("graphify-out/graph.json"), "{p:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_graph_is_a_warning_not_a_failed_scan() {
        let (edges, note) = read(std::path::Path::new("/nonexistent/graph.json"));
        assert!(edges.is_empty());
        assert!(note.is_some_and(|n| n.contains("does not exist")));
    }

    #[test]
    fn an_imported_confidence_cannot_outrank_a_parsed_edge() {
        // An external graph asserting certainty is still an external graph.
        assert_eq!(confidence(Some(0.99)), MAX_CONFIDENCE);
        assert_eq!(confidence(None), MAX_CONFIDENCE);
        assert_eq!(confidence(Some(0.2)), 0.2);
    }

    #[test]
    fn edges_are_read_from_the_documented_shape() {
        let root = project(Some("[scan]\nresolution = \"external-graph\"\n"));
        let p = root.join("g.json");
        std::fs::write(
            &p,
            r#"{"edges":[{"from":"a.py","to":"b.py","kind":"imports","confidence":0.9}]}"#,
        )
        .expect("write");
        let (edges, note) = read(&p);
        assert_eq!(edges.len(), 1, "{edges:?}");
        assert_eq!(edges[0].from, "a.py");
        assert!(note.is_none());
    }
}
