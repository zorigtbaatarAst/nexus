//! Read the NDJSON edge dump written by `nexus graph --edges`.

use serde::Deserialize;
use std::path::Path;

/// Mirrors `nexus_core::report::EdgeRecord`. Duplicated rather than imported because this
/// crate must not depend on `nexus-core` — it reads a file, not a library. The two must stay
/// in step field for field.
#[derive(Debug, Clone, Deserialize)]
pub struct Edge {
    pub src_fqn: String,
    pub src_file: String,
    pub site_line: Option<i64>,
    pub edge_type: String,
    pub dst_fqn: Option<String>,
    pub dst_file: Option<String>,
    pub dst_start_line: Option<i64>,
    pub dst_end_line: Option<i64>,
    pub resolution: String,
    pub confidence: f64,
}

pub fn load(path: &Path) -> std::io::Result<Vec<Edge>> {
    let body = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (n, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(e) => out.push(e),
            // One malformed line must not discard the run: say which, keep the rest.
            Err(e) => eprintln!(
                "{}:{}: skipped unparseable edge: {e}",
                path.display(),
                n + 1
            ),
        }
    }
    Ok(out)
}
