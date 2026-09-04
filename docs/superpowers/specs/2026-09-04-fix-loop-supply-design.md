# The fix loop is supplied, and the supply is measured

**Status:** design, 2026-09-04. Measured against the tree at `468face`, binary 0.3.0.

**One sentence.** When an agent is fixing a bug, Nexus should already have handed it what it
needs — and both halves of that claim, *did the supply contain the answer* and *what did the
supply cost when it had nothing to say*, become tests that fail the build.

---

## 1. What this is not

Named first, because the request that started this design could be read three ways and two of
them are wrong.

**Not capability chaining.** A debug prompt will not run BugHunter, and the fix loop will not
orchestrate `bughunter → review → verify`. Capabilities contribute *rules*; rules do not fix
bugs. Putting a reasoning sequence inside the platform is the inversion
[`AGENTS.md`](../../../AGENTS.md) exists to prevent — *"Nexus owns evidence, history and
verification. The AI agent owns reasoning."*

There is also a measured reason. BugHunter has three detector families, two Spring and one
GraphQL. The corpus's own headline planted bug is a concurrency race, and
[`tests/fixtures/specs/spring-payments/fixture.toml`](../../../tests/fixtures/specs/spring-payments/fixture.toml)
says of it: *"only fails under concurrency — which is why no linter and no type checker sees
it."* BugHunter does not see it either. Running it on every debug prompt buys latency and
noise in exchange for finding a class of defect it structurally cannot find.

**Not a repositioning.** Bug-fixing is context's most demanding *consumer*, not the product's
purpose. [`00-vision.md`](../../architecture/00-vision.md) and both READMEs stay as they are.

**Not the agent-in-the-loop cost harness.** Measuring tokens-to-correct-fix means running an
agent on the same bug twice, several times over, for a median. That is real money per run and
non-deterministic. [`13-evaluation.md`](../../architecture/13-evaluation.md) §G2 already
defines it — cost-per-success, *"< 30 % median reduction falsifies"* — and admits it is
"currently unmeasurable". It stays unmeasured here. This design builds the deterministic
proxy that must hold *before* that number could ever be good: if the package does not contain
the files the fix touches, no token saving is possible.

**Not the external-tool wrapper.** The decision stands — invoke `gitleaks` through the
existing allowlist executor, approach A of the 2026-09-04 discussion — and it waits. Wrapping
improves *what gets found*; this design is about *what is already in hand when you start*.

---

## 2. The problem, measured

### 2.1 Debug intent already works. Almost nothing else on that path does.

`Intent::Debug` is classified deterministically from the prompt —
`crates/nexus-core/src/context/intent.rs`, pinned by tests over `"fix the payment idempotency
bug"`, `"the checkout page is broken"`, `"this fails with a NullPointerException"` and any
stack trace. It flips expansion direction to `forward` (`context/expand.rs:24`). The
`UserPromptSubmit` hook already feeds it every prompt.

So the trigger [`07-agent-integration.md`](../../architecture/07-agent-integration.md) §6.4
calls for is built. What §6.4 additionally describes — `--purpose debug` — is not.

### 2.2 `Purpose` has five variants and two behaviours

`Engine::context` dispatches `Purpose::Session → session_package` and **everything else to
`task_package`** (`engine/query.rs:135-140`). `Purpose::Debug`, `Purpose::Review` and
`Purpose::Verify` are constructed nowhere in the workspace; their only appearance outside the
enum is `as_str`.

This is the shape `AGENTS.md` already names once: *"A value written by one function and read
by another that is not its inverse is a silent-failure machine."* Here it is not even read.

### 2.3 Every prompt re-sends what the session already sent

The `SessionStart` hook emits the project profile once. The `UserPromptSubmit` hook emits it
again, on every prompt, whatever the prompt says. Measured on this repository at 0.3.0:

| Prompt | Items | Tokens |
|---|---|---|
| `"yes, include make eval mention"` | 0 | 256 |
| `"ask questions one by one"` | 0 | 256 |
| `"should the context package contain the fix files"` | 0 | 234 |
| `"Purpose::Debug is dead code, should we delete Review and Verify"` | 10 | 884 |
| `"the context cache key is wrong for dirty trees"` | 4 | 529 |

