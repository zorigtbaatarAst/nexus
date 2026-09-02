//! `nexus ask` — the questions an agent actually has.
//!
//! Only the spelling lives here. Every answer is `Engine::ask`, because each one needs
//! several queries and the rule is that a caller needing two `Engine` calls has found a
//! missing `Engine` method. What remains is the mapping from the words a person types to the
//! question they mean, which is this surface's business and not the platform's.

// Re-exported so `render` keeps naming `ask::Answer`: where the type lives is the
// platform's business, and the renderer should not have to care that it moved.
pub use nexus_core::report::Answer;
use nexus_core::report::Question;
use nexus_core::{Engine, EngineError};

/// The verbs this surface accepts. Listed once, so the help text and the parser cannot drift.
pub const UNDERSTOOD: &[&str] = &[
    "changed",
    "affected <target>",
    "uses <target>",
    "known <target>",
    "facts",
    "next",
];

pub fn answer(engine: &Engine, question: &[String]) -> Result<Answer, EngineError> {
    let verb = question.first().map(String::as_str).unwrap_or("");
    let target = question.get(1..).map(|r| r.join(" ")).unwrap_or_default();

    let q = match verb {
        "changed" | "what-changed" => Question::Changed,
        "affected" | "uses" | "affects" => Question::Affected(target),
        "known" | "about" | "seen" => Question::Known(target),
        "facts" | "remember" => Question::Facts,
        "next" | "what-next" => Question::Next,
        _ => {
            return Ok(Answer::Unknown {
                asked: question.join(" "),
                understood: UNDERSTOOD.to_vec(),
            })
        }
    };
    engine.ask(&q)
}
