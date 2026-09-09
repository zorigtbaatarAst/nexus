//! Stage 2 — what in the code this is about.
//!
//! Seven sources, in the priority order §4 fixes. Every seed records which source found it,
//! because stage 5 weights an explicitly named symbol differently from one guessed at by
//! name, and because a seed nobody can account for produces a package nobody can argue with.
//!
//! Zero seeds is a legitimate answer and is stated in `notes` rather than left to be inferred
//! from an empty vector. That note is the contract this stage owes its caller: it says the
//! request named nothing this index knows, and it travels with whatever the caller decides to
//! return.
//!
//! What the caller decides is no longer "nothing". `Engine::task_package` falls back to
//! ranking file contents with BM25 when this stage anchors nothing and the prompt shares at
//! least two discriminating words with the corpus. ADR-027.
//!
//! The original reasoning for returning nothing still holds and is why the disclosure is not
//! optional: a package built from nothing sends an agent confidently into the wrong module, so
//! the note above survives, the package's `basis.selection` says "no symbol anchored", and
//! every item's `why` reads `bm25 <score>`. The agent is told it is a guess. What changed is
//! that the alternative to a labelled guess was measured — on two of five Tier 2 prompts and
//! on the bugs planted in `debug_supply` — and it was silence.

use super::TaskRequest;
use crate::context::intent::Intent;
use nexus_store::{Store, StoreError, SymbolRef};
use std::collections::BTreeMap;

/// How long a list `facts_for_seeds` may be asked about, wherever it was built.
///
/// `facts_for_seeds` emits one compound-`SELECT` arm per seed, and SQLite refuses a compound
/// `SELECT` past 500 terms — a query that fails to prepare, not one that answers short. Both
/// lists that reach it are unbounded at their source, so both are capped here:
///
///   * `resolve` below, over the candidate words in one request. Each is also one indexed
///     lookup on the `UserPromptSubmit` hook, whose budget is ADR-024's 150 ms — 400 ms is
///     `SessionStart`; 256 holds that to about 90 ms on this repository.
///   * `engine::query`, over the seeds plus everything expansion reached. Expansion runs to
///     `max_depth: 5` with no node cap — a four-symbol prompt on this repository already
///     reaches 189 — so the cap is generous enough never to bite an ordinary prompt.
///
/// One number, in one place: two would drift, and the one further from the failure would be
/// the one still wrong. When it bites, the package says so, because a silently narrowed
/// query is an error.
pub(crate) const SEED_QUERY_CAP: usize = 256;

/// Words a request uses *about* code rather than *as* code.
///
/// The Rust analyzer's PRELUDE deny-list solved this exact shape of problem the same way, and
/// for the same reason: a hint that matches everything produces a *wrong* seed rather than a
/// missing one. Deliberately short and boring — English function words, and the handful of
/// code words that appear in almost every sentence about a defect.
/// Long enough, and distinctive enough, to be worth anything on its own.
///
/// The one place the length floor and the stopword list are applied. `is_plain_word` asks it
/// when deciding whether a bare word may seed, and the lexical fallback's gate asks it through
/// `is_noise_word` when deciding whether a prompt corroborates anything — two callers, one
/// definition, so they cannot drift into disagreeing about what an ordinary word is.
///
/// The floor is why `it`, `by` and `one` never needed to be in the list below: nothing under
/// four characters reaches it.
fn is_ordinary_word(w: &str) -> bool {
    w.len() >= 4 && !STOPWORDS.contains(&w.to_ascii_lowercase().as_str())
}

/// The inverse, for callers that are filtering noise out rather than letting evidence in.
pub(crate) fn is_noise_word(w: &str) -> bool {
    !is_ordinary_word(w)
}

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
/// Noise control is not this function's job: `resolve` accepts a plain word only when it names
/// exactly one symbol whose own last segment *is* the word, or — when it is the name of none
/// of them — when the identifiers built from it are few enough to be one concept.
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

/// Internal evidence that a word was typed as code rather than written as prose.
///
/// A *leading* capital is not evidence: every English sentence starts with one, and reading it
/// as a type name is what let `After` seed `reset_after` and `NOTIFY_AFTER` on a prompt about
/// semaphores. What distinguishes an identifier is a convention *inside* the word — a second
/// capital, an interior underscore, or a path/FQN separator — and none of those depend on
/// where the word sits in a sentence.
fn looks_like_code(w: &str) -> bool {
    w.contains('.')
        || w.contains('/')
        || w.contains('#')
        || w.contains("::")
        || w.trim_matches('_').contains('_')
        || w.chars().skip(1).any(char::is_uppercase)
}

