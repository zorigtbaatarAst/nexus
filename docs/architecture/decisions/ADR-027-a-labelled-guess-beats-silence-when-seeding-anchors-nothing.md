# ADR-027 — A labelled guess beats silence when seeding anchors nothing

**Status:** Accepted (2026-09-06)

Supersedes the zero-seed policy stated in
[`05-context-engine.md`](../05-context-engine.md) §4 and in the `context::seeds` module
documentation. Both are rewritten to describe what ships; this record is why.

## Why it is needed

Seeding anchors on tokens that name something indexed. A prompt written the way developers
actually describe problems — by the symptom, naming no symbol — contains no such token.
"The idempotency key column is too short" cannot reach the field `idempotency_key`, because
nothing joins two prose words into one identifier. When nothing anchors, expansion over an
empty seed set reaches nothing, and the package is empty.

That was the documented, deliberate answer: an empty package plus "I could not anchor this to
the code" was held better than a package built from nothing, which sends an agent confidently
into the wrong module.

The cost of that answer was then measured, and it was higher than the argument assumed:

- **Two of the five Tier 2 benchmark prompts produced `considered 0 · included 0`** — a whole
  arm of the benchmark answering nothing, while the lexical control arm answered on the same
  two prompts.
- **`crates/nexus-core/tests/golden/debug_supply.json` recorded the same failure from another
  angle.** Across three bugs planted and described by symptom, the package reached **none** of
  the files a fix had to touch.

Neither number needs a sweep, and neither has to be taken on trust. Both are reproducible
offline for free. For each task, resolve its repository and start commit, scan a copy, and ask
for the package the per-prompt hook would have asked for:

```sh
read -r REPO COMMIT PROMPT < <(scripts/eval/task_lookup.py A1-idempotency-key-length)
cp -r "target/fixtures/$REPO" /tmp/t && git -C /tmp/t checkout "$COMMIT" && rm -rf /tmp/t/.nexus
nexus --project /tmp/t scan
nexus --project /tmp/t context --task "$PROMPT" --budget 4000 --brief                 # A1
nexus --project /tmp/t context --task "$PROMPT" --budget 4000 --brief --rank lexical   # A5
```

Bytes returned by the product arm, at each task's own start commit:

| task | before | after | lexical arm |
|---|---|---|---|
| `A1-idempotency-key-length` | **0** | 893 | 893 |
| `A2-shared-type-change` | 646 | 646 | — |
| `B1-rename-crosses-the-seam` | 1,169 | 1,169 | — |
| `B2-orphaned-field-diagnosis` | **0** | 942 | 942 |
| `C1-regression-recognised` | 287 | 287 | — |

The three that already anchored are byte-identical, which is the trigger being narrow rather
than a claim about it. On the two that did not, the product arm and the lexical arm now agree
exactly, because on those prompts the product *is* the lexical ranker — a tie by construction,
which is what §"Costs" below means about the benchmark's ranking comparison.
[`docs/eval/tier2.md`](../../eval/tier2.md) §"Pre-sweep measurement" records the same table with
the arms' full hook arguments. `debug_supply` reproduces with
`cargo test -p nexus-core --test debug_supply`.

The argument for silence was never wrong. It was made without a third option on the table:
there was no cheap way to supply *something* relevant when the graph could not anchor. The
lexical ranker built for the benchmark's control arm is that option, and it already exists.

## Decision

**When seed resolution anchors nothing, `Engine::task_package` ranks file contents with BM25
instead of returning an empty package — and labels every part of the result as a guess.**

| Property | Value |
|---|---|
| Trigger | `seeded.seeds.is_empty()` — nothing else |
| Gate | ≥ 2 discriminating query terms present in the corpus |
| Discriminating | not a stopword, ≥ 4 characters (`seeds::is_noise_word`), and in fewer than half the files |
| Ranker | `lexical_package` verbatim — same corpus, same budget, same `fill`/`finish` |
| Package notes | seeding's own "no seed" note, carried through unchanged |
| `basis.selection` | `no symbol anchored: bm25 over file contents, in rank order` |
| Item `why` | `bm25 <score>` |
| Intent | whatever the turn resolved to, `Unknown` included |

Four things are load-bearing:

- **The trigger is "seeding anchored nothing", not "the package came back thin".** Every
  candidate on the graph path comes from the seeds, from what expansion reached from them, or
  from facts about them, and `facts_for_seeds` returns nothing for an empty seed set. So the
  fallback replaces packages that were already empty and no others — which is what makes any
  regression here attributable to this change rather than argued about.

