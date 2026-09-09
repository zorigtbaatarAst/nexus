# Seeding Reads Prose As Code — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the seeding stage reading a sentence's first word as an identifier and deleting the words a codebase is about, then let the free oracle decide whether a $29 sweep is warranted.

**Architecture:** Three changes to `crates/nexus-core/src/context/seeds.rs`, one supporting query in `crates/nexus-store`. Each change is measured on its own by re-running the Tier 1 retrieval oracle, so a regression is attributable to one change rather than three. No change adds a refusal: they reclassify, admit more, or reweight.

**Tech Stack:** Rust 1.82+, `nexus-core`, `nexus-store` (rusqlite), the Tier 1 oracle (`make tier1-retrieval`), `scripts/eval/measure.sh` for hook latency.

**Spec:** [`docs/superpowers/specs/2026-09-09-seeding-reads-prose-design.md`](../specs/2026-09-09-seeding-reads-prose-design.md)

## Global Constraints

- **Rust 1.82+.** CI runs with `RUSTFLAGS=-D warnings`; any warning fails the build.
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- **SQL lives only in `nexus-store`.** `crates/nexus-cli/tests/boundaries.rs` enforces this — a query written anywhere else fails that test.
- **Do not add entries to `STOPWORDS`.** Adding `invalid`/`instead` after seeing them fail is fitting the instrument to the result. C1 is what makes the existing list reach them.
- **Do not edit the oracle or its baseline to make a change look good.** `crates/nexus-core/tests/golden/retrieval_tokio.json` moves only via `NEXUS_REBASELINE=1 make tier1-retrieval` with the diff read, and only upward.
- **The oracle's R1 must not fall below `1.0`.** It is the one task the ratchet can currently catch.
- **Intent classification is out of scope.** R3/R5 classify `unknown`; that is roadmap 5.7's.
- Existing constant: `TOKEN_FAMILY_NAME_CAP = 6` (`seeds.rs:254`) is the *floor* after Task 3, not the cap.

---

### Task 1: A symbol count the seeding stage can ask for

Task 3 needs the index's size to scale a cap. No query returns it, and SQL may not be written outside `nexus-store`.

**Files:**
- Modify: `crates/nexus-store/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn count_symbols(&self, project_id: i64) -> Result<usize>` on the store type, used by Task 3.

- [ ] **Step 1: Find the surrounding conventions**

Run: `grep -n "fn find_symbols_by_word" -A 20 crates/nexus-store/src/lib.rs`

Read how that method takes `project_id`, prepares a statement, and maps rows. Your new method must match that shape — the same error type, the same `project_id` filtering, the same naming style. Note the exact `Result` alias in use.

- [ ] **Step 2: Write the failing test**

Add a test beside the store's existing tests (find them with `grep -n "mod tests" crates/nexus-store/src/lib.rs`). Follow whatever fixture helper those tests already use to get a store with symbols in it; do not invent a new one.

```rust
#[test]
fn count_symbols_counts_only_this_project() {
    // Build a store with symbols in two projects, using the same helper the
    // neighbouring tests use. Then:
    assert_eq!(store.count_symbols(project_a).unwrap(), 2);
    assert_eq!(store.count_symbols(project_b).unwrap(), 1);
    assert_eq!(store.count_symbols(empty_project).unwrap(), 0);
}
```

The two-project assertion is the point: a `SELECT count(*) FROM symbols` with no `WHERE project_id` passes a single-project test and silently returns the whole database.

- [ ] **Step 3: Run it and watch it fail**

Run: `cargo test -p nexus-store count_symbols`
Expected: FAIL — `no method named 'count_symbols'`.

- [ ] **Step 4: Implement**

```rust
/// How many symbols this project has indexed.
///
/// Seeding scales a cap by index size: a word that is a token of twenty names is a theme in a
/// forty-symbol fixture and a rare term in an eight-thousand-symbol one. The count is the
/// denominator that tells those apart.
pub fn count_symbols(&self, project_id: i64) -> Result<usize> {
    let n: i64 = self.conn.query_row(
        "SELECT count(*) FROM symbols WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}
```

Adjust `self.conn`, the `Result` alias, and the parameter style to match what Step 1 showed you. If `symbols` has a soft-delete or status column that other queries filter on, filter on it here too — and say so in your report.

- [ ] **Step 5: Run it and watch it pass**

Run: `cargo test -p nexus-store count_symbols`
Expected: PASS.

- [ ] **Step 6: Check boundaries, format, lint**

