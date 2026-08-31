//! `nexus ask` — the questions an agent actually has.
//!
//! Every answer here is an existing query. The command exists because the questions are
//! phrased the way a person or an agent phrases them, and because two of them ("what do we
//! already know about this code", "what should I look at next") had no surface at all.

use nexus_core::impact::{Direction, ImpactQuery};
use nexus_core::report::Resolved;
use nexus_core::{Engine, EngineError};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "question", rename_all = "snake_case")]
pub enum Answer {
    Changed {
        since: Option<String>,
        symbols: Vec<String>,
        files: usize,
    },
    Affected {
        target: String,
        symbols: Vec<Affected>,
        crossed_seam: usize,
    },
    Known {
        target: String,
        findings: Vec<nexus_core::FindingSummary>,
        facts: Vec<nexus_core::Fact>,
    },
    Facts {
        facts: Vec<nexus_core::Fact>,
    },
    Next {
        suggestions: Vec<Suggestion>,
    },
    Unknown {
        asked: String,
        understood: Vec<&'static str>,
    },
}

#[derive(Debug, Serialize)]
pub struct Affected {
    pub fqn: String,
    pub score: f64,
    pub min_confidence: f64,
}

#[derive(Debug, Serialize)]
pub struct Suggestion {
    pub target: String,
    pub why: String,
    pub score: f64,
}

pub fn answer(engine: &Engine, question: &[String]) -> Result<Answer, EngineError> {
    let verb = question.first().map(String::as_str).unwrap_or("");
    let target = question.get(1..).map(|r| r.join(" ")).unwrap_or_default();

    match verb {
        "changed" | "what-changed" => {
            let rows = engine.changes(Some("symbol"))?;
            let files = engine.changes(Some("file"))?.len();
            Ok(Answer::Changed {
                since: engine.status()?.baseline.and_then(|b| b.scan_uid),
                symbols: rows.into_iter().filter_map(|(_, _, t, _)| t).collect(),
                files,
            })
        }

        // "What is affected by this change?" and "Where is this symbol used?" are the same
        // traversal asked from two directions, so they share an answer.
        "affected" | "uses" | "affects" => {
            let q = ImpactQuery {
                target: target.clone(),
                direction: Direction::Reverse,
                ..Default::default()
            };
            match engine.impact(&q)? {
                Resolved::One(r) => Ok(Answer::Affected {
                    target,
                    crossed_seam: r.crossed_seam,
                    symbols: r
                        .items
                        .into_iter()
                        .map(|i| Affected {
                            fqn: i.fqn,
                            score: i.score,
                            min_confidence: i.min_confidence,
                        })
                        .collect(),
                }),
                _ => Ok(Answer::Affected {
                    target,
                    symbols: Vec::new(),
                    crossed_seam: 0,
                }),
            }
        }

        // "Have we seen this problem before?" — the question worth asking before changing
        // anything, and the one persistent knowledge exists to answer.
        "known" | "about" | "seen" => Ok(Answer::Known {
            findings: engine.findings_for(&target)?,
            facts: engine.facts(Some(&target))?,
            target,
        }),

        "facts" | "remember" => Ok(Answer::Facts {
            facts: engine.facts(None)?,
        }),

        "next" | "what-next" => Ok(Answer::Next {
            suggestions: suggest(engine)?,
        }),

        _ => Ok(Answer::Unknown {
            asked: question.join(" "),
            understood: vec![
                "changed",
                "affected <target>",
                "uses <target>",
                "known <target>",
                "facts",
                "next",
            ],
        }),
    }
}

/// What to look at next: changed symbols, ranked by how much they affect and by whether
/// anything has gone wrong there before.
///
/// Both halves are already indexed, so this is a ranking rather than an analysis — which is
/// the point. Nexus does not need to think about what to examine; it already knows.
fn suggest(engine: &Engine) -> Result<Vec<Suggestion>, EngineError> {
    let changed: Vec<String> = engine
        .changes(Some("symbol"))?
        .into_iter()
        .filter_map(|(_, _, target, _)| target)
        .collect();

    let mut out = Vec::new();
    for fqn in changed.into_iter().take(40) {
        let q = ImpactQuery {
            target: fqn.clone(),
            ..Default::default()
        };
        let reach = match engine.impact(&q)? {
            Resolved::One(r) => r.items.len(),
            _ => 0,
        };
        let prior = engine.findings_for(&fqn)?.len();
        // Reach is the cost of being wrong; prior findings are evidence that this code has
        // been wrong before. Neither alone is a good reason to look.
        let score = reach as f64 + prior as f64 * 3.0;
        if score <= 0.0 {
            continue;
        }
        out.push(Suggestion {
            why: match (reach, prior) {
                (r, 0) => format!("changed, and {r} symbols depend on it"),
                (0, p) => format!("changed, and {p} findings already exist here"),
                (r, p) => format!("changed, {r} symbols depend on it, {p} findings already here"),
            },
            target: fqn,
            score,
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(10);
    Ok(out)
}
