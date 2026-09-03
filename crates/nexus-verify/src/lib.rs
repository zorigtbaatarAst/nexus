//! Verification: running a project's own commands and judging the result.
//!
//! "Done" is a claim about the world. This crate is what makes it a fact, or refuses to.
//!
//! It knows nothing about storage, and that is why it is a crate rather than a module.
//! Executing processes is a genuinely different risk surface from querying an index, and
//! mixing it into `nexus-core` would put process spawning inside the crate that must stay
//! deterministic and dependency-light. `tests/boundaries.rs` asserts the separation.
//!
//! # The two rules that eliminate command injection
//!
//! **No shell. Ever.** Commands are an explicit argv handed to [`std::process::Command`].
//! There is no `sh -c`, no `bash -c`, and no interpolation into a shell string anywhere in
//! this crate — a test greps the source to keep it that way. A test name of `foo; rm -rf /`
//! becomes one argument, which the runner rejects as an unknown test. Injection is not
//! escaped here; it is structurally impossible.
//!
//! **Templates with typed holes.** An allowlist entry is parsed once into segments and holes,
//! and a hole fills exactly one argv element after validation. `security.md` §3.
//!
//! # Why `Inconclusive` carries the design
//!
//! A missing toolchain, a killed timeout, an unreachable baseline, a suite that was already
//! red: none of these says anything about the change. Reporting them as `Failed` is precisely
//! how a gate earns a reputation for crying wolf, and a gate that cries wolf is switched off,
//! after which it verifies nothing at all. ADR-025 names this the decision that determines
//! whether the gate survives contact with a real project.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Captured output is truncated to this. A megabyte of Gradle output helps nobody and a
/// database column full of it helps less.
pub const MAX_CAPTURED_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("{0}")]
    Template(String),
    #[error("io error running {command}: {source}")]
    Io {
        command: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, VerifyError>;

/// What a check is checking. Ordered as the gate runs them: nothing is worth testing if it
/// does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Build,
    Test,
    Lint,
}

impl CheckKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckKind::Build => "build",
            CheckKind::Test => "test",
            CheckKind::Lint => "lint",
        }
    }
}

/// Why a check did not produce a usable answer. Separate from a failing check on purpose:
/// these are facts about the machine, not about the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum Blocked {
    /// The command is not on this machine.
    NotFound(String),
    /// It ran too long and was killed.
    TimedOut(u64),
    /// It could not be started for another reason.
    Failed(String),
}

/// One executed command and what came of it.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub kind: CheckKind,
    /// The argv actually executed, as executed. Reproducible by hand, which is the point.
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    /// `None` when the check ran; `Some` when the machine got in the way.
    pub blocked: Option<Blocked>,
    /// Tail of combined output, bounded by [`MAX_CAPTURED_BYTES`].
    pub output: String,
}

impl Check {
    /// Ran and exited zero.
    pub fn passed(&self) -> bool {
        self.blocked.is_none() && self.exit_code == Some(0)
    }
    /// Ran and exited non-zero. A blocked check is neither passed nor failed — that
    /// distinction is the whole point of the type.
    pub fn failed(&self) -> bool {
        self.blocked.is_none() && self.exit_code.is_some_and(|c| c != 0)
    }
}

/// The judgement.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Verified {
        checks: Vec<Check>,
        /// Set when the change fixed something that was already broken at the baseline.
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Failed {
        check: CheckKind,
        detail: String,
        checks: Vec<Check>,
        /// Set when the baseline half did not run. A failure that implies a comparison it
        /// never made is a lie by omission, and this is the case where a gate most easily
        /// blames a change for a suite that was already broken.
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Nothing could be concluded. Never a synonym for failure.
    Inconclusive { why: String, checks_run: Vec<Check> },
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Verified { .. } => "verified",
            Verdict::Failed { .. } => "failed",
            Verdict::Inconclusive { .. } => "inconclusive",
        }
    }
    pub fn checks(&self) -> &[Check] {
        match self {
            Verdict::Verified { checks, .. } => checks,
            Verdict::Failed { checks, .. } => checks,
            Verdict::Inconclusive { checks_run, .. } => checks_run,
        }
    }
}

