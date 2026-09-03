//! Project memory: what a fact is worth, and what a fact may be about.
//!
//! One retrieval formula, in one function, called by every consumer. `06-memory.md` §4 gives
//! it five factors and this file is all five. Two rankings over one table would disagree
//! eventually, and the one further from the data is the one that would be wrong.
//!
//! Retrieval is a *stage of the Context Engine*, not a subsystem: facts compete with symbols,
//! findings and changes on one scoring function and one budget. What lives here is the part
//! that is specific to a fact — its subject, its provenance, its lifecycle state and its age.

use nexus_store::FactRow;

/// The `fact_key` namespaces of §2. Flat, dotted, greppable, and closed.
///
/// The list is enforced rather than documented. §2 says `task.` is *deliberately* absent —
/// task history is already `finding_occurrences`, `changes` and `scans`, and a parallel
/// narrative log is a transcript by another name — and "deliberately absent" only means
/// something if something refuses it.
pub const NAMESPACES: &[&str] = &[
    "arch",       // how a module is actually structured
    "constraint", // a limit the project must respect
    "convention", // how this project does a thing
    "decision",   // a choice made, and why
    "discovery",  // something worked out that was expensive to work out
    "failure",    // an approach tried that did not work
    "incident",   // something that broke in production, and why
    "invariant",  // something that must always hold
    "pattern",    // a recurring shape worth recognising
    "risk",       // a known hazard not yet addressed
];

/// `Ok(())` when the key sits in a known namespace.
///
/// The error names every valid prefix, which is the difference between a wall and a door.
pub fn check_key(key: &str) -> Result<(), String> {
    let Some((prefix, rest)) = key.split_once('.') else {
        return Err(format!(
            "fact key '{key}' has no namespace — use one of: {}",
            NAMESPACES.join(", ")
        ));
    };
    if rest.is_empty() {
        return Err(format!("fact key '{key}' is only a namespace"));
    }
    if prefix == "task" {
        return Err(
            "'task.' is deliberately not a fact namespace: task history is already recorded as \
             finding occurrences, changes and scans, and a parallel narrative log is a \
             transcript by another name"
                .into(),
        );
    }
    if !NAMESPACES.contains(&prefix) {
        return Err(format!(
            "'{prefix}.' is not a fact namespace — use one of: {}",
            NAMESPACES.join(", ")
        ));
    }
    Ok(())
}

/// §4: exact FQN 1.0 · module prefix 0.6 · project 0.3.
///
/// With no seeds the answer is the project term: the caller asked for everything, so nothing
/// is more relevant than anything else, and 0.3 keeps the other factors deciding the order.
pub fn subject_match(subject: Option<&str>, seeds: &[String]) -> f64 {
    let Some(subject) = subject else {
        return 0.3;
    };
    if seeds.is_empty() {
        return 0.3;
    }
    seeds
        .iter()
        .map(|seed| {
            if seed == subject {
                1.0
            } else if seed.starts_with(subject) || subject.starts_with(seed.as_str()) {
                0.6
            } else {
                0.0
            }
        })
        .fold(0.0_f64, f64::max)
        .max(0.3)
}

/// §4: human 1.0 · deterministic 0.9 · ai 0.7.
pub fn source_weight(source: &str) -> f64 {
    match source {
        "human" => 1.0,
        "deterministic" => 0.9,
        _ => 0.7,
    }
}

/// §4: durable 1.0 · validated 0.85 · candidate 0.6.
///
/// The gap between candidate and validated is the whole point of the lifecycle: an assertion
/// nothing has re-checked is worth materially less than one three scans have.
pub fn state_weight(durable: bool, validated_count: i64) -> f64 {
    if durable {
        1.0
    } else if validated_count > 0 {
        0.85
    } else {
        0.6
    }
}

/// §4: gentle decay in scans, not days.
///
/// Scans are what this database can prove; a wall-clock age would need a timestamp the fact
/// does not carry and would measure how often someone runs the tool. Halving every 40 scans
/// keeps a year-old invariant near full weight, which is right — old facts are usually still
/// true, and the thing that makes one wrong is invalidation, not age.
pub fn recency_decay(created_scan_id: i64, current_scan_id: i64) -> f64 {
    let age = (current_scan_id - created_scan_id).max(0) as f64;
    (-age / 40.0).exp().clamp(0.05, 1.0)
}

