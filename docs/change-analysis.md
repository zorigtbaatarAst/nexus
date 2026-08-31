# BugHunter — Change Analysis

Covers change detection (§1–4), dependency/impact analysis (§5–7) and bug fingerprinting
(§8–10). These three algorithms are the product; everything else is plumbing around them.

---

## 1. The requirement

A rescan on a repository where one method body changed must not re-read the repository,
must not re-parse unaffected files, and must not hand an LLM anything but the affected
region. On a 5 MLOC monorepo the target is **under two seconds** for a no-op rescan.

That rules out the naive pipeline (`parse everything → diff everything`) and forces a
cascade in which each stage only sees what survived the previous one.

---

## 2. Change detection — the tiered cascade

```
Tier 0   repo gate          O(1)              ~5 ms
Tier 1   file set           O(files), stat-first
Tier 2   symbol set         O(changed files)
Tier 3   edge re-resolution O(changed FQNs), indexed
```

### Tier 0 — repo gate

```
baseline = SELECT commit_sha, working_tree_hash, dirty FROM scans JOIN baselines …
current  = (git rev-parse HEAD, git status --porcelain is empty)

if current.commit == baseline.commit && !current.dirty && !baseline.dirty:
    → "no changes since scan-NNN", exit 0
```

One `git rev-parse` and one `git status`. On a clean checkout at the baseline commit the
whole rescan finishes here. This case is common enough — CI re-runs, an agent asking twice,
a `status` call in a loop — that it is worth a dedicated tier.

With `vcs = 'none'` Tier 0 is skipped and Tier 1 runs a full walk.

### Tier 1 — changed file set

**Candidate generation.** Two sources, unioned:

```
git diff --name-status <baseline.commit>..HEAD     # committed changes
git status --porcelain --untracked-files=all       # working tree changes
```

If the baseline commit is unreachable (force-push, rebase, shallow clone) BugHunter falls
back to a full ignore-aware walk and records `kind='full'` on the scan. Detecting that and
silently producing a wrong diff would be far worse than a slow scan.

With no git, or with `--paranoid`, the candidate set is the full walk: `ignore`-crate
traversal honouring `.gitignore`, `.nexusignore` and `config.toml` `include`/`exclude`.

**Verification.** For each candidate, cheapest test first:

```
1. stat: (size_bytes, mtime_ns) == stored   → unchanged, skip hashing        [~1 µs]
2. blake3(bytes) == files.content_hash      → touched but unchanged, refresh mtime
3. otherwise                                → CHANGED
```

The stat fast path is what makes a full walk survivable: on a 50 k-file repo it eliminates
99 % of hashing work, and hashing is the only I/O-bound step. Hashing of the remainder is
parallel (`rayon`), typically 1–2 s for a cold 500 MB tree with `blake3`.

**Rename and move detection.** A path that disappeared and a path that appeared sharing a
`content_hash` is a rename. The file row keeps its `id`, its `first_seen_scan_id` and its
symbol children; `changes` records `change_type='renamed'`; `symbol_aliases` gets a row per
symbol mapping old FQN → symbol id. Without this, every package refactor invents a repo full
of "new" bugs.

**Symbol-level renames are resolved globally, not per file.** File-level hashing only catches
a move whose content is byte-identical, which a package rename never is — the package
declaration changes with it. So appearances and disappearances are buffered across every
changed file and matched at the end on `(name, sig_hash, body_hash)`: the tuple that survives
a move and changes for anything else.

Two rules keep it honest. A symbol whose body also changed does not match, because attaching
an old bug history to code that is no longer the same code is worse than losing the link. And
only unambiguous 1:1 matches count — generated accessors and `equals`/`hashCode` collide on
that key constantly, and carrying identity to an arbitrary one of five candidates is worse
than reporting a delete and an add.

Measured on a 27-file package rename in a real Spring project: 137 aliases recorded, and the
report reads as renames rather than 274 unrelated deletions and additions.

