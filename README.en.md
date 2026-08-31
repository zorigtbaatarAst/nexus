# Nexus

*English · [Монгол](README.md)*

**A platform for persistent code intelligence.** Nexus reads a codebase once, remembers its
structure, its history and what has gone wrong in it, and from then on works incrementally —
detecting what changed, computing what that touches, and running targeted analysis over the
affected region only.

> **Nexus understands the project; capabilities use that understanding.**

**BugHunter** is the first capability: deterministic bug detection. It works on its own and
through Nexus, and it is the shape every later capability takes — read the index, return
findings, get identity, lifecycle and history for free.

> **Status: working, incomplete.** Scanning, change detection, impact across the
> frontend/backend seam, four deterministic rules with the full finding lifecycle, persistent
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

If anything looks wrong, run `bughunter doctor`. Every check names what it found, where, and
the exact command that fixes it.

## How it works

```
bughunter init         detect language, framework, build system, databases, containers
nexus scan             index files, symbols, dependencies → establish a baseline
nexus rescan           diff against the baseline → changed symbols → impact
nexus impact <target>  blast radius of a symbol, a file or a name — across the stack
nexus graph            dependency graph size and how much of it resolved
nexus analyze [cap]    run a capability (BugHunter by default)
nexus findings         findings from every capability
nexus ask <question>   changed · affected X · known X · facts · next
nexus fact <key> <..>  remember something for the next session
nexus mcp              run as an MCP server for Claude Code, Codex or Copilot
bughunter investigate  a described screenshot → UI anchor → across the seam → suspects
bughunter verify       generate a reproduction test, run it, run it on the baseline, judge
```

There are two entry points. `rescan` answers *what did this commit break*. `investigate`
answers the way bugs actually arrive — someone points at a screen and says *this number is
wrong* — by anchoring the symptom to a component, crossing the frontend/backend seam at the
HTTP contract, and ranking suspects down to the repository method. The agent reads the
screenshot; BugHunter never receives it.

The first scan is the only one that reads the whole repository. A rescan costs what changed,
not what exists: a no-op rescan on a 5 MLOC monorepo targets under two seconds.

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
| [architecture.md](docs/architecture.md) | layers, crates, module boundaries, repo structure, constraint traceability |
| [architecture-decisions.md](docs/architecture-decisions.md) | 12 ADRs, each with alternatives and a revisit trigger |
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

---

## Built so far

| Crate | State |
|---|---|
| `nexus-types` · `nexus-store` · `nexus-vcs` · `nexus-lang` · `nexus-lang-java` · `nexus-lang-ts` · `nexus-core` · `nexus-cli` | working |
| `nexus-mcp` | working — sixteen tools |
| `cap-bughunter` | working — four deterministic rules |
| `nexus-lang-python` · `nexus-lang-rust` · `nexus-verify` | later |

The store carries the **full 21-table schema** from day one — adding the bug and
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

## Planned stack

Rust · Cargo workspace · one static binary · SQLite (WAL) · tree-sitter · git2 · rmcp · clap.
Java, TypeScript, Python and Rust analyzers behind a `LanguageAnalyzer` trait, with framework
packs as a separate extension point.

Language rationale, and the three alternatives rejected:
[ADR-001](docs/architecture-decisions.md#adr-001-rust-for-bughunter-core).
