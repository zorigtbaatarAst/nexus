# Tier 2 — the result

Run `20260907T093144Z`, started 2026-09-07 and finished 2026-09-09 across two sessions.
75 paid runs, $28.88, claude-opus-5, on the tokio corpus.
Full analyser output: [`runs/20260907T093144Z/REPORT.md`](runs/20260907T093144Z/REPORT.md).

**Both pre-registered thresholds miss — by roughly an order of magnitude, not a hair. What is
new is that they were measurable at all: the arms separated, in the direction the design
predicted.** The run this replaces could not say that much.

## The two thresholds

**T4** — median CPS reduction ≥ 30 % (A1 vs A0), bootstrap 95 % CI excluding zero.
Observed **2.1 %**, CI [−7.1 %, +13.7 %], favourable on 3 of 5 tasks. **Fails.**

**T7** — A1 CPS < A5 CPS, sign test p < 0.10. Observed **14.0 %** median reduction,
CI [−17.9 %, +19.5 %], favourable on 3 of 5, **p = 0.50**. **Fails.**

Both figures are token CPS, the pre-registered unit. §"Cache reads are counted at full weight"
in [`tier2.md`](tier2.md) named this as the one place the two units could plausibly disagree
about a verdict, so the dollar column is reported rather than left to the reader: pricing a
cache read at its real ~0.1× moves both numbers in A1's favour — T4 to 3.6 %, T7 to 13.1 % at
4 of 5 and p = 0.1875 — and neither across its threshold. The units disagree about details and
agree about the answer.

## The corpus separated the arms

| arm | CPS (tokens) | CPS ($) | median package | passed |
|---|---:|---:|---:|---:|
| A0 — bare agent | 291,299 | $0.3822 | — (no hooks) | 25/25 |
| A1 — full Nexus | **278,841** | **$0.3666** | 2,582 B | 25/25 |
| A5 — BM25 control | 297,428 | $0.4065 | 7,319 B | 25/25 |

A1 is the cheapest arm on both units, carrying a package roughly a third the size of the
control's. On the generated corpus A0 and A1 were tied within 1 %, because an agent that reads
an 8 kB repository in full spends the same tokens whether or not something handed it a package
first. At 868 files the arms differ. That is precisely what
[`tier2-corpus-verdict.md`](tier2-corpus-verdict.md) said no amount of task tuning could fix
and only a change of scale could, and it is the one claim this run establishes cleanly.

The aggregate is also the most flattering framing available, and it is 4.1 % against the bare
agent. Nothing here is close to 30 %.

## Per task, and where it inverts

Token CPS, five reps each:

| task | A0 | A1 | A5 | A1 vs A0 | A1 vs A5 |
|---|---:|---:|---:|---:|---:|
| R1 stream-map `size_hint` | 164,401 | 176,097 | 216,251 | −7.1 % | +18.6 % |
| R2 lines-codec invalid UTF-8 | 253,910 | 253,953 | 246,618 | −0.0 % | −3.0 % |
| R3 framed spurious decode | 444,201 | 383,522 | 325,329 | **+13.7 %** | **−17.9 %** |
| R4 abstract-socket leading NUL | 306,422 | 299,001 | 371,271 | +2.4 % | **+19.5 %** |
| R5 semaphore reopens after forget | 287,562 | 281,631 | 327,668 | +2.1 % | +14.0 % |

Three things the aggregate hides:

- **R3 is A1's best task against the bare agent and its worst against BM25.** The same package
  that saved 13.7 % over A0 cost 17.9 % against a lexical ranker on the same prompt. Whatever
  R3 needs, BM25 over file contents finds it more cheaply than the graph does — one task, so
  this is a lead, not a finding.
- **R4 is where the design predicted the effect, and the effect is there.** It is the task with
  no useful backtrace and two call sites to find across 868 files;
  the corpus ruling argued, before any A1 or A5 number existed, that the corpus was worth
  sweeping on exactly this evidence. A1 beat the lexical control by
  19.5 % there, its largest margin, on an 8,243-byte package that was byte-identical across all
  five reps. In dollars the same cell reads +9.1 % against A0 and +23.5 % against A5.
- **R2 is where A1 injected the wrong thing**, below.

## R2: three seeds, all of them wrong

A1's entire package for `R2-lines-codec-invalid-utf8`:

| symbol | why it was seeded |
|---|---|
| `tokio::time::error::Error#invalid` | the word **"invalid"** in the prompt |
| `resume_lets_time_move_forward_instead_of_resetting_it` | the word **"instead"**, inside a test name in `tokio/tests/test_clock.rs` |
| `tokio::time::error::Error` | depth-1 expansion off the first seed |

