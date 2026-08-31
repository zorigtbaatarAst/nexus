# BugHunter — Security Model

BugHunter reads proprietary source code, executes build and test commands, and can send
excerpts to third-party APIs. Each of those is a way to lose something that matters. The
design treats source code and command execution as sensitive by default and makes the
dangerous paths explicit, narrow and logged.

---

## 1. Threat model

| # | Threat | Mitigation |
|---|---|---|
| T1 | Secrets in source reach an LLM provider | path deny-list + redaction pass + `ai = "off"` default until configured (§5) |
| T2 | A generated test executes arbitrary code | sandbox + command allowlist + no shell (§3, §4) |
| T3 | A crafted symbol or test name injects a shell command | typed-hole templates expanded to argv; `sh -c` never used (§3) |
| T4 | BugHunter modifies production code | `SafeWriter` root jail; boundary rule 6 (§6) |
| T5 | An MCP client silently triggers execution | `execute`-class tools consult policy and return `permission_required` (§2) |
| T6 | A malicious repo escapes the scan via symlinks | canonicalize and confine traversal to the project root (§6) |
| T7 | Secrets leak into BugHunter's own storage | payload hashes in `audit_events`, never payloads (§7) |
| T8 | A dependency in the toolchain is compromised | pinned versions, `cargo-deny`, vendored grammars, reproducible builds (§8) |
| T9 | A test run exfiltrates data over the network | `--network=none` by default in the sandbox (§4) |

---

## 2. Explicit project permissions

`.nexus/policy.toml` is **committed to the repository**. Permissions are a property of
the project that a human reviewed in a pull request, not of whoever happens to run the tool.

```toml
[permissions]
read_paths    = ["src/**", "test/**", "build.gradle", "docker/**"]
deny_paths    = ["**/.env*", "**/*.pem", "**/*.key", "**/secrets/**", "**/credentials*"]
execute       = "docker"        # docker | host | none      (default: none)
allow_network = false           # inside the sandbox
ai            = "agent"         # agent | provider | off    (default: agent)

[execute]
timeout_seconds = 600
memory_limit    = "4g"
cpu_limit       = "4"

[execute.allowlist]
commands = [
  "./gradlew test --tests {test}",
  "mvn -q test -Dtest={test}",
  "npm test -- {test}",
  "pytest {test}",
  "cargo test {test}",
]

[ai]
provider          = "none"
max_context_tokens = 24000
redact            = true        # cannot be set false without --i-understand
```

Defaults are the safe end of every axis: `execute = "none"` and `ai = "agent"` mean a
freshly initialized project can index, diff and analyze, but cannot run anything and cannot
call any API until someone commits a change saying otherwise.

Policy is read once at startup, validated, and passed immutably into `Engine`. There is no
runtime mutation and no environment-variable override of a permission — an env var that
grants execute rights is a permission system with a hole in it.

---

## 3. Safe command execution

Two rules, and they eliminate a whole vulnerability class between them.

**1. No shell. Ever.** Commands are built as an explicit argv and passed to
`std::process::Command` with no `sh -c`, no `bash -c`, no string interpolation into a shell.
Shell metacharacters in a test name are then just characters.

**2. Templates with typed holes.** An allowlist entry is not a command line to be
string-formatted; it is a template parsed once into segments and holes:

```
"./gradlew test --tests {test}"
        → argv ["./gradlew", "test", "--tests", <one element: the expanded {test}>]
```

`{test}` is filled with exactly one argv element. A test name of
`foo; rm -rf /` becomes the single argument `foo; rm -rf /`, which Gradle rejects as an
unknown test. Injection is not escaped; it is structurally impossible.

Holes are typed and validated before expansion:

| Hole | Type | Validation |
|---|---|---|
| `{test}` | test selector | matches `[A-Za-z0-9_.#*$-]+`, ≤ 512 chars |
| `{file}` | path | canonicalized, must be inside the project root |
| `{module}` | module id | matches the detected module list |

Anything not on the allowlist does not run — including a command an AI proposes. There is no
"run this command" tool over MCP, and adding one is out of scope for every planned version.

---

## 4. Sandboxing

