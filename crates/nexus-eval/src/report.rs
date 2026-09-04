//! Assemble the run and render it in the column layout the spec fixes in §7.2.

use crate::matcher::Comparison;
use crate::metrics::{self, Rate, Scores, TierResult, Verdict};
use std::collections::HashSet;
use std::fmt::Write as _;

/// Below this share of files indexed, the metrics are advisory rather than a measurement.
const MIN_FILE_COVERAGE: f64 = 0.95;

#[derive(Debug, serde::Serialize)]
pub struct Run {
    pub oracle: String,
    pub file_coverage: Rate,
    /// True when the oracle did not index everything Nexus did. Metrics on a partial oracle
    /// read high, so this must travel with them rather than being inferable from a ratio
    /// nobody reads.
    pub partial: bool,
    pub scores: Scores,
    pub brier: f64,
    pub ece: f64,
    pub tiers: Vec<TierResult>,
    pub excluded_non_project: usize,
    pub excluded_oracle_blind: usize,
    pub sites_total: usize,
}

pub fn build(
    oracle_name: &str,
    nexus_files: &[String],
    oracle_files: &HashSet<String>,
    comparison: &Comparison,
) -> Run {
    let seen = nexus_files
        .iter()
        .filter(|f| oracle_files.contains(*f))
        .count() as u64;
    let file_coverage = Rate::new(seen, nexus_files.len() as u64);
    let tiers = metrics::calibrate(comparison);
    Run {
        oracle: oracle_name.to_string(),
        partial: file_coverage.value < MIN_FILE_COVERAGE,
        file_coverage,
        scores: metrics::score(comparison),
        brier: metrics::brier(comparison),
        ece: metrics::ece(&tiers),
        tiers,
        excluded_non_project: comparison.excluded_non_project,
        excluded_oracle_blind: comparison.excluded_oracle_blind,
        sites_total: comparison.sites_total,
    }
}

fn rate(r: &Rate) -> String {
    format!("{:.3} [{:.3}-{:.3}]", r.value, r.low, r.high)
}

pub fn render(run: &Run) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "oracle      {}", run.oracle);
    let _ = writeln!(
        s,
        "coverage    files: {} of {} indexed by oracle{}",
        run.file_coverage.k,
        run.file_coverage.n,
        if run.partial {
            "   PARTIAL — metrics below are advisory"
        } else {
            ""
        }
    );
    let judged_sites = run.scores.recall.n;
    let _ = writeln!(
        s,
        "comparable  {judged_sites} of {} sites   ({} excluded: {} non-project target, {} oracle-blind type)",
        run.sites_total,
        run.excluded_non_project + run.excluded_oracle_blind,
        run.excluded_non_project,
        run.excluded_oracle_blind
    );
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "precision   {}        recall  {}        F1  {:.3}",
        rate(&run.scores.precision),
        rate(&run.scores.recall),
        run.scores.f1
    );
    let _ = writeln!(s, "strict      {}", rate(&run.scores.strict));
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "calibration Brier {:.3}    ECE {:.3}",
        run.brier, run.ece
    );
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "{:<15}{:<9}{:<22}{:<7}verdict",
        "tier", "claims", "measured", "n"
    );
    for t in &run.tiers {
        let verdict = match (t.verdict, t.proposed) {
            (Verdict::Ok, _) => "ok".to_string(),
            (Verdict::UnderPowered, _) => "under-powered".to_string(),
            (Verdict::Miscalibrated, Some(p)) => format!("MISCALIBRATED -> {p:.2}"),
            // A miscalibrated verdict always carries a proposal — the branch that sets it
            // computes one — but saying so in the type would be a refactor this report does
            // not need. If it ever prints, the invariant broke and the report says which tier.
            (Verdict::Miscalibrated, None) => "MISCALIBRATED (no proposal)".to_string(),
        };
        let _ = writeln!(
            s,
            "{:<15}{:<9.2}{:<22}{:<7}{}",
            t.tier,
            t.claimed,
            rate(&t.measured),
            t.measured.n,
            verdict
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_for_test(nexus_files: &[String], oracle_files: &HashSet<String>) -> Run {
        build("test", nexus_files, oracle_files, &Comparison::default())
    }

    fn files(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_oracle_that_missed_files_marks_the_run_partial() {
        // scip-typescript silently skips files over 1MB; scip-python emits a partial index on
        // timeout rather than failing. A partial oracle *inflates* precision, because Nexus
        // edges in unindexed files fall out of the comparable set instead of being judged.
        // The harness's failure mode would otherwise be a flattering result.
        let nexus_files = ["a.rs", "b.rs", "c.rs", "d.rs"].map(String::from);
        let run = build_for_test(&nexus_files, &files(&["a.rs", "b.rs"]));
        assert!(
            run.partial,
            "2 of 4 files indexed must not be reported as a clean run"
        );
        assert_eq!(run.file_coverage.n, 4);
    }

    #[test]
    fn a_complete_oracle_is_not_partial() {
        let nexus_files = ["a.rs", "b.rs"].map(String::from);
        assert!(!build_for_test(&nexus_files, &files(&["a.rs", "b.rs"])).partial);
    }

    #[test]
    fn a_partial_run_says_so_in_the_rendering_beside_the_numbers() {
        // The flag is worthless if it only exists in the JSON: the person reading the
        // terminal is the one about to quote the precision figure in a README.
        let nexus_files = ["a.rs", "b.rs", "c.rs", "d.rs"].map(String::from);
        let text = render(&build_for_test(&nexus_files, &files(&["a.rs"])));
        assert!(
            text.contains("PARTIAL"),
            "the rendering must carry the caveat:\n{text}"
        );
    }
}
