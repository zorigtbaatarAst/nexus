//! Stage 1 — what is being asked.
//!
//! A verb table and a word matcher. Not a classifier, and emphatically not a model: §3 of the
//! Context Engine design rules that out, and the reason is not cost. Intent decides the
//! ranking weights, so a package is only explainable if the same words produce the same intent
//! on every run. A model cannot promise that; a table cannot break it.
//!
//! Three properties the tests pin, each of which was a bug in an obvious implementation:
//!
//!   * **Words, not substrings.** `prefix` contains `fix`. A substring match turns "the url
//!     prefix is wrong" into a debugging session about string matching.
//!   * **Most signals wins, not the first one seen.** "review the fix for the broken parser"
//!     is a debugging task wearing a review verb.
//!   * **Ties break by a written-down precedence.** A golden package whose intent depends on
//!     iteration order is not golden.

/// What the text is asking for. Distinct from [`Purpose`](super::Purpose), which is what the
/// *caller* asked for: an explicit `--purpose review` and the word "review" in a sentence are
/// different facts and must stay distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Debug,
    Build,
    Refactor,
    Review,
    Explain,
    /// The prompt refers to something the previous turn established — "now do the same for
    /// orders" — and names nothing this index can resolve. `13-evaluation.md` §14.1: the
    /// harness has the conversation, so it supplies the carried seeds; Nexus stays a pure
    /// function of (request, index, memory) and stores nothing.
    Referential,
    /// Nothing matched. Balanced weights downstream, and the package says it guessed nothing.
    Unknown,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Debug => "debug",
            Intent::Build => "build",
            Intent::Refactor => "refactor",
            Intent::Review => "review",
            Intent::Explain => "explain",
            Intent::Referential => "referential",
            Intent::Unknown => "unknown",
        }
    }
}

/// The classification and what produced it. `signal` is the evidence: an intent that cannot
/// name why it was chosen cannot be argued with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IntentMatch {
    pub intent: Intent,
    /// The matched phrase, or `None` for `Unknown`. Owned rather than borrowed because a
    /// package round-trips through the cache file (§11) and a `&'static str` cannot come
    /// back from disk.
    pub signal: Option<String>,
    /// False only for `Unknown`. Carried explicitly so a caller cannot mistake "we decided
    /// nothing" for "we decided Unknown".
    pub confident: bool,
}

/// The table from §3, in tie-break precedence order.
///
/// Precedence is deliberate rather than alphabetical: on an even split, a prompt that mentions
/// something being broken is treated as a bug before it is treated as anything else, because
/// the cost of missing a real defect exceeds the cost of over-weighting findings on a package
/// that turned out to be a refactor.
const TABLE: &[(Intent, &[&str])] = &[
    (
        Intent::Debug,
        &[
            "fix",
            "fixes",
            "fixing",
            "bug",
            "bugs",
            "broken",
            "breaks",
            "fails",
            "failing",
            "failure",
            "error",
            "errors",
            "crash",
            "crashes",
            "wrong",
            "regression",
        ],
    ),
    (
        Intent::Refactor,
        &[
            "refactor",
            "rename",
            "renames",
            "move",
            "moves",
            "extract",
            "inline",
            "restructure",
            "clean up",
            "tidy",
        ],
    ),
    (
        Intent::Build,
        &[
            "add",
            "adds",
            "implement",
            "implements",
            "build",
            "support",
            "create",
            "introduce",
            "write",
        ],
    ),
    (
        Intent::Review,
        &["review", "check", "is this safe", "done", "audit", "verify"],
    ),
    (
        Intent::Explain,
        &[
            "why",
            "how does",
            "how do",
            "what is",
            "what are",
            "explain",
            "understand",
        ],
    ),
];

/// Split on anything that is not alphanumeric, lowercased. Punctuation is a separator, so
/// "FIX the bug!" and "fix the bug" are the same prompt.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

/// A signal matches when it is a whole word, or — for a multi-word signal — a run of whole
/// words. Never a substring: `prefix` is not `fix`.
fn matches(signal: &str, words: &[String]) -> bool {
    let parts: Vec<&str> = signal.split(' ').collect();
    if parts.len() == 1 {
        return words.iter().any(|w| w == signal);
    }
    words
        .windows(parts.len())
        .any(|w| w.iter().zip(&parts).all(|(a, b)| a == b))
}

/// Java, Python and JavaScript frames all carry one of these, and none of them carries a verb
/// from the table. A pasted trace is the strongest bug signal there is; missing it means
/// ranking a crash report as `Unknown`.
fn looks_like_a_stack_trace(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("\n\tat ")
        || lower.contains("\n    at ")
        || lower.contains("traceback (most recent call last)")
        || lower.contains("exception")
        || lower.contains("panicked at")
}

