# Hook latency on real projects

ADR-024 ships hooks off by default and names the condition for turning them on:

> **Measured p95 stays under budget across several real projects**, and enabling is unanimous.

That measurement had never been taken. This is it.

## Method

Release binary, warm page cache, one un-counted warm-up then 10 timed runs (5 on
spring-boot), p95 over wall time of the exact command ADR-024's table puts in each hook.
Every repository is a throwaway clone, scanned cold from an empty `.nexus/`.
Reproduction: `scripts/eval/measure.sh <repo> <label>`.

Four repositories, spanning two orders of magnitude:

| repo | files | symbols | source LOC | index | cold scan |
|---|---:|---:|---:|---:|---:|
| spring-petclinic | 132 | 318 | 4 214 | 0.8 MB | 66 ms |
| nexus (self) | 339 | 2 309 | 44 929 | 2.0 MB | 228 ms |
| tokio | 868 | 8 470 | 180 987 | 7.1 MB | 820 ms |
| spring-boot | 11 519 | 81 612 | 876 585 | 86.4 MB | 7 019 ms |

## Result — p95, milliseconds

| | `SessionStart` ≤400 | `PostToolUse` ≤200 | `UserPromptSubmit` ≤150 | `UserPromptSubmit` ≤150 |
|---|---:|---:|---:|---:|
| | `context --session` | `rescan --quiet` (no-op) | seeded | lexical fallback |
| spring-petclinic | 5 | 6 | 11 | 25 |
| nexus | 13 | 8 | 9 | 68 |
| tokio | 38 | 13 | 21 | 113 |
| spring-boot | 276 | **251** ✗ | **302** ✗ | **694** ✗ |

Three of four budgets breach on spring-boot. Nothing breaches below it.

These are the numbers as measured, before the `resolve_edges` fix below. That fix moves the
rescan rows and leaves the two `UserPromptSubmit` rows alone; the post-fix rescan figures
are in the "Fixed" table.

The `UserPromptSubmit` column is **stale for a different reason**: it was measured before
`context/seeds.rs` was rewritten. See §"The prompt path" at the end.

## The rescan number above is the flattering one

`PostToolUse` fires on `Edit|Write`, so the repository has always just changed. Timed
after a real one-file edit rather than on a no-op:

| repo | rescan, 1 file edited |
|---|---:|
| spring-petclinic | 10 ms |
| nexus | 17 ms |
| tokio | 53 ms |
| spring-boot | **741 ms** |

Identical work — one file changed in each — and a 74× spread. This directly falsifies
[`performance.md`](../performance.md) §1, which at the time of this measurement read:

> The `rescan` rows are flat in repository size on purpose: their cost is proportional to
> *what changed*, not to how much code exists.

They are not flat. Cost tracks repository size, not change size. That passage has since been
corrected to say so and to cite these numbers, so the quote above is the superseded text, kept
because a finding that erases the claim it falsified cannot be checked.

### Confirmed, and it was not the FQN map

The first guess here was that rebuilding the FQN map per rescan was the cost. Instrumenting
`Store::resolve_edges` says otherwise — the map is a fifth of it:

| stage | spring-boot |
|---|---:|
| build the symbol lookup maps (81 612 symbols) | 92 ms |
| select the unresolved set | 37 ms |
| project packages + supertypes | 24 ms |
| **walk the unresolved set (80 699 edges)** | **325 ms** |
| `resolve_edges` total | 479 ms of a 741 ms rescan |

Two facts settle it. The cost is flat in how much changed —

| files edited | rescan | `files_changed` reported |
|---:|---:|---:|
| 0 | 243 ms | 0 |
| 1 | 737 ms | 1 |
| 32 | 752 ms | 32 |

— so a changed file costs ~0.5 ms and *anything having changed at all* costs ~494 ms. And
across three consecutive rescans the unresolved count came back `80699`, `80699`, `80699`:
the walk resolved nothing, three times, at 325 ms each. Those edges point at JDK and
library symbols that were never in the index and never will be, and every rescan retried
all of them.

