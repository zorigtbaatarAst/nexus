# Verification Intelligence (roadmap 4.1 – 4.9) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** "Done" gets checked. A completion claim is followed by a compile, a test run, a lint and a verdict, judged against the baseline revision so that a suite which was already red proves nothing about the change.

**Architecture:** A new crate, `nexus-verify`, owning process execution — a genuinely different risk surface from querying an index, which is why it does not go in `nexus-core`. It takes a `Plan` (commands derived from the detected profile) and returns a `Verdict`. It touches no database: `nexus-core` writes the ledger rows, which is what keeps the boundary test's rule that `nexus-verify` must not depend on `nexus-store` true rather than aspirational.

**Tech Stack:** Rust 1.82+, `std::process::Command` only. No shell, no new dependency.

**Spec:** [`08-verification.md`](../../architecture/08-verification.md) (scope, verdict, baseline matrix, persistence, feedback); [ADR-025](../../architecture/decisions/ADR-025-verification-ships-as-a-gate-before-a-reproducer.md); [`security.md`](../../security.md) §2–4 (permissions, argv templates, sandboxing); [`10-roadmap.md`](../../architecture/10-roadmap.md) Phase 4.

## Note on this plan's form

Ten tasks, condensed as in the last two plans: design decisions and an acceptance criterion per task, code only where the obvious implementation is wrong. Each ends with `make check` green and one commit naming its roadmap id.

## Global Constraints

