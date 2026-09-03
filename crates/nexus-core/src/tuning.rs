//! Reading the inclusion ledger back (roadmap 5.7).
//!
//! The roadmap's rule for this task is not "tune the weights". It is **"the first tuning
//! backed by evidence"**, and the success criterion is that a weight change cites ledger data
//! in its commit message. R8 names the alternative by its right name: tuning before
//! measurement is folklore.
//!
//! So this reads the packages the Context Engine has already cached — each one carrying every
//! candidate it considered and the reason it was kept or refused — and reports what they say.
//! **It refuses to recommend anything until there is enough of them.** A recommendation drawn
//! from four packages is a recommendation drawn from one afternoon, and shipping one would
//! convert an honest "we do not know yet" into a number somebody else will trust.

use crate::context::{ContextPackage, Decision};
use serde::Serialize;
use std::path::Path;

/// Packages below which no recommendation is made.
///
/// Round, and deliberately not tuned itself. The point is that it is large enough that a
/// number derived from it is about a project's shape rather than about one session.
pub const MINIMUM_PACKAGES: usize = 30;

/// What the accumulated ledgers say.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WeightsReport {
    pub packages: usize,
    pub items_considered: usize,
    pub items_included: usize,
    /// Mean contribution of each term across included items, most influential first. A term
    /// near zero everywhere is a term doing no work.
    pub mean_terms: Vec<(String, f64)>,
    /// How often each exclusion rule fired. The budget refusing everything and the floor
    /// refusing everything are different problems with different fixes.
    pub exclusions: Vec<(String, usize)>,
    /// Present when there is not enough evidence to say anything. A report that recommended
    /// anyway would be the folklore this task exists to avoid.
    pub insufficient: Option<String>,
}

fn reason_class(reason: &str) -> &'static str {
    if reason.starts_with("selected") {
        "selected"
    } else if reason.contains("budget exhausted") {
        "budget exhausted"
    } else if reason.contains("below floor") {
        "below floor"
    } else if reason.contains("at most") {
        "component cap"
    } else if reason.contains("anchor") {
        "no anchor"
    } else {
        "other"
    }
}

