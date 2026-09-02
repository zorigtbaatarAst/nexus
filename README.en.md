# Nexus

*English · [Монгол](README.md)*

**A platform for persistent code intelligence.** Nexus reads a codebase once, remembers its
structure, its history and what has gone wrong in it, and from then on works incrementally —
detecting what changed, computing what that touches, and running targeted analysis over the
affected region only.

> **Nexus understands the project; capabilities use that understanding.**

**Three capabilities read that one index**, one for each moment of working with a coding agent:

| Moment | Capability | The question it answers |
|---|---|---|
| the first scan | **Architect** | What is this project built from, and what does working in it lack? |
| after an edit | **Review** | What does this change reach, and what covers it? |
| a suspected bug | **BugHunter** | Where is it, and what proves it? |

Each is a crate that depends on the platform and nothing else. Each returns findings and gets
identity, lifecycle and history for free — so the same observation next week is recognised
rather than repeated.

> **Status: working, incomplete.** Scanning, change detection, impact across the
> frontend/backend seam, three capabilities with the full finding lifecycle, persistent
> memory, and an MCP server. Nothing is verified by reproduction yet, and every surface says
> so. See [`docs/roadmap.md`](docs/roadmap.md).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/nexus/main/install.sh | sh
```

One static binary, checksum-verified. No Java, no Node, no Docker, no runtime of any kind.

```bash
cd /path/to/your/project
nexus scan           # index it and set a baseline
nexus rescan         # what changed, and what it touches
nexus analyze        # run BugHunter over it
nexus ask next       # what is worth looking at
```

Two binaries are installed: `nexus`, the platform, and `bughunter`, the capability's own CLI.
They are the same image under two names — which one is running is read from `argv[0]`.

There is no separate setup step — `scan` initializes the project if it has not been set up,
because requiring `init` first is a step whose only outcome is the error "you forgot to run
init".

### Updating

```bash
# the binary — the intelligence
curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/nexus/main/install.sh | sh

# the Claude Code plugin — the prompts
/plugin marketplace update nexus
```

They are independent: updating one does not update the other. `nexus doctor` reports a
database schema older than the binary, which a rescan fixes.

### As a Claude Code plugin

```
/plugin marketplace add zorigtbaatarAst/nexus
/plugin install nexus@nexus
```

That brings the MCP server, eight slash commands and a skill that tells the agent when to
reach for Nexus unprompted — and how to read what it gets back. Or register the server
alone: `claude mcp add --scope user nexus -- nexus mcp`.

<details>
<summary>Other ways</summary>

```bash
# build from source (needs Rust)
curl -fsSL .../install.sh | sh -s -- --from-source

# a specific version, or a different directory
... | sh -s -- --version v0.1.0 --dir ~/bin

# remove it (project data in each repo's .nexus/ is left alone)
... | sh -s -- --uninstall

# from a clone
make install
```
</details>

If anything looks wrong, run `nexus doctor`. Every check names what it found, where, and
the exact command that fixes it.

## How it works

```
nexus init             detect language, framework, build system, databases, containers
nexus scan             index files, symbols, dependencies → establish a baseline
nexus rescan           diff against the baseline → changed symbols → impact
nexus impact <target>  blast radius of a symbol, a file or a name — across the stack
nexus graph            dependency graph size and how much of it resolved
nexus analyze [cap]    run a capability: architect | review | bughunter
nexus findings         findings from every capability
nexus ask <question>   changed · affected X · known X · facts · next
nexus fact <key> <..>  remember something for the next session
nexus status           baseline, drift, index size
nexus doctor           diagnose the environment and configuration
nexus mcp              run as an MCP server for Claude Code, Codex or Copilot

# planned — V1, not built yet. See docs/roadmap.md
nexus investigate      a described screenshot → UI anchor → across the seam → suspects
nexus verify           generate a reproduction test, run it, run it on the baseline, judge
```

### The loop

```
nexus scan                     → Architect: what this is, and what it lacks
  … the agent edits something …