- **The original reasoning survives, as the reason the disclosure survives.** A package built
  from nothing *would* send an agent confidently into the wrong module. That is exactly why
  seeding's note travels with the fallback, why the selection basis says "no symbol anchored"
  rather than naming a graph it did not walk, and why every item says `bm25`. The agent is
  told this is a guess and can weight it accordingly. What changed is the alternative, not the
  honesty requirement.

- **The fallback sits *below* the intent stage.** An unanchored turn resolves to
  `Intent::Unknown`, and reporting that is the package saying it does not know. The first
  version returned above the intent stage and dropped it;
  `an_unanchored_turn_..._reports_that_it_does_not_know` refused that version, correctly — it
  traded one honesty signal for another instead of keeping both.

- **One ranker, not two.** `lexical_package` is reused verbatim rather than reimplemented, so
  the product arm and the benchmark's control arm are genuinely identical on a zero-seed task —
  a clean tie rather than a comparison of two serialisers that have drifted apart.

**The measured result.** `debug_supply` moves from 0 of 3 to 1 of 3. The other two are honest
misses, and it matters which kind each is: the `next-storefront` bug anchored something already
(one item), so the fallback never fires — the trigger is narrow and the golden shows it. The
`spring-payments` c3 bug fails the gate, because `customer`, `charged`, `twice` and `order`
occur in **zero** files of that corpus. An ungated version did return `PaymentService.java` for
it, on stopword matches alone; the gate removed a coincidence, not a win.

## Alternatives considered

**Widen the seeding rule instead.** Split compound identifiers so "idempotency key" reaches
`idempotency_key`, relax the unique-exact-match requirement, or lower the four-character floor.
All plausible, all deferred. Rejected *here* because doing both at once makes a movement in
`debug_supply` unattributable to either change, and the golden's whole value is that a movement
in it can be explained bug by bug.

**Blend both rankers on every request.** Rejected: it changes every package the product
produces, not just the empty ones, and it makes the Tier 2 ranking comparison incoherent —
there would no longer be two arms to compare.

**Give `Purpose::Debug` its own seeding behaviour.** Declaring a debug purpose currently affects
expansion direction and ranking weights and contributes nothing to seeding. Whether it should is
a real question and a separate decision; it remains open.

**Keep the empty package and improve the note.** The cheapest option, and it is what the
original decision already does. Rejected on the evidence above: the note was already honest, and
honesty about having nothing is still nothing.

## Costs

- **A lexical ranker lives inside a graph-based context engine**, which reads like an accident
  to anyone who finds it without this record. That is the cost this ADR is paying down.

- **The gate is a threshold, and thresholds have a ceiling.** Two corroborating terms is the
  bar. A real symptom that shares only one distinctive word with the corpus still gets nothing,
  and that is a deliberate trade rather than an oversight: this path runs on the
  `UserPromptSubmit` hook, on every prompt of every session. Ungated, the fallback charged
  147–368 bytes for "thanks, that works" and "park it, decision kept" — exactly the cost
  `--brief` exists to hold at zero. `crates/nexus-cli/tests/golden/overhead.json` is the guard
  that caught it and the guard that keeps it caught.

- **Document frequency, not a score cutoff.** BM25 scores scale with corpus size and term
  frequency, so a threshold calibrated on an 11-file fixture would mean nothing anywhere else.
  "In fewer than half the files" means the same thing on every repository — but it is a
  heuristic, and on a corpus where the interesting term genuinely appears in most files it
  excludes the one word that mattered.

- **The benchmark's zero-injection counter no longer measures what its name says.** The
  condition it counted — the graph anchored nothing — is exactly as frequent as before; only
  the visible symptom is gone. It is repurposed to count fallback packages, and a sweep that
  looks healthy while every package came from the fallback is the failure mode to watch for.

## The signal that should make you change it

1. **A sweep shows most packages coming from the fallback.** Then seeding, not the fallback, is
   what needs fixing: widen the seeding rule (the alternative deferred above) rather than
   leaning harder on BM25.

2. **A graded fallback package sends an agent to the wrong file often enough to cost more than
   the empty package did.** The original reasoning would then have been vindicated by
   measurement, and the answer is to raise the gate or restore silence — the labels were never
   the point on their own.

3. **The gate's two-term bar is shown to be where real symptoms die.** If prompts that a human
   would call clearly on-topic keep landing at one discriminating term, the bar is wrong, not
   the fallback. Lower it *and* re-measure `overhead.json`, in that order; the two move
   together.

4. **The hook's p95 rises because of the corpus read.** `lexical_corpus` reads file contents on
   a path ADR-024 budgets 150 ms for. If that becomes the reason hooks get disabled, the
   fallback needs a cached corpus or it needs to stop running on `UserPromptSubmit`.