The justification for the walk is real but narrower than the code: `rescan.rs` says "an
added or renamed symbol can resolve edges elsewhere without those files changing". True —
and it means the walk is needed *only when a whole-project input to `resolve_edges` moved*.
There are exactly two: the name maps built from `live_symbols`, and the supertype map built
from `extends`/`implements` edges. Everything else it reads is already scoped — the
unresolved rows themselves, and the source path each row carries. A body-only edit — the
change a `PostToolUse` hook sees after an agent edits a function body — moves neither and
paid the full 325 ms anyway.

### Fixed

`ResolveScope` splits the two cases. A rescan that moved neither input resolves only the
edges it wrote (`replace_edges_for_file` stamps them with the scan id, so the scope is
exact); anything else still walks everything.

The first version of this guard watched only the symbol table, and that was wrong: adding an
`extends` clause to an existing class moves no symbol, yet it is exactly what lets the
inherited-member tier resolve a call in a file that did not change. A three-file fixture
caught it — rescan `edges=[("exact",2),("unresolved",1)]` against a cold scan's
`[("exact",2),("heuristic",1)]`. The guard now also compares each changed file's
`extends`/`implements` hints against what it contributed before. That costs one indexed
query per changed file and nothing measurable: on spring-boot, one-file-edit rescan p95 is
**421 ms before the widening and 420 ms after** (5 runs each, same protocol, same session),
and `edges_walked` stays at 324 rather than jumping to the full 80 699 — the shortcut is
still taken.

| repo | before | after |
|---|---:|---:|
| spring-petclinic | 10 ms | 8 ms |
| nexus | 17 ms | 10 ms |
| tokio | 53 ms | 19 ms |
| spring-boot | 741 ms | **400 ms** |

The 151 ms that remains on spring-boot is the symbol map, the supertype map and the
unresolved select. None of it can be scoped: a changed file's edge may point anywhere in
the repository, so the lookup table has to be complete. That residue is per *process*, not
per change — which is the warm-process argument in §"What this fires", now cleanly
separated from the waste that was removable.

`docs/testing-strategy.md` §4 names `full_scan ≡ scan-then-rescan` "the central invariant …
worth more than the rest of the property suite combined". **It did not exist.** It does
now, in `crates/nexus-core/tests/incremental_equals_full.rs`, as the halves this change
turns on: the work a rescan may skip, and the work it may not. Forcing the scoped path
unconditionally fails `a_symbol_added_elsewhere_…` with `edges=[("unresolved", 2)]` and
`an_extends_clause_added_by_a_rescan_…` with `[("exact",2),("unresolved",1)]`, so the guard
bites in both directions.

The skip needs its own test, because equivalence cannot see it: a *correct* skip writes no
rows, so every equivalence test above still passes with the scope forced back to `All`, and
the optimisation could be deleted in a refactor without a single failure.
`a_body_only_rescan_does_not_re_walk_the_unresolved_backlog` asserts the skip directly,
through `RescanReport::edges_walked` — the count `resolve_edges` returns. It expects 0 on a
body-only edit to a file with no edges, in an index that still holds an unresolved backlog;
forcing `All` makes it 2 and the test fails.

## Nexus meets its own budgets — it fails only the hook budgets

Against [`performance.md`](../performance.md) §1, spring-boot at 877 KLOC sits between
the 500 KLOC and 5 MLOC columns, and every operation passes:

| operation | measured | budget 500 KLOC → 5 MLOC |
|---|---:|---|
| cold full scan | 7.0 s | < 45 s → < 8 min |
| rescan, no changes | 251 ms | < 300 ms → < 2 s |
| rescan, files changed | 741 ms → 400 ms | < 600 ms → < 2.5 s |

So this is not a performance regression. It is two documents that were written
independently and disagree: `performance.md` allows `rescan` 600 ms–2.5 s on a large
repository, and ADR-024 puts that same command behind a 200 ms hook budget. The hook
budget is stricter than the operation's own budget, and no test compared them.

## Memory

Peak RSS on spring-boot, per invocation:

| command | peak RSS |
|---|---:|
| `--version` (floor) | 5 MB |
| `rescan --quiet` | 74 MB |
| `context --task …` (seeded) | 103 MB |
| `context --session` | 114 MB |
| `context --rank lexical` | **371 MB** |

