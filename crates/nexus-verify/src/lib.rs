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