nexus rescan                     what moved, down to the symbol
nexus analyze review --changed → Review: what it reaches, and what covers it
nexus analyze bughunter        → BugHunter, when a bug is suspected
```

**Architect** runs automatically with the first scan. It reports a datastore with no MCP
server configured to reach it, a project with no CI, and — the one that invalidates every
other answer — a scan that is looking at one module of something larger.

**Review** runs on what changed and nothing else. It reports a change no test reaches, a
contract change that reaches frontend code nobody touched, and a signature whose callers did
not move with it. None of those are visible in the diff, because none of them are in the files
that were edited. It has no opinion about naming, formatting or structure: those are taste,
and taste is what this deliberately stays out of.

There are two entry points. `rescan` answers *what did this commit break*. `investigate`
answers the way bugs actually arrive — someone points at a screen and says *this number is
wrong* — by anchoring the symptom to a component, crossing the frontend/backend seam at the
HTTP contract, and ranking suspects down to the repository method. The agent reads the
screenshot; BugHunter never receives it.

The first scan is the only one that reads the whole repository. A rescan costs what changed,
not what exists: a no-op rescan on a 5 MLOC monorepo targets under two seconds.

**What V1 will show.** The report below is the target, not current output — the verification
engine that produces `VERIFIED` does not exist yet, and every surface says so:

```
BugHunter
────────────────────────────────────────

Project: autoland
Baseline: a81f92c
Current:  c72aa11

Changes
  4 files
  17 symbols
  2 dependencies

Impact
  11 affected symbols
  8 related tests

Analysis
  Potential bugs: 3
  Verified:        1
  Unverified:      2

🚨 BUG-104
Duplicate payment under concurrency

Severity:   Critical
Confidence: 97%
Status:     VERIFIED

Reproduction:
PaymentConcurrencyTest

