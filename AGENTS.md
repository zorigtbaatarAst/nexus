# Nexus — briefing for an AI agent working in this repo

Read this before touching anything. These are the facts that are expensive to rediscover and
the constraints that will bite you if you do not know them.

## What this is

A platform for persistent code intelligence. **Nexus understands the project; capabilities
use that understanding.** BugHunter is the first capability, not the product.

The rename to `nexus` is complete: the directory, the repository and the crates all carry it.
One thing survives on purpose — `Engine::migrate_legacy_dir` moves a `.bughunter/` directory
to `.nexus/` on first open, so a project indexed before the rename is not silently re-scanned
from nothing.

It has **two entry shapes**, and confusing them is the fastest way to design something wrong.
`rescan` is change-driven — forward from a known changed symbol. `investigate` is
symptom-driven — backward from an unknown one, seeded by what an agent read off a screenshot.
The hard part of the second is *anchoring*, which the first has no concept of. **`investigate`
is designed and not built**; the anchoring half of it now exists as the Context Engine's text
seeding over `ui_strings`. See [`docs/investigation.md`](docs/investigation.md).

**Status: all six roadmap phases ship.** Eighteen crates, ~32k lines, 394 tests. On top of
the original cascade — `scan`, `rescan`, `status`, `changes`, `impact`, `graph`, `ask`,
`analyze`, `doctor` — the Context Engine (`context`), the fact lifecycle with Markdown export
and file-based sharing (`fact`, `memory`, `share`), and the verification gate (`verify`) all
work. Five languages are indexed: Java, TypeScript, GraphQL, Rust and Python. **Nexus indexes
itself** — 1,831 symbols where it once reported zero.

Still absent, and every surface says so rather than leaving anyone to infer it: any direct
LLM provider, and `investigate`. Two things are honest about being weaker than they look —
Rust edge resolution is 46 % against Java's 96 % (bare method hints need a receiver type),
and no ranking weight has been tuned, because tuning without ledger evidence is the folklore
`docs/architecture/11-risks.md` R8 names. `docs/architecture/10-roadmap.md` records what each
phase delivered and what it left undone.

A Rust workspace of eighteen crates producing one binary image under two names — `nexus` is
the platform, `bughunter` the capability's own CLI — which is both the CLI and the MCP server.
Which name is running is decided by `argv[0]`, so there is a single dispatch path.

## The one idea the whole design rests on

**Nexus owns evidence, history and verification. The AI agent owns reasoning.**

The corollary that shapes the crate layout: identity, lifecycle and storage belong to the
platform; only *rules* belong to a capability. That is why `bugs` became `findings` with a
capability column rather than staying BugHunter's private table.

If you find yourself adding reasoning to BugHunter, or evidence-gathering to the agent layer,
you are working against the grain of the design. Check the layer you are in.

## The hard constraints

These are not style preferences. Each is pinned by a test, and violating one produces a bug
that is hard to attribute.

0. **`nexus-core` must not depend on any `cap-*`, and no `cap-*` may depend on an adapter or
   on `nexus-store`.** Capabilities are registered by the composition root — `nexus-cli::open`
   and `nexus-mcp` — never compiled into the platform. Get this backwards and "add Code Review
   later" becomes a core change.

1. **`nexus-core` must not depend on `nexus-mcp`, `nexus-cli`, or any concrete AI provider.** It
   would depend on `nexus-ai` with `default-features = false`, so the deterministic build has
   no HTTP client in its dependency tree at all. *`nexus-ai` does not exist. The rule is
   enforced today in its stronger form: a `cargo metadata` test asserts `nexus-core` depends
   on no HTTP client whatsoever.*

1b. **`nexus-core` must not name a language either.** The analyzers are registered by the
   composition root through `nexus-lang-pack`, and the core may depend on neither a concrete
   `nexus-lang-*` nor the pack. Adding a language is a new crate and one line at the root —
   never an edit to the core.

2. **`nexus-mcp` must not depend on `nexus-store`, `nexus-lang*` or `nexus-verify`.** A handler reaches
   them only through `nexus-core`, so it physically cannot grow logic the CLI lacks. Every
   handler is: deserialize → one `Engine` call → serialize. If a handler needs two `Engine`
   calls, the missing method belongs in `nexus-core`.

3. **Only `nexus-store` contains SQL.** No exceptions, including "just this one query".

4. **`nexus-lang-*` must not depend on `nexus-store` or `nexus-core`.** An analyzer takes source text
   and returns a `ParsedFile`. It never learns about scans or baselines. This is also why
   parsing parallelizes cleanly.