/// Markers of a prompt that points at the previous turn rather than at the code.
///
/// Whole words only, same as the verb table: "that" must not match "thatch".
const REFERENTIAL: &[&str] = &[
    "the same", "that", "those", "it", "them", "also", "now do", "again", "likewise",
];

/// Does this prompt point backwards? Only meaningful when nothing in it names code — a
/// sentence can say "fix that in PaymentService" and be perfectly anchored.
pub fn is_referential(text: &str) -> bool {
    let w = words(text);
    REFERENTIAL.iter().any(|m| matches(m, &w))
}

/// Classify. Deterministic, allocation-light, and total: every input produces an answer, and
/// "no answer" is one of them.
pub fn classify(text: &str) -> IntentMatch {
    let unknown = IntentMatch {
        intent: Intent::Unknown,
        signal: None,
        confident: false,
    };
    if text.trim().is_empty() {
        return unknown;
    }
    let words = words(text);

    let mut best: Option<(Intent, &'static str, usize)> = None;
    for (intent, signals) in TABLE {
        let mut hits = 0usize;
        let mut first: Option<&'static str> = None;
        for signal in *signals {
            if matches(signal, &words) {
                hits += 1;
                if first.is_none() {
                    first = Some(signal);
                }
            }
        }
        if hits == 0 {
            continue;
        }
        let Some(signal) = first else { continue };
        // Strictly greater: TABLE order is the tie-break, so the earlier intent holds a draw.
        if best.is_none_or(|(_, _, prev)| hits > prev) {
            best = Some((*intent, signal, hits));
        }
    }

    // A trace beats a single incidental verb but not an explicit, repeated one — it is
    // evidence of a symptom, and the words are evidence of a request.
    if looks_like_a_stack_trace(text) && best.is_none_or(|(i, _, n)| i != Intent::Debug && n < 2) {
        return IntentMatch {
            intent: Intent::Debug,
            signal: Some("stack trace".into()),
            confident: true,
        };
    }

    match best {
        Some((intent, signal, _)) => IntentMatch {
            intent,
            signal: Some(signal.to_string()),
            confident: true,
        },
        None => unknown,
    }
}

/// Classification for a turn that may be referring to the previous one.
///
/// `anchored` is whether stage 2 found anything in the text to anchor on. This is deliberately
/// *not* decided here: a verb table has no index and cannot know whether `PaymentService`
/// exists, and guessing would put a classifier in the one place §3 keeps free of them.
///
/// The rule from §14.1: an unanchored prompt carrying a referential marker is `Referential`.
/// With carried seeds it uses them; without, the honest answer is `Unknown` — an empty-anchored
/// prompt with no carried seeds is a case where Nexus genuinely does not know, and saying so
/// beats inventing seeds.
pub fn classify_turn(text: &str, anchored: bool, has_carried_seeds: bool) -> IntentMatch {
    let base = classify(text);
    if anchored || !is_referential(text) {
        return base;
    }
    if has_carried_seeds {
        return IntentMatch {
            intent: Intent::Referential,
            signal: Some("refers to the previous turn".into()),
            confident: true,
        };
    }
    IntentMatch {
        intent: Intent::Unknown,
        signal: None,
        confident: false,
    }
}

#[cfg(test)]
mod referential_tests {
    use super::*;

    #[test]
    fn an_unanchored_referential_prompt_with_carried_seeds_is_referential() {
        let got = classify_turn("now do the same for orders", false, true);
        assert_eq!(got.intent, Intent::Referential, "{got:?}");
        assert!(got.confident);
    }

    #[test]
    fn the_same_prompt_without_carried_seeds_says_it_does_not_know() {
        // §14.1: an empty-anchored prompt with no carried seeds is a case where Nexus
        // genuinely does not know, and saying so beats inventing seeds.
        let got = classify_turn("now do the same for orders", false, false);
        assert_eq!(got.intent, Intent::Unknown, "{got:?}");
        assert!(!got.confident);
    }

    #[test]
    fn an_anchored_prompt_keeps_its_verb_even_when_it_says_that() {
        // "fix that in PaymentService" is anchored; treating it as referential would discard
        // the anchor the developer actually gave.
        let got = classify_turn("fix that in PaymentService", true, true);
        assert_eq!(got.intent, Intent::Debug, "{got:?}");
    }

    #[test]
    fn referential_markers_are_whole_words() {
        assert!(is_referential("do that now"));
        assert!(!is_referential("the thatch is broken"));
        assert!(is_referential("now do the same"));
        assert!(!is_referential("rename the sameness helper"));
    }
}
