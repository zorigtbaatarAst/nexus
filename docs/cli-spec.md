# BugHunter — CLI Specification

The CLI is not a debug harness for the MCP server. It is the primary interface for a
developer in a terminal over SSH, and it is the interface CI uses. Every capability reachable
over MCP is reachable here, because both are thin layers over the same `Engine`.

---

## 1. Commands

```bash
bughunter init                    # detect, create .nexus/, migrate the DB
bughunter scan                    # full scan; establishes the baseline
bughunter rescan                  # incremental; the everyday command
bughunter status                  # baseline, drift, open bugs, regressions
bughunter changes                 # what changed since the baseline
bughunter impact <target>         # blast radius of a symbol, file or module
bughunter bugs                    # list findings
bughunter bug <id>                # one finding in full
bughunter verify <id>             # generate + run a reproduction test
bughunter history                 # scan and bug history
bughunter investigate             # symptom-driven: from a described screenshot to suspects
bughunter doctor                  # environment and configuration diagnosis
bughunter mcp                     # run as an MCP server on stdio
```

Supporting commands: `hunt` (rescan + impact + detectors + optional AI in one pass),
`explain <target>`, `fact add|list`, `ignore <id>`, `export`, `import`, `prune`.

### Arguments that matter

```bash
bughunter impact mn.pay.PaymentService#createPayment    # fqn
bughunter impact src/main/java/mn/pay/PaymentService.java   # file
bughunter impact mn.pay                                  # module/package
bughunter impact --reverse mn.pay.PaymentRepository      # who depends on this (default)
bughunter impact --forward mn.pay.PaymentController      # what this reaches
bughunter impact --depth 3 --min-score 0.3 --tests

bughunter changes --since scan-012
bughunter changes --since a81f92c --entity symbol --type modified

bughunter bugs --status VERIFIED --severity critical --component PaymentService
bughunter verify BUG-104 --repetitions 5 --sandbox docker --promote
```

### Investigating a symptom

```bash
bughunter investigate \
  --description "cart total shows 0 but 3 items are listed" \
  --expected    "total should be 45,000" \
  --route       /checkout \
  --text        "Нийт дүн" --text "0 ₮" \
  --network     "GET /api/cart 200" \
  --since       yesterday
```

On a TTY this prompts interactively when the symptom is ambiguous. Under `--json` it returns
the same structured clarification an MCP client would receive, and `--answers` supplies the
replies non-interactively:

```bash
bughunter investigate --json --answers which_area=TotalsPanel
```

One mechanism, two presentations. The CLI therefore cannot drift into asking something the
MCP path silently assumes — a class of divergence that is otherwise discovered only when a
user reports different answers from the two surfaces.

There is no `--screenshot` flag that analyzes an image. `--screenshot <path>` records the
file's path and hash as provenance on any resulting bug, and nothing opens it: the agent
reads the image, BugHunter reads the index.

---

## 2. Global flags

| Flag | Effect |
|---|---|
| `--json` | machine-readable output on stdout; the only thing on stdout |
| `--quiet` | errors only; exit code carries the result |
| `--verbose` / `-v` `-vv` | progress and timings to **stderr**; `-vv` adds per-file detail |
| `--project <path>` | operate on a project other than the cwd |
| `--no-ai` | force `NullProvider` regardless of configuration |
| `--no-color` | also honours `NO_COLOR` and non-TTY stdout automatically |
| `--yes` | non-interactive; never prompt (does **not** grant execute permission) |

**stdout carries results; stderr carries everything else.** This is not a style preference:
`bughunter rescan --json | jq '.impact.symbols'` must work while `-v` is on, and a progress
spinner on stdout would corrupt the JSON. Every renderer writes results to stdout and
diagnostics to stderr, always.

`--yes` suppresses prompts. It does not escalate permissions — an execute action forbidden
by `policy.toml` stays forbidden. A flag that quietly grants privileges is how a convenience
becomes an incident.

---

## 3. Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | runtime error (I/O, corrupt DB, parse failure that aborted the run) |
| 2 | usage error (bad arguments, unknown target) |
| 3 | **findings at or above `--fail-on <severity>`** — the CI gate |
| 4 | policy denied the requested action |
| 5 | no baseline; run `bughunter scan` first |
| 6 | clarification required and none supplied — see the printed questions, or pass `--answers` |

Code 3 is what makes BugHunter usable in a pipeline:

```bash
bughunter rescan --fail-on high --json > bughunter.json || \
  { jq -r '.bugs[] | "\(.severity)\t\(.uid)\t\(.title)"' bughunter.json; exit 1; }
```

Without `--fail-on`, findings never fail the command — discovering a bug is a success, not
an error, and a tool that exits non-zero for doing its job gets removed from the pipeline
within a week.

---

## 4. Human output

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

The rules behind that format, specified as a renderer contract in `bh-cli::render`:

- **Counts before details.** The first screen answers "how much changed and how bad is it".
  Nobody reads 40 findings; everybody reads three numbers.
