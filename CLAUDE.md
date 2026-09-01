# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Read first

[`AGENTS.md`](AGENTS.md) is the long-form design briefing — the invariants, the deliberate
oddities, and the traps that cost real debugging time to find. Read it before changing
anything in `crates/`. Two things in it are now stale: the "Status: architecture only, no
code exists" header (the MVP ships), and the note that the repo is still called `bughunter`
(it was renamed to `nexus`; only the legacy `.bughunter/` directory migration remains, in
`Engine::migrate_legacy_dir`).

`docs/` is the design of record — 15 documents, still accurate. The table at the end of
AGENTS.md maps questions to documents.

## Commands

```bash
make build            # cargo build
make release          # optimized; produces target/release/{nexus,bughunter}
make test             # cargo test --workspace
make lint             # cargo clippy --workspace --all-targets -- -D warnings
make check            # fmt + lint + test — exactly what CI runs
make install          # -> $PREFIX/bin (default ~/.local/bin)
make smoke            # clone spring-petclinic, scan, rescan, assert the no-op rescan is empty
make demo REPO=/path  # prove the cascade on a real repo without touching it
```

Single test:

```bash
cargo test -p nexus-core renames                       # one file's tests in one crate
cargo test -p nexus-cli --test boundaries              # one integration test binary
cargo test -p cap-bughunter finding_lifecycle -- --nocapture
```

Rust 1.82+ (`rust-version` in the workspace manifest). CI runs with `RUSTFLAGS=-D warnings`,
so a warning fails the build.

Running the tool on itself while developing:

```bash
cargo run --bin nexus -- --project /some/repo scan --json
cargo run --bin nexus -- mcp        # MCP server on stdio
```

## Architecture

A Rust workspace of 11 crates producing **one binary image under two names**. `nexus` is the
platform, `bughunter` is the capability's own CLI; which one is running is decided by
`argv[0]` (`render::product_name()`), so there is a single dispatch path that cannot drift.

The layering, from the bottom:

```
nexus-types      shared ids/enums/DTOs; depends on serde and schemars only
nexus-store      SQLite. The only crate in the workspace containing SQL. migrations/*.sql
nexus-vcs        git2: HEAD, dirty state, diffs. Knows nothing of languages or storage
nexus-lang       LanguageAnalyzer / FrameworkPack traits + registry
nexus-lang-java  tree-sitter-java: symbols, sig_hash, body_hash, Spring pack
nexus-lang-ts    TypeScript/TSX + the GraphQL operations that cross the seam
nexus-lang-graphql  indexes .graphqls — the schema is the contract, not the annotations
nexus-core       the Engine: index, graph, change detection, impact, facts, finding lifecycle
cap-bughunter    BugHunter's detectors. Depends on nexus-core and nothing else
cap-architect    Architect: what the project is, and what working in it lacks
cap-review       Review: what a change reaches, and what covers it
nexus-mcp        rmcp adapter. deserialize -> one Engine call -> serialize
nexus-cli        composition root: parse flags, open store, register capabilities, dispatch
```

`nexus-core/src/engine.rs` (~2k lines) is the single public API: every CLI command and every
MCP tool is one call into it. If an MCP handler needs two `Engine` calls, the missing method
belongs in `nexus-core`.

`Capability` (`nexus-core/src/capability.rs`) is the one extension point: it is handed a
`ProjectContext` snapshot plus a `Scope`, and returns `Finding`s. Nexus owns identity,
lifecycle, storage and presentation; a capability owns only rules. Capabilities are
registered by the composition root, never compiled into the core. Three capabilities ship, one per moment of the loop: **Architect** at the first scan (what the
project is, what working in it lacks), **Review** after an edit (what it reaches, what covers
it), **BugHunter** for a suspected defect. Their rules live in `crates/cap-*/src/`.

Two capability outputs are *advisory* rather than defects — a recommendation is still a
finding, with evidence and a lifecycle (ADR-021), and anything the platform has no opinion
about rides in the `capability_data` JSON column.

State lives in the scanned project's `.nexus/` (`nexus.db`, `config.toml`, `policy.toml`),
not here. Nexus writes `.nexus/.gitignore` itself.

## Boundaries are tests, not conventions

`crates/nexus-cli/tests/boundaries.rs` reads `cargo metadata` and fails the build on any of
these. Do not work around one — the design is what it is because of them.

- `nexus-core` must not depend on `nexus-mcp`, `nexus-cli`, any `cap-*`, or any HTTP client
  (`reqwest`/`hyper`/`ureq`). The deterministic build carries no network stack at all.
- `nexus-mcp` must not depend on `nexus-store`, `nexus-lang*` or `nexus-verify`.
- No `cap-*` may depend on `nexus-cli`, `nexus-mcp` or `nexus-store`.
- No `nexus-lang-*` may depend on `nexus-store` or `nexus-core`.
- Only `nexus-store` may depend on `rusqlite`. No exceptions, including "just this one query".

Also enforced in code: `#![forbid(unsafe_code)]` everywhere, and
`deny(clippy::unwrap_used, clippy::expect_used)` outside tests in `nexus-core` and
`cap-bughunter` — a panic loses the whole scan, an error loses one file.

## Invariants that bite

- **Ledger tables are append-only** — `scans`, `changes`, `commits`, `bug_occurrences`,
  `bug_verifications`, `test_runs`, `audit_events` are never `UPDATE`d. An `UPDATE` there
  destroys regression detection.
- **Cache invalidation must include tool versions** (`scans.tool_versions_json`). Bump a
  tree-sitter grammar without it and the index silently keeps the old wrong symbols.
- **`normalize_body` is per-language and is the most dangerous function here.** Pinned by
  fixtures: a whole-repo reformat must produce zero symbol changes; a one-line change must
  produce exactly one.
- **Two hashes per symbol.** `sig_hash` change = API break rippling to every caller;
  `body_hash`-only = ripples along data/effect edges only. Collapsing them makes impact noise.
- **Soft deletes** — nearly every query needs `WHERE deleted = 0`; forgetting it is silent.
- **Model confidence is clamped at 0.75**; deterministic detector findings are not clamped
  (nothing was asked of a model). Nothing is verified by reproduction yet, and every surface
  says so.
- **stdout is results, stderr is everything else.** `--json | jq` must work with `-v` on.
- **Exit codes are interface**: 0 ok, 1 runtime, 2 usage, 3 findings (`--fail-on`),
  5 no baseline, 6 ambiguous target. Finding a change is success, not an error.

## Plugin surface

The repo is also a Claude Code plugin (`.claude-plugin/`, `commands/`, `skills/nexus/`,
`mcp.json`) and ships MCP configs for Codex and Copilot in `integrations/`. The plugin's
`version` in `.claude-plugin/plugin.json` and the skill's `metadata.version` track the
workspace version — bump them together with `Cargo.toml`.

Releasing: `.github/workflows/release.yml` fails a tag that disagrees with the workspace
version, because `install.sh` verifies checksums and `nexus --version` is what people check
before updating. Bump `Cargo.toml` first, then tag `v<version>`.
