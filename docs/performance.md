# BugHunter — Performance and Scaling

The target range is 10 KLOC to several million LOC in one repository. Those are not the same
product unless the architecture refuses one specific temptation:

```
   ✗   every scan → parse everything → send everything to an LLM
```

That pipeline works beautifully in a demo on a 5 000-line project and is unusable at 500 000.
Everything below exists to avoid it.

```
   ✓   hash-based change detection
         ↓  symbol-level change detection
         ↓  dependency graph traversal
         ↓  minimal AI context
```

---

## 1. Budgets

Wall-clock targets on a developer laptop (8 cores, NVMe), used as CI assertions:

| Operation | 10 KLOC | 500 KLOC | 5 MLOC |
|---|---|---|---|
| `init` | < 1 s | < 3 s | < 10 s |
| `scan` (cold, full) | < 2 s | < 45 s | < 8 min |
| `rescan` — no changes | < 50 ms | < 300 ms | < 2 s |
| `rescan` — 4 files changed | < 200 ms | < 600 ms | < 2.5 s |
| `impact` on one symbol | < 20 ms | < 60 ms | < 250 ms |
| `bugs` / `status` | < 20 ms | < 30 ms | < 80 ms |

The row that defines the product is **`rescan` with no changes**. It runs constantly — every
agent turn, every CI job, every `status`. If it is not effectively instant, nothing else
matters. It is one `git rev-parse`, one `git status`, and one indexed row read.

The `rescan` rows are flat in repository size on purpose: their cost is proportional to
*what changed*, not to how much code exists. The residual growth is the `git status` call.

---

## 2. Where the time actually goes

Measured shape of a full scan, from which the optimizations follow:

```
walk + stat        5 %      ignore-crate traversal, parallel
hashing           20 %      blake3, rayon; I/O bound on a cold page cache
parsing           55 %      tree-sitter; the dominant cost, CPU bound, parallel
symbol extraction 10 %      per-language visitor
edge resolution    8 %      import tables + FQN matching
DB writes          2 %      batched, one transaction per 1 000 rows
```

Parsing dominates, so the entire incremental design is about **not parsing**. A rescan that
re-parses 4 files instead of 42 000 is not 10 % faster; it is three orders of magnitude
faster, and that is the difference between a tool used on every change and a tool used once.

---

## 3. The layered defence against work

Each layer's job is to make the next layer's input smaller.

| Layer | Eliminates | Cost |
|---|---|---|
| Tier 0 repo gate | the entire scan when nothing changed | 2 syscalls |
| stat fast path | hashing 99 % of files on a full walk | ~1 µs/file |
| content hash | parsing files that were touched but not changed | ~1 GB/s |
| `body_hash`/`sig_hash` | ripple analysis for unchanged symbols in changed files | in-memory |
| edge filter by change kind | traversing edges a body-only change cannot affect | in-memory |
| score threshold + depth cap | 90 % of the reverse-reachable set | in-memory |
| `ContextBuilder` budget | sending anything but the top-ranked evidence | in-memory |

The `body_hash`/`sig_hash` split earns its place here as much as in correctness: a
`spotlessApply` across the repository changes every file's content hash, but almost no
symbol's body hash, so the expensive stages see nearly nothing.

---

## 4. Parallelism

- **Hashing and parsing** are `rayon` over the file list. Parsing is embarrassingly parallel
  — a `LanguageAnalyzer` takes source text and returns a `ParsedFile` with no shared state,
  which is exactly why boundary rule 5 forbids analyzers from touching the store.
- **Database writes are single-threaded** through one writer connection. SQLite in WAL mode
  has one writer; pretending otherwise buys `SQLITE_BUSY` retries, not throughput. Workers
  push `ParsedFile` values into a bounded channel and the writer batches them into
  transactions of ~1 000 rows.
- **Edge resolution runs after** the symbol table is complete, because resolving an FQN
  requires knowing every symbol. It is parallel over source files, reading an immutable
  in-memory FQN map built once.
- **Reads never block.** WAL means `impact`, `bugs` and `status` run against a snapshot
  while a scan is writing.

---

## 5. Caching

