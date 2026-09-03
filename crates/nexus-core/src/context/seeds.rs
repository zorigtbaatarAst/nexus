//! Stage 2 — what in the code this is about.
//!
//! Six sources, in the priority order §4 fixes. Every seed records which source found it,
//! because stage 5 weights an explicitly named symbol differently from one guessed at by
//! name, and because a seed nobody can account for produces a package nobody can argue with.
//!
//! Zero seeds is a legitimate answer and is stated in `notes` rather than left to be inferred
//! from an empty vector. §4 is explicit about why: an empty package plus "I could not anchor
//! this to the code" lets the agent ask a better question, where a package built from nothing
//! sends it confidently into the wrong module.

use super::{Purpose, TaskRequest};
use crate::context::intent::Intent;
use nexus_store::{Store, StoreError, SymbolRef};
use std::collections::BTreeMap;

/// How a seed was found, in priority order — `Ord` is the priority, so a symbol found twice
/// keeps its best provenance by comparison rather than by a rule written in a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSource {
    /// The caller named it. A hook editing a file knows.
    Explicit,
    /// An exact FQN or repository path appearing in the text.
    Exact,
    /// The symbols this rescan reports as changed. Free: the cascade already computed it.
    Changed,
    /// A bare symbol name in the text, matched exactly and then by suffix.
    NameMatch,
    /// A user-visible label, via `ui_strings`. Empty until roadmap 5.5.
    TextMatch,
    /// The text names a module some fact is about.
    FactSubject,
    /// Carried forward by the harness from the previous turn (§14.1). Last in priority: a
    /// seed this prompt names is better evidence than one the last prompt named.
    Carried,
}

impl SeedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SeedSource::Explicit => "explicit",
            SeedSource::Exact => "exact",
            SeedSource::Changed => "changed",
            SeedSource::NameMatch => "name match",
            SeedSource::TextMatch => "text match",
            SeedSource::FactSubject => "fact subject",
            SeedSource::Carried => "carried from the previous turn",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Seed {
    pub symbol: SymbolRef,
    pub source: SeedSource,
    pub why: String,
}

#[derive(Debug, Clone, Default)]
pub struct SeedResult {
    pub seeds: Vec<Seed>,
    /// What the stage could not do, and why. Never empty when `seeds` is.
    pub notes: Vec<String>,
}

/// Candidate words from the prompt that could name a symbol: anything containing a dot,
/// slash, hash or `::` (an FQN or a path), an underscore (a `snake_case` identifier), or
/// starting with a capital (a type name).
///
/// The capital rule alone was a Java rule wearing "every convention the indexed languages
/// use". On a Rust or Python project it seeded nothing: `normalize_body`, `record_fact` and
/// `fill` are all lowercase, so `nexus context --task "refactor normalize_body"` returned an
/// empty package on the repository Nexus itself lives in.
///
/// Filtering here rather than querying every word keeps the stage at a handful of indexed
/// lookups instead of one per token, which is what ADR-024's 150 ms budget for a per-prompt
/// hook can afford.
pub(crate) fn targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | '(' | ')'))
        .map(|w| w.trim_end_matches(['.', '?', '!', ':']))
        .filter(|w| w.len() > 2)
        .filter(|w| {
            w.contains('.')
                || w.contains('/')
                || w.contains('#')
                || w.contains("::")
                // An underscore inside a word is an identifier, not English prose. Leading
                // and trailing ones are stripped first so `_private` and a markdown `_word_`
                // do not both arrive as targets.
                || w.trim_matches('_').contains('_')
                || w.chars().next().is_some_and(char::is_uppercase)
        })
        .map(str::to_string)
        .collect();
    // A member is stored as `Owner#name` in every language, because the platform needs one
    // separator. A Rust or C++ developer writes `Engine::context`, so the last `::` is also
    // offered as a `#` — otherwise the most natural way to name a method finds nothing.
    let aliases: Vec<String> = out
        .iter()
        .filter_map(|w| w.rsplit_once("::").map(|(o, n)| format!("{o}#{n}")))
        .collect();
    out.extend(aliases);
    out.sort();
    out.dedup();
    out
}