/// A word with no code shape (see `looks_like_code`) that also clears the length floor and the
/// stopword list — it only earns a lookup if it is long enough to be distinctive and is not a
/// word every sentence about a defect contains.
///
/// One definition, called from both `targets` (is this word worth an indexed lookup at all?)
/// and `resolve` (does this target need to prove uniqueness before it can seed?). The two
/// questions must agree on what "plain" means, or they drift the way `subject_match` and
/// `subject_prefixes` drifted on the module-boundary rule before `is_anchored_prefix` unified
/// them — and a function that only answered *half* the question under a name that promised
/// the whole thing is exactly how that kind of drift starts unnoticed.
fn is_plain_word(w: &str) -> bool {
    !looks_like_code(w) && is_ordinary_word(w)
}

/// The last name in a qualified path, whichever separator wrote it.
pub(crate) fn last_segment(fqn: &str) -> &str {
    let after_member = fqn.rsplit('#').next().unwrap_or(fqn);
    let after_member = after_member.split('(').next().unwrap_or(after_member);
    let after_colons = after_member.rsplit("::").next().unwrap_or(after_member);
    after_colons.rsplit('.').next().unwrap_or(after_colons)
}

/// How many candidates the index is asked for about one word.
///
/// Wide enough that `token_family` is deciding about the *family* and not about a sample of
/// it. The query it bounds over-matches on purpose — `LIKE '%order%'` also returns `orders`
/// and `reorder`, which are not the word — so a narrow window fills with near-misses and the
/// real members fall off the end. A family diluted that way arrives *under* the cap and seeds,
/// which turns a refusal into a coin toss on row order. At 200 the window holds every family
/// this rule is meant to admit; a word that fills it is refused outright by `token_family`
/// rather than judged on what happened to fit.
///
/// The cost of the larger window is rows materialized, not rows scanned: `LIKE '%x%'` is
/// unindexed and reads the table whatever the `LIMIT` says. Measured on a synthetic
/// 81 612-symbol index — spring-boot's size — the path this replaced,
/// `find_symbols(word, 8)`, costs 47.9 ms/word against 51.0 ms/word for
/// `find_symbols_by_word(word, 200)`: **+6.5 %**, which is the materialization and nothing
/// else. `docs/eval/hook-latency.md` §"The prompt path" carries the measurement, and the
/// caveat that the seeds this now emits have not been costed end to end.
const WORD_HIT_LIMIT: usize = 200;

/// How many **names** a word may be a token of before it names nothing.
///
/// Names, not symbols, and the difference is not pedantry: a family member that is a container
/// fans out to its own members at the end of `resolve` exactly as any other seed does, so the
/// symbol-level ceiling for one word is this number times the `members_of` limit. That is
/// bounded but it is not six. The rule this constant states is about how many distinct
/// identifiers a word is allowed to be *part of*; what a seed then reaches is the container
/// rule's business and is the same for a word that names one class exactly.
///
/// A word that is the whole of a name is adjudicated by `exactly_named` and must be unique:
/// two symbols actually called `handler` are two different things, and picking either is a
/// coin flip. A word that is one token of several names is a different situation —
/// `idempotency` reaches `idempotencyKey`, `getIdempotencyKey`, `existsByIdempotencyKey` and
/// `findByIdempotencyKey`, which are one concept seen four ways. Seeding all four is not a
/// guess between them, it is the family the word actually names.
///
/// The cap is what stops that argument running away, because past a handful a shared token is
/// a *theme* rather than a name. Measured on the `spring-payments` fixture (39 symbols): the
/// widest real family is `idempotency` at 4, while `payment` — the word that would drag the
/// whole repository in — is a token of 13 of them. Six sits between the two and is deliberately
/// nearer the family.
const TOKEN_FAMILY_NAME_CAP: usize = 6;

/// The symbols a word names outright: their own last segment *is* the word.
///
/// Filter before counting, not after. `find_symbols_by_word` matches through SQL `LIKE`, which
/// SQLite treats case-insensitively for ASCII by default, so a word like "resolved" comes back
/// with both `ResolveStats#resolved` and `report::Resolved` — two hits, neither of them wrong
/// to return. Counting arity first would see 2 and refuse both, discarding the one piece of
/// information, exact-case `last_segment`, that actually tells them apart. A real, unique match
/// must not be hidden behind a mere case-variant elsewhere in the index.
fn exactly_named<'a>(hits: &'a [SymbolRef], word: &str) -> Vec<&'a SymbolRef> {
    hits.iter()
        .filter(|s| last_segment(&s.fqn) == word)
        .collect()
}