- **Scope is 4.1 through 4.9.** Explicitly **not** built here, and named as Phase 5 by ADR-025: reproduction-test generation, the `SafeWriter` jail, the Docker sandbox. They arrive together, because the jail is the precondition for writing into a project, never a follow-up.
- **No shell. Ever.** `std::process::Command` with an explicit argv. No `sh -c`, no `bash -c`, no string interpolation into a shell. A test name containing `; rm -rf /` becomes one argument that the runner rejects. Injection is not escaped, it is made structurally impossible.
- **Nothing runs that is not on the allowlist**, and the allowlist lives in the committed `.nexus/policy.toml`. There is no "run this command" tool and there will not be one.
- **`execute = "none"` is the default and yields a result, never an execution.** Over MCP it is `permission_required`.
- **`Inconclusive` is never collapsed into `Failed`.** An already-red suite, a missing toolchain, an unreachable baseline: all inconclusive. This single rule decides whether the gate survives contact with a real project.
- **`nexus-verify` must not depend on `nexus-store`** (4.9 asserts it) and `nexus-mcp` must not depend on `nexus-verify` (the existing boundary test already asserts it).
- **Ledger tables stay append-only**: `test_runs`, `finding_verifications`.
- **Every process has a timeout**, and captured output is bounded.
- **`make check` after every task.** Baseline: **301 passing tests**.
- **`git add` names files**, and `Cargo.lock` goes in the same commit as any manifest change.
- Commit per task, naming the roadmap id, ending with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC
  ```

---

## Decisions taken before writing code

**1. `nexus-verify` knows nothing about storage, and that is the whole reason it is a crate.** It receives a plan and returns a verdict. `nexus-core` derives the plan, calls it, and writes the rows. Mixing execution into the core would put process spawning inside the crate that must stay deterministic and dependency-light — ADR-025 rejects that explicitly, and 4.9 turns the rejection into a test.

**2. A timeout is a kill and an `Inconclusive`, never a `Failed`.** A suite that ran out of time said nothing about the change. The same is true of a missing binary: `command not found` is infrastructure, and reporting it as a failing test is exactly how a gate earns its reputation for crying wolf.

**3. The baseline run uses a detached worktree and never `git stash`.** 4.3b is explicit and the reason is that `stash` mutates the developer's working tree. A verifier that can lose uncommitted work will be uninstalled the first time it does, and rightly. The worktree is created under the scratch area, keyed by sha, reused, and removed.

**4. Output is captured, bounded, and stored as counts plus a path — never inline.** `test_runs` has `log_path` for exactly this. A megabyte of Gradle output in a database column is a database nobody can query.

**5. Coverage from runner output replaces the filename match, and the change is visible.** 4.5 retires `impact::is_test` *as the coverage source*. The function stays: it is still a reasonable way to say "this file looks like a test" when no run has happened. What changes is that Review's flagship finding cites a `test_coverage` row when one exists, and says which source it used when it does not.

**6. Findings move on verification, and only in the directions §6 allows.** `Failed` on a finding's hypothesis means the defect is real: `UNVERIFIED → VERIFIED`. A previously `FIXED` finding failing again is `REGRESSED`, carrying both histories. Nothing here ever marks a finding `FIXED` — that is the scan's job, on evidence of absence.

---

## Tasks

### 4.1 — The `nexus-verify` crate

**Files:** create `crates/nexus-verify/{Cargo.toml,src/lib.rs}`; modify root `Cargo.toml`, `Cargo.lock`.

**Deliverable.** `Plan`, `Check`, `CheckKind`, `Verdict`, `Runner`. A `Command` is built from an allowlist template parsed into segments and typed holes; expansion produces argv, never a string. Every run has a timeout, bounded captured output, and a recorded exit code and duration.

**Acceptance.** A template with `{test}` expands a value containing shell metacharacters into exactly one argv element. A hole failing validation refuses to expand. A command that does not exist yields `Inconclusive`. A command exceeding its timeout is killed and yields `Inconclusive`. No `sh -c` anywhere in the crate, asserted by a test that greps its own source.

### 4.2 — Command derivation from the profile

**Files:** modify `crates/nexus-verify/src/lib.rs` (or a `plan` module), `crates/nexus-core/src/detect.rs` if a field is missing.

**Deliverable.** Build, test and lint commands derived from the detected build system: gradle, maven, npm/pnpm/yarn, cargo, pip/pytest. A build system with no known mapping yields a plan with no checks and a reason.

**Acceptance.** A Cargo project derives `cargo build`, `cargo test`, `cargo clippy`. An unknown build system yields an empty plan whose reason names the build system, and a verdict of `Inconclusive` rather than `Verified` — a gate that passes because it ran nothing is worse than no gate.

### 4.3 — The baseline run and the four-cell matrix

**Files:** modify `crates/nexus-verify/src/lib.rs`, `crates/nexus-core/src/engine/verify.rs` (new).

**Deliverable.** §3's matrix, entire: pass/pass verified, pass/fail failed, fail/fail inconclusive, fail/pass verified with a note that the change fixed a pre-existing failure. Where the baseline is unreachable or absent, the run is skipped and the verdict says so.

**Acceptance.** All four cells are asserted with a synthetic runner, so the matrix is tested without running a real build. An already-red baseline yields `Inconclusive`, never `Failed` — the assertion ADR-025 calls the one that decides whether the gate survives.

### 4.3b — Detached-worktree baseline with a per-sha cache

**Files:** modify `crates/nexus-vcs/src/lib.rs`, `crates/nexus-core/src/engine/verify.rs`.

**Deliverable.** `Repo::detached_worktree(sha, dir)` and its removal. Baselines are computed once per sha under `.nexus/cache/baseline/<sha>` and reused. `git stash` is never used, anywhere.

**Acceptance.** A dirty tree still gets a baseline verdict. The worktree is removed after use. A test greps the workspace for `stash` and finds none.

### 4.4 — Populate `test_runs` and `finding_verifications`

**Files:** create `crates/nexus-store/migrations/0008_verification.sql` if a column is missing; modify `nexus-store/src/lib.rs`, `engine/verify.rs`.

**Deliverable.** Every run appends a `test_runs` row: command, exit code, duration, counts, revision, sandbox, log path. A verification against a specific finding appends a `finding_verifications` row.

**Acceptance.** Two runs produce two rows and never an `UPDATE`. "This suite has been red for N runs" is answerable by query. The log lives on disk and the row holds its path.

### 4.5 — Real coverage, retiring the filename match

**Files:** modify `nexus-store/src/lib.rs`, `crates/nexus-core/src/engine/verify.rs`, `crates/cap-review/src/*`.

**Deliverable.** Test names and outcomes parsed from runner output where the runner emits them, written to `tests` and `test_coverage`. Review's "nothing tests this" finding cites a coverage row when one exists and names its source when it does not.

**Acceptance.** After a run, a covered symbol has a `test_coverage` row with `source = 'runtime'`. Review's finding text distinguishes evidence from a filename guess. `impact::is_test` still exists and is still used as the fallback.

### 4.6 — `nexus verify` and `nexus_verify`

**Files:** modify `crates/nexus-cli/src/main.rs`, `render.rs`, `crates/nexus-mcp/src/lib.rs`.

**Deliverable.** `nexus verify [--changed]` and the `nexus_verify` MCP tool. Exit codes: 0 verified, 3 failed, and inconclusive is its own outcome rather than an error.

**Acceptance.** `policy.execute = "none"` returns `permission_required` over MCP and never executes. The MCP handler makes one `Engine` call.

### 4.7 — The `Stop` and `PostToolUse` hooks

**Files:** modify `crates/nexus-cli/src/hooks.rs`, `tests/hooks.rs`.

**Deliverable.** `PostToolUse` on edit and write runs `nexus rescan --quiet`; `Stop` runs `nexus verify --changed`. Same fail-open string form, same installer.

**Acceptance.** Four hooks installed, idempotent, each fail-open with `nexus` off the path.

### 4.8 — Verification feeds the finding lifecycle

**Files:** modify `crates/nexus-core/src/engine/verify.rs`, `findings.rs`.

**Deliverable.** `Failed` against a finding's hypothesis moves `UNVERIFIED → VERIFIED`; a `FIXED` finding failing again becomes `REGRESSED` with both histories attached. Nothing here sets `FIXED`.

**Acceptance.** On a fixture, a finding transitions unverified → verified → regressed with the correct commits recorded, and no path in this code can reach `FIXED`.

### 4.9 — The boundary test

**Files:** modify `crates/nexus-cli/tests/boundaries.rs`.

**Deliverable.** `nexus-verify` must not depend on `nexus-store`, and it joins the list in `every_guarded_crate_is_actually_in_the_workspace` so the rule cannot go quiet if the crate is renamed.

**Acceptance.** The rule fails if the dependency is added. The crate is named as a subject, not only as a target.

---

## Self-review

**Spec coverage.** §1's seven-step gate: 4.2, 4.3, 4.6. §2's verdict with `Inconclusive` load-bearing: 4.1, 4.3. §3's matrix and the skip conditions: 4.3, 4.3b. §4's argv-only execution, allowlist, `execute = "none"`, timeouts: 4.1, 4.6. §5's three tables: 4.4, 4.5. §6's feedback edges: 4.8. ADR-025's two binding rules: `Inconclusive` first-class (4.1, 4.3) and the baseline run kept in v1 (4.3, 4.3b).

**Deliberately deferred, with the ADR's own reasoning.** Test generation, the `SafeWriter` jail and the Docker sandbox are Phase 5 and arrive together. Until then `sandbox` on a `test_runs` row is `host`, and host execution is opt-in, committed and recorded — which is what §4 already requires of it.

**Risk.** This phase executes commands from the developer's project. Every mitigation is structural rather than procedural: argv not strings, an allowlist not a filter, a default of `none` not a prompt. The single test that matters most is the one asserting an already-red suite is `Inconclusive`, because a gate that cries wolf is switched off and then verifies nothing at all.