The budget behaves correctly — 4,000 is a ceiling, not a target, and a prompt naming no symbol
seeds nothing. The waste is the header: **234–256 tokens repeated per prompt**, which is half
of a small package and the entirety of an empty one. Over a fourteen-prompt conversation that
is ~3,500 tokens of duplication; over fifty prompts, ~12,500.

### 2.4 The session package opens with findings that do not exist

`analyze bughunter` on this repository reports 19 findings. Every one of the 11 returned comes
from the secrets detector, and none is a credential: ten are the detector's own prefix table
in `crates/cap-bughunter/src/detectors/secrets.rs` — the literals `"ghp_"`, `"sk_live_"` that
*define* the patterns — and the eleventh anchors on `let idx = index(` inside a test in
`crates/nexus-core/src/context/signals.rs`.

These are not confined to `analyze`. Open findings lead the session package, so **every agent
session on this repository begins by being told about eight fabricated critical bugs**, before
the user types anything. That is not a token cost; it is misinformation at the top of the
context window.

[`09-tooling.md`](../../architecture/09-tooling.md) §12 already forbids the detector's
existence — *"Must not build: rules that duplicate a linter"* — and justifies `cap-bughunter`
on the grounds that its three detectors are *"three things no linter checks"*. For committed
credentials that is false: `gitleaks` and `trufflehog` do it with hundreds of patterns,
entropy scoring and allowlists.

### 2.5 The diagnostic command mislabels itself

Three user-facing strings hardcode the capability's name where every other site routes through
`render::product_name()`: the doctor title (`nexus-cli/src/main.rs:1013`) and the rescan remedy
(`nexus-core/src/engine/query.rs:1279`, `nexus-cli/src/render.rs:403`). `nexus doctor` prints
`BugHunter doctor` and advises `run: bughunter rescan`. The second is also a layering smell — a
CLI name inside `nexus-core`.

### 2.6 Silent version skew cost a whole session

The installed binary was **0.2.0**; the `context` subcommand did not exist in it. The hook is
`nexus context ... 2>/dev/null || true`, so every prompt spawned a process, failed with exit 2,
and said nothing. `nexus doctor` could not report it, being 0.2.0 itself. The MCP plugin showed
the same skew — 16 tools running against 20 in source, missing `nexus_get_context`.

This is the fail-open trap working exactly as documented and exactly as designed, with no
compensating signal for the case where the *binary itself* is behind.

---

## 3. Decisions

| # | Decision | Because |
|---|---|---|
| D1 | A declared `--purpose` overrides the derived intent | An agent that knows it is debugging must not depend on the verb table reading `"have a look at this"` correctly |
| D2 | `Purpose::Review` and `Purpose::Verify` are deleted | Nothing constructs them; they behave as `Task`. A variant that changes nothing is a branch that will one day be believed |
| D3 | The prompt hook stops repeating the profile, and prints nothing when it has nothing | §2.3. Opt-in by flag, so a human running the command directly still gets the header |
| D4 | Ground truth is the files the fixing commit touched, not the planted line | A package that names the buggy line but omits the file where the lock must go still forces a blind search |
| D5 | The gate is golden-style, not a threshold | A number chosen before evidence is the folklore [`11-risks.md`](../../architecture/11-risks.md) R8 names. Record behaviour, fail on change, review the diff |
| D6 | The same golden records cost on prompts that should produce nothing | Supply quality without overhead is half a measurement. A change that starts injecting on `"yes"` must fail the build |
| D7 | `secrets.rs` is deleted now, ahead of the wrapper | Its output leads every session package. Deleting a detector needs no new machinery, and baselines recorded while it exists would bake in eleven findings that do not |
| D8 | `doctor` reports version skew | §2.6 cost a session in silence, and doctor is the compensating control fail-open requires |

---

## 4. Work

### 4.1 `--purpose` becomes real

`nexus context --task <TEXT> --purpose <debug|task>` and the equivalent field on
`nexus_get_context`. When present, the purpose sets the intent rather than being ignored;
when absent, classification is unchanged. `Purpose::Review` and `Purpose::Verify` are removed
from the enum.

**Checkable:** `context --task "have a look at this" --purpose debug` produces a package whose
recorded intent is `debug` and whose expansion ran forward; without the flag the same text
classifies `Unknown`.

### 4.2 `--brief` for the hook

