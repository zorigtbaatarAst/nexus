# BugHunter — Error Handling and Testing Strategy

A tool whose product is *trustworthy findings* cannot have a vague relationship with its own
failures. If BugHunter is unsure, it must say so; if it gave up, it must say that too.

---

## 1. Error handling

### 1.1 Types

`thiserror` for a typed error enum per crate; `anyhow` **only** at the two composition roots
(`bh-cli::main`, `bh-mcp::serve`). A library that returns `anyhow::Error` has told its caller
nothing, and `bh-core` is a library.

```rust
#[derive(thiserror::Error, Debug)]
pub enum ScanError {
    #[error("no baseline for project; run `bughunter scan` first")]
    NoBaseline,
    #[error("baseline commit {0} is unreachable (force-push or shallow clone?)")]
    BaselineUnreachable(String),
    #[error("database schema {found} is newer than this binary supports ({max})")]
    SchemaTooNew { found: u32, max: u32 },
    #[error("parse failed for {path}: {source}")]
    Parse { path: PathBuf, #[source] source: LangError },
}
```

Every message names the thing that failed and, where one exists, the command that fixes it.

`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` outside tests. A panic in
a scan loses the whole run; an error loses one file.

### 1.2 Partial failure is a first-class outcome

The most important error decision in the system. One unparseable file in a 42 000-file repo
has three possible handlings, and two of them are wrong:

- **Abort the scan** — one bad generated file makes the tool useless. Wrong.
- **Skip it silently** — the index now claims a file has no symbols, impact analysis quietly
  under-reports, and nobody ever finds out. Much worse.
- **Record and continue** — `files.parse_status = 'failed'`, `parse_error` stored,
  `scans.files_failed` incremented, and the run reports `2 files failed to parse
  (see --verbose)`. Correct.

```rust
pub struct ScanReport {
    pub status:   ScanStatus,     // Ok | Degraded | Failed
    pub failures: Vec<FileFailure>,
    // …
}
```

`Degraded` is a real status, surfaced in the CLI, in `--json`, and in every MCP response. The
brief's principle — errors should never pass silently — is not satisfied by logging; it is
satisfied by the result type carrying the degradation to whoever consumes it.

### 1.3 Confidence under failure

When something fails, confidence is left **unchanged**, never adjusted:

| Situation | Confidence |
|---|---|
| verification test fails to compile | unchanged — the harness failed, not the hypothesis |
| sandbox unavailable | unchanged |
| test times out | unchanged |
| test runs and does not reproduce | × 0.5 — this is evidence |
| test runs and reproduces | ≥ 0.95 — this is evidence |

Moving a number because of an infrastructure problem manufactures information out of a
broken pipe.

### 1.4 Failures that must be loud

Silent recovery is forbidden for: a schema newer than the binary (refuse to run), an
unreachable baseline commit (fall back to a full scan and *say so*), a `SafeWriter` path
escape (hard error plus audit row), a policy violation (exit 4 with the exact config line
needed), and a checksum mismatch on a cached parse (drop the cache entry and re-parse, with
a warning — a silently wrong cache is the worst outcome in the system).

---

## 2. Testing strategy — the shape

```
unit           pure functions: hashing, normalization, fingerprints, score decay
golden         real fixture repos with planted bugs, driven end to end     ← the backbone
property      invariants: incremental ≡ full, fingerprint stability
schema        migrations up and down against populated databases
conformance   MCP over stdio against recorded JSON-RPC sessions
performance   generated repos with wall-clock assertions
```

---

## 3. Golden fixture repositories

The backbone. Four small but *real* projects under `tests/fixtures/`, each a genuine git
repository with a scripted history that plants a specific bug at a known commit.

```
tests/fixtures/spring-payments/     Java 21 · Spring Boot 3.5 · JPA
  commit 1  baseline: PaymentService with an idempotency check
  commit 2  refactor: extract PaymentValidator            (no bug — must find nothing)
  commit 3  BUG: exists() check moved outside @Transactional → duplicate under concurrency
  commit 4  reformat everything with spotless             (must find nothing, no churn)
  commit 5  rename mn.pay → mn.payments                   (must not duplicate BUG from c3)
  commit 6  fix: unique index + optimistic locking        (bug → FIXED)
  commit 7  BUG returns: index dropped in a migration     (bug → REGRESSED)

tests/fixtures/next-storefront/     TypeScript · Next.js · Prisma
tests/fixtures/fastapi-orders/      Python · FastAPI · SQLAlchemy
tests/fixtures/cargo-ledger/        Rust · axum · sqlx
```