/// Read every cached package under `.nexus/cache/context` and summarise it.
///
/// A cache entry that will not parse is skipped rather than fatal: the cache is disposable by
/// construction, and failing a report because one file is stale would make the report less
/// reliable than the thing it reports on.
pub fn report(cache_dir: &Path) -> WeightsReport {
    let mut out = WeightsReport::default();
    let mut sums = [0.0_f64; 9];
    let mut counts: std::collections::BTreeMap<&'static str, usize> = Default::default();

    let entries = match std::fs::read_dir(cache_dir.join("context")) {
        Ok(e) => e,
        Err(_) => {
            out.insufficient = Some(
                "no context packages have been cached yet, so there is nothing to learn from"
                    .into(),
            );
            return out;
        }
    };

    for entry in entries.flatten() {
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(pkg) = serde_json::from_str::<ContextPackage>(&raw) else {
            continue;
        };
        out.packages += 1;
        out.items_considered += pkg.items_considered;
        out.items_included += pkg.items_included;
        for item in &pkg.items {
            let t = &item.terms;
            for (i, v) in [
                t.seed, t.graph, t.churn, t.recency, t.history, t.fact, t.test, t.arch, t.cost,
            ]
            .into_iter()
            .enumerate()
            {
                sums[i] += v;
            }
        }
        for row in &pkg.ledger.rows {
            if row.decision == Decision::Excluded {
                *counts.entry(reason_class(&row.reason)).or_default() += 1;
            }
        }
    }

    let n = out.items_included.max(1) as f64;
    let names = [
        "seed", "graph", "churn", "recency", "history", "fact", "test", "arch", "cost",
    ];
    let mut means: Vec<(String, f64)> = names
        .iter()
        .zip(sums)
        .map(|(name, sum)| ((*name).to_string(), sum / n))
        .collect();
    means.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
    out.mean_terms = means;

    let mut exclusions: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    exclusions.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    out.exclusions = exclusions;

    if out.packages < MINIMUM_PACKAGES {
        out.insufficient = Some(format!(
            "{} package(s) cached, and no recommendation is made below {MINIMUM_PACKAGES}. \
             A number drawn from this many is about one session rather than about the \
             project, and shipping it would turn an honest 'we do not know yet' into a \
             figure somebody else trusts",
            out.packages
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("nexus-tuning-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("context")).expect("mkdir");
        d
    }

    fn package(json: &str, d: &Path, n: usize) {
        for i in 0..n {
            std::fs::write(d.join("context").join(format!("{i}.json")), json).expect("write");
        }
    }

    const ONE: &str = r#"{
        "purpose":"task",
        "project":{"name":"p","profile":null,"files":1,"symbols":1},
        "items":[{"kind":"symbol","anchor":{"file":"a.rs","line":1,"note":""},
                  "score":1.0,"terms":{"seed":1.0,"graph":0.0,"churn":0.5,"recency":0.0,
                  "history":0.0,"fact":0.0,"test":0.0,"arch":0.0,"cost":-0.1},
                  "why":"seed","text":"a","tokens":3}],
        "ledger":{"rows":[
            {"kind":"symbol","label":"a","decision":"included","reason":"selected","score":1.0,"tokens":3},
            {"kind":"symbol","label":"b","decision":"excluded","reason":"below floor (0.15)","score":0.01,"tokens":3}
        ]},
        "basis":{"dirty":false,"selection":"ranked"},
        "budget_tokens":4000,"tokens_estimated":3,"items_considered":2,"items_included":1
    }"#;

    #[test]
    fn nothing_cached_means_nothing_to_learn_from() {
        let d = dir("empty");
        let r = report(&d);
        assert_eq!(r.packages, 0);
        assert!(r.insufficient.is_some());
    }

    #[test]
    fn too_few_packages_yields_no_recommendation_and_says_why() {
        // R8: tuning before measurement is folklore. Refusing is the deliverable.
        let d = dir("few");
        package(ONE, &d, 3);
        let r = report(&d);
        assert_eq!(r.packages, 3);
        let why = r.insufficient.expect("refused");
        assert!(why.contains("one session"), "{why}");
    }

    #[test]
    fn enough_packages_yields_a_reading_of_which_terms_did_the_work() {
        let d = dir("enough");
        package(ONE, &d, MINIMUM_PACKAGES);
        let r = report(&d);
        assert!(r.insufficient.is_none(), "{:?}", r.insufficient);
        assert_eq!(r.mean_terms[0].0, "seed", "{:?}", r.mean_terms);
        // A term contributing nothing everywhere is a term doing no work, and that is what a
        // tuning would act on.
        assert!(r.mean_terms.iter().any(|(n, v)| n == "graph" && *v == 0.0));
    }

    #[test]
    fn exclusions_are_grouped_by_the_rule_that_refused_them() {
        // The budget refusing everything and the floor refusing everything are different
        // problems with different fixes, and a single "excluded" count hides which.
        let d = dir("reasons");
        package(ONE, &d, MINIMUM_PACKAGES);
        let r = report(&d);
        assert_eq!(r.exclusions[0].0, "below floor");
        assert_eq!(r.exclusions[0].1, MINIMUM_PACKAGES);
    }

    #[test]
    fn an_unreadable_cache_entry_is_skipped_rather_than_fatal() {
        // The cache is disposable by construction. Failing the report over one stale file
        // would make it less reliable than the thing it reports on.
        let d = dir("corrupt");
        package(ONE, &d, 2);
        std::fs::write(d.join("context").join("bad.json"), "{ not json").expect("write");
        assert_eq!(report(&d).packages, 2);
    }
}