| Cache | Key | Invalidated by |
|---|---|---|
| file content hash | `(path, size, mtime_ns)` | stat mismatch |
| `ParsedFile` | `blake3(content) + grammar_version` | either component changing |
| symbol/body hash | content hash + normalizer version | either |
| `symbol_edges` | derived table | changed FQN set (Tier 3) or `--rebuild-graph` |
| `test_coverage` | derived table | test or symbol change |
| resolved FQN map | scan-scoped, in memory | not persisted |

Every cache key includes the version of the code that produced the value. This is the piece
that is easy to omit and expensive to omit: upgrade `tree-sitter-java`, and without a version
in the key the content hashes still match, nothing re-parses, and the index silently keeps
symbols the old grammar produced — with no error, indefinitely. `scans.tool_versions_json`
makes the mismatch detectable and forces a re-parse of the affected language.

Parse caches live in `.nexus/cache/ast/<hash>.bin` as `bincode`, are content-addressed
(so identical vendored files across modules parse once), and are pruned by LRU at a
configurable cap (default 2 GB).

---

## 6. Query performance

The two queries that must stay fast under all conditions:

**Reverse traversal** — one indexed seek per frontier node on `idx_edges_dst`. The BFS
touches at most `fan_out_cap × max_depth` nodes by construction, so worst-case cost is
bounded by configuration rather than by graph size. A `Utils` class with 3 000 callers costs
the cap, not 3 000.

**Unresolved-edge sweep** — `idx_edges_unresolved` is a partial index over only the
unresolved edges, typically 2–5 % of the table. Tier-3 re-resolution after a rename is one
indexed lookup per changed FQN. Without the partial index this becomes a full scan of
`symbol_edges` on every rescan that adds a symbol, which is the difference between a 200 ms
rescan and a 40 s one on a large repo.

Prepared statements are cached per connection. `ANALYZE` runs after a full scan so the query
planner has statistics that match the actual distribution.

---

## 7. Memory

A full scan streams. Files are processed in chunks and `ParsedFile` values are dropped after
their rows are written; the process never holds the whole index in memory. The FQN map used
for edge resolution is the one large structure — roughly 100 bytes per symbol, so ~40 MB at
400 k symbols, which is acceptable.

Impact traversal holds only the visited set and the frontier, bounded by the depth and
fan-out caps.

`mmap_size = 256 MB` lets SQLite map the hot pages of the index without copying, which is
where most read performance on a warm cache comes from.

---

## 8. Monorepos

Beyond roughly 1 MLOC, "the project" stops being the right unit.

- `config.toml` defines **modules** with their own include globs. `bughunter rescan --module
  payments` scans and analyzes one module, using the full graph for cross-module impact but
  only re-parsing within the module.
- Cross-module edges are resolved normally; the graph is not partitioned. Partitioning the
  graph would break exactly the queries a monorepo needs most — "who outside my module calls
  this".
- The database stays single-file. SQLite is comfortable at tens of gigabytes, and a
  multi-file scheme would add a consistency problem to solve a problem that has not appeared.
  See §10 for when that changes.

---

## 9. When AI is involved

Cost and latency are dominated by tokens, so the same discipline applies:

- `ContextBuilder` has a hard budget (default 24 k tokens). There is no widening path.
- Only the **affected region** is ever a candidate for context — the output of impact
  analysis, not the changed file set, and never the repository.
- Candidates are ranked and truncated by impact score, so a bigger diff produces a
  *better-prioritized* bundle, not a bigger one.
- Under the default `ai = "agent"`, BugHunter spends no tokens at all: the reasoning happens
  in an agent that already has a context window and a user paying for it.

---

## 10. When this design should change

Honest triggers to revisit, rather than a claim that it scales forever:

| Signal | Change |
|---|---|
| `rescan` no-op exceeds 2 s | the `git status` call dominates — add a watcher; this is the V2 daemon's real justification |
| `impact` exceeds 500 ms at p95 | keep a warm in-memory graph in the daemon rather than rebuilding per process |
| SQLite write contention on shared CI | move to one database per module, or a server-backed store |
| Full scan exceeds 30 min | shard scanning across processes by module and merge |
| Parse cache exceeds available disk | lower the cap; content-addressing already deduplicates |
| Impact recall measurably poor | add the LSP sidecar tier for exact resolution — precision, at a cost |

None of these are hypothetical enough to build for now, and each is a localized change
behind an existing interface. That is the actual test of whether the layering was worth it.
