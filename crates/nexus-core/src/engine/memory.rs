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

/// The confidence an imported claim carries.
///
/// A model wrote it, so §5's model ceiling of 0.75 applies, and it sits below that: nothing
/// here was verified against the code, and a claim read out of a document is weaker evidence
/// than one a session worked out while looking at the failure.
const IMPORTED_CONFIDENCE: f64 = 0.5;

/// `Hooks fail open` -> `hooks-fail-open`. Bounded, because a fact key is an identifier a
/// person greps for, not a sentence.
fn slug(label: &str) -> String {
    let mut out = String::new();
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 60 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

impl Engine {
    /// Import an external graph's prose claims as facts (roadmap 2.12).
    ///
    /// graphify's structural pass is free and its edges already arrive through the scan. Its
    /// *semantic* pass costs model calls and produces claims about the project — "Hooks fail
    /// open", "No stage calls a model" — and those were being discarded. This is the door
    /// they come through, and once through it they are ordinary facts: ranked against
    /// everything else, capped by the same budget, invalidated by the same moved anchors.
    ///
    /// It costs an agent nothing per request. The budget is fixed, so more knowledge changes
    /// *which* items a package carries, never how many tokens it is.
    pub fn import_graphify(&mut self, path: &Path) -> Result<ImportReport> {
        let graph = crate::graphify::read(path);
        let mut report = ImportReport {
            path: path.display().to_string(),
            concepts_read: graph.concepts.len(),
            facts_recorded: 0,
            anchored_on_code: 0,
            skipped: 0,
            skipped_not_a_claim: 0,
            warnings: graph.note.into_iter().collect(),
        };

        for c in &graph.concepts {
            let key = slug(&c.label);
            if key.is_empty() {
                report.skipped += 1;
                continue;
            }
            // Rule four, and it is not redundant: rules one to three drop
            // `nexus-cli::main composition root`, a heading by shape that names code, which
            // is the entire reason to keep a claim.
            let anchor = self.symbol_named_in(&c.label)?;
            if !c.is_claim && anchor.is_none() {
                report.skipped_not_a_claim += 1;
                continue;
            }
            // A rationale is a reason for a decision; a concept is a description of how
            // something is put together. Both namespaces exist already and neither is new.
            let namespace = if c.kind == "rationale" {
                "decision"
            } else {
                "arch"
            };

            // Anchor on the code when the claim names exactly one indexed symbol, and on the
            // document that states it otherwise. graphify's own prose-to-code edges cannot do
            // this job: only 34 of 681 prose nodes here touch a code node at all, and those
            // point at ids derived from the citing document rather than at the code.
            let (scope, subject, evidence) = match &anchor {
                Some(sym) => (
                    "symbol",
                    Some(sym.fqn.clone()),
                    CodeRef {
                        file: sym.file_path.clone(),
                        line: sym.start_line as u32,
                        note: format!("stated in {}", c.source_file),
                    },
                ),
                None => (
                    "file",
                    Some(c.source_file.clone()),
                    CodeRef {
                        file: c.source_file.clone(),
                        line: c.line,
                        note: String::new(),
                    },
                ),
            };
            if anchor.is_some() {
                report.anchored_on_code += 1;
            }

            self.record_fact(FactInput {
                key: format!("{namespace}.{key}"),
                scope: scope.into(),
                subject,
                claim: c.label.clone(),
                source: "ai".into(),
                evidence: vec![evidence],
                confidence: IMPORTED_CONFIDENCE,
            })?;
            report.facts_recorded += 1;
        }
        Ok(report)
    }

    /// The one indexed symbol a claim names, if it names exactly one.
    ///
    /// Reuses the seed stage's target extraction rather than growing a second matcher, so a
    /// claim is read for symbol names exactly the way a prompt is. Ambiguity is an answer:
    /// two candidates means the label did not identify anything, and guessing between them
    /// would anchor a design claim on the wrong function.
    fn symbol_named_in(&self, label: &str) -> Result<Option<nexus_store::SymbolRef>> {
        for target in crate::context::seeds::targets(label) {
            // A word out of prose is only worth looking up when it is shaped like an
            // identifier: `Integration`, `Agent` and `Hooks` are sentence words, and looking
            // them up anchored design claims on whatever symbol happened to end with them.
            let distinctive = target.len() >= 4
                && (target.contains("::")
                    || target.contains('#')
                    || target.contains('_')
                    || target.contains('/')
                    || target.contains('.')
                    || target.chars().filter(|c| c.is_uppercase()).count() >= 2);
            if !distinctive {
                continue;
            }
            if let Some(sym) =
                crate::context::seeds::uniquely_named_symbol(&self.store, self.project_id, &target)?
            {
                return Ok(Some(sym));
            }
        }
        Ok(None)
    }
}
