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
    pub fn verify(&self) -> Result<VerifyReport> {
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

        // The baseline half. Skipped, with the reason said out loud, when there is nothing
        // comparable to run against.
        let (baseline_checks, baseline_note) = self.baseline_run(&plan);

        let verdict = judge(head, baseline_checks, &plan);
        let (name, why, note) = match &verdict {
            Verdict::Verified { note, .. } => ("verified", None, note.clone()),
            Verdict::Failed { detail, note, .. } => ("failed", Some(detail.clone()), note.clone()),
            Verdict::Inconclusive { why, .. } => ("inconclusive", Some(why.clone()), None),
        };
        Ok(VerifyReport {
            verdict: name.into(),
            why,
            checks: verdict.checks().to_vec(),
            baseline: baseline_note,
            note,
            duration_ms: started.elapsed().as_millis(),
        })
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
