# Ambiguous Exact Names Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a prose word that is the exact name of several symbols seed all of them at a discounted grade, instead of seeding nothing at all.

**Architecture:** One new `SeedStrength` variant, one changed match arm in `crates/nexus-core/src/context/seeds.rs`, bounded by a constant so a language idiom cannot flood the package. Then the free oracle decides whether a paid benchmark follows.

**Tech Stack:** Rust 1.82+, `nexus-core`, the Tier 1 oracle (`make tier1-retrieval`, `make tier1-rebaseline`), `scripts/eval/measure.sh`.

**Spec:** [`docs/superpowers/specs/2026-09-09-ambiguous-exact-names-design.md`](../specs/2026-09-09-ambiguous-exact-names-design.md)

## Global Constraints

- **Rust 1.82+.** CI runs with `RUSTFLAGS=-D warnings`; any warning fails the build.
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- **Do not add entries to `STOPWORDS`.** `drop` and `poll` are excluded by count, not by a hand-maintained list.
- **Do not touch the token-family cap** (`TOKEN_FAMILY_NAME_CAP = 6`). Reopening it is what was reverted last cycle.
- **Do not touch the other two match arms** — the unique-match arm and the token-family fallback are out of scope.
- **`crates/nexus-core/tests/golden/debug_supply.json` and the `golden_packages.rs` assertions must not move.** If they do, report exactly how and stop.
- **The retrieval baseline may be re-recorded only if the ratchet passes.** Current committed values: R1 `1.0`, R2 `0.0`, R3 `0.0`, R4 `0.0`, R5 `1.0`. The ratchet now also fails on a materially worse rank or a materially larger package, not just on recall.
- **The rebaseline command is `make tier1-rebaseline`.** `NEXUS_REBASELINE=1 make tier1-retrieval` is refused by a guard.
- Existing shape: `offer(&mut found, symbol, SeedSource, SeedStrength, why: String)`.

---

### Task 1: The `ProseAmbiguous` grade

**Files:**

- Modify: `crates/nexus-core/src/context/seeds.rs:131-161` — the `SeedStrength` enum and its `weight()`

**Interfaces:**

- Consumes: nothing.
- Produces: `SeedStrength::ProseAmbiguous`, weight `0.45`, ordered between `ProseToken` and `ProseExact`. Used by Task 2.

- [ ] **Step 1: Write the failing test**

Add to the test module in `seeds.rs` (find it with `grep -n "mod tests" crates/nexus-core/src/context/seeds.rs`):

```rust
/// Ambiguity is a discount, not a disqualification. A word that is the exact name of three
/// symbols is evidence about all three — weaker than a unique exact match because it cannot
/// say which, but stronger than a word that merely appears as one token inside other names.
#[test]
fn seed_strength_puts_an_ambiguous_exact_name_between_token_and_unique() {
    assert!(SeedStrength::ProseToken < SeedStrength::ProseAmbiguous);
    assert!(SeedStrength::ProseAmbiguous < SeedStrength::ProseExact);
    assert!(SeedStrength::ProseExact < SeedStrength::CodeShape);

    assert_eq!(SeedStrength::ProseAmbiguous.weight(), 0.45);
    assert!(SeedStrength::ProseToken.weight() < SeedStrength::ProseAmbiguous.weight());
    assert!(SeedStrength::ProseAmbiguous.weight() < SeedStrength::ProseExact.weight());
}
```

The `Ord` assertions matter as much as the weights: the merge rule in `offer` keeps the stronger grade with `.max()`, so a wrong declaration order would silently downgrade a seed that deserved better.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p nexus-core --lib seed_strength_puts_an_ambiguous`
Expected: FAIL to compile — `no variant named ProseAmbiguous`.

- [ ] **Step 3: Implement**

Add the variant to `SeedStrength` **between `ProseToken` and `ProseExact`** — the enum derives `Ord`, so declaration order is the ordering:

```rust
    /// A prose word that is the exact name of several symbols. The word cannot say which, so
    /// it is discounted below a unique match — but it is evidence about every one of them,
    /// and treating that as no evidence at all is what left `semaphore` (three matches) and
    /// `framed` (two) contributing nothing to the prompts they were the subject of.
    ProseAmbiguous,
```

and its weight, in the same order:

```rust
            SeedStrength::ProseAmbiguous => 0.45,
