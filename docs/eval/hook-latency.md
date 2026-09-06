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

| | `SessionStart` ≤400 | `PostToolUse` ≤200 | `UserPromptSubmit` ≤150 | | |
|---|---:|---:|---:|---:|---:|
| | `context --session` | `rescan --quiet` (no-op) | seeded | lexical fallback |
| spring-petclinic | 5 | 6 | 11 | 25 |
| nexus | 13 | 8 | 9 | 68 |
| tokio | 38 | 13 | 21 | 113 |
| spring-boot | 276 | **251** ✗ | **302** ✗ | **694** ✗ |

Three of four budgets breach on spring-boot. Nothing breaches below it.

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
[`performance.md`](../performance.md) §1:

> The `rescan` rows are flat in repository size on purpose: their cost is proportional to
> *what changed*, not to how much code exists.

They are not flat. Cost tracks repository size, not change size.

**Hypothesis, not yet confirmed:** §4 of the same document says edge resolution "runs
after the symbol table is complete, because resolving an FQN requires knowing every
symbol… reading an immutable in-memory FQN map built once." Built once *per scan* is
correct; built once *per rescan* makes an operation documented as O(change) into O(repo).
81 612 symbols is the right order of magnitude for the 741 ms. Confirm before fixing.

## Nexus meets its own budgets — it fails only the hook budgets

Against [`performance.md`](../performance.md) §1, spring-boot at 877 KLOC sits between
the 500 KLOC and 5 MLOC columns, and every operation passes:

| operation | measured | budget 500 KLOC → 5 MLOC |
|---|---:|---|
| cold full scan | 7.0 s | < 45 s → < 8 min |
| rescan, no changes | 251 ms | < 300 ms → < 2 s |
| rescan, files changed | 741 ms | < 600 ms → < 2.5 s |

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