The task is a decoder bug in `tokio-util/src/codec/lines_codec.rs`. Nothing from that file was
injected. Two of the three seeds are ordinary English words that happen to occur inside
unrelated Rust identifiers, and the ≥ 4-character floor at
`crates/nexus-core/src/context/seeds.rs:62` does not catch them — "invalid" and
"instead" are seven characters each.

This is the same seeding heuristic that produced the empty packages of the previous sweep,
presenting differently: not an absent package but a confident and wrong one. R2 is the task
where A1 is indistinguishable from A0 (−0.0 %) and loses to BM25 (−3.0 %), which is what a
package with no bearing on the prompt should look like.

## Correctness measured nothing, and that was known in advance

| arm | runs | passed |
|---|---:|---:|
| A0 — bare agent | 25 | 25 |
| A1 — full Nexus | 25 | 25 |
| A5 — BM25 control | 25 | 25 |

75 of 75, no infra drops, no adjudicator flags, no false-done. The pre-registered ≥ 2/5 A0
failure gate would have dropped all five tasks. The ruling that authorised this sweep was
taken on A0 data alone, before any A1 or A5 number existed: that gate is a *sufficient*
condition for a discriminating corpus, not a necessary one. T4 and T7 are cost thresholds,
computable at a 100 % pass rate, where the pass rate enters only as the denominator. That is
why this sweep ran, and the pass rate is stated here as a property of the corpus rather than
a gate quietly dropped.

tokio is also in the model's training data. Contamination inflates every arm, which is
survivable for the relative comparison this run reports and fatal for any absolute pass rate.
No absolute rate here means anything.

## The instrument held

The two defects that invalidated `20260906T131608Z` did not recur.

- **0 empty packages and 0 lexical-fallback packages** out of A1's 24. On the generated corpus
  2 of 7 prompts anchored nothing; on tokio the graph anchored every prompt it saw. The old
  failure was corpus scale, not the ranker.
- **Provenance is checked by content, not by version string.** `meta.json` pins the image id and
  the sha256 of every artifact the image bakes from the tree, read out of the image. This run
  was interrupted at 28 of 75 cells and resumed two days later against the same pinned image
  (`sha256:8ef914a0…`, `nexus` `sha256:77c2f94c…`); the resume guard compared both before
  spending. Cells 1–28 and 29–75 ran the same binary, and that is checkable rather than
  asserted.

## One run injected nothing, and no count shows it

`R2-lines-codec-invalid-utf8/A1/3` logged 169 bytes: the SessionStart block and no prompt
injection at all. Setup was clean (`nexus scan`, 784 files, 7,616 symbols,
exit 0), stderr empty. The other four reps of that cell injected 725 bytes each.

The analyser reports A1's injection statistics over **24** packages against 25 runs, and because
the run produced no package it counts as neither `empty` nor `lexical`. A hook that silently
no-ops is currently invisible in both columns — the one place this report cannot see its own
blind spot. Recorded here as a defect against the harness, not against the result.

## `make bench STAMP=…` cannot resume a sweep

The Makefile documents `make bench STAMP=<stamp>` as the resume path. It is structurally unable
to resume: `bench` depends on `bench-image`, which rebuilds the image, and `make fixtures`
regenerates `target/fixtures` and so invalidates the `COPY target/fixtures /warm` layer at
`scripts/eval/Dockerfile:84` and every layer after it. The image id moves, and `sweep.sh`'s
resume guard then correctly refuses to half-mix a rebuilt image into a stamped sweep.

The second half of this run was therefore driven by `sweep.sh` directly with `IMAGE` pinned to
the stamped image — the entry point the incident write-up below blames for the stale-binary
sweep. What that incident argues against is a *stale* image being reported as HEAD; here the
pinned image's `nexus` is byte-identical to the tree's, so pinning is what preserved
provenance and rebuilding is what would have broken it. The guard was doing its job; the
documented entry point cannot satisfy it.

## What this does and does not license

- **Licensed:** on a corpus that discriminates, Nexus did not demonstrate the effect it was
  built to demonstrate. 2.1 % against a 30 % threshold is not a near miss.
- **Licensed:** the corpus change worked. Arms that were tied within 1 % now separate, and the
  separation is largest on the task designed to require search.
- **Licensed:** one concrete defect — R2's seeding anchored three unrelated symbols off common
  English words.
- **Not licensed:** "Nexus is worse than BM25." A1 beat A5 on 3 of 5 tasks and on both aggregate
  units; p = 0.50 is the absence of evidence.
- **Not licensed:** deleting the Context Engine on T7's pre-registered consequence. T7 missed,
  but A1 is the cheaper arm in aggregate and beat the control by 19.5 % on the search-heavy
  task. A falsifier that fires at p = 0.50 against a 3-of-5 split is not firing.