/// A command to run, already reduced to argv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub kind: CheckKind,
    pub argv: Vec<String>,
}

/// What to run, and for how long.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub timeout_seconds: u64,
    /// Present when the plan is empty and says why. An empty plan with no reason would let a
    /// gate report success for having run nothing, which is worse than having no gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Plan {
    pub fn empty(reason: impl Into<String>) -> Self {
        Plan {
            steps: Vec::new(),
            timeout_seconds: 600,
            reason: Some(reason.into()),
        }
    }
}

// ─────────────────────────── templates ───────────────────────────

/// A hole in an allowlist template, with the type that validates its filling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hole {
    /// A test selector: `[A-Za-z0-9_.#*$-]`, at most 512 characters.
    Test,
    /// A repository-relative path, which must stay inside the project root.
    File,
    /// A module identifier.
    Module,
}

impl Hole {
    fn parse(name: &str) -> Option<Hole> {
        match name {
            "test" => Some(Hole::Test),
            "file" => Some(Hole::File),
            "module" => Some(Hole::Module),
            _ => None,
        }
    }

    /// Validate a filling. Rejection is the point: an unvalidated hole is an argv element the
    /// caller chose, and the allowlist exists so that nobody chooses those.
    pub fn accepts(self, value: &str) -> bool {
        if value.is_empty() || value.len() > 512 {
            return false;
        }
        match self {
            Hole::Test => value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.#*$-".contains(c)),
            Hole::File | Hole::Module => !value.contains('\0') && !value.starts_with('-'),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Hole(Hole),
}

/// One allowlist entry, parsed once.
///
/// Parsed rather than formatted: a template is a list of argv elements, and a hole *is* one
/// element. There is no point at which a filling could become two arguments or a redirection,
/// because there is no point at which the command is a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    segments: Vec<Segment>,
}

impl Template {
    pub fn parse(entry: &str) -> Result<Template> {
        let mut segments = Vec::new();
        for word in entry.split_whitespace() {
            if let Some(name) = word.strip_prefix('{').and_then(|w| w.strip_suffix('}')) {
                let hole = Hole::parse(name)
                    .ok_or_else(|| VerifyError::Template(format!("unknown hole {{{name}}}")))?;
                segments.push(Segment::Hole(hole));
            } else if word.contains('{') || word.contains('}') {
                // A hole must be a whole word. `--tests={test}` would put a filling inside a
                // larger argument, which is where an escaping bug would live if there were
                // any escaping.
                return Err(VerifyError::Template(format!(
                    "'{word}': a hole must be its own argument, not part of one"
                )));
            } else {
                segments.push(Segment::Literal(word.to_string()));
            }
        }
        if segments.is_empty() {
            return Err(VerifyError::Template("empty template".into()));
        }
        Ok(Template { segments })
    }

    pub fn holes(&self) -> Vec<Hole> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                Segment::Hole(h) => Some(*h),
                Segment::Literal(_) => None,
            })
            .collect()
    }

    /// Expand to argv. Each hole becomes exactly one element, after validation.
    pub fn expand(&self, fillings: &[String]) -> Result<Vec<String>> {
        let mut next = fillings.iter();
        let mut argv = Vec::new();
        for segment in &self.segments {
            match segment {
                Segment::Literal(l) => argv.push(l.clone()),
                Segment::Hole(h) => {
                    let value = next
                        .next()
                        .ok_or_else(|| VerifyError::Template("too few values".into()))?;
                    if !h.accepts(value) {
                        return Err(VerifyError::Template(format!(
                            "{value:?} is not a valid {h:?} value"
                        )));
                    }
                    argv.push(value.clone());
                }
            }
        }
        Ok(argv)
    }
}

// ─────────────────────────── execution ───────────────────────────

