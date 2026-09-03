# CLAUDE.md

Guidance for Claude Code working in this repository.

## Read first

[`AGENTS.md`](AGENTS.md) is the briefing: what this is, the constraints that are pinned by
tests, the things that look wrong and are deliberate, and the traps that cost real debugging
time. **Read it before changing anything in `crates/`.**

`docs/` is the design of record — 15 documents. The table at the end of `AGENTS.md` maps a
question to the document that answers it. `docs/architecture/` is the plan for what Nexus
should become; its `10-roadmap.md` records what each phase delivered.

> **This file holds nothing that `AGENTS.md` or `docs/` already says.** That is deliberate.
> A fact written in two places is a fact that will eventually disagree with itself — the
> crate count once lived here, in `AGENTS.md`, and in both READMEs, and three of the four
> were wrong. Add design facts to `AGENTS.md`; keep this file to how the repo is driven.

## Commands

```bash
make check            # fmt + lint + test — exactly what CI runs
make build            # cargo build
make release          # optimized; produces target/release/{nexus,bughunter}
make test             # cargo test --workspace
make lint             # cargo clippy --workspace --all-targets -- -D warnings
make install          # -> $PREFIX/bin (default ~/.local/bin)
make smoke            # clone spring-petclinic, scan, rescan, assert the no-op rescan is empty
make demo REPO=/path  # prove the cascade on a real repo without touching it
make fixtures         # build the benchmark corpus -> target/fixtures
```

Single test:

```bash
cargo test -p nexus-core renames                  # one file's tests in one crate
cargo test -p nexus-cli --test boundaries         # one integration test binary
cargo test -p cap-bughunter finding_lifecycle -- --nocapture
```

Rust 1.82+ (`rust-version` in the workspace manifest). CI runs with `RUSTFLAGS=-D warnings`,
so a warning fails the build.

Running the tool on itself while developing — it indexes Rust, so this works on this repo:

```bash
cargo run --bin nexus -- --project /some/repo scan --json
cargo run --bin nexus -- mcp        # MCP server on stdio
```

## Where state lives

In the *scanned* project's `.nexus/` — `nexus.db`, `config.toml`, `policy.toml` — never here.
Nexus writes `.nexus/.gitignore` itself.

## Releasing

The repo is also a Claude Code plugin (`.claude-plugin/`, `commands/`, `skills/nexus/`,
`mcp.json`) and ships MCP configs for Codex and Copilot in `integrations/`. The plugin's
`version` in `.claude-plugin/plugin.json` and the skill's `metadata.version` track the
workspace version — bump them together with `Cargo.toml`.

`.github/workflows/release.yml` fails a tag that disagrees with the workspace version, because
`install.sh` verifies checksums and `nexus --version` is what people check before updating.
Bump `Cargo.toml` first, then tag `v<version>`.

## Working agreements

- **Issue tracker** — GitHub Issues on `zorigtbaatarAst/nexus`, via `gh`. See
  [`docs/agents/issue-tracker.md`](docs/agents/issue-tracker.md).
- **Triage labels** — five canonical roles, each label string equal to its name. See
  [`docs/agents/triage-labels.md`](docs/agents/triage-labels.md).
- **Domain docs** — single-context; the ADRs are sections of `docs/architecture-decisions.md`,
  not a `docs/adr/` directory. See [`docs/agents/domain.md`](docs/agents/domain.md).