Run: `cargo test -p nexus-cli --test boundaries && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: boundaries green, clippy silent.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-store/src/lib.rs
git commit -m "feat(store): count a project's indexed symbols"
```

---

### Task 2: Code shape, not sentence position

`is_plain_word` disqualifies any word starting with a capital, on the theory it "was typed as code". Every English sentence's first word is capitalised, so every prompt's opening word skips the stopword list and the uniqueness rule.

**Files:**
- Modify: `crates/nexus-core/src/context/seeds.rs:196-204`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `fn looks_like_code(w: &str) -> bool`; `is_plain_word` keeps its signature.

- [ ] **Step 1: Write the failing test**

Add to the test module in `seeds.rs` (find it with `grep -n "mod tests" crates/nexus-core/src/context/seeds.rs`):

```rust
/// A leading capital is English orthography, not a naming convention. Every sentence has one,
/// and before this every prompt's first word was read as an identifier — skipping both the
/// stopword list and the uniqueness rule. All five tokio prompts open with a capital.
#[test]
fn a_sentence_initial_capital_is_prose_not_code() {
    // The opening word of each tokio benchmark prompt. None was typed as code.
    for w in ["After", "Summing", "Feeding", "Binding", "Rebuilding"] {
        assert!(is_plain_word(w), "{w} opens a sentence; it is prose");
    }
    // `after` is already in STOPWORDS, so once `After` is prose it must not seed at all.
    assert!(!is_ordinary_word("After"), "a capitalised stopword is still a stopword");

    // Internal evidence of a naming convention still reads as code.
    for w in ["StreamMap", "NOTIFY_AFTER", "lines_codec", "tokio::sync", "src/lib.rs"] {
        assert!(!is_plain_word(w), "{w} carries code shape");
    }

    // A single-word type name has no internal evidence and becomes prose. That is deliberate:
    // it must then prove it names exactly one symbol, and `Semaphore` names five in tokio.
    assert!(is_plain_word("Semaphore"));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p nexus-core --lib a_sentence_initial_capital`
Expected: FAIL on the first assertion — `After opens a sentence; it is prose`.

- [ ] **Step 3: Implement**

Replace the body of `is_plain_word` (currently at `seeds.rs:196-204`) and add the helper above it:

```rust
/// Internal evidence that a word was typed as code rather than written as prose.
///
/// A *leading* capital is not evidence: every English sentence starts with one, and reading it
/// as a type name is what let `After` seed `reset_after` and `NOTIFY_AFTER` on a prompt about
/// semaphores. What distinguishes an identifier is a convention *inside* the word — a second
/// capital, an interior underscore, or a path/FQN separator — and none of those depend on
/// where the word sits in a sentence.
fn looks_like_code(w: &str) -> bool {
    w.contains('.')
        || w.contains('/')
        || w.contains('#')
        || w.contains("::")
        || w.trim_matches('_').contains('_')
        || w.chars().skip(1).any(char::is_uppercase)
}

fn is_plain_word(w: &str) -> bool {
    !looks_like_code(w) && is_ordinary_word(w)
}
```

Keep the existing doc comment on `is_plain_word` — it explains why one definition serves both `targets` and `resolve`, which is still true.

- [ ] **Step 4: Run the test and the whole crate**

Run: `cargo test -p nexus-core`
Expected: the new test passes. **`golden_packages.rs` and `debug_supply.rs` may now differ — do not re-baseline either.** If they moved, record exactly how in your report and stop; a change in the generated-fixture goldens is real information about C1's blast radius and it is the controller's call, not yours.

- [ ] **Step 5: Read what the change did to the oracle**

Run: `NEXUS_TIER1_REQUIRED=1 cargo test -p nexus-core --test debug_supply the_task_package_reaches -- --nocapture`

The ratchet compares against the committed baseline (R1 `1.0`, R2–R5 `0.0`). Recall may rise; it must not fall. Record the failure text verbatim if it does.

- [ ] **Step 6: Record the per-task reading**

Run: `NEXUS_REBASELINE=1 make tier1-retrieval && git diff crates/nexus-core/tests/golden/retrieval_tokio.json`