- **Not licensed:** quoting any absolute pass rate. See contamination, above.

## What to do next, in order

1. **Fix R2's seeding.** Common words inside identifiers should not seed. The ≥ 4-character
   floor is the wrong instrument for it; nothing here says what the right one is.
2. **Chase R3.** BM25 beat the graph by 17.9 % on one task. One task is a lead, but it is the
   only place in this run where the control is decisively better, and T7's consequence rests on
   exactly that comparison.
3. **Make the no-injection run visible** in the analyser's `empty`/`lexical` accounting.
4. **Fix the resume path** so `make bench STAMP=…` either resumes or refuses with the reason,
   rather than documenting a workflow its own rebuild defeats.
5. **More tasks before any threshold is gated on.** Five tasks cannot carry a release gate;
   `analyse.py` says so on every line, and this run does not change it.

---

# The superseded run — `20260906T131608Z`

95 runs, $37.62, on the generated corpus. Its verdict is
[`tier2-corpus-verdict.md`](tier2-corpus-verdict.md): the fixtures were 6–8 kB, a bare agent
read them whole, and T4 and T7 were never measurable there at any task difficulty. Its analyser
output survives at [`runs/20260906T131608Z/REPORT.md`](runs/20260906T131608Z/REPORT.md).

The incident write-up below is kept in place because it is the only account of how a sweep came
to be reported against a binary a day older than itself, and `scripts/eval/check_image_artifacts.sh`
cites it by name.

## Correction: the arm ran a stale binary

**The first version of this write-up said the lexical fallback's `query_overlap >= 2` gate
never cleared. That was wrong, and the truth is worse.**

The fallback did not fire because **it was not in the binary the sweep ran.**

| | |
|---|---|
| `nexus-bench:latest` built | 2026-09-05 17:11 |
| fallback landed (`0215372`) | 2026-09-06 17:42 |
| `"no symbol anchored"` in the image binary | **0 occurrences** |
| same string in the HEAD binary | 1 occurrence |
| `nexus --version`, both | `nexus 0.3.0` |

The gate is fine. Measured directly on the two failing prompts, `query_overlap` returns
**exactly 2** for both — `idempotency` (df 3) and `column` (df 1) over 12 docs, ceiling 6;
`orders` (df 6) and `total` (df 1) over 15 docs, ceiling 8. It passes, on the bar.

Three things had to line up:

1. `make bench` rebuilds the image before sweeping (`Makefile:79`). **The sweep was started
   with `bash scripts/eval/sweep.sh` directly, which does not.** That was operator error —
   mine — and `sweep.sh` prints `make bench STAMP=…` as the resume command, so the right
   entry point was on screen the whole time.
2. `sweep.sh` pins the image id so a *changing* image cannot be half-mixed into a resumed
   sweep (`sweep.sh:186`), but nothing checks the image against the tree it is reported
   against.
3. The provenance line it does stamp is the **host** binary's `--version`. The version was
   not bumped between those commits, so host and image both read `nexus 0.3.0` and the stamp
   matched a binary a day older than itself.

So `meta.json` recorded a provenance that was true of the wrong binary. A version string
cannot detect this class of drift and never could.

**What it changes.** The A1 arm in run `20260906T131608Z` is not HEAD's A1. On the two tasks
where seeding anchors nothing it injected nothing, where HEAD would have injected a BM25
package. The other five tasks are unaffected — the harness supplies its own `nexus-hook.sh`
wrapper that reads the prompt from stdin in shell, so the `--task-stdin` fix (`7778592`,
also missing from the image) did not matter to them.

**What it does not change.** A0 passed 25/25 on a binary-independent path — the control uses
no Nexus at all. The corpus failure of [`tier2-corpus-verdict.md`](tier2-corpus-verdict.md)
stands on its own and is fatal to that sweep by itself.

## Why the packages were empty (the real cause)

Seeding anchors nothing on those two prompts for three stacked reasons, none of them the gate:

- **`find_symbols` matches by suffix only** (`crates/nexus-store/src/lib.rs:1651`), so
  `idempotency` cannot reach `idempotencyKey` and `total` cannot reach `getTotalAmount`.
- **`uniquely_named_symbol` bails at arity ≥ 2** (`crates/nexus-core/src/context/seeds.rs:238`):
  `orders` ties `graphql:api:Query.orders` against `OrderController#orders()`, so B2's best
  word seeds nothing.
- **The ≥ 4-character floor** (`seeds.rs:62`) deletes `key` (A1) and `nan` (B2) before any
  lookup happens.