```

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p nexus-core --lib seed_strength_puts_an_ambiguous`
Expected: PASS.

- [ ] **Step 5: Confirm nothing else moved**

Run: `cargo test -p nexus-core`
Expected: all green. Adding an unused variant must not change any golden. If `debug_supply.json`, `retrieval_tokio.json` or `golden_packages.rs` move here, stop and report — nothing offers this grade yet, so a movement means the enum is being serialised somewhere unexpected.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/nexus-core/src/context/seeds.rs
git commit -m "feat(seeds): a grade for a word that names several symbols"
```

---

### Task 2: The ambiguous arm seeds, bounded

**Files:**

- Modify: `crates/nexus-core/src/context/seeds.rs` — the `match exactly_named(...)` in `resolve()`'s plain-word branch, and a new constant beside `TOKEN_FAMILY_NAME_CAP`

**Interfaces:**

- Consumes: `SeedStrength::ProseAmbiguous` from Task 1.
- Produces: `AMBIGUOUS_NAME_CAP: usize = 4`; the arm's new behaviour, measured by Task 4.

- [ ] **Step 1: Write the failing test**

The existing integration tests for seeding live in `crates/nexus-core/tests/symptom_seeds.rs`. Read it first (`grep -n "fn \|fixture\|scanned" crates/nexus-core/tests/symptom_seeds.rs | head -30`) to see which fixture helper the neighbouring tests use, and follow it — do not build a new fixture.

Add a test asserting both halves of the rule:

```rust
/// A word naming two or three symbols seeds all of them; a word naming a language idiom's
/// every definition seeds none.
///
/// Both halves are the requirement. Without the first, `semaphore` and `framed` contribute
/// nothing to the prompts they are the subject of. Without the second, `drop` — the exact
/// name of 104 symbols in tokio — would seed all of them and expand from each, inside a hook
/// with a 150 ms budget.
#[test]
fn an_ambiguous_exact_name_seeds_every_match_up_to_the_cap() {
    // Use the same fixture helper the neighbouring tests use. Pick a word that is the exact
    // name of 2-4 symbols in that fixture and one that names more than 4; if the fixture has
    // no such pair, say so in your report rather than inventing one.
    let seeds = seeds_for("<a prompt containing the ambiguous word>");

    let graded: Vec<_> = seeds
        .iter()
        .filter(|s| s.strength == SeedStrength::ProseAmbiguous)
        .collect();
    assert!(
        graded.len() >= 2,
        "every match must seed, not one of them: {seeds:?}"
    );
    assert!(
        graded.iter().all(|s| s.why.contains("names")),
        "the explanation must say the name was ambiguous: {graded:?}"
    );

    let over_cap = seeds_for("<a prompt containing the over-cap word>");
    assert!(
        !over_cap.iter().any(|s| s.strength == SeedStrength::ProseAmbiguous),
        "a name shared by more than the cap must seed nothing: {over_cap:?}"
    );
}
```

Replace the two bracketed prompts and `seeds_for` with the fixture's real helper and real words. **If the generated fixture has no word that is the exact name of more than four symbols**, assert the over-cap half as a unit test on the cap logic instead, and say so in your report — do not skip it.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p nexus-core --test symptom_seeds an_ambiguous_exact_name`
Expected: FAIL — no seed carries `ProseAmbiguous`, because the arm still returns empty.

- [ ] **Step 3: Add the cap**

Beside `TOKEN_FAMILY_NAME_CAP` in `seeds.rs`:

```rust
/// How many symbols may share an exact name before the word is an idiom rather than a subject.
///
/// Measured in tokio: `drop` is the exact name of 104 symbols, `poll` 90, `poll_next` 58,
/// `from` 42 — trait-method idioms, every one. Against a smooth distribution (431 words name
/// two symbols, 144 name three, 63 name four), so there is no knee to appeal to and this
/// number is chosen partly because it admits `semaphore` (3) and `framed` (2). That is fitting
/// a constant to its examples and is recorded as such in the spec rather than dressed up.
///
/// What guards it is not the number but the instrument: the retrieval ratchet now fails on a
/// materially worse rank or a materially larger package, which is what a too-generous cap
/// would produce and what it failed to catch when a previous cap change shipped.
const AMBIGUOUS_NAME_CAP: usize = 4;
```

