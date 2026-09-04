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
    /// Both counted in **edges**, not sites — the rendering must say so, because §7.2's
    /// `comparable` line is in sites and mixing the two produces a line that cannot be
    /// reconciled with itself.
    pub excluded_non_project: usize,
    pub excluded_oracle_blind: usize,
    pub sites_total: usize,
    /// No comparable edge survived. §8.2's rule one level up: an oracle that could not speak
    /// about anything says nothing about the resolver, and 0.000 is a score, not a silence.
    pub inconclusive: bool,
    /// False when the coverage denominator came from the edge dump rather than
    /// `nexus graph --files`, which understates it and therefore flatters coverage.
    pub coverage_denominator_is_complete: bool,
}

/// The file extension, lowercased, or `""` when there is none.
fn extension(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn build(
    oracle_name: &str,
    nexus_files: &[String],
    oracle_files: &HashSet<String>,
    comparison: &Comparison,
) -> Run {
    // §8.1 asks whether the oracle indexed everything Nexus did — but one SCIP indexer
    // speaks one language, and Nexus indexes five. Measured against every file, a
    // rust-analyzer oracle on this repository read 102 of 287 and marked a complete run
    // partial: the 77 Markdown files were never its job, and a caveat printed over numbers
    // that do not need one stops being read.
    //
    // So the denominator is scoped to the extensions the oracle actually produced documents
    // for. The limitation that leaves is worth naming: an extension the oracle skipped
    // *entirely* drops out of the check rather than failing it. Catching that needs to know
    // which languages the indexer claims to cover, which is a fact no SCIP index carries.
    let judged_extensions: HashSet<String> = oracle_files.iter().map(|f| extension(f)).collect();
    let comparable_files: Vec<&String> = nexus_files
        .iter()
        .filter(|f| judged_extensions.contains(&extension(f)))
        .collect();
    let seen = comparable_files
        .iter()
        .filter(|f| oracle_files.contains(**f))
        .count() as u64;
    let file_coverage = Rate::new(seen, comparable_files.len() as u64);
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
        inconclusive: comparison.judged.is_empty(),
        // The caller overwrites this when it knows better; the honest default is pessimistic.
        coverage_denominator_is_complete: false,
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
        "coverage    files: {} of {} indexed by oracle{}{}",
        run.file_coverage.k,
        run.file_coverage.n,
        if run.partial {
            "   PARTIAL — metrics below are advisory"
        } else {
            ""
        },
        if run.coverage_denominator_is_complete {
            ""
        } else {
            "   (denominator from the edge dump — pass --files for the true one)"
        }
    );
    // Sites and edges in one line, each labelled: the exclusion counters are per edge and
    // §7.2's `comparable` figure is per site, so an unlabelled line cannot be reconciled.
    let _ = writeln!(
        s,
        "comparable  {} of {} sites   ({} edges judged; {} excluded: {} non-project target, {} oracle-blind type)",
        run.scores.recall.n,
        run.sites_total,
        run.scores.precision.n,
        run.excluded_non_project + run.excluded_oracle_blind,
        run.excluded_non_project,
        run.excluded_oracle_blind
    );
    if run.inconclusive {
        let _ = writeln!(
            s,
            "            INCONCLUSIVE — no comparable edge survived; the figures below are not \
             a measurement of the resolver"
        );
    }
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
    fn a_file_the_oracle_does_not_index_the_language_of_is_not_a_gap() {
        // A rust-analyzer oracle has no opinion about Markdown, and Nexus indexes five
        // languages. Counting every file made a complete run read 102 of 287 and marked it
        // partial — a caveat over numbers that did not need one.
        let nexus_files = ["a.rs", "b.rs", "README.md", "notes.md"].map(String::from);
        let run = build_for_test(&nexus_files, &files(&["a.rs", "b.rs"]));
        assert!(!run.partial, "the Markdown was never the oracle's job");
        assert_eq!(run.file_coverage.n, 2, "only the .rs files are comparable");
    }

    #[test]
    fn a_missing_file_of_a_language_the_oracle_does_index_is_still_a_gap() {
        // The scoping must not swallow the case it exists to catch: scip-typescript skipping
        // a 1 MB file leaves its siblings indexed, so the extension is still in play.
        let nexus_files = ["a.rs", "b.rs", "big.rs", "README.md"].map(String::from);
        let run = build_for_test(&nexus_files, &files(&["a.rs"]));
        assert!(run.partial, "1 of 3 Rust files indexed is a partial oracle");
        assert_eq!(run.file_coverage.n, 3);
    }

    #[test]
    fn nothing_comparable_is_inconclusive_not_a_score_of_zero() {
        // §8.2 one level up: an oracle that could not speak about a single edge says nothing
        // about the resolver. Printing `precision 0.000` there is the harness reporting a
        // catastrophe it did not observe — which is exactly what the first real run did
        // before the line-base fix, and it read as a broken resolver rather than a broken
        // ruler.
        let nexus_files = ["a.rs"].map(String::from);
        let run = build_for_test(&nexus_files, &files(&["a.rs"]));
        assert!(run.inconclusive);
        assert!(
            render(&run).contains("INCONCLUSIVE"),
            "the caveat must reach the person reading the terminal"
        );
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
