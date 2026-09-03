//! `verify` — the gate. "Done" gets checked.
//!
//! The engine derives a plan from the detected profile, hands it to `nexus-verify`, and
//! writes down what came back. It does not execute anything itself: process spawning lives in
//! the crate that owns that risk, and the boundary test asserts that crate cannot reach the
//! database.
//!
//! The baseline run is the expensive half and it stays. Without it, a suite that was already
//! failing is indistinguishable from a suite the change broke — which is the entire question
//! being asked, so halving the cost by removing the answer is not an optimisation (ADR-025).

use super::*;
use nexus_verify::{judge, plan_for, run_plan, HostRunner, Plan, Verdict};

impl Engine {
    /// Run the project's own build, test and lint commands and judge the result.
    ///
    /// Execution is refused unless the committed policy permits it. `execute = "none"` is the
    /// default and produces a *result* saying so, never an execution: a permission system
    /// that can be talked into running something is not one.
    pub fn verify(&mut self) -> Result<VerifyReport> {
        let started = Instant::now();
        let policy = crate::policy::load_execution(&self.root.join(NEXUS_DIR).join("policy.toml"));
        if policy.execute == "none" {
            return Ok(VerifyReport {
                verdict: "permission_required".into(),
                why: Some(
                    "policy.execute is \"none\", so nothing was run. Set it to \"host\" in \
                     .nexus/policy.toml — committed, so the decision is reviewed."
                        .into(),
                ),
                checks: Vec::new(),
                baseline: None,
                note: None,
                duration_ms: started.elapsed().as_millis(),
            });
        }

        let profile = self.load_profile()?;
        let plan = plan_for(
            profile.as_ref().and_then(|p| p.build_system.as_deref()),
            policy.timeout_seconds,
        );

        let head = run_plan(&HostRunner, &plan, &self.root);
        // The ledger, before the judgement: what ran is a fact whatever the verdict turns out
        // to be, and a run that is only recorded when it is interesting is a run nobody can
        // count.
        self.record_run(&head)?;

        // The baseline half. Skipped, with the reason said out loud, when there is nothing
        // comparable to run against.
        let (baseline_checks, baseline_note) = self.baseline_run(&plan);

        let verdict = judge(head, baseline_checks, &plan);
        let (name, why, note) = match &verdict {
            Verdict::Verified { note, .. } => ("verified", None, note.clone()),
            Verdict::Failed { detail, note, .. } => ("failed", Some(detail.clone()), note.clone()),
            Verdict::Inconclusive { why, .. } => ("inconclusive", Some(why.clone()), None),
        };
        self.feed_findings(name, verdict.checks())?;
        Ok(VerifyReport {
            verdict: name.into(),
            why,
            checks: verdict.checks().to_vec(),
            baseline: baseline_note,
            note,
            duration_ms: started.elapsed().as_millis(),
        })
    }

    /// Everything a run leaves behind: the ledger row, the tests it named, and the coverage
    /// those tests prove.
    ///
    /// Coverage from a run that actually happened is different in kind from a filename match.
    /// `impact::is_test` stays — it is still a reasonable guess when nothing has run — but a
    /// `runtime` row is evidence, and Review says which of the two it used.
    fn record_run(&mut self, checks: &[nexus_verify::Check]) -> Result<()> {
        let Some(baseline) = self.store.baseline(self.project_id)? else {
            return Ok(());
        };
        let (commit, _) = self.head();
        let logs = self.root.join(NEXUS_DIR).join("cache").join("verify-logs");
        let _ = std::fs::create_dir_all(&logs);

        for check in checks {
            let tests = nexus_verify::parse_tests(&check.output);
            let counts = nexus_verify::counts_of(&tests);
            // Output on disk, a path in the row. A megabyte of build output in a database
            // column is a database nobody can query.
            let log_path = {
                let name = format!(
                    "{}-{}.log",
                    check.kind.as_str(),
                    nexus_store::now().replace(':', "-")
                );
                let path = logs.join(&name);
                match std::fs::write(&path, &check.output) {
                    Ok(()) => Some(path.display().to_string()),
                    Err(_) => None,
                }
            };

            let tx = self.store.transaction()?;
            Store::insert_test_run(
                &tx,
                self.project_id,
                Some(baseline.scan_id),
                commit.as_deref(),
                &check.argv.join(" "),
                nexus_verify::sandbox_name(),
                check.exit_code,
                check.duration_ms as i64,
                (counts.passed, counts.failed, counts.skipped),
                log_path.as_deref(),
                &nexus_store::now(),
            )?;
            tx.commit().map_err(nexus_store::StoreError::from)?;

            for test in tests.iter().filter(|t| t.passed) {
                self.record_coverage_for(baseline.scan_id, &test.name)?;
            }
        }
        Ok(())
    }