- [ ] **Step 4: Change the arm**

The match currently reads `match exactly_named(&hits, target).as_slice()`. Bind the vector first so the arm can use its length, then replace the empty arm:

```rust
            let named = exactly_named(&hits, target);
            match named.as_slice() {
```

and, in place of `[_, _, ..] => {}`:

```rust
                // Several symbols are actually called this, so the word cannot say which —
                // but it is evidence about all of them, and treating that as no evidence is
                // what left `semaphore` (three matches) and `framed` (two) contributing
                // nothing to the prompts they were the subject of. Discounted below a unique
                // match, and bounded: seeding every definition of `drop` would flood the
                // package and the hook's latency budget alike.
                [_, _, ..] if named.len() <= AMBIGUOUS_NAME_CAP => {
                    for s in &named {
                        offer(
                            &mut found,
                            (*s).clone(),
                            SeedSource::NameMatch,
                            SeedStrength::ProseAmbiguous,
                            format!(
                                "'{target}' names {} symbols and this is one of them",
                                named.len()
                            ),
                        );
                    }
                }
                // Above the cap the word is an idiom, not a subject. Recorded rather than
                // dropped in silence: an omission nobody can see is the failure the inclusion
                // ledger exists to prevent.
                [_, _, ..] => notes.push(format!(
                    "'{target}' is the exact name of {} symbols, too many to tell apart",
                    named.len()
                )),
```

**Check that `notes` is in scope at this point** — it is a `Vec<String>` used elsewhere in `resolve`. If it is not reachable inside this loop, say so in your report and record the exclusion by whatever mechanism the surrounding code already uses; do not thread a new parameter without saying why.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-core`
Expected: the new test passes. `debug_supply.json` and `golden_packages.rs` must not move — if they do, stop and report exactly how.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/nexus-core/src/context/seeds.rs crates/nexus-core/tests/symptom_seeds.rs
git commit -m "fix(seeds): an ambiguous name is weak evidence, not none"
```

---

### Task 3: `measure.sh` records the prompts it measures

The latency harness takes its prompts from the environment and records none of them, so two of the three tables in `docs/eval/hook-latency.md` compare nothing to nothing. That defect produced a spring-boot regression claim last cycle that did not survive re-measurement. The next task's numbers are worthless until this is fixed.

**Files:**

- Modify: `scripts/eval/measure.sh`
- Modify: `docs/eval/hook-latency.md` — the §Method section

**Interfaces:**

- Consumes: nothing.
- Produces: a recorded prompt alongside every measurement, used by Task 4.

- [ ] **Step 1: Read what it does now**

Run: `cat scripts/eval/measure.sh`

Identify exactly where each prompt comes from — an environment variable, a default, or a literal — and where the script writes its results. Write both into your report before changing anything.

- [ ] **Step 2: Record the prompts with the numbers**

Change the script so every run it emits carries the exact prompt text it measured, in the same output the numbers appear in. Two properties matter and you should satisfy both:

- A reader of the output can tell which prompt produced which number.
- Two runs can be compared, which means it must be visible when they used _different_ prompts.

Follow the script's existing output conventions rather than inventing a format.

- [ ] **Step 3: Prove it round-trips**

Run the script against the smallest repository the document names — `spring-petclinic`, per §Method — and confirm the output names the prompt it used. Paste that output into your report. If the script needs a repository you do not have, clone it as §Method describes.

- [ ] **Step 4: Update §Method**

`docs/eval/hook-latency.md`'s §Method describes the protocol. Add one sentence saying that the prompt is recorded with the measurement and why: two of that document's own tables are uncomparable because it was not.

- [ ] **Step 5: Commit**

```bash
git add scripts/eval/measure.sh docs/eval/hook-latency.md
git commit -m "fix(eval): a latency number means nothing without the prompt that produced it"
```

---

### Task 4: Measure, then rule on the gate

**Files:**

- Modify: `crates/nexus-core/tests/golden/retrieval_tokio.json` (only if the ratchet passes)
- Modify: `docs/eval/hook-latency.md`
- Create: `docs/eval/ambiguous-names-gate.md`

**Interfaces:**

- Consumes: Task 2's behaviour, Task 3's prompt recording.
- Produces: the gate verdict.