Each fixture asserts the full chain, commit by commit:

1. `scan` at commit 1 produces the expected symbol count and the expected edges.
2. `rescan` at each subsequent commit reports **exactly** the expected changed symbol set —
   no more (false churn) and no less (missed change).
3. `impact` returns the expected affected set, and the expected *paths*.
4. The planted bug is found at commit 3 and **not before**.
5. Commit 4 (reformat) produces **zero** symbol changes and zero new bugs. This single
   assertion protects the `body_hash` normalization, which is the thing most likely to
   regress silently.
6. Commit 5 (rename) does not create a duplicate bug — fingerprint aliasing works.
7. Verification at commit 3 reproduces; at commit 6 it does not, and the bug becomes `FIXED`.
8. Commit 7 flips it to `REGRESSED` with both commits recorded.

This is expensive to build and it is the only way to know the product works. Steps 5 and 6
in particular cannot be tested any other way — they are about behaviour *across time*, which
is precisely what BugHunter claims to provide.

---

## 4. Property tests

**The central invariant:**

```
for any fixture repo, at any commit:
    full_scan(repo)  ≡  scan(commit_1) then rescan through to commit_N
```

Compared on the normalized index: symbols, edges, hashes, coverage. Any divergence is a bug
in the incremental path, and the incremental path is the entire performance story. This one
test is worth more than the rest of the property suite combined.

Others, with `proptest`:

- **Fingerprint stability** — reformat, rename a parameter, move the file, add an import
  above: the fingerprint must not change. Move the method to a different class, or change
  the shared state involved: it must change.
- **Impact monotonicity** — raising `--depth` or lowering `--min-score` never removes a
  symbol from the result.
- **Traversal termination** — on randomly generated cyclic graphs, BFS terminates within the
  depth cap and reports the highest-scoring path.
- **Hash normalization** — comment and whitespace edits never alter `body_hash`; a literal
  change always does.
- **Command templates** — for arbitrary hole values, including shell metacharacters, the
  expanded argv has exactly the expected element count and no element is ever split.

---

## 5. Schema and store tests

Every migration applied forward against a database populated by the fixtures, then a
smoke-read of every table. Foreign-key violations are checked with `PRAGMA foreign_key_check`
after each migration. A migration that drops or rewrites data in an immutable ledger table
fails a dedicated test — that is the guardrail on the doctrine in
[data-model.md](data-model.md) §2, which is otherwise only a convention.

---

## 6. Boundary tests

The architecture's module rules are tests, not documentation:

```rust
#[test]
fn boundaries_hold() {
    let g = cargo_metadata_graph();
    assert!(!g.depends("bh-core", "bh-mcp"));
    assert!(!g.depends("bh-core", "bh-cli"));
    assert!(!g.depends("bh-mcp",  "bh-store"));
    assert!(!g.depends("bh-mcp",  "bh-verify"));
    assert!(!g.depends("bh-lang-java", "bh-store"));
    // AI is optional: the deterministic build has no HTTP client
    assert!(!g.depends_with_default_features("bh-core", "reqwest"));
}
```

This is how constraints 1, 2, 3 and 12 stay true after six months of feature work by people
who never read this document.

---

## 7. MCP conformance and performance

**Conformance.** Recorded JSON-RPC sessions in `tests/conformance/` replayed against
`bughunter mcp` over stdio: tool discovery, every tool's schema, pagination and cursors,
`permission_required` on an `execute` tool under `policy.execute = "none"`, and structured
errors for every `kind`. Response size is asserted under the token budget — a tool that
returns 2 MB fails the build.

**Performance.** A generated repository (configurable, default 1 MLOC across 12 000 files)
with wall-clock assertions matching the budgets in [performance.md](performance.md) §1.
Run nightly rather than per-commit, with generous margins so it catches regressions of kind
rather than of a few percent. The no-op `rescan` budget is asserted per-commit, because it is
cheap to measure and it is the number that decides whether the tool is pleasant to use.

---

## 8. What is deliberately not tested

- **LLM output quality.** Non-deterministic and provider-specific. What *is* tested is the
  boundary: a `BugCandidate` with no evidence is rejected; evidence pointing at a nonexistent
  file is rejected; model confidence is clamped at 0.75. The model's judgement is not
  BugHunter's to guarantee — its handling of that judgement is.
- **Third-party build tools.** Fixtures assert that BugHunter forms the right command, not
  that Gradle works.
- **Docker itself.** Sandbox tests assert the container arguments and the fallback
  behaviour; container tests run only where a daemon is available and are skipped with a
  message elsewhere, never silently passed.
