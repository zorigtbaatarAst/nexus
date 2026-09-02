# Memory

Structured engineering knowledge that survives sessions, agents and machines — and that stops
being retrieved when the code it describes moves.

Storage stays SQLite. Markdown/Obsidian is a **view**, never the substrate.

---

## 1. What memory is not

- **Not conversation history.** A transcript is unqueryable, unbounded, unverifiable and
  provider-specific. Storing every agent interaction produces a corpus whose signal-to-noise
  ratio falls with every session — the opposite of the goal.
- **Not embeddings.** No vector database until a measured retrieval miss rate defeats graph +
  lexical + git ranking. That trigger is written down in [`principles.md`](02-principles.md) §10
  precisely so the decision is not made by whoever is most excited about it.
- **Not a wiki.** Nothing is stored that has no evidence and no key.

Everything Nexus learns becomes a row before it is stored. **Anything that cannot be converted
is not stored.**

---

## 2. Categories

The `fact_key` namespace, flat and greppable, extended from the existing five:

| Namespace | Holds | Example key |
|---|---|---|
| `decision.` | A choice made and why | `decision.storage.mongo-over-postgres` |
| `constraint.` | A limit the project must respect | `constraint.api.no-breaking-v1` |
| `convention.` | How this project does a thing | `convention.error-handling` |
| `invariant.` | Something that must always hold | `invariant.payment.status-transitions` |
| `discovery.` | Something worked out that was expensive to work out | `discovery.auth.token-refresh-race` |
| `failure.` | An approach tried that did not work | `failure.cache.write-through-attempt` |
| `incident.` | Something that broke in production, and why | `incident.2026-07.payment-timeout` |
| `pattern.` | A recurring shape worth recognising | `pattern.repository.soft-delete` |
| `risk.` | A known hazard not yet addressed | `risk.payment.no-optimistic-locking` |
| `arch.` | How a module is actually structured | `arch.payment.idempotency` |

`task.` history is deliberately **absent**. Task history is already recorded as
`finding_occurrences`, `changes` and `scans` — the ledgers. A parallel narrative log would be a
transcript by another name.

No new table. A category is a key prefix, which keeps retrieval one query and keeps the
namespace greppable from a terminal — which is how this project's author actually works.

---

## 3. Lifecycle

```
  Observed ──▶ Candidate ──▶ Validated ──▶ Durable
                   │             │            │
                   └─ dropped    └────────────┴──▶ Invalidated
                      (no evidence)                (evidence moved)
                                                        │
                                                        └──▶ re-established or forgotten
```

| State | Entry condition | Storage | Retrieved? |
|---|---|---|---|
| **Observed** | An agent asserts something | **Not stored** | No |
| **Candidate** | Assertion carries a checkable `file:line` that resolves in the index | `facts`, `source='ai'`, `confidence ≤ 0.75` | Yes, marked as candidate |
| **Validated** | Survived a scan: evidence anchor still exists and its symbol's hashes are unchanged | `validated_scan_id` set | Yes, full weight |
| **Durable** | Validated across ≥ 3 scans, or `source='human'` | `durable = 1` | Yes, highest weight |
| **Invalidated** | A scan changed a symbol named in the evidence, or deleted its file | `invalidated_at` set | **No** |

**Observed → dropped is the important edge.** An agent's assertion with no evidence is not
stored at a low confidence — it is refused at the boundary and counted. A memory of an
unfounded guess is worse than no memory: it will be retrieved later and read as established.

### The invalidation rule — the bug this fixes

[`memory-model.md`](../memory-model.md) §2 rule 3 specifies invalidation-by-change. `grep` says
`facts.invalidated_at` is **read in the retrieval query and never written anywhere in the
workspace** (`current-state.md` P10).

So today a fact anchored at `PaymentService#pay():48` outlives that method's deletion and is
served forever as established knowledge, with evidence pointing at a line that no longer means
what it did. That is the exact trap the document warns against, live in the product.

The fix, at the end of every scan:

```
for each live fact:
    for each CodeRef in evidence:
        if file deleted                          → invalidate
        if symbol at anchor deleted              → invalidate
        if symbol.sig_hash or body_hash changed  → invalidate
```

Invalidated rows are **kept**, never deleted — "what did Nexus believe at scan 12, and what
changed its mind" must stay answerable. Append-only, like every other ledger here.

A fact can be re-established: a new fact with the same `fact_key` and fresh evidence supersedes
the invalidated one via the existing `superseded_by` chain.

Estimated cost: ~50 lines plus one store method. It is the highest value-per-line change in
this entire roadmap, because until it lands, every other memory improvement compounds rot.

---

## 4. Retrieval