**Working tree hash.** A merkle root over sorted `(path, content_hash)` of every indexed
file, one `blake3` over the concatenation. It is the `working_tree_hash` in the baseline and
gives a single-value "is anything at all different" check for dirty trees, where a commit
sha says nothing.

### Tier 2 — changed symbol set

Only files marked CHANGED are re-parsed. Each is parsed once by its `LanguageAnalyzer`
into a `ParsedFile`, then diffed against the stored symbols for that file, keyed by FQN:

| Condition | Change | Ripples through |
|---|---|---|
| FQN present before and after, `sig_hash` differs | `API_CHANGED` | all reverse edges |
| `sig_hash` same, `body_hash` differs | `BODY_CHANGED` | `reads`/`writes`/`persists`/`emits` only |
| `annotations_json` differs | `CONTRACT_CHANGED` | all reverse edges + framework expansion |
| FQN only after | `ADDED` | forward edges; unresolved-edge sweep |
| FQN only before | `DELETED` | all reverse edges, at full weight |
| same `sig_hash`+`body_hash`, different name, same parent | `RENAMED` | alias, no ripple |

`body_hash` is computed over a **normalized** body: comments stripped, whitespace collapsed,
string literals preserved. Normalization is per-language (`LanguageAnalyzer::normalize_body`)
because what is semantically irrelevant differs — Python indentation is not whitespace in
the way Java's is. A reformat therefore produces zero symbol changes, which is the whole
point: `gradle spotlessApply` must not trigger a bug hunt.

`CONTRACT_CHANGED` is separated from `API_CHANGED` because annotation edits are frequently
the *most* dangerous change in a Spring codebase and carry no signature diff at all.
Removing `@Transactional` from a method changes nothing a compiler would notice and
everything about correctness under concurrency.

### Tier 3 — edge re-resolution

Adding, deleting or renaming a symbol can resolve or break edges elsewhere in the graph
without those files changing. Rather than rebuild the graph:

```sql
-- newly resolvable: previously unresolved edges pointing at an added/renamed FQN
SELECT id FROM symbol_edges
 WHERE project_id = ?1 AND dst_symbol_id IS NULL AND dst_fqn_hint IN (added_or_renamed);

-- newly broken: resolved edges pointing at a deleted symbol
SELECT id FROM symbol_edges WHERE dst_symbol_id IN (deleted_symbol_ids);
```

The first uses `idx_edges_unresolved`, the second `idx_edges_dst`. Both are indexed lookups
proportional to the number of changed FQNs, not to graph size.

---

## 3. Cache invalidation

`scans.tool_versions_json` records the version of every component whose output is cached:

```json
{ "schema": 3, "nexus-lang-java": "0.4.1", "grammar:java": "0.21.0",
  "grammar:tsx": "0.20.4", "normalizer:java": 2 }
```

On rescan, any component whose version differs from the baseline scan forces **all files
in its scope** to be re-parsed, hashes notwithstanding, and the scan is recorded as `full`
for that language.

This is the single most easily forgotten piece of an incremental system. Upgrade
`tree-sitter-java` so it now extracts record components it previously missed, and without a
version gate the content hashes still match, nothing re-parses, and the index silently keeps
the old, wrong symbols — forever, with no error anywhere.

---

## 4. Change detection — worked example

```
PaymentService.java changed
        │
        ├── createPayment(String, Money)   BODY_CHANGED   (body_hash differs)
        ├── refund(String)                 API_CHANGED    (added a parameter)
        ├── audit(Payment)                 DELETED
        └── PaymentService (class)         CONTRACT_CHANGED (@Transactional removed)
```

Four symbol changes from one file, each with a different ripple rule. A file-granular system
would produce one undifferentiated "PaymentService changed" and hand the LLM the whole class.

---

## 5. Impact analysis — the model

The dependency graph is `symbol_edges`: a directed multigraph over symbols where an edge
carries a type, the resolution tier that produced it, and a confidence.

```
Controller ──routes──▶ Service ──injects──▶ Repository ──persists──▶ Entity/Table
```

Forward traversal answers "what does this reach". Reverse traversal — the important one —
answers "who breaks if I change this":

