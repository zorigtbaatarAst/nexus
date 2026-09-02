# ADR-024 — Hooks are the deterministic invocation tier, and they ship off by default

**Status:** Accepted (2026-09-02)

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
| `UserPromptSubmit` | `nexus context --task "$PROMPT" --budget 4000` | 4000 tok / **150 ms** |
| `PostToolUse` (Edit\|Write) | `nexus rescan --quiet` | 0 tok / 200 ms |
| `Stop` | `nexus verify --changed` | ~300 tok / 5 s |

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