Read every line of that diff and copy it into your report — this is C1's isolated effect and the reason each change gets its own task. Then commit the baseline **only if every task held or improved**.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/nexus-core/src/context/seeds.rs crates/nexus-core/tests/golden/retrieval_tokio.json
git commit -m "fix(seeds): a leading capital is orthography, not a naming convention"
```

---

### Task 3: The family cap becomes a fraction of the index

`TOKEN_FAMILY_NAME_CAP = 6` deletes any word that is a token of more than six names. It was calibrated on a 39-symbol fixture. tokio holds 8,470, where `semaphore` is a token of 20 names and `framed` of 11 — both deleted, while a rare accident like `After` (3) survives.

**Files:**
- Modify: `crates/nexus-core/src/context/seeds.rs` — `token_family` (~line 318-331), its call site (~line 470)

**Interfaces:**
- Consumes: `count_symbols(project_id) -> Result<usize>` from Task 1.
- Produces: `fn family_cap(symbol_count: usize) -> usize`; `token_family` gains a `cap: usize` parameter.

- [ ] **Step 1: Write the failing test**

```rust
/// The cap is a fraction of the index, floored at the value measured on `spring-payments`.
///
/// That fixture has 39 symbols, and 6 was chosen there because `idempotency` is a token of 4
/// names and `payment` — the word that would drag the whole repository in — is a token of 13.
/// Both readings must survive. tokio has 8,470 symbols, where `semaphore` (20 names) and
/// `framed` (11) are rare terms, not themes, and an absolute 6 deleted both.
#[test]
fn the_family_cap_scales_with_the_index() {
    // spring-payments: unchanged, so the calibration that produced 6 still holds.
    assert_eq!(family_cap(39), 6, "the floor preserves the measured calibration");
    assert!(13 > family_cap(39), "`payment` is still a theme at 39 symbols");
    assert!(4 <= family_cap(39), "`idempotency` still seeds at 39 symbols");

    // tokio: 1% of 8,470 is 84.7, so 85.
    assert_eq!(family_cap(8470), 85);
    assert!(20 <= family_cap(8470), "`semaphore` is a rare term at 8,470 symbols");
    assert!(11 <= family_cap(8470), "`framed` is a rare term at 8,470 symbols");

    // The floor wins whenever a fraction of the index is smaller than it.
    assert_eq!(family_cap(0), 6);
    assert_eq!(family_cap(600), 6, "1% of 600 is 6 — the boundary, not above it");
    assert_eq!(family_cap(601), 7);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p nexus-core --lib the_family_cap_scales`
Expected: FAIL — `cannot find function 'family_cap'`.

- [ ] **Step 3: Implement the cap**

Add beside `TOKEN_FAMILY_NAME_CAP`, and change that constant's doc comment to say it is now the floor rather than the cap:

```rust
/// How many names a word may be a token of before it names a *theme* rather than a family.
///
/// A fraction, not a count, because "how many names contain this word" means nothing without
/// "out of how many names". `TOKEN_FAMILY_NAME_CAP` is the floor, and it is the number measured
/// on `spring-payments` (39 symbols, `idempotency` a token of 4, `payment` of 13) — so small
/// corpora behave exactly as they did. One percent puts tokio's cap at 85, which admits
/// `semaphore` (20) and `framed` (11): rare terms in an 8,470-symbol index, and the words those
/// prompts are actually about.
fn family_cap(symbol_count: usize) -> usize {
    TOKEN_FAMILY_NAME_CAP.max((symbol_count as f64 * 0.01).ceil() as usize)
}
```

- [ ] **Step 4: Thread it through `token_family`**

Change the signature to take the cap, and the comparison to use it:

```rust
fn token_family<'a>(hits: &'a [SymbolRef], word: &str, cap: usize) -> Vec<&'a SymbolRef> {
```

and inside it replace `if family.len() > TOKEN_FAMILY_NAME_CAP {` with `if family.len() > cap {`.

At the call site (~line 470, inside the `[] =>` arm), compute the cap once before the loop over `targets` — not per word, which would run one `count_symbols` query per candidate word on the hook's critical path:

```rust
let cap = family_cap(store.count_symbols(project_id)?);
```

then pass `cap` to `token_family(&hits, target, cap)`. Put that binding next to where `targets` is built so it is obviously computed once.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-core`
Expected: the new test passes. As in Task 2, if `golden_packages.rs` or `debug_supply.rs` move, do not re-baseline — report and stop.

- [ ] **Step 6: Read the oracle and record C2's isolated effect**

Run: `NEXUS_TIER1_REQUIRED=1 make tier1-retrieval`

Then record and read the diff exactly as in Task 2 Step 6:

```bash
NEXUS_REBASELINE=1 make tier1-retrieval
git diff crates/nexus-core/tests/golden/retrieval_tokio.json
```

Copy the diff into your report. Commit the baseline only if every task held or improved.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/nexus-core/src/context/seeds.rs crates/nexus-core/tests/golden/retrieval_tokio.json
git commit -m "fix(seeds): a theme is a fraction of the index, not a fixed count"
```

---

### Task 4: Seed strength follows evidence strength

Every seed scores `seed_proximity: 1.0` — "a seed is 1.0 by definition" (`context/rank.rs:17`). So R2's prose word `invalid`, which happens to name `tokio::time::error::Error#invalid` exactly, anchors the package as strongly as a name someone typed as code.

**This task's code is specified by design rather than transcribed**, because `seed_proximity` is assigned outside `crates/nexus-core/src/context/`, and the exact seam must be read before it is changed. Step 1 is that reading. Do not guess it.

**Files:**
- Modify: `crates/nexus-core/src/context/seeds.rs` — the `Seed` struct (line 121) and the `offer` call sites
- Modify: wherever `seed_proximity` is assigned (Step 1 finds it)

**Interfaces:**
- Consumes: `is_plain_word` / `looks_like_code` from Task 2.
- Produces: a strength carried on `Seed` and multiplied into `seed_proximity`.

- [ ] **Step 1: Find the seam**

Run: `grep -rn "seed_proximity" crates/nexus-core/src/ | grep -v "context/rank.rs"`

That names the file and line where a seed becomes a ranker input. Read it and write into your report: what type carries the seed there, and whether `SeedSource` is already visible at that point. **If `SeedSource` is already available and no field needs adding, prefer weighting on `SeedSource` alone** — that is less plumbing for the same result.

- [ ] **Step 2: Write the failing test**

Write a unit test asserting the ordering, not the absolute numbers — the numbers are a tuning choice and the ordering is the requirement:

```rust
/// A seed's strength is how good the evidence was, not merely that something matched.
///
/// Three grades. A word carrying code shape was typed as an identifier. A prose word that
/// names exactly one symbol is real but weaker evidence — in R2 the prose word `invalid` named
/// `Error#invalid` exactly and anchored a lines-codec bug on `tokio/src/time/`. A prose word
/// that is merely a token of some names is weaker still.
#[test]
fn seed_strength_ranks_code_shape_above_prose() {
    assert!(seed_strength_for(/* code-shaped */) > seed_strength_for(/* prose, names exactly one */));
    assert!(seed_strength_for(/* prose, names exactly one */) > seed_strength_for(/* prose, token of a family */));
    assert!(seed_strength_for(/* prose, token of a family */) > 0.0, "weaker is not deleted");
}
```

Replace the comment placeholders with whatever the Step 1 reading showed is actually available — `SeedSource` variants, or a new field. Keep the three assertions and their ordering exactly.

- [ ] **Step 3: Run it and watch it fail**

Run: `cargo test -p nexus-core --lib seed_strength`
Expected: FAIL to compile until Step 4 exists.

- [ ] **Step 4: Implement**

Use `1.0` for code shape, `0.6` for a prose word naming exactly one symbol, and `0.3` for a prose token-family member. Multiply the strength into `seed_proximity` at the seam Step 1 found. **No seed is dropped** — the lowest grade still seeds, it merely loses rank contests.

Record in your report why you placed each `offer` call site in the grade you did.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-core`
Expected: the new test passes; the goldens are the same story as Tasks 2 and 3 — do not re-baseline on your own initiative.

- [ ] **Step 6: Read R2's package by hand — this change has no number**

Run:

```bash
cd target/fixtures/tokio && git checkout -q $(python3 -c "
import json;print([c['sha'] for c in json.load(open('../../../target/fixtures/tokio.manifest.json'))['commits'] if c['task'].startswith('R2-')][0])")
rm -rf .nexus
../../release/nexus --project . context --task "Feeding a line-delimited stream that ends in invalid UTF-8 makes the decoder panic with a slice range error on the following read instead of returning a decode error." --explain
```

Site recall is set membership — it cannot see that a wrong item ranked lower. Paste the `Why` block into your report and say plainly whether `Error#invalid` still outranks anything from `tokio-util/src/codec/`. That reading **is** this task's acceptance evidence.

- [ ] **Step 7: Format, lint, commit**

```bash
cd /opt/tools/nexus && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add -u && git commit -m "fix(seeds): weight a seed by how strong its evidence was"
```

---

### Task 5: Measure the cost, then evaluate the pre-registered gate

Two things decide whether this work ships and whether a $29 sweep follows: whether the hook still fits its budget, and whether recall rose on at least 3 of 5 tasks.

**Files:**
- Modify: `docs/eval/hook-latency.md`
- Create: `docs/eval/seeding-gate.md`

**Interfaces:**
- Consumes: the baseline at `crates/nexus-core/tests/golden/retrieval_tokio.json` after Tasks 2–4.
- Produces: a written gate verdict.

- [ ] **Step 1: Re-measure hook latency**

`UserPromptSubmit` already breached its 150 ms budget on spring-boot at 302 ms before this work, and Task 3 admits more seeds. Read `docs/eval/hook-latency.md` §Method for the exact protocol, then re-run it:

```bash
./scripts/eval/measure.sh <repo-path> <label>
```

on the same four repositories the document names — spring-petclinic, this repo, tokio, spring-boot. Build them as that document says.

- [ ] **Step 2: Record the result, and stop if spring-boot regressed**

Add the new numbers to `docs/eval/hook-latency.md` as a dated table beside the existing one — do not overwrite the old figures, they are the comparison. **A further breach of `UserPromptSubmit` on spring-boot blocks the change** per the spec's §7; if that happens, report it and stop rather than continuing to Step 3.

- [ ] **Step 3: Evaluate the gate**

The pre-registered condition, fixed before implementation:

> The Tier 2 sweep is re-run only if site recall rises on at least **3 of the 5** tokio tasks, relative to the committed baseline R1 `1.0`, R2 `0.0`, R3 `0.0`, R4 `0.0`, R5 `0.0`.

Compare the current `retrieval_tokio.json` against that baseline. Count the tasks whose recall rose. Do not reinterpret the condition.

- [ ] **Step 4: Write the verdict**

Create `docs/eval/seeding-gate.md`. State: the before and after recall per task, the count that rose, whether the gate is met, and the hook-latency comparison. If the gate is **not** met, say so plainly and state that no sweep follows — a written negative result is the correct outcome, not a failure, and the reasoning is in the spec's §6.

Include the `--explain` reading from Task 4 Step 6, since C3 has no number of its own.

- [ ] **Step 5: Verify everything**

```bash
make check
make tier1-retrieval
```
Expected: both exit 0.

- [ ] **Step 6: Commit**

```bash
git add docs/eval/hook-latency.md docs/eval/seeding-gate.md
git commit -m "eval(seeds): what the seeding fix moved, and whether it earns a sweep"
```

---

## Self-Review

**Spec coverage.** §3 C1 → Task 2; C2 → Tasks 1 and 3; C3 → Task 4. §4 non-goals are carried as Global Constraints (no `STOPWORDS` edits, no intent work, no oracle edits). §5 per-defect judgement → the isolated oracle reading in Tasks 2, 3 and 4. §6 the gate → Task 5 Steps 3-4. §7 hook-latency risk → Task 5 Steps 1-2, with the blocking condition stated; the C3-has-no-number risk → Task 4 Step 6; the boundary risk → Task 1 Step 6. §8 acceptance: 1 → Task 2 Step 1, 2 → Task 3 Step 1, 3 → the ratchet in every oracle step, 4 → Tasks 2/3/4 Step 6 and Task 5, 5 → Task 5 Step 2, 6 → Task 5 Step 5.

**Placeholder scan.** Tasks 1, 2, 3 and 5 carry complete commands and code. **Task 4 deliberately does not**, and says so at its head: `seed_proximity` is assigned outside `context/`, and its Step 1 is the reading that resolves this. Its test placeholders are marked and its three assertions are fixed. This is a known gap, disclosed rather than papered over — Task 4 should go to a stronger model than Tasks 1-3.

**Type consistency.** `count_symbols(&self, project_id: i64) -> Result<usize>` (Task 1) is called in Task 3 Step 4 with `project_id` and fed to `family_cap(symbol_count: usize) -> usize`. `token_family` gains `cap: usize` in Task 3 and is called with it at the one site. `looks_like_code(w: &str) -> bool` (Task 2) is referenced by Task 4 Step 2. `TOKEN_FAMILY_NAME_CAP` keeps its name and becomes the floor inside `family_cap`.

**One spec statement not implemented as written.** §3 C2 says the cap is `max(6, ceil(0.01 × symbol_count))`; Task 3 implements exactly that, but the 1% fraction is a literal in `family_cap` rather than a named constant. Left inline deliberately: it appears once, and the doc comment carries its derivation — a named constant here would be a second place for the same number to live.