```
Database ──▲── Repository ──▲── Service ──▲── Controller
```

Reverse traversal is a single index seek per node on `idx_edges_dst`.

## 6. Impact analysis — the algorithm

Bounded, weighted, bidirectional BFS with score decay.

```
INPUT   seeds: [(symbol_id, change_kind)]
OUTPUT  [(symbol_id, score, path, min_confidence)]  +  ranked tests

score[seed]     = 1.0
frontier        = seeds at depth 0

for depth in 1..=max_depth:                        # default 5
    for node in frontier:
        edges = reverse_edges(node) filtered by edge_filter(change_kind_of_seed)
        if edges.len() > fan_out_cap:              # default 200
            mark node truncated; keep the highest-confidence fan_out_cap edges
        for e in edges:
            s = score[node] * w(e.edge_type) * e.confidence
            if s < threshold: continue             # default 0.15
            if s > score[e.src]:
                score[e.src] = s
                path[e.src]  = path[node] + [(node, e.edge_type)]
                next_frontier.push(e.src)
```

**Edge weights** — how much of an upstream change survives one hop:

| Edge | w | Reasoning |
|---|---|---|
| `calls` | 0.90 | a caller of a changed method is almost always affected |
| `implements` / `extends` | 0.85 | contract change propagates to the hierarchy |
| `injects` | 0.80 | DI: the holder of the bean depends on its behaviour |
| `routes` | 0.70 | HTTP boundary; affected, but often only in serialization |
| `persists` | 0.70 | schema/entity coupling |
| `reads` / `writes` | 0.60 | shared state; real but weaker |
| `imports` | 0.30 | mere visibility; mostly noise, kept for completeness |

**Edge filter by change kind** — the reason `sig_hash`/`body_hash` were split:

```
API_CHANGED, CONTRACT_CHANGED, DELETED  → all reverse edge types
BODY_CHANGED                            → reads, writes, persists, emits, calls-with-effects
RENAMED                                 → none (alias absorbs it)
```

A body-only edit does not break its callers' compilation; it can only affect them through
shared state or observable effects. Filtering there is what keeps a one-line change from
reporting 400 affected symbols.

**Termination.** Depth cap, score threshold, and a visited map keyed by `symbol_id` holding
the best score — so cycles terminate naturally and the reported path is the strongest one,
not the first one found.

**Truncation is reported, never silent.** A `Utils` class with 3 000 callers hits the fan-out
cap; the result carries `truncated: true` with the count that was dropped. Silently
returning 200 of 3 000 and calling it the impact set is exactly the kind of quiet lie that
makes a tool untrustworthy.

**Every result carries its path and `min_confidence`** — the smallest edge confidence along
the chain. A three-hop heuristic chain scoring 0.4 with `min_confidence: 0.55` is honestly
labelled as a guess, and both the CLI and the calling agent can treat it as one.

## 7. Framework expansion

After the generic BFS, each active `FrameworkPack` gets `expand_impact()`. Spring examples:

| Trigger | Expansion |
|---|---|
| method on an interface changed | all `@Service`/`@Component` implementations |
| `@Entity` field changed | repositories and `@Query` methods referencing it, plus migrations |
| `@Transactional` added/removed | every symbol in the call subtree inside the boundary |
| `@RequestMapping` path changed | the route, its integration tests, and API clients in-repo |
| `@Bean` return type changed | every injection point resolving to that type |
| Spring Data method name changed | the derived query and the collection/table it targets |

These edges are inserted with `resolution='framework'`. They express knowledge a generic
call graph cannot: nothing syntactically "calls" a `@Bean` method.

**Test selection.** For every affected symbol, union `test_coverage`, ranked by
`impact_score × coverage_confidence`, with `runtime` sources outranking `static` outranking
`naming`. Output is the "8 related tests" line in the CLI report and the input to
verification.

---

## 8. Bug fingerprinting — the requirement

A fingerprint must be **stable** across reformatting, line drift, parameter renames and file
moves, and **discriminating** enough that two genuinely different bugs in the same class do
not collide. Get it too loose and real bugs get swallowed as duplicates; too tight and every
`spotlessApply` invents a new backlog.