/// Runs steps. A trait so the judgement can be tested without a toolchain: the four-cell
/// matrix is logic, and logic that can only be tested by running Gradle is logic nobody tests.
pub trait Runner {
    fn run(&self, step: &Step, cwd: &Path, timeout: Duration) -> Check;
}

/// The real one.
pub struct HostRunner;

fn truncate(mut bytes: Vec<u8>) -> String {
    if bytes.len() > MAX_CAPTURED_BYTES {
        // Keep the tail: a failure explains itself at the end of the output, not the start.
        bytes = bytes.split_off(bytes.len() - MAX_CAPTURED_BYTES);
    }
    String::from_utf8_lossy(&bytes).to_string()
}

impl Runner for HostRunner {
    fn run(&self, step: &Step, cwd: &Path, timeout: Duration) -> Check {
        let started = Instant::now();
        let blocked_check = |blocked: Blocked, started: Instant| Check {
            kind: step.kind,
            argv: step.argv.clone(),
            exit_code: None,
            duration_ms: started.elapsed().as_millis(),
            blocked: Some(blocked),
            output: String::new(),
        };
        let Some((program, args)) = step.argv.split_first() else {
            return blocked_check(Blocked::Failed("empty argv".into()), started);
        };

        // No shell: the program and every argument are separate values that never pass
        // through a parser that could reinterpret them.
        let mut child = match Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Infrastructure, not code. This is the commonest reason a gate would
                // otherwise report a failure it has no evidence for.
                return blocked_check(Blocked::NotFound(program.clone()), started);
            }
            Err(e) => return blocked_check(Blocked::Failed(e.to_string()), started),
        };

        // Poll rather than block, so a hung command is killed rather than hanging the caller.
        // A verifier that can wedge a developer's session is uninstalled once and never
        // reinstalled.
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let out = child.wait_with_output().ok();
                    let mut bytes = Vec::new();
                    if let Some(o) = out {
                        bytes.extend_from_slice(&o.stdout);
                        bytes.extend_from_slice(&o.stderr);
                    }
                    return Check {
                        kind: step.kind,
                        argv: step.argv.clone(),
                        exit_code: status.code(),
                        duration_ms: started.elapsed().as_millis(),
                        blocked: None,
                        output: truncate(bytes),
                    };
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return blocked_check(Blocked::TimedOut(timeout.as_secs()), started);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(e) => return blocked_check(Blocked::Failed(e.to_string()), started),
            }
        }
    }
}

/// Run a plan at one revision. Stops at the first failing check: nothing after a failed build
/// tells you anything you did not already know.
pub fn run_plan(runner: &dyn Runner, plan: &Plan, cwd: &Path) -> Vec<Check> {
    let timeout = Duration::from_secs(plan.timeout_seconds);
    let mut checks = Vec::new();
    for step in &plan.steps {
        let check = runner.run(step, cwd, timeout);
        let stop = check.failed() || check.blocked.is_some();
        checks.push(check);
        if stop {
            break;
        }
    }
    checks
}

/// Where a run happened. `docker` arrives with the sandbox in Phase 5; until then every run is
/// on the host, which `security.md` §4 permits as an opt-in and requires to be recorded.
pub fn sandbox_name() -> &'static str {
    "host"
}

/// A convenience for a caller that only has a directory: turn checks into a verdict with no
/// baseline to compare against.
pub fn judge_single(checks: Vec<Check>, plan: &Plan) -> Verdict {
    if let Some(reason) = &plan.reason {
        return Verdict::Inconclusive {
            why: reason.clone(),
            checks_run: checks,
        };
    }
    if checks.is_empty() {
        return Verdict::Inconclusive {
            why: "no checks to run".into(),
            checks_run: checks,
        };
    }
    if let Some(blocked) = checks.iter().find(|c| c.blocked.is_some()) {
        let why = match &blocked.blocked {
            Some(Blocked::NotFound(p)) => format!("{p} is not on PATH"),
            Some(Blocked::TimedOut(s)) => format!("{} timed out after {s}s", blocked.kind.as_str()),
            Some(Blocked::Failed(e)) => format!("{} could not run: {e}", blocked.kind.as_str()),
            None => "blocked".into(),
        };
        return Verdict::Inconclusive {
            why,
            checks_run: checks,
        };
    }
    match checks.iter().find(|c| c.failed()) {
        Some(f) => Verdict::Failed {
            check: f.kind,
            detail: format!("{} exited {}", f.kind.as_str(), f.exit_code.unwrap_or(-1)),
            checks,
            note: None,
        },
        None => Verdict::Verified { checks, note: None },
    }
}