Introduced:
a81f92c
```

That 97 % is not a model grading its own work. It means: the predicted failure happened, it
happened every time, and it did not happen before the change.

---

## Use it from any agent

One binary, one MCP server, no per-agent implementation.

```jsonc
// Claude Code — .mcp.json
{ "mcpServers": { "nexus": { "command": "nexus", "args": ["mcp"] } } }
```

```toml
# Codex — ~/.codex/config.toml
[mcp_servers.nexus]
command = "nexus"
args    = ["mcp"]
```

```jsonc
// GitHub Copilot — .vscode/mcp.json
{ "servers": { "nexus": { "command": "nexus", "args": ["mcp"] } } }
```

---

## Design principles

1. **Deterministic evidence beats AI assumptions.** A finding without a verifiable
   `file:line` is rejected at the boundary, not stored.
2. **AI is optional.** The deterministic build contains no HTTP client at all.
3. **Never send the repository anywhere.** Context is a ranked, token-budgeted evidence
   bundle, and there is no code path that widens it to "just include the file".
4. **Never modify production code.** Generated tests live in a jailed directory; the
   developer's working tree is never checked out, stashed or reset.
5. **Errors never pass silently.** A file that fails to parse is recorded and reported;
   truncated results say so; an unreachable baseline triggers a full scan *and says it did*.
6. **`FIXED` requires evidence.** A bug not seen in an incremental scan was not examined —
   it is not fixed.
7. **Ask, do not guess.** When a request is under-specified, any tool returns concrete
   questions — each with the reason it is being asked — instead of picking a candidate and
   sounding certain about it.

---

## Documentation

| Document | Contents |
|---|---|
| [AGENTS.md](AGENTS.md) | the briefing: invariants, deliberate oddities, and the traps that cost real time |
| [architecture.md](docs/architecture.md) | layers, crates, module boundaries, repo structure, constraint traceability |
| [architecture-decisions.md](docs/architecture-decisions.md) | 21 ADRs, each with alternatives and a revisit trigger |
| [data-model.md](docs/data-model.md) | entities, the immutability doctrine, full SQLite DDL, indexes |
| [memory-model.md](docs/memory-model.md) | project / code / historical / bug memory, and facts |
| [capabilities.md](docs/capabilities.md) | the capability contract, and how to add one |
| [change-analysis.md](docs/change-analysis.md) | change detection, impact analysis, bug fingerprinting |
| [investigation.md](docs/investigation.md) | screenshot to suspect: UI anchoring, the cross-stack seam, the clarification protocol |
| [verification-engine.md](docs/verification-engine.md) | plan → emit → run → run baseline → judge |
| [mcp-api.md](docs/mcp-api.md) | tool contracts, budgeting, permission gating, client config |
| [ai-integration.md](docs/ai-integration.md) | `AiProvider`, agent-as-provider, redaction, Claude Code integration |
| [cli-spec.md](docs/cli-spec.md) | commands, flags, exit codes, output contracts |
| [security.md](docs/security.md) | threat model, permissions, sandbox, secrets, audit, data flow |
| [performance.md](docs/performance.md) | budgets, caching, parallelism, monorepo scaling |
| [testing-strategy.md](docs/testing-strategy.md) | error handling, golden fixtures, property tests |
| [roadmap.md](docs/roadmap.md) | MVP → V1 → V2, with triggers rather than dates |
| [diagrams/](docs/diagrams/) | system architecture, scan, rescan, verification, MCP |
| [docs/architecture/](docs/architecture/README.md) | **what Nexus should become**: the Context Engine, memory lifecycle, verification gate, evaluation design, and a Phase 0–5 roadmap. A plan, not a description |
| [tests/fixtures/README.md](tests/fixtures/README.md) | the benchmark corpus: four repositories generated deterministically from specifications |

---

## Built so far

| Crate | State |
|---|---|
| `nexus-types` · `nexus-store` · `nexus-vcs` · `nexus-lang` · `nexus-lang-java` · `nexus-lang-ts` · `nexus-lang-graphql` · `nexus-core` · `nexus-cli` | working |
| `nexus-mcp` | working — nineteen tools |
| `cap-architect` · `cap-review` · `cap-bughunter` | working — three, three and four deterministic rules |
| `nexus-fixtures` | working — builds the benchmark corpus from specifications, the same shas every time |
| `nexus-lang-python` · `nexus-lang-rust` · `nexus-verify` | later |

The store carries the **full 21-table schema** from day one — adding the history and
verification tables later would mean migrating a populated database for no reason.

Measured on a real 880-file Spring + Next.js project: 5,665 symbols and **96 % of
in-project edges resolved** in 641 ms, including 402 contract edges across the GraphQL seam.
On a 109-file Spring Boot repository: full scan 32 ms, no-op rescan 3 ms, one edited method
body 5 ms — and reformatting all 109 files (14,000 changed lines) produces **zero** symbol
changes.

```
$ bughunter impact 'mn.autoland.sales.vehicle.service.VehicleService#list' --paths

  0.81  VehicleGraphQLController#vehicles(...)
  0.57  graphql:Query.vehicles
  0.46  graphql:op:Vehicles
  0.37  frontend/src/app/(sales)/vehicles/page#VehiclesPage
        VehicleService#list --calls--> Controller#vehicles --routes-->
        graphql:Query.vehicles --calls_graphql--> graphql:op:Vehicles --calls_graphql-->
```

## Working on Nexus itself

`make check` is what CI runs — fmt, clippy with warnings denied, every test. `make fixtures`
builds the benchmark corpus into `target/fixtures/`; `make fixtures-verify` generates it twice
and fails if a sha moved. `/nexus-architect` orients from the code, then plans or implements
one task from the roadmap in `docs/architecture/`. Read [`AGENTS.md`](AGENTS.md) first.

## Planned stack

Rust · Cargo workspace · one static binary · SQLite (WAL) · tree-sitter · git2 · rmcp · clap.
Java, TypeScript and GraphQL analyzers ship; Python and Rust are planned. Each sits behind a
`LanguageAnalyzer` trait, with framework packs as a separate extension point.

Language rationale, and the three alternatives rejected:
[ADR-001](docs/architecture-decisions.md#adr-001-rust-for-bughunter-core).
