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

/// Words a request uses *about* code rather than *as* code.
///
/// The Rust analyzer's PRELUDE deny-list solved this exact shape of problem the same way, and
/// for the same reason: a hint that matches everything produces a *wrong* seed rather than a
/// missing one. Deliberately short and boring — English function words, and the handful of
/// code words that appear in almost every sentence about a defect.
const STOPWORDS: &[&str] = &[
    // English.
    "that", "this", "with", "from", "when", "then", "than", "them", "they", "there", "these",
    "those", "have", "does", "done", "into", "over", "only", "some", "same", "such", "were",
    "will", "what", "which", "while", "would", "should", "could", "after", "before", "about",
    "because", "returns", "return", "still", "just", "make", "made", "much", "more", "most",
    "less", "very", "also", "even", "never", "always", "again",
    // Code words a prompt uses about code.
    "test", "tests", "error", "errors", "value", "values", "result", "results", "file", "files",
    "line", "lines", "call", "calls", "type", "types", "data", "code", "name", "names", "case",
    "cases", "item", "items", "list", "lists", "null", "none", "true", "false", "class", "method",
    "function", "field", "module", "package", "project", "symbol",
];

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
/// slash, hash or `::` (an FQN or a path), an underscore (a `snake_case` identifier), a
/// capital (a type name), or a plain lowercase word of four characters or more that is not a
/// stopword.
///
/// The plain-word arm is what lets a *symptom* find code. `cache` is indexed as
/// `nexus_core::context::cache`, and refusing it because it carries no capital meant four
/// real defects, handed to the context engine as their symptoms, produced zero hits and three
/// empty packages.
///
/// It is affordable, measured rather than assumed: one word that passes this filter is one
/// indexed lookup, and 3 target words cost 12 ms against 40 target words at 23 ms — about
/// 0.3 ms each. A 25-word symptom adds roughly 7 ms to ADR-024's 150 ms budget. The previous
/// comment here justified the narrow filter with that budget and was over-cautious by an
/// order of magnitude.
///
/// Noise control is not this function's job: `resolve` accepts a plain word only when it
/// names exactly one symbol whose own last segment *is* the word.
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
                || is_plain_word(w)
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

/// A word with no shape a caller already qualified: no dot, slash, hash or `::` (an FQN or a
/// path), no interior underscore (a `snake_case` identifier), no leading capital (a type
/// name) — and, since none of those give it away, it only earns a lookup if it is long enough
/// to be distinctive and is not a word every sentence about a defect contains.
///
/// One definition, called from both `targets` (is this word worth an indexed lookup at all?)
/// and `resolve` (does this target need to prove uniqueness before it can seed?). The two
/// questions must agree on what "plain" means, or they drift the way `subject_match` and
/// `subject_prefixes` drifted on the module-boundary rule before `is_anchored_prefix` unified
/// them — and a function that only answered *half* the question under a name that promised
/// the whole thing is exactly how that kind of drift starts unnoticed.
fn is_plain_word(w: &str) -> bool {
    !w.contains('.')
        && !w.contains('/')
        && !w.contains('#')
        && !w.contains("::")
        && !w.trim_matches('_').contains('_')
        && !w.chars().next().is_some_and(char::is_uppercase)
        && w.len() >= 4
        && !STOPWORDS.contains(&w.to_ascii_lowercase().as_str())
}

/// The last name in a qualified path, whichever separator wrote it.
pub(crate) fn last_segment(fqn: &str) -> &str {
    let after_member = fqn.rsplit('#').next().unwrap_or(fqn);
    let after_member = after_member.split('(').next().unwrap_or(after_member);
    let after_colons = after_member.rsplit("::").next().unwrap_or(after_member);
    after_colons.rsplit('.').next().unwrap_or(after_colons)
}

/// The one indexed symbol a word names, if it names exactly one.
///
/// `find_symbols` matches by suffix, which is right for a name a person typed and wrong
/// wherever the word came out of prose: without the last-segment check, "integration" once
/// anchored six imported design claims on `NoContinuousIntegration`.
///
/// Two callers, one rule: the seed stage reading a request, and the graphify import reading a
/// claim's label. Two copies of this would drift, and the copy further from the failure would
/// be the one still wrong.
pub(crate) fn uniquely_named_symbol(
    store: &Store,
    project_id: i64,
    word: &str,
) -> Result<Option<SymbolRef>, StoreError> {
    let hits = store.find_symbols(project_id, word, 8)?;
    // Filter before counting, not after. `find_symbols` matches through SQL `LIKE`, which
    // SQLite treats case-insensitively for ASCII by default, so a word like "resolved" comes
    // back with both `ResolveStats#resolved` and `report::Resolved` — two hits, neither of
    // them wrong to return. Counting arity first would see 2 and refuse both, discarding the
    // one piece of information, exact-case `last_segment`, that actually tells them apart. A
    // real, unique match must not be hidden behind a mere case-variant elsewhere in the index.
    let matches: Vec<&SymbolRef> = hits
        .iter()
        .filter(|s| last_segment(&s.fqn) == word)
        .collect();
    let [only] = matches.as_slice() else {
        return Ok(None);
    };
    Ok(Some((*only).clone()))
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
        // A plain lowercase word is weaker evidence than a name someone qualified, so it is
        // accepted only when it identifies one symbol outright. Without that rule the word
        // "integration" reaches `NoContinuousIntegration`, and a symptom seeds the wrong file
        // with confidence. `is_plain_word` is the same predicate `targets` used to let this
        // word through in the first place — re-deriving "what counts as plain" here would be
        // a second copy of that rule, and the two would drift the moment either one changed.
        if is_plain_word(&target) {
            if let Some(s) = uniquely_named_symbol(store, project_id, &target)? {
                offer(
                    &mut found,
                    s,
                    SeedSource::NameMatch,
                    format!("'{target}' in the request names exactly one symbol"),
                );
            }
            continue;
        }
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