/// The scratch root a baseline worktree lives under, given a project root.
pub fn baseline_dir(project_root: &Path, sha: &str) -> PathBuf {
    project_root
        .join(".nexus")
        .join("cache")
        .join("baseline")
        .join(&sha[..12.min(sha.len())])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(argv: &[&str]) -> Step {
        Step {
            kind: CheckKind::Test,
            argv: argv.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_hole_fills_exactly_one_argument() {
        // security.md §3: `foo; rm -rf /` becomes one argument the runner rejects as an
        // unknown test. Injection is not escaped, it is impossible.
        let t = Template::parse("./gradlew test --tests {test}").expect("parse");
        let argv = t.expand(&["foo.Bar#baz".to_string()]).expect("expand");
        assert_eq!(argv, ["./gradlew", "test", "--tests", "foo.Bar#baz"]);
    }

    #[test]
    fn a_shell_metacharacter_is_refused_by_the_hole_type() {
        let t = Template::parse("./gradlew test --tests {test}").expect("parse");
        for bad in ["foo; rm -rf /", "$(whoami)", "a`b`", "x|y", "a b"] {
            assert!(
                t.expand(&[bad.to_string()]).is_err(),
                "{bad:?} was accepted as a test selector"
            );
        }
    }

    #[test]
    fn a_hole_must_be_its_own_argument() {
        // `--tests={test}` would put a filling inside a larger argument, which is exactly
        // where an escaping bug would live if there were any escaping.
        assert!(Template::parse("./gradlew --tests={test}").is_err());
        assert!(Template::parse("{").is_err());
    }

    #[test]
    fn an_unknown_hole_is_refused_at_parse_time() {
        // Not at expansion time: an allowlist entry nobody can fill is a configuration error,
        // and finding it when someone runs a verification is finding it too late.
        assert!(Template::parse("run {command}").is_err());
    }

    #[test]
    fn a_missing_binary_is_inconclusive_and_never_a_failure() {
        let checks = run_plan(
            &HostRunner,
            &Plan {
                steps: vec![step(&["definitely-not-a-real-binary-xyz"])],
                timeout_seconds: 5,
                reason: None,
            },
            Path::new("."),
        );
        assert!(matches!(checks[0].blocked, Some(Blocked::NotFound(_))));
        assert!(
            !checks[0].failed(),
            "a missing toolchain is not a failed test"
        );
        let v = judge_single(
            checks,
            &Plan {
                steps: Vec::new(),
                timeout_seconds: 5,
                reason: None,
            },
        );
        assert_eq!(v.as_str(), "inconclusive", "{v:?}");
    }

    #[test]
    fn a_command_that_runs_too_long_is_killed_and_inconclusive() {
        let checks = run_plan(
            &HostRunner,
            &Plan {
                steps: vec![step(&["sleep", "30"])],
                timeout_seconds: 1,
                reason: None,
            },
            Path::new("."),
        );
        assert!(
            matches!(checks[0].blocked, Some(Blocked::TimedOut(_))),
            "{:?}",
            checks[0]
        );
        assert!(checks[0].duration_ms < 10_000, "it was actually killed");
    }

    #[test]
    fn exit_status_is_reported_as_pass_or_fail() {
        let ok = run_plan(
            &HostRunner,
            &Plan {
                steps: vec![step(&["true"])],
                timeout_seconds: 5,
                reason: None,
            },
            Path::new("."),
        );
        assert!(ok[0].passed(), "{:?}", ok[0]);
        let bad = run_plan(
            &HostRunner,
            &Plan {
                steps: vec![step(&["false"])],
                timeout_seconds: 5,
                reason: None,
            },
            Path::new("."),
        );
        assert!(bad[0].failed(), "{:?}", bad[0]);
    }

    #[test]
    fn a_plan_stops_at_the_first_failure() {
        // Nothing after a failed build tells you anything you did not already know.
        let checks = run_plan(
            &HostRunner,
            &Plan {
                steps: vec![step(&["false"]), step(&["true"])],
                timeout_seconds: 5,
                reason: None,
            },
            Path::new("."),
        );
        assert_eq!(checks.len(), 1);
    }

    #[test]
    fn an_empty_plan_is_inconclusive_not_verified() {
        // A gate that reports success for having run nothing is worse than no gate.
        let plan = Plan::empty("no build system detected");
        let v = judge_single(Vec::new(), &plan);
        assert_eq!(v.as_str(), "inconclusive", "{v:?}");
    }

    #[test]
    fn output_is_bounded() {
        let big = truncate(vec![b'x'; MAX_CAPTURED_BYTES * 3]);
        assert_eq!(big.len(), MAX_CAPTURED_BYTES);
    }

    #[test]
    fn this_crate_never_invokes_a_shell() {
        // The rule is structural, so the check is too. If this fails, someone has reopened a
        // whole vulnerability class that argv-only execution had closed.
        //
        // Only the implementation is scanned. The prose above names the shells in order to
        // forbid them, and this test names them in order to look for them; a substring count
        // over the whole file counts those and is a test of its own comments.
        let src = include_str!("lib.rs");
        let implementation = src.split_once("#[cfg(test)]").map_or(src, |(code, _)| code);
        let code_only: String = implementation
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in [
            "sh\", \"-c",
            "Command::new(\"sh",
            "Command::new(\"bash",
            ".arg(\"-c",
        ] {
            assert!(
                !code_only.contains(banned),
                "{banned:?} appears in the implementation — argv-only execution is what makes \
                 a shell metacharacter in a test name just a character"
            );
        }
    }
}

// ─────────────────────────── planning ───────────────────────────

/// Build, test and lint commands for a detected build system.
///
/// Derived from the profile rather than configured, because a project that already tells you
/// how it builds should not have to tell you twice. A build system with no mapping yields an
/// empty plan **with a reason**, which becomes `Inconclusive` — a gate that reports success
/// for having run nothing is worse than having no gate at all.
///
/// These are the ordinary invocations, not the allowlist: the allowlist governs commands a
/// caller *chooses*, and nobody chooses these. They are what the project's own toolchain does.
pub fn plan_for(build_system: Option<&str>, timeout_seconds: u64) -> Plan {
    let steps: Vec<(CheckKind, &[&str])> = match build_system {
        Some("cargo") => vec![
            (CheckKind::Build, &["cargo", "build", "--workspace"]),
            (CheckKind::Test, &["cargo", "test", "--workspace"]),
            (
                CheckKind::Lint,
                &["cargo", "clippy", "--workspace", "--all-targets"],
            ),
        ],
        Some("gradle") => vec![
            (CheckKind::Build, &["./gradlew", "assemble"]),
            (CheckKind::Test, &["./gradlew", "test"]),
            (CheckKind::Lint, &["./gradlew", "check", "-x", "test"]),
        ],
        Some("maven") => vec![
            (CheckKind::Build, &["mvn", "-q", "-B", "compile"]),
            (CheckKind::Test, &["mvn", "-q", "-B", "test"]),
        ],
        Some("npm") => vec![
            (CheckKind::Build, &["npm", "run", "build"]),
            (CheckKind::Test, &["npm", "test"]),
            (CheckKind::Lint, &["npm", "run", "lint"]),
        ],
        Some("pnpm") => vec![
            (CheckKind::Build, &["pnpm", "build"]),
            (CheckKind::Test, &["pnpm", "test"]),
            (CheckKind::Lint, &["pnpm", "lint"]),
        ],
        Some("yarn") => vec![
            (CheckKind::Build, &["yarn", "build"]),
            (CheckKind::Test, &["yarn", "test"]),
            (CheckKind::Lint, &["yarn", "lint"]),
        ],
        Some("pip") | Some("poetry") | Some("uv") => {
            vec![(CheckKind::Test, &["pytest", "-q"])]
        }
        Some(other) => {
            return Plan::empty(format!(
                "no build, test or lint commands are known for '{other}' — verification cannot \
                 conclude anything without running something"
            ))
        }
        None => {
            return Plan::empty(
                "no build system was detected, so there is nothing to build, test or lint",
            )
        }
    };
    Plan {
        steps: steps
            .into_iter()
            .map(|(kind, argv)| Step {
                kind,
                argv: argv.iter().map(|s| s.to_string()).collect(),
            })
            .collect(),
        timeout_seconds,
        reason: None,
    }
}

// ─────────────────────────── judgement ───────────────────────────

/// What one revision's run said, reduced to the only thing the matrix needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// Could not be established. A blocked check, a missing baseline, an unreachable commit.
    Unknown,
}

/// Reduce a set of checks to one outcome.
pub fn outcome_of(checks: &[Check]) -> Outcome {
    if checks.is_empty() || checks.iter().any(|c| c.blocked.is_some()) {
        return Outcome::Unknown;
    }
    if checks.iter().any(|c| c.failed()) {
        Outcome::Failed
    } else {
        Outcome::Passed
    }
}

/// §3's four-cell matrix, entire.
///
/// | baseline | head | verdict |
/// |---|---|---|
/// | pass | pass | verified |
/// | pass | fail | failed — the change did it |
/// | fail | fail | inconclusive — already broken |
/// | fail | pass | verified, and it fixed something |
///
/// Halving this to save time removes the ability to tell "this change introduced a bug" from
/// "this suite was already red", which is the entire question being asked.
pub fn judge(head: Vec<Check>, baseline: Option<Vec<Check>>, plan: &Plan) -> Verdict {
    if let Some(reason) = &plan.reason {
        return Verdict::Inconclusive {
            why: reason.clone(),
            checks_run: head,
        };
    }
    let head_outcome = outcome_of(&head);

    // With no comparable baseline the honest answer is the single-revision one, and the
    // caller is told the comparison did not happen rather than left to assume it did.
    let Some(baseline) = baseline else {
        const UNCOMPARED: &str = "no baseline run, so a pre-existing failure could not be ruled \
                                  out — this verdict is about the current revision alone";
        return match judge_single(head, plan) {
            Verdict::Verified { checks, .. } => Verdict::Verified {
                checks,
                note: Some(UNCOMPARED.into()),
            },
            Verdict::Failed {
                check,
                detail,
                checks,
                ..
            } => Verdict::Failed {
                check,
                detail,
                checks,
                note: Some(UNCOMPARED.into()),
            },
            other => other,
        };
    };

    match (outcome_of(&baseline), head_outcome) {
        (Outcome::Passed, Outcome::Passed) => Verdict::Verified {
            checks: head,
            note: None,
        },
        (Outcome::Passed, Outcome::Failed) => {
            let detail = head
                .iter()
                .find(|c| c.failed())
                .map(|c| format!("{} passed at the baseline and fails here", c.kind.as_str()))
                .unwrap_or_else(|| "a check that passed at the baseline fails here".into());
            let kind = head
                .iter()
                .find(|c| c.failed())
                .map_or(CheckKind::Test, |c| c.kind);
            Verdict::Failed {
                check: kind,
                detail,
                checks: head,
                note: None,
            }
        }
        (Outcome::Failed, Outcome::Failed) => Verdict::Inconclusive {
            why: "this was already failing at the baseline, so the change is not what broke it"
                .into(),
            checks_run: head,
        },
        (Outcome::Failed, Outcome::Passed) => Verdict::Verified {
            checks: head,
            note: Some("this change fixed a failure that already existed at the baseline".into()),
        },
        // Either side unknown: a blocked check on one revision says nothing about the other.
        (_, _) => Verdict::Inconclusive {
            why: "a check could not run at one of the two revisions, so the pair cannot be \
                  compared"
                .into(),
            checks_run: head,
        },
    }
}

#[cfg(test)]
mod judgement_tests {
    use super::*;

    fn check(kind: CheckKind, exit: Option<i32>, blocked: Option<Blocked>) -> Check {
        Check {
            kind,
            argv: vec!["x".into()],
            exit_code: exit,
            duration_ms: 1,
            blocked,
            output: String::new(),
        }
    }
    fn pass() -> Vec<Check> {
        vec![check(CheckKind::Test, Some(0), None)]
    }
    fn fail() -> Vec<Check> {
        vec![check(CheckKind::Test, Some(1), None)]
    }
    fn plan() -> Plan {
        Plan {
            steps: vec![Step {
                kind: CheckKind::Test,
                argv: vec!["x".into()],
            }],
            timeout_seconds: 5,
            reason: None,
        }
    }

    #[test]
    fn the_four_cells_are_exactly_what_the_design_specifies() {
        assert_eq!(judge(pass(), Some(pass()), &plan()).as_str(), "verified");
        assert_eq!(judge(fail(), Some(pass()), &plan()).as_str(), "failed");
        assert_eq!(
            judge(fail(), Some(fail()), &plan()).as_str(),
            "inconclusive",
            "an already-red suite proves nothing about the change"
        );
        assert_eq!(judge(pass(), Some(fail()), &plan()).as_str(), "verified");
    }

    #[test]
    fn an_already_red_baseline_is_never_reported_as_a_failure() {
        // ADR-025 calls this the single assertion that decides whether the gate survives
        // contact with a real project. A gate that blames the change for a suite that was
        // already broken gets switched off, and then it verifies nothing at all.
        match judge(fail(), Some(fail()), &plan()) {
            Verdict::Inconclusive { why, .. } => {
                assert!(why.contains("already failing"), "{why}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fixing_a_pre_existing_failure_is_verified_and_says_so() {
        match judge(pass(), Some(fail()), &plan()) {
            Verdict::Verified { note, .. } => {
                assert!(
                    note.is_some_and(|n| n.contains("fixed")),
                    "the note is the point"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn without_a_baseline_the_verdict_says_the_comparison_did_not_happen() {
        match judge(pass(), None, &plan()) {
            Verdict::Verified { note, .. } => assert!(
                note.is_some_and(|n| n.contains("could not be ruled out")),
                "a verdict that implies a comparison it did not make is a lie by omission"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_blocked_check_on_either_side_is_inconclusive() {
        let blocked = vec![check(
            CheckKind::Test,
            None,
            Some(Blocked::NotFound("gradle".into())),
        )];
        assert_eq!(
            judge(blocked.clone(), Some(pass()), &plan()).as_str(),
            "inconclusive"
        );
        assert_eq!(
            judge(pass(), Some(blocked), &plan()).as_str(),
            "inconclusive"
        );
    }

    #[test]
    fn a_plan_with_nothing_to_run_is_inconclusive_whatever_happened() {
        let p = Plan::empty("no build system detected");
        assert_eq!(judge(pass(), Some(pass()), &p).as_str(), "inconclusive");
    }

    #[test]
    fn a_cargo_project_gets_build_test_and_lint() {
        let p = plan_for(Some("cargo"), 600);
        let kinds: Vec<CheckKind> = p.steps.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [CheckKind::Build, CheckKind::Test, CheckKind::Lint]);
        assert_eq!(p.steps[0].argv[0], "cargo");
        assert!(p.reason.is_none());
    }

    #[test]
    fn an_unknown_build_system_yields_an_empty_plan_that_says_why() {
        for bs in [Some("bazel"), None] {
            let p = plan_for(bs, 600);
            assert!(p.steps.is_empty(), "{bs:?}");
            assert!(
                p.reason.is_some(),
                "an empty plan must explain itself: {bs:?}"
            );
        }
    }
}