Decided: **Docker when available, host with explicit opt-in.**
See [ADR-009](architecture-decisions.md#adr-009-docker-preferred-sandbox-with-explicit-host-opt-in).

```
policy.execute = "docker"     container required; refuse (exit 4) if unavailable
policy.execute = "host"       host execution permitted — explicit, committed, audited
policy.execute = "none"       generate tests, never run them                [default]
```

Container profile:

```
--read-only                             repository mounted read-only
-v .nexus/generated-tests:rw        the only writable project path
-v <build-cache>:rw                     gradle/npm/pip cache, so runs are not glacial
--network=none                          unless policy.allow_network = true
--memory 4g --cpus 4 --pids-limit 512
--user <caller uid:gid>                 never root; no new files owned by root
--security-opt no-new-privileges
timeout 600s → kill → outcome "inconclusive"
```

The repository is read-only *inside the container* as well: a test that tries to rewrite a
source file fails on the filesystem rather than on trust.

Host execution is honestly necessary — testcontainers, GPU tests, licensed toolchains, and
the plain fact that many suites do not containerize. It is not treated as a failure state,
but it is opt-in, recorded in `test_runs.sandbox`, and written to the audit log every time.

---

## 5. Secret detection and redaction

Three layers, in order, so a secret has to get past all of them:

1. **Exclusion.** `deny_paths` files are never read into the index at all. `.env`, `*.pem`,
   `*.key`, `secrets/**` and `credentials*` are excluded by default, before parsing.
2. **Detection.** Every indexed file is scanned for credential shapes: cloud key prefixes,
   GitHub/Slack/Stripe token formats, JWTs, PEM blocks, connection strings with embedded
   credentials, assignments to `password`/`secret`/`token`/`api_key`, and string literals
   over 32 characters with Shannon entropy above 4.0.
3. **Redaction.** Any bundle leaving the process for an AI provider has detections replaced
   with `«REDACTED:aws_key»`. Redaction runs on the serialized bundle, after assembly, so it
   cannot be bypassed by a new context source forgetting to call it.

Detections are also **findings**: a hardcoded credential becomes a `security` bug with the
value redacted in the report. Protecting the secret and reporting it are the same pass.

`redact = false` requires the `--i-understand` flag on the command line as well as the config
change. Two deliberate acts, because the failure mode is unrecoverable.

Under `ai = "agent"` — the default — nothing leaves the machine at all. Redaction still runs,
because the evidence bundle goes to an MCP client that may itself be a hosted model, and
BugHunter should not be the component that made that leak possible.

---

## 6. Filesystem confinement

- **Traversal** is confined to the project root. Symlinks pointing outside are not followed;
  they are recorded as skipped with a reason, not silently ignored.
- **Writes** go through `SafeWriter`, rooted at `.nexus/generated-tests/`, with the
  parent path canonicalized *before* the prefix check so `..` and symlinks cannot escape. An
  attempted escape is a hard error plus an audit row.
- **BugHunter never writes to a build file.** Telling a build system where a generated test
  lives is done with command-line arguments, never by editing `build.gradle`,
  `package.json`, `pom.xml` or `Cargo.toml`.
- **The primary working tree is never mutated** during verification. Baseline runs use
  `git worktree add --detach` into `.nexus/cache/worktrees/`; no checkout, no stash, no
  reset of the developer's tree.

---

## 7. Audit log

Every execution and every AI call produces an `audit_events` row **and** a JSONL line in
`.nexus/audit.log`, so the record survives a database reset and can be tailed.

```json
{"at":"2026-08-31T11:42:07Z","actor":"mcp:claude-code","action":"exec",
 "target":"BUG-104","sandbox":"docker","revision":"c72aa11",
 "argv":["./gradlew","test","--tests","*BugHunter_BUG104*"],
 "exit":1,"duration_ms":48210,"outcome":"reproduced"}

{"at":"2026-08-31T11:41:55Z","actor":"cli","action":"ai_request",
 "provider":"claude","task":"FindBugs","tokens_in":18422,"tokens_out":1204,
 "redactions":2,"payload_hash":"b41c…","outcome":"ok"}
```

Recorded: what ran, who asked, under which policy, against which revision, with what result.
**Not recorded: the payload.** Only its hash. Storing prompts would rebuild, inside
BugHunter's database, precisely the exposure the redactor exists to prevent — and that
database is not covered by the deny-list protecting the repository.

`bughunter history --audit` renders it; the file is plain JSONL for `jq`.

---

## 8. Supply chain

- Exact pinned versions in `Cargo.lock`, committed; `cargo-deny` in CI for advisories,
  licences and duplicate crates.
- Tree-sitter grammars vendored at a pinned revision rather than fetched at build time. A
  grammar is a parser generator's output executing on customer source; it is not a
  dependency to float.
- The binary makes **no network calls** except through a configured `AiProvider`. No
  telemetry, no update check, no crash reporting. A tool that reads proprietary source and
  also phones home is a tool that gets banned.
- Reproducible release builds with published checksums; `install.sh` verifies before
  installing.

---

## 9. Data-flow statement

Printed by `bughunter doctor --ai`, and true for every configuration:

```
Stays on this machine, always:
  source code · the SQLite index · the dependency graph · git history
  bug records · verification results · the audit log

Leaves the machine ONLY when policy.ai = "provider" AND a provider is configured:
  a token-budgeted evidence bundle — capped source excerpts of changed symbols,
  impact paths, relevant facts, prior bug digests, test names — after redaction.

Never leaves the machine, under any configuration:
  whole files · whole repositories · git objects · the database · secrets ·
  files matching deny_paths · the contents of the audit log

Under the default policy.ai = "agent":
  nothing leaves the BugHunter process. The evidence bundle is returned over MCP
  to the calling agent, which is responsible for its own data handling.
```

That last paragraph is the honest one: under the default configuration BugHunter sends
nothing anywhere, but the agent it is talking to may well be a hosted model. BugHunter's
obligation is to hand that agent redacted, minimal evidence — never a repository — and to
say plainly that the boundary moves at that point.
