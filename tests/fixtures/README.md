# Fixture corpus

The benchmark repositories of [`docs/architecture/13-evaluation.md`](../../docs/architecture/13-evaluation.md)
§3, generated from declarative specifications.

```text
fixture specification  →  deterministic generator  →  git repository  →  benchmark fixture
```

```bash
make fixtures            # build the corpus into target/fixtures/
make fixtures-verify     # generate twice, prove the shas agree — the CI gate

nexus fixture list
nexus fixture generate [--fixture NAME] [--out DIR] [--force] [--emit-tasks DIR]
nexus fixture verify   [--fixture NAME]
```

Fixtures are **not committed**. They are a pure function of `tests/fixtures/specs/`, they
rebuild in under a second, and a checked-in copy would be one more thing to keep in sync. Build
them when you need them.

---

## Why generate rather than scrape

The properties the evaluation asserts do not exist in repositories you can clone:

- a commit that reformats every file and must move **zero** symbols;
- a rename that must carry a finding's identity rather than duplicate it;
- a bug planted at a known commit, fixed at another, and re-opened at a third — by a migration
  whose message is about something else;
- a plausible, well-named, entirely dead code path that a naive retriever will rank first.

Each is a behaviour *across time*, which is exactly what Nexus claims to see and nothing else
does. They have to be built on purpose.

---

## The corpus

| Fixture | Role | Commits | Carries |
|---|---|---:|---|
| `spring-payments` | payments | 7 | The full history: refactor → bug → reformat → rename → fix → regression. A dirty-start patch, a Family H decoy, and a multi-turn task |
| `next-storefront` | realistic full-stack | 3 | Spring GraphQL API and a Next.js frontend **in one repository**, joined only by a schema. Codegen output that must not be indexed |
| `acme-monorepo` | multi-service | 3 | Three Gradle modules over one shared library, plus a feature branch. Sibling-vs-external resolution and narrow-scan detection |
| `legacy-billing` | legacy / deceptive | 4 | Three plausible invoice calculators, one of them live. The harmful-context fixture |

Backend and frontend share a repository in `next-storefront` deliberately: Family B asks whether
a backend contract change reaches the components that render it, and that question only exists
inside a single indexed project.

### Relationship to `13-evaluation.md` §3

§3 names `spring-payments`, `next-storefront`, `fastapi-orders` and `cargo-ledger`. The first two
are here. `fastapi-orders` and `cargo-ledger` exist to exercise the Python and Rust analyzers,
which are Phase 5 work — generating them now would produce repositories nothing can index.

`acme-monorepo` and `legacy-billing` are additions, covering the multi-service and
deceptive-context roles that §4's families D, H and A require and that §3's table does not name.
**§3's table should gain two rows.** That is a documentation gap this work found, not a change
of plan.

---

## The specification format

A fixture is a manifest plus a directory of real source files:

```text
tests/fixtures/specs/spring-payments/
  fixture.toml     the manifest: commits, operations, patches, tasks
  blobs/           the file contents each operation writes
```

Content lives in `blobs/` as ordinary `.java`, `.ts` and `.graphqls` files rather than inside
TOML strings, so it stays diffable, syntax-highlightable and free of escaping. A fixture whose
source is unreadable is a fixture nobody will maintain.

### Identity and the clock

```toml
[fixture]
name        = "spring-payments"
description = "..."
stack       = ["java", "spring"]
role        = "payments"
base_epoch        = 1735689600   # 2025-01-01T00:00:00Z
commit_interval_s = 86400
default_branch    = "main"

[author]
name  = "Nexus Fixtures"
email = "fixtures@nexus.invalid"
```

Commit *n* is stamped `base_epoch + n * commit_interval_s` at offset zero. **No clock is ever
read**, which is most of why the shas are stable.

### Commits and operations

```toml
[[commit]]
id      = "c3"                       # logical; tasks and patches refer to this, never a sha
message = "..."
branch  = "feature/x"                # optional; default_branch otherwise

write      = [{ path = "A.java", blob = "a-v2.java" }]   # or content = "inline"
delete     = [{ path = "Old.java" }]
move       = [{ from = "a/A.java", to = "b/A.java" }]
substitute = [{ extensions = ["java"], find = "mn.pay", replace = "mn.payments" }]
transform  = [{ kind = "double-indent", extensions = ["java"], under = "src/" }]
```

