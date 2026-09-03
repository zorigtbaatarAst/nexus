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