/// Resolve the request to seeds. Sources run in priority order and a symbol keeps the best
/// source that found it.
pub fn resolve(
    store: &Store,
    project_id: i64,
    req: &TaskRequest,
    intent: Intent,
) -> Result<SeedResult, StoreError> {
    let mut found: BTreeMap<i64, Seed> = BTreeMap::new();
    let mut notes = Vec::new();

    /// A symbol that declares others. The graph has no edges into one of these, so seeding
    /// it alone reaches nothing.
    fn is_container(kind: &str) -> bool {
        matches!(
            kind,
            "class" | "interface" | "enum" | "record" | "module" | "package" | "trait" | "struct"
        )
    }

    let offer =
        |found: &mut BTreeMap<i64, Seed>, symbol: SymbolRef, source: SeedSource, why: String| {
            found
                .entry(symbol.id)
                .and_modify(|existing| {
                    if source < existing.source {
                        existing.source = source;
                        existing.why = why.clone();
                    }
                })
                .or_insert(Seed {
                    symbol,
                    source,
                    why,
                });
        };

    // 1 — explicit. The caller has the anchors; nothing here is a guess.
    for fqn in &req.symbols {
        for s in store.find_symbols(project_id, fqn, 25)? {
            offer(
                &mut found,
                s,
                SeedSource::Explicit,
                format!("named in the request: {fqn}"),
            );
        }
    }
    for path in &req.files {
        for s in store.find_symbols(project_id, path, 200)? {
            offer(
                &mut found,
                s,
                SeedSource::Explicit,
                format!("in a named file: {path}"),
            );
        }
    }

    // 2 and 4 — an FQN or path in the text, then a bare name. One lookup per candidate word;
    // `find_symbols` decides which kind it is, so the two sources differ only in how the
    // result is labelled.
    for target in targets(&req.text) {
        let exact_shape = target.contains('.') || target.contains('/') || target.contains('#');
        for s in store.find_symbols(project_id, &target, 10)? {
            let source = if exact_shape {
                SeedSource::Exact
            } else {
                SeedSource::NameMatch
            };
            offer(&mut found, s, source, format!("'{target}' in the request"));
        }
    }

    // 3 — the changed set. Free for a review: the rescan already computed it.
    if matches!(intent, Intent::Review) || req.purpose == Purpose::Review {
        match store.baseline(project_id)? {
            Some(b) => {
                for (_, _, target, _) in store.changes_for_scan(b.scan_id, Some("symbol"))? {
                    let Some(fqn) = target else { continue };
                    for s in store.find_symbols(project_id, &fqn, 5)? {
                        offer(
                            &mut found,
                            s,
                            SeedSource::Changed,
                            "changed in this scan".into(),
                        );
                    }
                }
            }
            None => notes.push("no baseline, so the changed set could not seed anything".into()),
        }
    }

    // 5 — text match. The strongest signal a bug report carries: someone names the words on
    // the screen and nothing else, and those words are in the repository. Matching the
    // *value* is what reaches a non-English interface, where the source holds an English key.
    if !req.text.is_empty() {
        let hits = store.search_ui_strings(project_id, &req.text, 20)?;
        if hits.is_empty() && store.ui_string_count(project_id)? == 0 {
            notes.push(
                "no screen strings are indexed for this project, so a user-visible label \
                 cannot seed anything"
                    .into(),
            );
        }
        for (path, matched) in hits {
            for s in store.find_symbols(project_id, &path, 50)? {
                offer(
                    &mut found,
                    s,
                    SeedSource::TextMatch,
                    format!("{matched:?} appears in {path}"),
                );
            }
        }
    }

    // 6 — a fact's subject named in the text. The cheapest way to reach a module the project
    // already recorded knowledge about.
    if !req.text.is_empty() {
        let lower = req.text.to_lowercase();
        for fact in store.facts(project_id, None)? {
            let Some(subject) = fact.subject.as_deref() else {
                continue;
            };
            if subject.len() > 2 && lower.contains(&subject.to_lowercase()) {
                for s in store.find_symbols(project_id, subject, 10)? {
                    offer(
                        &mut found,
                        s,
                        SeedSource::FactSubject,
                        format!("subject of fact {}", fact.key),
                    );
                }
            }
        }
    }

    // Carried seeds last, and only as a fallback source: they are what the harness
    // remembered, not what this prompt said.
    for fqn in &req.carry_seeds {
        for s in store.find_symbols(project_id, fqn, 25)? {
            offer(
                &mut found,
                s,
                SeedSource::Carried,
                format!("carried from the previous turn: {fqn}"),
            );
        }
    }

    // Nothing calls a class. The dependency graph is method-level, so a seed that names a
    // container has no incoming edges and expansion from it reaches nothing at all — while
    // naming the class is the commonest way a person names the code. Its members are what
    // the request actually meant, so they are seeded at the same strength, and the `why`
    // says which container brought them.
    // The closure above borrows `found` mutably for its whole lifetime, so members are
    // collected and inserted directly rather than through it.
    let containers: Vec<(String, SeedSource, String)> = found
        .values()
        .filter(|s| is_container(&s.symbol.kind))
        .map(|s| (s.symbol.fqn.clone(), s.source, s.why.clone()))
        .collect();
    for (fqn, source, why) in containers {
        for member in store.members_of(project_id, &fqn, 100)? {
            let why = format!("{why} (member of {fqn})");
            found
                .entry(member.id)
                .and_modify(|existing| {
                    if source < existing.source {
                        existing.source = source;
                        existing.why = why.clone();
                    }
                })
                .or_insert(Seed {
                    symbol: member,
                    source,
                    why,
                });
        }
    }

    let mut seeds: Vec<Seed> = found.into_values().collect();
    seeds.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.symbol.fqn.cmp(&b.symbol.fqn))
    });

    if seeds.is_empty() {
        notes.push(
            "no seed: nothing in the request matched a symbol, a path or a fact subject".into(),
        );
    }
    Ok(SeedResult { seeds, notes })
}
