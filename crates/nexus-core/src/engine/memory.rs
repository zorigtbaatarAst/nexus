//! What a scan does to what Nexus remembers.
//!
//! A fact is anchored by its evidence — a file and a line. Before a scan rewrites the index
//! the anchor is resolved to the symbol spanning that line and its hashes; after the scan
//! has written its symbols, an anchor that no longer holds invalidates its fact. The check
//! is SQL in `Store::invalidate_moved_facts`; what lives here is the step the store cannot
//! take, because it does not know what evidence JSON means.
//!
//! Evidence that does not parse becomes a scan warning and the fact is left as it is:
//! silently invalidating on a malformed row would look exactly like the rule working.

use super::*;
use nexus_store::FactAnchor;

impl Engine {
    /// Every live fact's evidence, resolved against the index as it is *now*. Call this
    /// before the scan's transaction opens — afterwards the symbols it would compare
    /// against are the new ones, and nothing would ever look moved.
    pub(super) fn fact_anchors(&self, warnings: &mut Vec<String>) -> Result<Vec<FactAnchor>> {
        let mut anchors = Vec::new();
        for fact in self.store.live_facts(self.project_id)? {
            let Some(json) = fact.evidence_json.as_deref() else {
                continue;
            };
            let refs: Vec<CodeRef> = match serde_json::from_str(json) {
                Ok(refs) => refs,
                Err(e) => {
                    warnings.push(format!(
                        "fact {}: evidence is not readable, so it cannot be checked against this scan: {e}",
                        fact.id
                    ));
                    continue;
                }
            };
            for r in refs {
                let symbol = self
                    .store
                    .symbol_at(self.project_id, &r.file, i64::from(r.line))?;
                anchors.push(FactAnchor {
                    fact_id: fact.id,
                    path: r.file,
                    symbol,
                });
            }
        }
        Ok(anchors)
    }
}