Retrieval is a stage of the [Context Engine](05-context-engine.md), not a separate subsystem.
Facts compete with symbols, findings and changes on one scoring function and one budget.

```
relevance(fact) = subject_match(fact.subject, seeds)   // exact fqn 1.0 · module 0.6 · project 0.3
                × source_weight(fact.source)           // human 1.0 · deterministic 0.9 · ai 0.7
                × state_weight(fact)                   // durable 1.0 · validated 0.85 · candidate 0.6
                × fact.confidence
                × recency_decay(created_scan_id)       // gentle: old facts are usually still true
                , excluding invalidated_at IS NOT NULL
```

Today's implementation is `ORDER BY source, confidence DESC` — the source and confidence terms
only. Subject match, state weight, recency and the budget are missing. This closes that gap.

Top-K under budget, K defaulting to 12. **Nothing is included "just in case"** — that phrase is
the failure mode the whole engine exists to prevent.

---

## 5. Machine memory vs human knowledge

Two audiences, two representations, **one source of truth**. Conflating them is the mistake that
turns a memory system into a wiki nobody trusts.

|  | Machine memory | Human knowledge |
|---|---|---|
| **Reader** | The Context Engine, ranked and budgeted | A developer, in a PR or on a phone |
| **Store** | SQLite — `facts`, `findings`, ledgers | Markdown files under `docs/knowledge/` |
| **Shape** | Rows with evidence, confidence, provenance, state | Prose with links |
| **Written by** | `nexus fact add`, `nexus_record_fact`, scans | `nexus memory export --markdown` |
| **Authority** | **The source of truth** | A rendering of it |
| **Lifecycle** | Validated, superseded, invalidated by change | Regenerated; never merged back |

### Why SQLite is the substrate and not Markdown

The queries the Context Engine performs are **joins**: facts by subject prefix, filtered on
`invalidated_at IS NULL`, ranked by source and state, intersected with the impact set, all under
a token budget, in under 150 ms on the per-prompt path.

Markdown cannot serve that. Making it try means building an index over the Markdown — which is
SQLite again, with a worse schema and a parsing step in front of it.

SQLite additionally gives transactions (a half-written scan cannot corrupt the index), real
indexes (`idx_facts_subject` is what makes retrieval bounded), and a single file that survives a
`cp`. It is already the substrate and the evaluation confirms it: **keep, unchanged.**

### Why Markdown is still necessary

Nothing about a SQLite row is reviewable in a pull request, readable on a phone, or diffable by a
human. Those are real needs and a generated view serves all three completely — **provided the
direction stays one-way.**

### Where Obsidian sits

Obsidian is a viewer over a folder of Markdown. The exporter emits `[[fact-key]]` wikilinks
between related facts, and that single string convention is the entire integration — one line of
formatting, no plugin, no sync, no schema.

Treating Obsidian as an integration rather than a viewer would invite all three. It is **optional
and essentially free**, and that is exactly how much investment it should receive.

### The rule that keeps the separation honest

> **Nexus never reads `docs/knowledge/`.**

A round trip through Markdown would make an unvalidated text file authoritative over an
evidence-checked row. To *add* knowledge, a human runs `nexus fact add` — entering at
`source='human'`, straight to durable, with provenance recorded. Human knowledge is not
second-class; it just comes through the door that records where it came from.

## 6. The human layer: Markdown as a view

Markdown and Obsidian are for humans, and they are **generated**:

```bash
nexus memory export --markdown docs/knowledge/
```

writes one file per namespace, each fact a section with its claim, evidence links, source,
state and the scan it was learned in.

Rules that keep this a view rather than a second source of truth:

- **Generated, never edited.** The header says so. An edit is overwritten on the next export.
- **Never read back as truth.** Nexus does not parse `docs/knowledge/`. A round trip through
  Markdown would make a text file authoritative over an evidence-checked row, which inverts the
  entire design.
- **Committable.** It is the artefact a team reviews in a PR and reads on a phone. That is a
  real need and it is fully served by a view.
- To *add* knowledge a human writes `nexus fact add`, which enters at `source='human'` and goes
  straight to Durable. Human knowledge is not second-class; it just enters through the door
  that records provenance.

---

## 7. Portability

Unchanged from the existing design, and correct:

- `nexus export --findings` / `--facts` — statuses, history, evidence *references* (paths and
  lines, never source text). Safe to commit or share.
- `nexus import` — merges by fingerprint and fact key. A status conflict is **reported, not
  silently resolved**.
- **The code index is deliberately not exportable.** It is derived and a `scan` rebuilds it in
  under a second. Shipping it invites a stale index that disagrees with the checkout — the
  hardest class of bug to notice and to explain.