- [ ] **Step 1: Read the oracle**

Run: `NEXUS_TIER1_REQUIRED=1 cargo test --locked -p nexus-core --test debug_supply the_task_package_reaches -- --nocapture`

The ratchet permits a rise or a hold and fails on a fall — including a materially worse rank or a materially larger package, not only lower recall. If it fails, paste the text verbatim into your report and stop. Do not re-baseline past a fall.

- [ ] **Step 2: Record, and read the whole diff**

Run:

```bash
make tier1-rebaseline
git diff crates/nexus-core/tests/golden/retrieval_tokio.json
```

Copy the complete diff into your report. Read every line: recall per task, which sites were found and at what rank, `items_included`, `tokens_estimated`. State plainly whether any package grew without its recall improving — that is the harm pattern a previous cap change produced.

- [ ] **Step 3: Re-measure latency**

Follow `docs/eval/hook-latency.md` §Method on the four repositories it names, now with Task 3's prompt recording in place. Add the figures as a new dated table beside the existing ones — **do not overwrite them**.

`UserPromptSubmit` already breaches its 150 ms budget on spring-boot, and this change adds up to four extra seeds per ambiguous word, each expanding. If spring-boot is impractical to measure in your environment, record it as **unverified** and say so — do not present the other three as if they cleared the risk.

- [ ] **Step 4: Rule on the gate**

The condition, fixed in the spec before any of this was implemented:

> The Tier 2 sweep is re-run only if site recall rises on at least **2 of the 3** tasks currently at `0.0` — R2, R3, R4 — with R1 and R5 holding at `1.0`.

Count the tasks that rose. Do not reinterpret the condition.

- [ ] **Step 5: Write the verdict**

Create `docs/eval/ambiguous-names-gate.md`: before and after recall per task, the count that rose, the gate verdict, the latency comparison, and what the change actually did. Match the voice of `docs/eval/seeding-gate.md` — dense, specific, willing to say plainly what failed.

**If nothing moved, say so and say what it means.** The spec predicts this outcome is informative: for R3, neither the ambiguous matches nor the token family contains a symbol in the required file, so the target is reachable only by expansion. A null result here is evidence that **the lever is expansion, not seeding** — which is worth more than a marginal win, and should be written up as the finding rather than as a disappointment.

- [ ] **Step 6: Verify and commit**

```bash
make check
make tier1-retrieval
git add crates/nexus-core/tests/golden/retrieval_tokio.json docs/eval/hook-latency.md docs/eval/ambiguous-names-gate.md
git commit -m "eval(seeds): what the ambiguous-name grade moved, and whether it earns a sweep"
```

---

## Self-Review

**Spec coverage.** §3's new grade → Task 1; the arm change and the cap → Task 2. §4's justification for the constant is carried into the constant's own doc comment in Task 2 Step 3. §6's restated gate → Task 4 Step 4, quoted verbatim. §7's risks: the nothing-lands-in-the-target risk is Task 4 Step 5's explicit instruction to write a null result up as a finding; the untested 0.45 is visible in `--explain` and cheap to move; the latency risk → Task 3 (the precondition) and Task 4 Step 3. §8 acceptance: 1 → Task 1, 2 and 3 → Task 2, 4 → the ratchet in Task 4 Step 1, 5 → Task 4 Step 4, 6 → Tasks 3 and 4 Step 3, 7 → Task 4 Step 6.

**Placeholder scan.** Tasks 1, 3 and 4 carry complete commands and code. **Task 2's Step 1 test carries two bracketed placeholders** — the fixture's helper and the words it holds — because which words are ambiguous in the _generated_ fixture cannot be known without reading it, and the measured counts in this plan are tokio's. The step says so, names what to substitute, and gives an explicit fallback if the fixture has no over-cap word. This is disclosed, not hidden.

**Type consistency.** `SeedStrength::ProseAmbiguous` (Task 1) is offered in Task 2 through the existing `offer(&mut found, symbol, SeedSource, SeedStrength, String)` shape, matching the neighbouring token-family call. `AMBIGUOUS_NAME_CAP: usize` is compared against `named.len()`, also `usize`. `named` is the bound result of `exactly_named(&hits, target)`, whose elements are references — hence `(*s).clone()`, matching the existing unique-match arm's `(*only).clone()`.