/// §4's formula, entire. Invalidated and superseded rows never reach here — the store's query
/// excludes them, because a stale memory that reads as authority is the failure the whole
/// lifecycle exists to prevent.
pub fn relevance(fact: &FactRow, seeds: &[String], current_scan_id: i64) -> f64 {
    subject_match(fact.subject.as_deref(), seeds)
        * source_weight(&fact.source)
        * state_weight(fact.durable, fact.validated_count)
        * fact.confidence
        * recency_decay(fact.created_scan_id, current_scan_id)
}

/// Facts, most relevant first. Ties break on the key so the order is reproducible.
pub fn rank(mut facts: Vec<FactRow>, seeds: &[String], current_scan_id: i64) -> Vec<FactRow> {
    facts.sort_by(|a, b| {
        relevance(b, seeds, current_scan_id)
            .total_cmp(&relevance(a, seeds, current_scan_id))
            .then_with(|| a.key.cmp(&b.key))
    });
    facts
}

/// One fact, rendered for a human (§6).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportedFact {
    pub key: String,
    pub namespace: String,
    pub claim: String,
    pub subject: Option<String>,
    pub source: String,
    pub state: &'static str,
    pub confidence: f64,
    pub evidence: Vec<String>,
    pub created_scan_id: i64,
    pub validated_count: i64,
}

/// The state name §3 gives this row. Invalidated facts never reach here: the store's query
/// excludes them, and a view that showed them would be showing knowledge Nexus has withdrawn.
pub fn state_name(durable: bool, validated_count: i64) -> &'static str {
    if durable {
        "durable"
    } else if validated_count > 0 {
        "validated"
    } else {
        "candidate"
    }
}

pub fn namespace_of(key: &str) -> &str {
    key.split_once('.').map_or("other", |(ns, _)| ns)
}

impl ExportedFact {
    pub fn from_row(row: &FactRow) -> Self {
        let evidence = row
            .evidence_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(j).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| {
                Some(format!(
                    "{}:{}",
                    v.get("file")?.as_str()?,
                    v.get("line")?.as_u64()?
                ))
            })
            .collect();
        ExportedFact {
            key: row.key.clone(),
            namespace: namespace_of(&row.key).to_string(),
            claim: row.claim.clone(),
            subject: row.subject.clone(),
            source: row.source.clone(),
            state: state_name(row.durable, row.validated_count),
            confidence: row.confidence,
            evidence,
            created_scan_id: row.created_scan_id,
            validated_count: row.validated_count,
        }
    }
}

/// The header every generated file carries.
///
/// Says what the file is and what will happen to an edit, because §6's whole separation rests
/// on nobody treating this as a place to write. Nexus never reads it back: a round trip
/// through Markdown would make an unvalidated text file authoritative over an
/// evidence-checked row, which inverts the design.
pub fn generated_header(namespace: &str, count: usize) -> String {
    format!(
        "<!-- Generated by nexus memory export. Do not edit: the next export overwrites this \
file. To add knowledge, run `nexus fact <key> <claim> --evidence PATH:LINE`, which records \
where it came from. Nexus never reads this directory. -->\n\n# {namespace}\n\n{count} \
fact(s). The database is the source of truth; this is a view of it.\n\n"
    )
}