5. **`nexus-verify` writes only through `SafeWriter`**, rooted at `.nexus/generated-tests/`,
   canonicalizing the parent path *before* the prefix check. A jail that compares unresolved
   paths is not a jail — a textual prefix check accepts a symlink inside the root that points
   at `/etc`, and there is a test that builds exactly that. The crate must also not depend on
   `nexus-store` or `nexus-core`: it takes a plan and returns a verdict, and `nexus-core`
   writes down what it decided.

6. **Ledger tables are append-only.** `scans`, `changes`, `commits`, `finding_occurrences`,
   `finding_verifications`, `test_runs`, `audit_events` are never `UPDATE`d. See
   [`docs/data-model.md`](docs/data-model.md) §2. An `UPDATE` on one of these destroys
   regression detection, which is the strongest thing the product does.

## Things that look wrong and are deliberate

- **`changes.path` and `changes.fqn` duplicate data reachable through `entity_id`.**
  Intentional. The evidence must stay readable after the symbol is deleted; a historical
  record that resolves to `NULL` two refactors later is not a record.

- **Two hashes per symbol (`sig_hash`, `body_hash`).** This is not redundancy. A `sig_hash`
  change is an API break that ripples to every caller; a `body_hash`-only change ripples
  only through data and effect edges. Collapse them and impact analysis becomes noise.
  [ADR-010](docs/architecture-decisions.md#adr-010-two-hashes-per-symbol).

- **Verification runs the same test twice** — once on HEAD, once on the baseline revision in
  a detached worktree. Halving this to save time also destroys the ability to tell "this
  change introduced a bug" from "this suite was already red".

- **An infrastructure failure leaves confidence unchanged**, never lowered. A test that would
  not compile says nothing about the hypothesis.

- **`FIXED` requires the stored reproduction test to pass.** Absence from an incremental scan
  means the region was not examined. Treating absence as a fix silently closes real bugs.

- **Confidence from a model is clamped at 0.75.** Only the verification engine can go higher,
  and only by reproducing the bug. Note that a **contract-mismatch finding is not clamped**:
  it scores 0.90 because no model was asked — both sides are indexed and comparing them is a
  join.

- **BugHunter never opens the screenshot.** `SymptomReport.screenshot` is a path and a hash
  recorded as provenance on the bug. Adding an OCR or vision dependency to read it would put
  a whole class of sensitive data inside a component whose redaction pass works on text.

- **`clarification_required` is a result, not an error.** It carries what is already
  resolved, so no work is discarded, and `can_proceed_without` separates "cannot proceed"
  from "can proceed at 0.35". Collapsing those two turns every soft ambiguity into a hard
  block, and a tool that blocks constantly gets scripted around.

- **A `BugCandidate` with empty `evidence` is rejected, not down-ranked.** Rejections are
  counted and reported, because a silently discarded finding is indistinguishable from a
  model that found nothing.

- **An imported claim is anchored on a symbol only when it names one exactly.** graphify's
  semantic pass produces claims about the project, and `nexus memory import` records them as
  facts. Subject resolution requires the matched symbol's own last segment to *be* the word:
  `find_symbols` matches by suffix, which is right for a prompt someone typed and wrong for
  English prose, where "integration" anchored a design claim on `NoContinuousIntegration`.
  A claim that names nothing anchors on the document that states it.

## Traps

- **The context budget is measured, never estimated.** `tokens_estimated` is the serialized
  package; a candidate's cost is the serialized `ContextItem`, keys included. Every earlier
  shortcut here under-reported by more than an order of magnitude — the package that claimed
  253 tokens shipped 11,113 — because the estimate counted item text and the payload is
  mostly everything else.

- **The context cache key must include everything that changes an answer.** Intent, seeds,
  commit, dirty hash, budget, weights, `explain`, the memory fingerprint, and the build
  version. It has been wrong in three separate ways: a recorded fact was invisible until an
  unrelated file moved, an `--explain` request was served a ledger-less hit, and a package
  outlived the upgrade that changed how packages are built.

- **A call-site hint is a bare member name, and the index is keyed `Owner#member`.** An
  analyzer sees one file, so `self.foo()` and `obj.foo()` yield `#foo` or `foo` — never the
  owner. `Store::resolve_edges` has a `by_member` tier for exactly this; without it method
  calls never resolved, and Rust sat at 23% while the README advertised a Java project's 96%.
  The tier refuses a name shared by more than four symbols: five wrong edges are worse than
  none.

- **Cache invalidation must include tool versions.** `scans.tool_versions_json` holds grammar
  and analyzer versions. Upgrade `tree-sitter-java` without bumping it and the content hashes
  still match, nothing re-parses, and the index keeps the old wrong symbols forever, with no
  error anywhere. This is the single easiest thing to get wrong here.

- **`normalize_body` is per-language and is the most dangerous function in the codebase.**
  Strip too much and real changes become invisible. It is guarded by a fixture assertion: the
  reformat commit must produce exactly zero symbol changes, and a literal change must always
  produce one.

- **Soft-deletes mean nearly every query needs `WHERE deleted = 0`.** Forgetting it is silent.
  `nexus-store` should expose filtered views rather than raw tables.

- **`idx_edges_dst` and `idx_edges_unresolved` are load-bearing.** Without the first, every
  impact query is a table scan. Without the second, a rescan that adds a symbol scans the
  whole edge table — a 200 ms rescan becomes 40 s.

- **Commands are argv, never strings.** Allowlist entries are templates with typed holes;
  `{test}` becomes exactly one argv element. `sh -c` is never used, anywhere.

- **stdout is results, stderr is everything else.** `--json | jq` must work with `-v` on, and
  a command emits **exactly one** JSON document — two concatenated objects parse as neither.
  `scan` once printed its report and then Architect's findings separately, and the project's
  own CI smoke check died on `Extra data: line 28`. `nexus-cli/tests/json_contract.rs` pins it.

- **Exit codes are interface**: 0 ok, 1 runtime, 2 usage, 3 findings (`--fail-on`), 5 no
  baseline, 6 ambiguous target. Finding a change is success, not an error — a tool that exits
  non-zero for doing its job is removed from the pipeline within a week.

- **Never generate a clarifying question from a template.** Ask only when the ambiguity was
  actually measured — one anchor candidate means no question. Asking something BugHunter
  already knows is how a tool teaches people to ignore its questions, and once that happens
  the mechanism is dead.

- **Comments are part of the `modifiers` node.** Commenting out `@Transactional` puts a
  comment where the annotation was, and `modifier_words` must skip it — otherwise the
  signature changes and the edit reports `API_CHANGED` instead of `CONTRACT_CHANGED`. Found
  by running the scanner on a real Spring repository, not by reading the code; pinned by
  `a_comment_among_the_modifiers_does_not_touch_the_signature`.

- **Annotations get their own canonical form**, not `normalize_body`. Token-stream
  normalization joins with a space, so a line-wrapped `@PreAuthorize(\n  "x"\n)` would not
  equal `@PreAuthorize("x")`. `canonical_annotation` concatenates with no separator, which
  is safe only because annotation arguments are constant expressions.

- **Renames are resolved after every changed file has been seen, never per file.** The two
  halves of a package move live in different files, so appearances and disappearances are
  buffered and matched at the end on `(name, sig_hash, body_hash)` — the tuple that survives
  a move and nothing else. Only unambiguous 1:1 matches count: generated accessors collide
  on that key constantly, and carrying identity to an arbitrary candidate is worse than
  reporting a delete and an add.

- **The `.graphqls` schema is the contract, not the Java annotations.** Taking resolvers
  from `@QueryMapping` alone assumed every served field carries an annotation shape this
  analyzer recognizes. On a real Spring for GraphQL project that is false, and the orphan
  detector produced thirteen confident reports that a field "no resolver serves" was missing
  when the schema declared it plainly. Index the schema; it is what codegen generates the
  frontend types from, so both sides already agree on it.

- **Introspection meta-fields are not selections.** Apollo adds `__typename` to almost every
  document. It is valid on every type by spec and declared in no schema, so emitting it as a
  root field makes nearly every operation look broken.

- **`FIXED` for a deterministic detector is different from `FIXED` for an AI finding.**
  Absence is not evidence in general — but a rule that ran again over the same index and did
  not fire *is* evidence. The sweep therefore checks which detector families ran, not merely
  which fingerprints were seen.

- **The fixed-sweep matches on bug ids, not fingerprints.** A bug matched through a rename
  alias has the *old* fingerprint stored, so a fingerprint-based sweep closes the very bug it
  just found.

- **A capability must iterate `scoped.symbols`, not `ctx.symbols`.** Narrowing is done once
  in `ctx.scoped(scope)`; a rule that reaches past it makes a targeted analysis cost what a
  full one costs, silently. Reaching past it is sometimes right — the self-invocation rule
  needs the callee's annotations even when the callee is out of scope — which is why `ctx` is
  passed alongside `scoped` rather than replaced by it.

- **A narrowed analysis may not close what it did not examine.** The fixed-sweep runs only
  under `Scope::Everything`. Absence is evidence only when something actually looked.

- **The fixed-sweep matches bug ids, not fingerprints.** A finding matched through a rename
  alias still carries the old fingerprint, so a fingerprint-based sweep closes the very
  finding it just matched.

- **A static import is not a call on the enclosing class.** `import static
  org.mockito.Mockito.when;` makes `when(...)` a call on Mockito. Attributing unqualified
  calls to the enclosing type without checking static imports invented ~600 edges to methods
  that do not exist, in test files alone. A *wildcard* static import makes the name
  genuinely undecidable from one file, so nothing is emitted.

- **Record components are methods.** `record SaleDto(String orderStatus)` implies
  `orderStatus()`. Without emitting those accessors, every `dto.orderStatus()` in the
  codebase is an unresolvable call — this alone was most of the gap between 68 % and 96 %
  resolution on a real project.

- **Codegen output must not be indexed.** `graphql-generated.ts` is thousands of symbols
  nobody wrote and nobody can change. `walk::is_excluded` drops it, along with
  `node_modules`, `build`, `.next` and `target`.

- **`external` is not `unresolved`.** An edge to `org.springframework` is correctly outside
  the index; counting it as a failure hides real bugs inside a large constant. See
  [ADR-017](docs/architecture-decisions.md#adr-017-external-is-a-resolution-outcome-not-a-failure).

- **A changed symbol's `ChangeKind` was hardcoded, and nothing could see it.**
  `Engine::analyze` built every `ChangedSymbol` with `kind: BodyChanged`, discarding what the
  ledger knew — so every capability rule asking "did the contract move?" was unreachable, with
  no error anywhere. It survived because BugHunter never looks at `ChangeKind`; the second
  capability found it in an afternoon. The kind is stored across two columns, `change_type`
  and `detail`, and `ChangeKind::from_ledger` is their joint inverse, kept beside the two
  functions that write it. **A value written by one function and read by another that is not
  its inverse is a silent-failure machine** — pin the round trip with a test over every
  variant.

- **An advisory finding still needs `file:line`.** A rule about something the project *lacks*
  has no obvious line, and the temptation is to relax the evidence requirement for it. Do not:
  anchor on where the missing thing belongs — the build file CI would have invoked, the
  compose line that proved the datastore — and if no such place is in the index, emit nothing.
  A guessed filename is evidence naming a file that may not exist, which is worse than
  silence. ADR-021.

- **`sibling` is not `external` either.** The same reasoning applied twice: an edge to a
  module of *this* project that was not scanned is outside the index, but it is code an edit
  here can break and a wider scan resolves. Both were `external` until 2026-09-01, and on a
  six-service monorepo **6,247 of 9,514 "external" edges were the project's own code** —
  `impact` on the base class every entity extends answered "no symbol matches". Sibling
  edges count in the resolution denominator; `external` still does not. The owner root is
  *inferred*, never configured: reverse-DNS means the first two package segments name the
  owner, and where no owner holds a majority the classification declines rather than
  guesses. A high `sibling` count means the scan is too narrow, not that the analyzer is
  broken.

- **`.nexus/` must be excluded wherever candidates come from.** The walker filters it
  structurally, but Tier 1 candidates also come from `git diff`, which knows nothing about
  that filter — so `walk::is_excluded` exists for both paths to consult. Without it
  BugHunter indexes its own config files.

- **`ui_strings` must index every locale's i18n values, not just keys.** The screenshot may
  be in Mongolian while the source holds an English key. Matching the value reaches the key,
  and the key reaches the component. Index keys only and a non-English UI becomes
  unanchorable by text, which removes the strongest investigation signal.

- **An unmatched `calls_http` edge is reported, never dropped.** It means a dead endpoint, an
  unconfigured gateway rewrite, or a service outside the repo — all three worth knowing. A
  silently dropped seam edge makes the trace lie by omission.

## Where to look

| Question | Document |
|---|---|
| how do the crates fit together | [architecture.md](docs/architecture.md) §3–4 |
| why is it built this way | [architecture-decisions.md](docs/architecture-decisions.md) |
| what does the schema look like | [data-model.md](docs/data-model.md) |
| how does rescan avoid re-parsing | [change-analysis.md](docs/change-analysis.md) §2 |
| how is a bug proven | [verification-engine.md](docs/verification-engine.md) |
| screenshot → suspect, and the seam | [investigation.md](docs/investigation.md) |
| how the GraphQL seam actually works | [ADR-014 revision](docs/architecture-decisions.md#adr-014-join-the-stack-at-the-http-contract) |
| what can an agent call | [mcp-api.md](docs/mcp-api.md) |
| what is safe to execute | [security.md](docs/security.md) §3–4 |
| how to add a capability | [capabilities.md](docs/capabilities.md) |
| how context is selected and ranked | [architecture/05-context-engine.md](docs/architecture/05-context-engine.md) |
| how a fact is validated and retired | [architecture/06-memory.md](docs/architecture/06-memory.md) |
| what each phase delivered, and what it left undone | [architecture/10-roadmap.md](docs/architecture/10-roadmap.md) |
| what Nexus will not build, and what would reverse that | [architecture/12-non-goals.md](docs/architecture/12-non-goals.md) |
