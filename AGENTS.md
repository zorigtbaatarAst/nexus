# BugHunter — briefing for an AI agent working in this repo

Read this before touching anything. These are the facts that are expensive to rediscover and
the constraints that will bite you if you do not know them.

## What this is

A change-aware software intelligence system: index a codebase once, keep a baseline, detect
what changed on rescan, compute the blast radius, find bugs in the affected region, and prove
them by generating and running a reproduction test.

**Status: architecture only.** `docs/` is complete; no code exists. Do not start implementing
outside the MVP scope in [`docs/roadmap.md`](docs/roadmap.md) — the design deliberately
defers things that look easy and are not.

Planned as a Rust workspace producing one binary, `bughunter`, which is both the CLI and the
MCP server.

## The one idea the whole design rests on

**BugHunter owns evidence, history and verification. The AI agent owns reasoning.**

If you find yourself adding reasoning to BugHunter, or evidence-gathering to the agent layer,
you are working against the grain of the design. Check the layer you are in.

## The hard constraints

These are not style preferences. Each is pinned by a test, and violating one produces a bug
that is hard to attribute.

1. **`bh-core` must not depend on `bh-mcp`, `bh-cli`, or any concrete AI provider.** It
   depends on `bh-ai` with `default-features = false`, which means the deterministic build
   has no HTTP client in its dependency tree at all. A `cargo metadata` test asserts this.

2. **`bh-mcp` must not depend on `bh-store`, `bh-lang*` or `bh-verify`.** A handler reaches
   them only through `bh-core`, so it physically cannot grow logic the CLI lacks. Every
   handler is: deserialize → one `Engine` call → serialize. If a handler needs two `Engine`
   calls, the missing method belongs in `bh-core`.

3. **Only `bh-store` contains SQL.** No exceptions, including "just this one query".

4. **`bh-lang-*` must not depend on `bh-store` or `bh-core`.** An analyzer takes source text
   and returns a `ParsedFile`. It never learns about scans or baselines. This is also why
   parsing parallelizes cleanly.

5. **`bh-verify` writes only through `SafeWriter`**, rooted at `.bughunter/generated-tests/`,
   canonicalizing the parent path *before* the prefix check. A jail that compares unresolved
   paths is not a jail.

6. **Ledger tables are append-only.** `scans`, `changes`, `commits`, `bug_occurrences`,
   `bug_verifications`, `test_runs`, `audit_events` are never `UPDATE`d. See
   [`docs/data-model.md`](docs/data-model.md) §2. An `UPDATE` on one of these destroys
   regression detection, which is the strongest thing the product does.

## Things that look wrong and are deliberate

- **`changes.path` and `changes.fqn` duplicate data reachable through `entity_id`.**
  Intentional. The evidence must stay readable after the symbol is deleted; a historical
  record that resolves to `NULL` two refactors later is not a record.

- **Two hashes per symbol (`sig_hash`, `body_hash`).** This is not redundancy. A `sig_hash`
  change is an API break that ripples to every caller; a `body_hash`-only change ripples
  only through data and effect edges. Collapse them and impact analysis becomes noise.
  [ADR-010](docs/architecture-decisions.md#adr-010-two-hashes-per-symbol).

- **Verification runs the same test twice** — once on HEAD, once on the baseline revision in
  a detached worktree. Halving this to save time also destroys the ability to tell "this
  change introduced a bug" from "this suite was already red".

- **An infrastructure failure leaves confidence unchanged**, never lowered. A test that would
  not compile says nothing about the hypothesis.

- **`FIXED` requires the stored reproduction test to pass.** Absence from an incremental scan
  means the region was not examined. Treating absence as a fix silently closes real bugs.

- **Confidence from a model is clamped at 0.75.** Only the verification engine can go higher,
  and only by reproducing the bug.

- **A `BugCandidate` with empty `evidence` is rejected, not down-ranked.** Rejections are
  counted and reported, because a silently discarded finding is indistinguishable from a
  model that found nothing.

## Traps

- **Cache invalidation must include tool versions.** `scans.tool_versions_json` holds grammar
  and analyzer versions. Upgrade `tree-sitter-java` without bumping it and the content hashes
  still match, nothing re-parses, and the index keeps the old wrong symbols forever, with no
  error anywhere. This is the single easiest thing to get wrong here.

- **`normalize_body` is per-language and is the most dangerous function in the codebase.**
  Strip too much and real changes become invisible. It is guarded by a fixture assertion: the
  reformat commit must produce exactly zero symbol changes, and a literal change must always
  produce one.

- **Soft-deletes mean nearly every query needs `WHERE deleted = 0`.** Forgetting it is silent.
  `bh-store` should expose filtered views rather than raw tables.

- **`idx_edges_dst` and `idx_edges_unresolved` are load-bearing.** Without the first, every
  impact query is a table scan. Without the second, a rescan that adds a symbol scans the
  whole edge table — a 200 ms rescan becomes 40 s.

- **Commands are argv, never strings.** Allowlist entries are templates with typed holes;
  `{test}` becomes exactly one argv element. `sh -c` is never used, anywhere.

- **stdout is results, stderr is everything else.** `--json | jq` must work with `-v` on.

## Where to look

| Question | Document |
|---|---|
| how do the crates fit together | [architecture.md](docs/architecture.md) §3–4 |
| why is it built this way | [architecture-decisions.md](docs/architecture-decisions.md) |
| what does the schema look like | [data-model.md](docs/data-model.md) |
| how does rescan avoid re-parsing | [change-analysis.md](docs/change-analysis.md) §2 |
| how is a bug proven | [verification-engine.md](docs/verification-engine.md) |
| what can an agent call | [mcp-api.md](docs/mcp-api.md) |
| what is safe to execute | [security.md](docs/security.md) §3–4 |
| what should I build first | [roadmap.md](docs/roadmap.md) |