/// One namespace as a Markdown document.
///
/// `[[key]]` wikilinks are the entire Obsidian integration — one string convention, no
/// plugin, no sync, no schema. That is exactly as much investment as a viewer should get.
pub fn to_markdown(namespace: &str, facts: &[ExportedFact]) -> String {
    let mut out = generated_header(namespace, facts.len());
    for f in facts {
        out.push_str(&format!("## {}\n\n{}\n\n", f.key, f.claim));
        out.push_str(&format!(
            "- **state** {} · **source** {} · **confidence** {:.2}\n",
            f.state, f.source, f.confidence
        ));
        if let Some(subject) = &f.subject {
            out.push_str(&format!("- **subject** `{subject}`\n"));
        }
        out.push_str(&format!(
            "- **learned in** scan {} · validated {} time(s)\n",
            f.created_scan_id, f.validated_count
        ));
        if f.evidence.is_empty() {
            out.push_str("- **evidence** none — nothing checks this against a later scan\n");
        } else {
            for e in &f.evidence {
                out.push_str(&format!("- **evidence** `{e}`\n"));
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(key: &str, subject: Option<&str>, source: &str, durable: bool, n: i64) -> FactRow {
        FactRow {
            key: key.into(),
            scope: "symbol".into(),
            subject: subject.map(str::to_string),
            claim: "c".into(),
            source: source.into(),
            confidence: 0.9,
            evidence_json: None,
            validated_count: n,
            durable,
            created_scan_id: 1,
        }
    }

    #[test]
    fn the_ten_namespaces_are_accepted_and_nothing_else_is() {
        for ns in NAMESPACES {
            assert!(check_key(&format!("{ns}.x")).is_ok(), "{ns}");
        }
        for bad in ["nonsense.x", "x", "arch", "arch."] {
            assert!(check_key(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn task_history_is_refused_with_its_reason() {
        // §2 keeps `task.` out on purpose, and "deliberately absent" only means something if
        // something refuses it.
        let err = check_key("task.did-a-thing").expect_err("refused");
        assert!(err.contains("transcript"), "{err}");
    }

    #[test]
    fn an_error_names_every_valid_prefix_so_it_is_a_door_not_a_wall() {
        let err = check_key("nonsense.x").expect_err("refused");
        for ns in NAMESPACES {
            assert!(err.contains(ns), "{ns} missing from: {err}");
        }
    }

    #[test]
    fn a_durable_human_fact_about_the_exact_symbol_outranks_a_candidate_ai_fact() {
        let seeds = vec!["mn.pay.PaymentService#pay".to_string()];
        let best = fact(
            "invariant.a",
            Some("mn.pay.PaymentService#pay"),
            "human",
            true,
            9,
        );
        let worst = fact("arch.b", Some("mn.pay"), "ai", false, 0);
        assert!(
            relevance(&best, &seeds, 1) > relevance(&worst, &seeds, 1),
            "{} vs {}",
            relevance(&best, &seeds, 1),
            relevance(&worst, &seeds, 1)
        );
    }

    #[test]
    fn the_lifecycle_actually_moves_the_ranking() {
        // If state weight did not separate them, the whole validation pass would buy nothing.
        let seeds = vec!["mn.pay.A".to_string()];
        let candidate = fact("arch.x", Some("mn.pay.A"), "ai", false, 0);
        let validated = fact("arch.x", Some("mn.pay.A"), "ai", false, 1);
        let durable = fact("arch.x", Some("mn.pay.A"), "ai", true, 3);
        let r = |f: &FactRow| relevance(f, &seeds, 1);
        assert!(r(&candidate) < r(&validated), "candidate < validated");
        assert!(r(&validated) < r(&durable), "validated < durable");
    }

    #[test]
    fn an_exact_subject_beats_a_module_prefix_which_beats_no_relation() {
        let seeds = vec!["mn.pay.PaymentService#pay".to_string()];
        assert_eq!(
            subject_match(Some("mn.pay.PaymentService#pay"), &seeds),
            1.0
        );
        assert_eq!(subject_match(Some("mn.pay"), &seeds), 0.6);
        // The floor is the project term: an unrelated fact is still a fact about the project.
        assert_eq!(subject_match(Some("mn.orders"), &seeds), 0.3);
        assert_eq!(subject_match(None, &seeds), 0.3);
    }

    #[test]
    fn with_no_seeds_every_subject_scores_the_same_so_other_factors_decide() {
        let none: Vec<String> = Vec::new();
        assert_eq!(subject_match(Some("anything"), &none), 0.3);
        let human = fact("arch.a", Some("x"), "human", true, 3);
        let ai = fact("arch.b", Some("x"), "ai", false, 0);
        assert!(relevance(&human, &none, 1) > relevance(&ai, &none, 1));
    }

    #[test]
    fn age_decays_gently_and_never_to_nothing() {
        // Old facts are usually still true. What makes one wrong is invalidation, not age —
        // so this must never be the term that hides a durable invariant.
        assert!((recency_decay(1, 1) - 1.0).abs() < 1e-9);
        assert!(recency_decay(1, 41) > 0.36, "one halving is gentle");
        assert!(recency_decay(1, 10_000) >= 0.05, "and it has a floor");
    }

    #[test]
    fn ranking_is_reproducible_for_equal_scores() {
        let seeds = vec!["mn.pay.A".to_string()];
        let input = vec![
            fact("arch.c", Some("mn.pay.A"), "ai", false, 0),
            fact("arch.a", Some("mn.pay.A"), "ai", false, 0),
            fact("arch.b", Some("mn.pay.A"), "ai", false, 0),
        ];
        let keys: Vec<String> = rank(input.clone(), &seeds, 1)
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert_eq!(keys, vec!["arch.a", "arch.b", "arch.c"]);
        for _ in 0..10 {
            let again: Vec<String> = rank(input.clone(), &seeds, 1)
                .into_iter()
                .map(|f| f.key)
                .collect();
            assert_eq!(again, keys);
        }
    }
}
