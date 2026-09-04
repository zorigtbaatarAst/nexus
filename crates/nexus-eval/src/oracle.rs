//! Read a SCIP index into a definition map and a reference list.
//!
//! The oracle is produced by a real compiler frontend — `rust-analyzer scip`, `scip-java`,
//! `scip-typescript`, `scip-python` — so it knows what a call site actually binds to. This
//! module does nothing but read it faithfully; every judgement lives in `matcher`.

use protobuf::Message;
use scip::types::{Index, SymbolRole};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    #[error("reading {0}: {1}")]
    Io(String, std::io::Error),
    #[error("{0} is not a SCIP index: {1}")]
    Parse(String, protobuf::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub file: String,
    pub line: i64,
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub file: String,
    pub line: i64,
    pub symbol: String,
}

#[derive(Debug, Default)]
pub struct Oracle {
    /// Index-wide, because a cross-file reference's definition lives in another `Document`.
    pub defs: HashMap<String, Position>,
    pub refs: Vec<Reference>,
    /// Every file the oracle actually indexed, for the coverage cross-check.
    pub files: HashSet<String>,
}

/// The start line of an occurrence, handling both encodings.
///
/// `Occurrence.range` is deprecated in the proto in favour of `typed_range`, and every
/// current indexer still emits the deprecated form. A reader that handles only one silently
/// sees zero occurrences — which would read as "Nexus resolved nothing correctly".
fn start_line(occ: &scip::types::Occurrence) -> Option<i64> {
    if let Some(first) = occ.range.first() {
        return Some(*first as i64);
    }
    match occ.typed_range.as_ref()? {
        scip::types::occurrence::Typed_range::SingleLineRange(r) => Some(r.line as i64),
        scip::types::occurrence::Typed_range::MultiLineRange(r) => Some(r.start_line as i64),
        // `Typed_range` is `#[non_exhaustive]`: a future encoding is one this reader cannot
        // place, and an occurrence with no line is not comparable. Skipped rather than
        // guessed — and it cannot pass silently at scale, because an oracle that placed
        // nothing shows up as zero references, not as a bad score.
        _ => None,
    }
}

impl Oracle {
    pub fn load(path: &Path) -> Result<Self, OracleError> {
        let name = path.display().to_string();
        let file = std::fs::File::open(path).map_err(|e| OracleError::Io(name.clone(), e))?;
        let mut reader = std::io::BufReader::new(file);
        let index =
            Index::parse_from_reader(&mut reader).map_err(|e| OracleError::Parse(name, e))?;

        let mut out = Oracle::default();
        for doc in &index.documents {
            out.files.insert(doc.relative_path.clone());
            for occ in &doc.occurrences {
                // The `local ` prefix is reserved by the grammar and its numbering restarts
                // per document in two of the four indexers, so a bare `local N` is not a key.
                if occ.symbol.starts_with("local ") {
                    continue;
                }
                let Some(line) = start_line(occ) else {
                    continue;
                };
                if occ.symbol_roles & (SymbolRole::Definition as i32) != 0 {
                    out.defs.insert(
                        occ.symbol.clone(),
                        Position {
                            file: doc.relative_path.clone(),
                            line,
                        },
                    );
                } else {
                    out.refs.push(Reference {
                        file: doc.relative_path.clone(),
                        line,
                        symbol: occ.symbol.clone(),
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message;
    use scip::types::{Document, Index, Occurrence, SymbolRole};

    fn occurrence(symbol: &str, line: i32, definition: bool) -> Occurrence {
        let mut o = Occurrence::new();
        o.symbol = symbol.to_string();
        // The legacy three-element form: [line, startChar, endChar]. Deprecated in the proto
        // in favour of `typed_range`, and still what every current indexer emits.
        o.range = vec![line, 0, 10];
        o.symbol_roles = if definition {
            SymbolRole::Definition as i32
        } else {
            0
        };
        o
    }

    fn index() -> Index {
        let mut def_doc = Document::new();
        def_doc.relative_path = "src/a.rs".into();
        def_doc.occurrences = vec![
            occurrence("rust-analyzer cargo demo 0.1.0 Alpha#save().", 41, true),
            occurrence("local 3", 4, true),
        ];

        let mut ref_doc = Document::new();
        ref_doc.relative_path = "src/b.rs".into();
        ref_doc.occurrences = vec![
            occurrence("rust-analyzer cargo demo 0.1.0 Alpha#save().", 7, false),
            occurrence("rust-analyzer cargo std 1.0.0 Vec#push().", 9, false),
        ];

        let mut ix = Index::new();
        ix.documents = vec![def_doc, ref_doc];
        ix
    }

    /// One file per test process *and* per test: these run in parallel and a shared path
    /// means one test truncates the file another is reading.
    fn write(ix: &Index, name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("scip-{name}-{}.scip", std::process::id()));
        let mut f = std::fs::File::create(&p).expect("create");
        ix.write_to_writer(&mut f).expect("write");
        p
    }

    #[test]
    fn a_definition_is_found_across_documents() {
        // The definition of a symbol referenced in b.rs lives in a.rs. The map must be
        // index-wide: an indexer only sets the Definition role at the defining position,
        // never on the reference site.
        let o = Oracle::load(&write(&index(), "across")).expect("load");
        let pos = o
            .defs
            .get("rust-analyzer cargo demo 0.1.0 Alpha#save().")
            .expect("definition found");
        assert_eq!(pos.file, "src/a.rs");
        assert_eq!(pos.line, 41);
    }

    #[test]
    fn a_reference_with_no_definition_in_the_index_is_not_an_error() {
        // `Vec#push` is defined in std, which was not indexed. SymbolInformation in
        // `external_symbols` carries no file and no range, so such a symbol is positionally
        // unlocatable — the normal case, not a failure.
        let o = Oracle::load(&write(&index(), "nodef")).expect("load");
        assert!(!o
            .defs
            .contains_key("rust-analyzer cargo std 1.0.0 Vec#push()."));
        assert_eq!(o.refs.len(), 2, "both references are still recorded");
    }

    #[test]
    fn local_symbols_are_skipped() {
        // `local N` numbering restarts per document in scip-typescript and scip-java, so
        // `local 3` names different entities in different files. Function-scoped locals carry
        // no cross-file edges anyway.
        let o = Oracle::load(&write(&index(), "locals")).expect("load");
        assert!(o.defs.keys().all(|k| !k.starts_with("local ")));
    }

    #[test]
    fn every_document_is_recorded_for_the_coverage_check() {
        // scip-typescript silently skips files over 1MB and scip-python emits partial
        // indexes on timeout. A partial oracle inflates precision, so the file set is a
        // first-class output.
        let o = Oracle::load(&write(&index(), "files")).expect("load");
        assert!(o.files.contains("src/a.rs") && o.files.contains("src/b.rs"));
    }
}