## 9. The fingerprint

```
fingerprint = hex(blake3(
      bug_type                     ‖ "\0"     -- 'concurrency'
    ‖ component                    ‖ "\0"     -- 'PaymentService'  (class, not file path)
    ‖ anchor_symbol_fqn_shape      ‖ "\0"     -- 'mn.pay.PaymentService#createPayment'
    ‖ detector_family              ‖ "\0"     -- 'ai-logic' | 'semgrep:java.lang.security.x'
    ‖ structural_key                         -- detector-supplied, normalized, sorted
)[..16])
```

**Excluded on purpose** — each changes without the bug changing:

| Excluded | Why |
|---|---|
| file path | a package move is not a new bug |
| line numbers | an import added above shifts every line |
| commit sha | the bug outlives the commit |
| title / description wording | an LLM rephrases it every run |
| confidence, severity | these are *properties* of the bug, not its identity |

**Included on purpose:**

- `anchor_symbol_fqn_shape` — package + class + method name with generics, parameter names
  and parameter types normalized away. Survives a parameter rename or an added overload;
  changes when the method genuinely moves to a different class, which *is* a different bug.
- `structural_key` — the detector's own normalization of what the bug is *about*. For a
  concurrency finding: the sorted set of shared-state identifiers involved
  (`payment.status,paymentRepository`). For a Semgrep finding: the rule id plus the sink.
  This is what separates "duplicate payment under concurrency" from "duplicate refund under
  concurrency" in the same class.

**Display identity is separate.** `bugs.slug` holds the human form
(`payment-duplicate-concurrent-create`), `bugs.bug_uid` the short id (`BUG-104`). The hash is
never shown; the slug is never trusted for identity. Two fields, two jobs.

**Alias resolution.** Before computing a fingerprint, `anchor_symbol_fqn_shape` is passed
through `symbol_aliases`, so a bug found under the old FQN and a bug found under the new one
land on the same hash across a rename.

**Near-duplicate handling.** If the exact hash misses but an open bug shares
`(bug_type, component)` and has ≥ 0.85 token similarity on the hypothesis, the new bug is
recorded **and** linked `duplicate_of` the existing one, surfaced for a human decision. It
is not auto-merged: in the face of ambiguity, refuse the temptation to guess. Merging two
distinct races because their descriptions rhymed is unrecoverable; an extra row is not.

## 10. Bug lifecycle

```
                    detector fires
                          │
                          ▼
                     SUSPECTED
                          │  deterministic evidence attached (CodeRef set non-empty)
                          ▼
                     UNVERIFIED ──────────────┐
                          │ reproduction test │ user: bughunter ignore
                          │ fails now,        │
                          │ passes on baseline▼
                          ▼               IGNORED
                      VERIFIED
                          │  reproduction test passes on a later revision
                          ▼
                        FIXED
                          │  the same test fails again
                          ▼
                      REGRESSED ──▶ (VERIFIED on re-confirmation)
```

Rules that make the machine trustworthy:

1. **`FIXED` requires evidence.** Specifically: the stored reproduction test now passes on
   the current revision. Absence of a bug from an incremental scan means *the region was not
   examined*, never that the bug is gone. This single rule is the difference between a
   history you can act on and a history that quietly closes real bugs whenever someone
   touches an unrelated file.
2. **Every transition appends**, it does not overwrite: a `bug_occurrences` row for the
   sighting, a `bug_verifications` row for the attempt, and only then an `UPDATE` of the
   `bugs` summary.
3. **`REGRESSED` is only reachable from `FIXED`**, and records `regression_of` in
   `bug_relations` pointing at itself for query convenience, plus the two commits.
4. **`IGNORED` is sticky.** A bug a human dismissed does not come back on the next scan; it
   is re-surfaced only if its `structural_key` changes, meaning it is no longer the same
   finding.
5. **Confidence never rises without evidence.** A detector re-firing does not increase
   confidence — the same guess made twice is still one guess. Only a verification run moves
   it up.