- **One blank line between blocks, two-space indent for values.** Greppable and `awk`-able.
- **Severity is a word, not only a colour.** Colour is removed under `NO_COLOR`, on a pipe,
  and in CI logs; a format that loses meaning without it is broken by default.
- **Confidence is a percentage with no decimals.** `97%` is a claim; `0.9713` is false
  precision about an estimate.
- **Emoji only as a severity glyph** (`🚨` critical, `⚠` high, `·` medium and below), and
  suppressed under `--no-color` and non-UTF-8 locales.
- **Every bug block ends with reproduction and provenance.** "Where did this come from" and
  "how do I see it myself" are the first two questions a developer has.
- **Truncation is stated**: `showing 10 of 47 — use --all or --severity high`.

Long-running commands render progress to stderr:

```
  scanning   ████████████░░░░░░░░  8120/42311 files   parse   12.4s
```

Progress is suppressed under `--quiet`, `--json` and when stderr is not a TTY.

---

## 5. JSON output

`--json` emits one object, schema-versioned, with no ANSI codes and no progress:

```json
{
  "bughunter": "0.1.0",
  "schema": 1,
  "command": "rescan",
  "project": "autoland",
  "baseline": { "scan_id": "scan-013", "commit": "a81f92c", "working_tree_hash": "9f2c…" },
  "current":  { "scan_id": "scan-014", "commit": "c72aa11", "dirty": false },
  "changes":  { "files": 4, "symbols": 17, "dependencies": 2,
                "items": [ { "entity":"symbol", "fqn":"mn.pay.PaymentService#createPayment",
                             "change_type":"modified", "detail":"body" } ] },
  "impact":   { "symbols": 11, "tests": 8, "truncated": false,
                "items": [ { "fqn":"mn.pay.PaymentController#pay", "score":0.81,
                             "min_confidence":0.9,
                             "path":["mn.pay.PaymentService#createPayment","calls"] } ] },
  "bugs":     [ { "uid":"BUG-104", "fingerprint":"3f9a…", "slug":"payment-duplicate-concurrent-create",
                  "title":"Duplicate payment under concurrency", "type":"concurrency",
                  "severity":"critical", "confidence":0.97, "status":"VERIFIED",
                  "introduced_commit":"a81f92c",
                  "verification":{"outcome":"reproduced","test":"BugHunter_BUG104_DuplicatePaymentTest",
                                  "runs":{"current":"3/3 fail","baseline":"3/3 pass"}},
                  "evidence":[{"file":"src/main/java/mn/pay/PaymentService.java","line":88}] } ],
  "warnings": [ "2 files failed to parse (see --verbose)" ],
  "exit": 3
}
```

`schema` is versioned independently of the binary and only ever changes additively within a
major version, so a script written today keeps working. `warnings` is always present, so a
consumer never has to detect the difference between "no warnings" and "an older version that
did not report them".

---

## 6. `bughunter doctor`

The highest-value command in the tool, and the first thing to ask anyone reporting a problem.

```
BugHunter doctor
────────────────────────────────────────
  ✓ git                 2.55.0, repository at /srv/autoland
  ✓ database            .nexus/nexus.db, schema 3 (current)
  ✓ config              config.toml, policy.toml valid
  ✓ languages           java (tree-sitter-java 0.21.0), typescript (0.20.4)
  ✓ frameworks          spring-boot 3.5 (build.gradle:24)
  ✓ build system        gradle 8.7 — ./gradlew present and executable
  ⚠ baseline            scan-013 is 47 commits behind HEAD — run: bughunter rescan
  ✓ sandbox             docker 29.6.2 available; policy.execute = "docker"
  ⚠ ai                  no provider configured; policy.ai = "agent" (MCP clients only)
  ✓ disk                .nexus/ 82 MB (db 41 MB, cache 39 MB)

2 warnings, 0 errors
```

Each check states what it found, where it found it, and — for anything not ✓ — the exact
command or config line that fixes it. A diagnostic that reports a problem without a remedy
has done half a job.

---

## 7. Implementation notes

`clap` derive, one module per command in `bh-cli::commands`. Each command:

```rust
pub struct RescanCmd { /* flags */ }

impl Command for RescanCmd {
    type Output = RescanReport;                                  // serde::Serialize
    fn run(&self, engine: &Engine) -> Result<Self::Output>;      // one Engine call
    fn render_human(&self, out: &mut impl Write, r: &Self::Output) -> io::Result<()>;
}
```

`--json` serializes `Output`; the default path calls `render_human`. The two can never
disagree about the facts because they render the same value — a JSON mode assembled
separately from the human mode drifts within a month, and the drift is always discovered by
a CI script that silently stopped catching bugs.

`bh-cli::main` is the composition root: parse flags → load config and policy → open the
store → build the analyzer registry → select the `AiProvider` → construct `Engine` → dispatch.
It is the only place in the codebase that knows about all of those at once, and the only
place `anyhow` is used.
