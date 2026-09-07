# ADR-024 — Hooks are the deterministic invocation tier, and they ship off by default

**Status:** Accepted (2026-09-02), amended (2026-09-07) — see *Amendment* below.

## Why it is needed

Nexus currently helps only when the model *chooses* to call it. There are no hooks anywhere in
the plugin surface, and `skills/nexus/SKILL.md` is a carefully argued plea to remember.

That is the distance between "gives the agent better context" and "hopes the agent asks". A
capability that fires probabilistically has a value equal to its quality times the probability it
is invoked, and nobody is measuring the second term.

But the obvious fix — put Nexus on every prompt — places it on the developer's critical path,
where it can do real harm.

## Decision

**Hooks are the primary invocation tier, and they are opt-in.**

| Hook | Command | Budget |
|---|---|---|
| `SessionStart` | `nexus context --session --budget 800` | 800 tok / 400 ms |
| `UserPromptSubmit` | `nexus context --task-stdin --budget 4000` | 4000 tok / **150 ms** |
| `PostToolUse` (Edit\|Write) | `nexus rescan --quiet` | 0 tok / 200 ms |
| `Stop` | `nexus verify --changed` | ~300 tok / 5 s |

The prompt reaches `UserPromptSubmit` as JSON on stdin; no environment variable carries it.
This table first read `--task "$PROMPT"`, and the implementation took that placeholder for a
variable name and shipped `$CLAUDE_USER_PROMPT` — never set, so it expanded to empty, asked for
context about nothing, injected nothing and exited 0 on every prompt, indistinguishable from a
healthy hook whose ranker found nothing. Parsing the payload in a shell pipeline would have
fixed the mechanism and broken the second property below, so `--task-stdin` reads it in the
binary instead.

Non-negotiable properties:

- **Fail open.** Hard timeout, `exit 0` on any failure, nothing printed on failure. Removing
  `nexus` from `PATH` mid-session must leave the harness fully working.
- **No logic in a hook.** Each is `nexus <verb>` with a timeout. All intelligence is in the
  binary, so a hook regression costs the automatic path and nothing else.
- **Off by default**, enabled by `nexus init --hooks` after latency has been measured on the
  developer's own project.
- p95 is asserted in CI, not hoped for.

## Alternatives considered

**On by default.** The strongest argument for the product: a feature nobody enables is a feature
nobody has. Rejected because a per-prompt hook whose latency has not been measured on *this*
project is exactly the "change how you work" the mission forbids, and the failure is
asymmetric — a slow hook is disabled once and never reconsidered, whereas an off hook can be
turned on at any time by someone who wants it.

**MCP only (status quo).** Zero risk to the developer's critical path, and it is what exists.
Rejected: it leaves the invocation probability unaddressed, which is the whole point.

**A daemon that pushes context proactively.** Lowest latency, and the natural home for session
awareness. Rejected here for the reasons in ADR-022 and deferred behind ADR-006's trigger, which
has not fired (641 ms for a *full* 880-file scan against a 2 s threshold).

**Prompt-engineering the skill harder.** Cheapest possible change. Rejected: it optimises the
probability term by persuasion, which does not compound and cannot be measured.

## Costs

- **Two invocation paths to keep in agreement** — hooks and MCP. Mitigated by both being shells
  over identical CLI verbs.
- **Hooks are a Claude Code interface and interfaces move** (R12). Mitigated by hooks containing
  no logic, and by `nexus doctor` reporting hook health explicitly — necessary precisely because
  fail-open makes a hook failure invisible by construction.
- **Off by default means low initial adoption.** Accepted deliberately: earning the enable is
  better than defaulting into it and being switched off.

## The signal that should make you change it

1. **Measured p95 stays under budget across several real projects, and enabling is unanimous.**
   Then default-on is justified by evidence rather than by hope.
2. **p95 cannot be brought under 150 ms by caching.** ADR-006's daemon trigger has effectively
   fired from a direction it did not anticipate, and the answer is a warm process, not a slower
   hook.
3. **`doctor` reports hooks silently failing in the field.** Fail-open was the wrong default for
   that hook, and it needs a visible degraded mode instead.

## Amendment (2026-09-07) — signal 1 was tested and did not fire

The measurement signal 1 asks for now exists:
[`docs/eval/hook-latency.md`](../../eval/hook-latency.md), p95 over four repositories
spanning two orders of magnitude, from spring-petclinic (132 files) to spring-boot
(11 519 files, 81 612 symbols).

**It did not clear the bar.** Three of the four budgets breach on spring-boot, and nothing
breaches below it:

| hook | budget | spring-boot p95 |
|---|---:|---:|
| `SessionStart` | 400 ms | 276 ms |
| `PostToolUse` (no-op) | 200 ms | **251 ms** ✗ |
| `PostToolUse` (1 file edited) | 200 ms | 741 ms → **400 ms** ✗ |
| `UserPromptSubmit` (seeded) | 150 ms | **302 ms** ✗ |
| `UserPromptSubmit` (lexical fallback) | 150 ms | **694 ms** ✗ |

So **hooks stay off by default**, and the decision above stands on evidence rather than on
caution.

Two things this establishes beyond the verdict:

- The `PostToolUse` row was mostly waste. `resolve_edges` re-walked the whole unresolved set
  on every rescan and resolved nothing — 325 ms of 741 ms on spring-boot, three consecutive
  rescans returning the same 80 699 unresolved. Scoping that walk to what actually moved cut
  the row to 400 ms. It still breaches.
- **What remains is per *process*, not per change.** The symbol lookup map, the supertype map
  and the unresolved select cannot be scoped: a changed file's edge may point anywhere in the
  repository, so the table has to be complete. A hook pays that setup on every invocation and
  a warm process pays it once.

That last point is signal 2's territory — "p95 cannot be brought under 150 ms by caching" —
and it is now half-fired, honestly: nothing here proves caching *cannot* close the gap,
because no caching was attempted. What is established is that the residual cost scales with
index size on a path that is supposed to scale with change size, which is the shape a warm
process fixes and a bigger timeout does not. `docs/performance.md` §10 names the same trigger
from the other side. Both point at ADR-006's daemon.

Still outstanding from the same document, and not addressed by this amendment: "p95 is
asserted in CI, not hoped for" — no CI assertion exists for any of these numbers.