The lexical arm reads every tracked file body into one `Vec<(String, String)>`
(`engine/query.rs`, `lexical_corpus`) — no cap, no size filter, no early exit — so the
working tree is resident on the prompt path. 371 MB is a 154 MB working tree plus its
`String` overhead. This is the cost ADR-027 accepted when it chose a labelled guess over
silence; it was measured on 12-file fixtures, where it is invisible.

## What this fires

Not signal 1 of ADR-024 — p95 does *not* stay under budget. Signal 2:

> **p95 cannot be brought under 150 ms by caching.** ADR-006's daemon trigger has
> effectively fired from a direction it did not anticipate, and the answer is a warm
> process, not a slower hook.

Half-fired, and honestly so: nothing here proves caching *cannot* close the gap, because
no caching was attempted. What is established is that the per-invocation cost scales with
index size on a path that is supposed to scale with change size — which is the shape a
warm process fixes and a bigger timeout does not.

`performance.md` §10 lists the same trigger from the other side ("keep a warm in-memory
graph in the daemon rather than rebuilding per process"), against `impact` rather than
`rescan`. Both now point at one thing.

## Open

- The rescan hypothesis above is unconfirmed.
- Nothing between 868 and 11 519 files was measured; the breach point is somewhere in
  that gap and is not located.
- Every number is one machine, warm cache, single run of the suite. Cold-cache and CI
  numbers will be worse, and no CI assertion exists for any of this despite ADR-024's
  "p95 is asserted in CI, not hoped for."

## Conclusion

Hooks stay off by default. The measurement ADR-024 asked for has been taken and it did
not clear the bar.

The `resolve_edges` fix removes the part of the gap that was waste — 46 % of a spring-boot
rescan, resolving nothing — and rescan is now within an order of magnitude of flat again.
It does not clear the hook budget: 400 ms against 200 ms. What is left is per-process setup
that a warm process removes and a bigger timeout does not, which is the signal ADR-024
already named.

The prompt path moved too, and the `UserPromptSubmit` column above was measured before it —
see the next section.

## The prompt path

The `UserPromptSubmit` figures in the result table were taken before this branch rewrote
`crates/nexus-core/src/context/seeds.rs`, so they are the cost of the *old* seeding. What
changed: the per-word lookup went from `find_symbols(word, 8)` — suffix match, eight rows —
to `find_symbols_by_word(word, WORD_HIT_LIMIT)` at 200, so `token_family` judges a family
instead of a window of one.

Measured on a synthetic index built to spring-boot's size — 81 612 symbols:

| lookup | per word |
|---|---:|
| `find_symbols(8)` | 47.9 ms |
| `find_symbols_by_word(200)` | 51.0 ms |

**+6.5 %.** `LIKE '%x%'` is unindexed and reads the table whatever the `LIMIT` says, so the
wider window costs rows materialized and nothing else, and the measurement agrees.

That is not the whole change, and the rest is **not measured end to end.** `token_family`
now emits up to `TOKEN_FAMILY_NAME_CAP` seeds for a word that previously emitted zero, and
every seed feeds expansion (`max_depth: 5`, no node cap). The per-word query cost is +6.5 %;
the downstream cost of seeds that did not exist before has not been timed on a real
repository. Re-run `scripts/eval/measure.sh` against spring-boot before quoting the
`UserPromptSubmit` column again.

## Re-measured, 2026-09-09 — after the seeding-reads-prose branch

That re-run. HEAD `23fa712`, release binary, same protocol, same four throwaway clones (fresh
clones, not the ones above — spring-boot's upstream moved between measurements).

| repo | files scanned | symbols | index | cold scan |
|---|---:|---:|---:|---:|
| spring-petclinic | 132 | 318 | 840 KB | 65 ms |
| nexus (self) | 2 954 | 2 518 | 3 212 KB | 311 ms |
| tokio | 868 | 8 641 | 7 368 KB | 864 ms |
| spring-boot | 11 515 | 81 955 | 88 388 KB | 7 589 ms |

nexus (self) is 2 954 files against the 339 measured before — this repository has grown
(`docs/eval/runs/`, `.superpowers/`, `graphify-out/`), not a scanning regression. spring-boot
and tokio match their earlier file/symbol counts closely; both moved upstream by a handful of
commits between measurements, which is the source of the small drift.

### Result — p95, milliseconds

| | `SessionStart` ≤400 | `PostToolUse` ≤200 | `UserPromptSubmit` ≤150 | `UserPromptSubmit` ≤150 |
|---|---:|---:|---:|---:|
| | `context --session` | `rescan --quiet` (no-op) | seeded | lexical fallback |
| spring-petclinic | 6 | 7 | 9 | 24 |
| nexus | 11 | 18 | 8 | **323** |
| tokio | 38 | 10 | 19 | 110 |
| spring-boot | 303 | **292** ✗ | **380** ✗ | **737** ✗ |

Same shape as before on the first three repositories, same three breaches on spring-boot.
`PostToolUse` and the two right-hand columns are within measurement noise of the pre-seeding
numbers on spring-petclinic and tokio — the seeding rewrite does not touch the code paths those
columns exercise on a small-to-medium index. nexus's lexical-fallback column jumped 68 ms → 323
ms; that tracks the repository's own growth to 2 954 files (`lexical_corpus` reads every
tracked file body, see below), not the seeding change — tokio, which did not grow, moved 113 ms
→ 110 ms.

**spring-boot's seeded column is the one number this measurement exists to take, and it got
worse: 302 ms → 380 ms.** That is a further breach of the 150 ms budget, which is exactly the
condition the spec's §7 named as blocking: "a further breach on spring-boot blocks the change
rather than being footnoted." Acceptance criterion 5 in
[`2026-09-09-seeding-reads-prose-design.md`](../superpowers/specs/2026-09-09-seeding-reads-prose-design.md)
required `UserPromptSubmit` "has not regressed" on spring-boot. **It regressed.**

### The natural symptom-only prompt no longer falls back to lexical, and that costs more than lexical did

The script's third `UserPromptSubmit` probe — a prompt naming no symbol, run without forcing
`--rank lexical` — is not in the table above because it is not one of the two columns the
original table tracked, and the two no longer agree closely enough to fold together:

| repo | symptom-only prompt, natural ranking |
|---|---:|
| spring-petclinic | 9 ms |
| nexus | 13 ms |
| tokio | 40 ms |
| spring-boot | **1 016 ms** |

On spring-boot this is worse than the *forced-lexical* column (737 ms) and worse than the
seeded column (380 ms). The per-run output confirms why: both spring-boot probes report
`engine`, not "no symbol anchored" — before this branch, a prompt naming no symbol reliably
found nothing to seed and fell through to the lexical path (which is presumably what the
original 694 ms "lexical fallback" figure measured in practice). C2's cap now admits far more
names per prose word on an 81 955-symbol index (the cap itself is `max(6, ⌈0.01×symbols⌉)` =
820 here), so an ordinary symptom sentence now seeds broadly instead of seeding nothing, and
`max_depth: 5` expansion off a wide seed set costs more than one linear pass over the working
tree. This is precisely the downstream cost the previous section flagged as unmeasured
("`token_family` now emits up to `TOKEN_FAMILY_NAME_CAP` seeds for a word that previously
emitted zero... the downstream cost... has not been timed on a real repository"). It is now
timed, and on the repository the budget is actually failing on, admitting more is slower than
admitting nothing.

### Conclusion of the re-measurement

Two of spring-boot's three `UserPromptSubmit` readings are worse than before this branch, not
comparable-within-noise: the seeded path (+26 %) and the natural path (which stopped being a
lexical fallback and got 46 % slower than the forced-lexical figure it used to resemble).
`PostToolUse` did not move outside noise. This is a second, independent finding alongside the
retrieval gate in [`seeding-gate.md`](seeding-gate.md): even setting recall aside, this branch
does not clear the latency bar the spec set for itself. Hooks were already off by default per
ADR-024's condition never having been met; nothing here reopens that question, and this branch
gives it one more reason to stay closed.
