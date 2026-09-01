# Three capabilities over one index — as built

*2026-09-01. Written after the work rather than before it, because what the work found changed
what it was for. The plan this began from is at `~/.claude/plans/`; where the two disagree,
this is what is in the code.*

---

## What this was for

Nexus should cover the whole loop of working with a coding agent, with a capability at each
moment rather than one tool that does everything:

```
nexus scan        → Architect    what is this project, and what does working in it lack
agent edits X     → Review       what does this change reach, and what covers it
bug suspected     → BugHunter    where is it, and what proves it
```

The underlying claim being tested is older than this session: the platform split was performed
so that adding a capability would cost a crate rather than a restructure. Until now that was
proven by a forty-line test fixture. **Two real capabilities were added in a day, each
depending on `nexus-core` and nothing else, and `tests/boundaries.rs` covered both without a
line of test being written** — it discovers `cap-*` crates by prefix. The claim holds.

## What shipped

| Crate | Rules | What each reports |
|---|---|---|
| `cap-architect` | `datastore-tooling` | a datastore this project uses with no MCP server configured to reach it |
| | `no-ci` | no CI for a project that has a build system |
| | `partial-scan` | a module's worth of `sibling` edges — this scan covers one module of something larger |
| `cap-review` | `untested-change` | a changed method no test reaches |
| | `crosses-seam` | a contract change reaching frontend code that did not change with it |
| | `stale-callers` | a signature that moved while its callers did not |

Six rules, chosen to be few. Both capabilities are registered at the composition root
(`nexus-cli::open`, `nexus-mcp`) and never compiled into the platform.

## Two decisions worth knowing

**A recommendation is a finding (ADR-021).** Architect's output describes what a project
*lacks*, which fits neither the `Finding` shape nor `FindingType`'s twelve defect kinds. It
became a finding anyway, with the ordinary lifecycle — so *"there is no CI"* becomes `FIXED`
when a workflow appears and `REGRESSED` when someone deletes it, which is exactly what the
ledger is for. Two things changed and both are load-bearing:

- Evidence still required, anchored **where the missing thing belongs** — the build file CI
  would have invoked, the compose line that proved the datastore. If no such place is in the
  index the rule emits nothing, because a guessed filename is evidence naming a file that may
  not exist.
- `severity` and `confidence` answer a different question for an advisory: *how much this
  matters* and *how sure the rule is that the situation applies*, not *how bad* and *how sure
  it is real*.

**The code-review non-goal was revised, not deleted.** `docs/roadmap.md` listed *"be a code
review tool"* as a never-do, while `architecture.md`, `capabilities.md` and ADR-018 all named
Code Review as the canonical second capability. The wording went; the guardrail stayed —
nothing in `cap-review` has an opinion about naming, formatting or structure. Every rule
reports something the index can prove. That constraint is structural, not aspirational: it is
what makes the revision honest.

## What the work found

Writing a second and third capability surfaced two platform bugs that one capability could not
reach. Both are fixed; both are in `AGENTS.md` as traps.

**`ChangeKind` was hardcoded.** `Engine::analyze` built every `ChangedSymbol` with
`kind: BodyChanged`, discarding what the ledger already knew — so every rule asking *"did the
contract move?"* was permanently unreachable, silently, with no error anywhere. It survived
because BugHunter never looks at `ChangeKind`. The kind is stored across two columns
(`change_type`, `detail`) and had no joint inverse; `ChangeKind::from_ledger` is now that
inverse, beside the two functions that write it, with an exhaustive round-trip test.

> **The general lesson, worth more than the fix:** a value written by one function and read by
> another that is not its inverse is a silent-failure machine. Pin the round trip.

**An advisory rule wanted to relax the evidence requirement**, and was not allowed to. The
no-CI rule originally fell back to anchoring on `README.md` when no build file was indexed.
It now emits nothing instead.

## Verified against the real monorepo

Every measurement below is from a throwaway clone of `autoland-management` in scratch. **Your
working copy was never touched.**

```
nexus scan          23,784 symbols · 98 % of 27,519 in-project edges · 2.2 s
                    → ARC-1  mongodb is used here and no mongodb MCP server is configured
                             service/frontend/.env.example:17

  (change a GraphQL controller's return type)

nexus analyze review --changed
                    → REV-2  mySalary changed its contract and 2 frontend symbols depend on it
                             high · sales/backend/.../SalaryGraphQLController.java:30
                    → REV-1  mySalary changed and no test reaches it
                             medium

nexus analyze bughunter
                    → BUG-1  credential committed to TotpService.java   critical
```

On a scan pointed at one module rather than the root, Architect reports **6,247 references
reaching code the scan does not cover**, at high severity — because that silently understates
every impact answer the tool gives.

## What to check when you are back

1. **`BUG-1` is real and it is in your code.** `shared/auth/.../TotpService.java:40`, critical.
   The finding deliberately does not store the value and nobody has looked at it. Worth
   handling independently of any of this.
2. **`main` is ahead of `origin` and nothing was pushed.** Something was pushed from elsewhere
   while this ran, so check the history before you push.
3. **The schema is now 5 and the version is 0.3.0.** A v0.2.0 binary meeting this database
   refuses with `SchemaTooNew`. Nothing is released, so it costs nothing today — but it needs
   a release note when you tag, and the release workflow will fail a tag that disagrees with
   `Cargo.toml`.
4. **`README.md` (Монгол) was not updated.** `README.en.md` has the three capabilities; the
   Mongolian one still describes BugHunter as the only capability. Left for you deliberately
   rather than machine-translated.

## What is open, in the order I would take it

1. **A dismissal must generalize.** Today `nexus ignore` is sticky for *that finding* and
   teaches nothing about the next one. With one capability that was tolerable; with Review
   running on every edit it is what decides whether the tool survives a fortnight. This is the
   highest-value item in the project right now, and it is not large.
2. **Live with the six rules before adding a seventh.** Review's rule list is where a small
   capability becomes a product. The risk is not a missing rule — it is a flood.
3. **Rank by importance, not just proximity.** Impact scoring has no global centrality signal,
   so a method twenty things call ranks the same as a private helper. Matters more now that a
   capability runs on every edit.
4. **`nexus-verify`** — still designed in full, still deferred, still the only thing that makes
   `VERIFIED` reachable.
5. **Nexus cannot see itself.** 33 Rust files, zero analyzed. The tool that says what a change
   breaks cannot say it about its own — which is also why none of this session's work could be
   checked with the thing it was building.

## Known limits of what shipped

- **Architect returns nothing under a narrowed scope.** *"This project has no CI"* is not a
  statement about three edited files. Deliberate, tested.
- **Review sees only what the ledger recorded as changed.** Adding a parameter changes a
  method's FQN, so it records as *delete + add* rather than `API_CHANGED` — and the seam and
  stale-caller rules, which filter on contract changes, will not fire for it. Return-type and
  annotation changes work. This is a real gap, not a bug, and fixing it means teaching the
  rules to read a delete/add pair as a signature change.
- **`is_frontend` is a file-extension test.** Good enough for this stack; a framework pack
  would be better.
- **66 symbols still change on a whole-repo reformat.** Pre-existing — verified identical on
  the pre-session binary — and probably legitimate text blocks, but nobody has confirmed it.