/// The camelCase and `snake_case` tokens a name is built from.
///
/// `getTotalAmount` is built from `get`, `Total` and `Amount`; `idempotency_key` from
/// `idempotency` and `key`; `HTTPServer` from `HTTP` and `Server`. This is the word boundary
/// SQL cannot express, and the reason `find_symbols_by_word` is allowed to over-match.
fn name_tokens(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        // A capital starts a new token unless it is inside a run of capitals — an acronym is
        // one token, and the capital that ends it belongs to the word that follows.
        if !cur.is_empty() && c.is_uppercase() {
            let previous_is_upper = chars[i - 1].is_uppercase();
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if !previous_is_upper || next_is_lower {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The symbols whose names are *built from* the word, when the word is the name of none of
/// them — and only while they are few enough to be one concept rather than a theme.
///
/// Case-insensitive, because the hump that starts a token inside an identifier is exactly the
/// capital a person writing prose does not type.
///
/// A filled window is a refusal, not a sample to judge. `hits` is what SQL returned under
/// `WORD_HIT_LIMIT`, and if it came back full the family below is a lower bound rather than
/// the family — some members are outside the window and the count cannot be compared with a
/// cap. A word the index has more than `WORD_HIT_LIMIT` matches for is a theme by any reading,
/// so it is refused on the fact of truncation instead of on an arithmetic that cannot be
/// trusted.
fn token_family<'a>(hits: &'a [SymbolRef], word: &str) -> Vec<&'a SymbolRef> {
    if hits.len() >= WORD_HIT_LIMIT {
        return Vec::new();
    }
    let family: Vec<&SymbolRef> = hits
        .iter()
        .filter(|s| {
            name_tokens(last_segment(&s.fqn))
                .iter()
                .any(|t| t.eq_ignore_ascii_case(word))
        })
        .collect();
    if family.len() > TOKEN_FAMILY_NAME_CAP {
        return Vec::new();
    }
    family
}

/// The one indexed symbol a word names, if it names exactly one.
///
/// The index is searched by suffix and by name, which is right for a name a person typed and
/// wrong wherever the word came out of prose: without the last-segment check, "integration"
/// once anchored six imported design claims on `NoContinuousIntegration`.
///
/// Two callers, one rule: the seed stage reading a request, and the graphify import reading a
/// claim's label. Two copies of this would drift, and the copy further from the failure would
/// be the one still wrong. The seed stage needs the *other* two answers as well — several
/// symbols, or none named outright — so it reads `exactly_named` directly; a claim's label may
/// only ever anchor on a name it identifies, so this is what it asks.
pub(crate) fn uniquely_named_symbol(
    store: &Store,
    project_id: i64,
    word: &str,
) -> Result<Option<SymbolRef>, StoreError> {
    let hits = store.find_symbols_by_word(project_id, word, WORD_HIT_LIMIT)?;
    let named = exactly_named(&hits, word);
    let [only] = named.as_slice() else {
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

    // The candidate words, resolved once and capped, because this stage reads them twice:
    // here and at source 6 below. `targets` costs what its input costs, and a prompt is not
    // the 25-word symptom sentence its doc comment budgets for — paste a stack trace, a diff
    // or a file into one and it yields thousands of words. Measured on this repository: 5,305
    // candidate words, one indexed lookup each, 1.9 s against a 400 ms budget; and source 6
    // handed that same unbounded list to `facts_for_seeds`, which passed
    // SQLITE_MAX_COMPOUND_SELECT at 499 seeds and failed the whole request. The hook that
    // runs this discards stderr, so that error reached the developer as no context at all.
    // One cap here, on the binding both sources read, rather than one at each use.
    let mut targets = targets(&req.text);
    if targets.len() > SEED_QUERY_CAP {
        notes.push(format!(
            "the request names {} candidate words; the index was asked about \
             {SEED_QUERY_CAP} of them, qualified names first",
            targets.len()
        ));
        // Qualified names first. A word carrying internal evidence of code — a dot, slash,
        // `#`, `::`, an interior underscore, or a second capital (see `looks_like_code`) —
        // was typed as code; a plain word is prose that only earns a seed by naming exactly
        // one symbol. `is_plain_word` is the same predicate the loop below uses to tell those
        // apart, and `sort_by_key` is stable, so alphabetical order survives inside each group
        // and the cut falls on prose first.
        targets.sort_by_key(|t| is_plain_word(t));
        targets.truncate(SEED_QUERY_CAP);
    }

    // 2 and 4 — an FQN or path in the text, then a bare name. One lookup per candidate word;
    // `find_symbols` decides which kind it is, so the two sources differ only in how the
    // result is labelled.
    for target in &targets {
        let exact_shape = target.contains('.') || target.contains('/') || target.contains('#');
        // A plain lowercase word is weaker evidence than a name someone qualified, so it is
        // accepted only when it identifies its symbols rather than merely matching them.
        // Without that rule the word "integration" reaches `NoContinuousIntegration`, and a
        // symptom seeds the wrong file with confidence. `is_plain_word` is the same predicate
        // `targets` used to let this word through in the first place — re-deriving "what
        // counts as plain" here would be a second copy of that rule, and the two would drift
        // the moment either one changed.
        if is_plain_word(target) {
            let hits = store.find_symbols_by_word(project_id, target, WORD_HIT_LIMIT)?;
            match exactly_named(&hits, target).as_slice() {
                [only] => offer(
                    &mut found,
                    (*only).clone(),
                    SeedSource::NameMatch,
                    format!("'{target}' in the request names exactly one symbol"),
                ),
                // Several symbols are actually called this. The word cannot tell them apart,
                // and guessing between them anchors the package on the wrong one — which is
                // the whole reason this arm exists.
                [_, _, ..] => {}
                // Nothing is called this, so the word may still be a *part* of names. A prompt
                // says "the idempotency key" where the code says `idempotencyKey`, and before
                // this arm a prompt written that way anchored nothing at all.
                [] => {
                    for s in token_family(&hits, target) {
                        offer(
                            &mut found,
                            s.clone(),
                            SeedSource::NameMatch,
                            format!("'{target}' is a word in the name {}", last_segment(&s.fqn)),
                        );
                    }
                }
            }
            continue;
        }
        for s in store.find_symbols(project_id, target, 10)? {
            let source = if exact_shape {
                SeedSource::Exact
            } else {
                SeedSource::NameMatch
            };
            offer(&mut found, s, source, format!("'{target}' in the request"));
        }
    }

    // 3 — the changed set. Free for a review: the rescan already computed it.
    // `req.purpose == Purpose::Review` used to be tested here too. It is redundant now:
    // a declared purpose sets the intent before seeds are resolved, so this sees `Review`
    // either way, and the rule is stated in one place instead of two that could disagree.
    if matches!(intent, Intent::Review) {
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

    // 6 — a fact's subject named in the text. Inverted from a scan of every fact: a subject
    // the text names is, by construction, a target of that text, so the words `targets`
    // already yields (the same list sources 2 and 4 above seed from) are asked of
    // `facts_for_seeds` as seeds, rather than testing every live fact's subject against the
    // text by hand. This narrows what source 6 matches to the same rule the rest of stage 2
    // already applies — see the doc comment on `targets` for what that excludes.
    if !req.text.is_empty() {
        for fact in store.facts_for_seeds(project_id, &targets)? {
            let Some(subject) = fact.subject.as_deref() else {
                continue;
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A leading capital is English orthography, not a naming convention. Every sentence has
    /// one, and before this every prompt's first word was read as an identifier — skipping
    /// both the stopword list and the uniqueness rule. All five tokio prompts open with a
    /// capital.
    #[test]
    fn a_sentence_initial_capital_is_prose_not_code() {
        // The opening word of each tokio benchmark prompt. None was typed as code, so none
        // carries code shape — that's `looks_like_code`, not `is_plain_word`: `after` is a
        // stopword, so `is_plain_word("After")` must still be false once it reads as prose.
        for w in ["After", "Summing", "Feeding", "Binding", "Rebuilding"] {
            assert!(!looks_like_code(w), "{w} opens a sentence; it is prose");
        }
        // The four non-stopword openers are prose *and* worth seeding.
        for w in ["Summing", "Feeding", "Binding", "Rebuilding"] {
            assert!(
                is_plain_word(w),
                "{w} is prose and not a stopword; it should seed"
            );
        }
        // `after` is already in STOPWORDS. Once `After` is read as prose instead of an
        // identifier, the stopword list finally reaches it and it seeds nothing at all —
        // that is the whole point of the fix.
        assert!(
            !is_ordinary_word("After"),
            "a capitalised stopword is still a stopword"
        );
        assert!(
            !is_plain_word("After"),
            "a capitalised stopword must not seed"
        );

        // Internal evidence of a naming convention still reads as code.
        for w in [
            "StreamMap",
            "NOTIFY_AFTER",
            "lines_codec",
            "tokio::sync",
            "src/lib.rs",
        ] {
            assert!(!is_plain_word(w), "{w} carries code shape");
        }

        // A single-word type name has no internal evidence and becomes prose. That is
        // deliberate: it must then prove it names exactly one symbol, and `Semaphore` names
        // five in tokio.
        assert!(is_plain_word("Semaphore"));
    }
}