    /// Link one passing test to the symbols it reaches.
    ///
    /// Reachability from a test that ran, rather than from a file whose name looks like a
    /// test. A test name the index cannot resolve records nothing at all: an invented
    /// coverage row would make Review cite evidence that does not exist, which is worse than
    /// the guess it replaces.
    fn record_coverage_for(&mut self, scan_id: i64, test_name: &str) -> Result<()> {
        let matches = self.store.find_symbols(self.project_id, test_name, 2)?;
        let [test_symbol] = matches.as_slice() else {
            return Ok(());
        };
        let reached = impact::run(
            &self.store,
            self.project_id,
            std::slice::from_ref(test_symbol),
            &ImpactQuery {
                direction: impact::Direction::Forward,
                max_depth: 4,
                ..Default::default()
            },
        )?;
        let tx = self.store.transaction()?;
        let test_id = Store::upsert_test(&tx, self.project_id, scan_id, test_name, None)?;
        for item in &reached.items {
            if let Some(id) = Store::symbol_id_by_fqn(&tx, self.project_id, &item.fqn)? {
                Store::record_coverage(&tx, test_id, id, "runtime", item.min_confidence)?;
            }
        }
        tx.commit().map_err(nexus_store::StoreError::from)?;
        Ok(())
    }

    /// Run the same plan at the baseline commit, in a detached worktree.
    ///
    /// `git stash` is never used: a verifier that mutates the working tree can lose
    /// uncommitted work, and one that does is uninstalled the first time. The worktree is
    /// cached per sha, because a commit's contents do not change and a second full build buys
    /// nothing.
    fn baseline_run(&self, plan: &Plan) -> (Option<Vec<nexus_verify::Check>>, Option<String>) {
        if plan.steps.is_empty() {
            return (None, None);
        }
        let Some(repo) = self.repo.as_ref() else {
            return (
                None,
                Some("not a git repository, so there is no baseline".into()),
            );
        };
        let baseline = match self.store.baseline(self.project_id) {
            Ok(Some(b)) => b,
            _ => return (None, Some("no baseline scan yet".into())),
        };
        let Some(sha) = baseline.commit_sha.clone() else {
            return (None, Some("the baseline scan recorded no commit".into()));
        };
        let dir = nexus_verify::baseline_dir(&self.root, &sha);
        match repo.detached_worktree(&sha, &dir) {
            Ok(_) => {
                let checks = run_plan(&HostRunner, plan, &dir);
                (
                    Some(checks),
                    Some(format!("compared against {}", Repo::short_sha(&sha))),
                )
            }
            Err(e) => (
                None,
                // A force-push or a shallow clone. Worth saying, because it changes what the
                // verdict can mean.
                Some(format!("no baseline run: {e}")),
            ),
        }
    }
}

impl Engine {
    /// What a verdict means for the findings this project already holds (§6).
    ///
    /// Attribution is deliberately narrow. Without a reproduction test — Phase 5 — a failing
    /// suite does not say *which* finding it failed for, so a finding is only credited when
    /// the failing output actually names its anchor or its component. Everything else records
    /// an attempt and changes no status.
    ///
    /// Nothing here can set `FIXED`. That is the scan's job, on evidence of absence: a test
    /// passing is not evidence that a defect is gone, only that this run did not hit it.
    fn feed_findings(&mut self, verdict_name: &str, checks: &[nexus_verify::Check]) -> Result<()> {
        let Some(baseline) = self.store.baseline(self.project_id)? else {
            return Ok(());
        };
        let failing: String = checks
            .iter()
            .filter(|c| c.failed())
            .map(|c| c.output.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        for finding in self.store.all_findings_brief(self.project_id)? {
            let named = !failing.is_empty()
                && finding
                    .file_path
                    .as_deref()
                    .is_some_and(|f| failing.contains(f));
            let outcome = match (verdict_name, named) {
                // The failing output names this finding's file: the defect is real and this
                // run reproduced it.
                ("failed", true) => "reproduced",
                // It failed at the baseline too, so the failure predates the change. Recorded
                // as its own outcome rather than as a reproduction, because "this is broken"
                // and "this change broke it" are different claims.
                ("inconclusive", true) => "reproduced_preexisting",
                ("verified", _) => "not_reproduced",
                _ => "inconclusive",
            };

            let next = match (finding.status.as_str(), outcome) {
                ("UNVERIFIED", "reproduced") | ("SUSPECTED", "reproduced") => Some("VERIFIED"),
                // The strongest thing this ledger can say: it broke, it was fixed, and it
                // broke again — with both histories still on disk to prove it.
                ("FIXED", "reproduced") => Some("REGRESSED"),
                _ => None,
            };

            let tx = self.store.transaction()?;
            Store::insert_finding_verification(
                &tx,
                finding.id,
                baseline.scan_id,
                &format!("{} still holds", finding.uid),
                None,
                outcome,
                Some(&format!("verdict {verdict_name}")),
            )?;
            tx.commit().map_err(nexus_store::StoreError::from)?;

            if let Some(status) = next {
                self.store
                    .set_finding_status(self.project_id, &finding.uid, status)?;
            }
        }
        Ok(())
    }
}