Operations apply in a **fixed order** — writes, moves, substitutions, transforms, deletes — not
in declaration order, so a spec cannot come to depend on how its TOML tables were interleaved.

**File selectors are not globs.** `paths` (exact), or `extensions` + `under` (prefix). A
hand-rolled glob matcher is a well-known source of quiet wrongness, and a fixture that silently
reformatted the wrong file set would corrupt the very assertion the reformat commit exists to
make.

`substitute` is **literal**, never regex — which is what lets a spec say plainly that
`mn.pay → mn.payments` is safe because `mn.payments` does not yet exist anywhere.

`transform` kinds: `double-indent` (a whitespace-only reformat) and `trim-trailing`.

### Bugs, decoys and expectations

```toml
plants_bug.id      = "duplicate-payment-under-concurrency"
plants_bug.kind    = "concurrency"
plants_bug.summary = "..."
plants_bug.anchor   = "src/main/java/mn/pay/PaymentService.java:27"
plants_bug.fixed_by = "c6"

expect.symbol_changes = 0            # the reformat commit's assertion
expect.new_findings   = 0
expect.note = "..."

[[deprecated_path]]
id        = "legacy-payment-calculator"
paths     = ["src/main/java/mn/pay/legacy/LegacyPaymentCalculator.java"]
live_path = "src/main/java/mn/pay/PaymentService.java"
decoy_for = "H1-rounding-in-totals"
note      = "why it looks right, and why it is wrong"
```

`expect` is **recorded, never checked here.** The generator has no index and, by boundary rule,
cannot acquire one — a generator that marked its own work would be no check at all.

### Patches and tasks

```toml
[[patch]]
id   = "wip-refund"
blob = "wip-refund.patch"
base = "c2"                          # proved to apply here, or generation fails

[[task]]
id      = "A1-idempotency-key-length"
family  = "A"                        # the families of §4
commit   = "c2"                      # logical id; resolved to a sha on generation
start_state = { dirty = "wip-refund" }   # or omit for clean
prompt  = "..."
required_sites = ["..."]             # §7 grades L3 against this
hidden_tests   = ["tests/eval/hidden/A1"]

[[task.turns]]                       # multi-turn instead of `prompt`
prompt = "Now do the same for OrderService."
required_anchors = ["mn.pay.PaymentService"]
```

Tasks are authored against a **logical commit id** and resolved to a sha at generation.
Hand-copying shas into task files is the kind of clerical step that is wrong once and then wrong
forever. `--emit-tasks DIR` writes each task in the §3 wire form with its sha pinned.

---

## Determinism

A commit's sha hashes its tree, parents, message and signatures. Every one is pinned:

| Input | Pinned by |
|---|---|
| tree | blob bytes, normalised to `\n`; git sorts tree entries, so operation order cannot leak in |
| parents | the history is declared, not discovered |
| message | the specification |
| signatures | one fixed identity; `base_epoch + n * interval`, offset zero |
| branch name | `initial_head`, because `init.defaultBranch` is a per-machine setting |
| line endings | `core.autocrlf = false`, forced |

`make fixtures-verify` generates every fixture twice and fails if any sha moved. It is cheap
enough for every CI run and worth it: a corpus that drifts makes every measurement taken against
it a measurement of the corpus.

Verified stronger than in-process — the `verify` command's heads match a separate `generate`
run's heads, across different output directories and different processes.

---

## Isolation

- Default output is `target/fixtures/`, which is already git-ignored.
- Generation **refuses** an output directory containing `Cargo.toml`, `package.json` or
  `pom.xml` — the cost of a mistyped `--out` is somebody's source tree.
- A non-empty target needs `--force`.
- Paths that escape the repository (`../`, absolute, `.git/`) are refused.
- Nothing but content is written into the repository: manifests and patches land beside it, so a
  fixture's own metadata is never indexed by the scan it exists to exercise.

---

## Limitations

- **No executable files.** Recording a mode bit that some filesystems do not carry would make
  every sha depend on where generation ran. Nothing in the corpus needs one.
- **No merges.** Branches fork and run linearly; a merge commit has no operation form yet.
- **`substitute` is literal.** A regex form would need a new dependency and a new class of spec
  bug; add it when a fixture genuinely cannot be written without it.
- **`expect` is recorded, not verified.** Checking it needs an index, which is the boundary.
- **Fixtures are small.** They prove behaviours, not scale. §16 of the evaluation says so
  plainly, and the corpus does not pretend otherwise.