One flag on `nexus context`: suppress the project profile, and emit nothing at all when no
item is included. The `UserPromptSubmit` hook template gains it. Default behaviour is
unchanged, because for a human at a terminal the header is the useful part.

**Checkable:** with `--brief`, a prompt naming no symbol produces **zero bytes** on stdout; a
prompt naming one produces the items without the profile block. The hook's installed command
string contains `--brief`, asserted in `nexus-cli/tests/hooks.rs`.

### 4.3 The debug-supply golden

A test beside `nexus-core/tests/golden_packages.rs`:

1. Generate the fixture corpus into a temp directory (`nexus-fixtures` is already a dependency
   of the composition root, so no boundary moves).
2. For each `plants_bug` in each spec, check out the planting commit, scan, and build the
   package for a request written **by hand and committed in the golden**, one per bug — the
   sentence a person would type on noticing the symptom. Not generated from
   `plants_bug.summary`: that text names the cause, and a request naming the cause tests
   nothing, because the answer is already in the question.
3. Ground truth is `git diff <planting commit>..<fixed_by> --name-only`.
4. Record per bug: which fix files the package contained, at what rank, and the package's
   token cost.

Golden-style: the recorded file is committed, a change fails the test, `NEXUS_REBASELINE=1`
re-records, and the diff is the review.

**Checkable:** the golden file exists and is byte-identical across two runs on a clean tree.

### 4.4 The overhead arm of the same golden

A fixed list of prompts that should produce nothing — `"yes"`, `"park it"`, `"ask questions
one by one"`, `"thanks, that works"` — with their token cost recorded in the same file under
`--brief`. Today that cost should be zero bytes; the golden is what stops it silently becoming
250 tokens again.

**Checkable:** the overhead section of the golden lists every prompt with a cost, and the test
fails if any cost changes.

### 4.5 Delete the secrets detector

Remove `crates/cap-bughunter/src/detectors/secrets.rs` and its registration in
`detectors/mod.rs`, with its tests. `cap-bughunter` keeps the two Spring detectors and the
GraphQL one. `09-tooling.md` §12's justification is corrected: it claims three things no
linter checks, and credentials is not one of them.

**Checkable:** `analyze bughunter` on this repository returns zero findings, and the session
package's open-findings section is empty rather than eight fabrications.

### 4.6 `doctor` reports version skew, and stops calling itself the wrong name

A check comparing the running MCP server's advertised tool count against the binary's, warning
when they differ, with the remedy naming the plugin update command. The three hardcoded
strings route through `product_name()`.

**Checkable:** `nexus doctor` prints `Nexus doctor` and `run: nexus rescan`; `bughunter doctor`
prints `BugHunter doctor` and `run: bughunter rescan`. Asserted for both argv names.

### 4.7 Documentation

`07-agent-integration.md` §6.4 corrected: debug intent fires today; the scope handoff to
BugHunter is a decision *against*, recorded with its reason, not an unbuilt item.

---

## 5. Risks

**R-a · The golden pins the wrong thing.** A hand-written request that happens to name the
buggy symbol makes the gate pass on a phrasing nobody would type — the answer smuggled into
the question. Mitigation: the request text is committed in the golden and reviewed as part of
it, and a request naming the anchor symbol outright is a review failure, not a passing test.

**R-b · Deleting `secrets.rs` leaves a real gap.** With no wrapper yet, Nexus checks for
committed credentials not at all. Accepted: a detector with measured 0 % precision is not
coverage, and `09-tooling.md` §12 says this work belongs to a tool that already does it well.
The gap is named in the docs rather than papered over.

**R-c · `--brief` hides a broken hook.** Zero bytes is both "nothing relevant" and "the binary
is 0.2.0 again". This is why 4.6 ships with it: fail-open needs a compensating signal, and
`doctor` is it.

**R-d · The overhead golden is brittle.** Token counts move when unrelated serialisation
changes. Accepted: that is what re-baselining is for, and a golden that never moves is one
nobody reads.

---

## 6. What this design does not claim

It does not claim Nexus makes bug-fixing cheaper. It builds the measurement that would have to
hold first, and leaves the cost-per-success number to the harness `13-evaluation.md` §G2
describes and nobody has built. The honest statement after this work is: *"the debug package
contains the files the fix touches, in N of M planted bugs, and costs nothing on prompts that
name no code."* That is a smaller claim than the README's, and unlike the README's it is
reproducible.
