# BugHunter — Project Memory Model

> **Status: built.** The `facts` table, supersession, evidence-checked recording, the
> candidate/validated/durable lifecycle and the full retrieval formula all work. §3's
> retrieval lives in `nexus_core::memory::relevance`, called by both the ask path and the
> Context Engine, under a token budget.
> Invalidation-by-change (§2 rule 3) is implemented: `Engine::fact_anchors` resolves each
> fact's evidence before a scan, and `Store::invalidate_moved_facts` sets `invalidated_at`
> inside the scan's transaction when the file is gone or the symbol at the anchor is gone or
> has a different `sig_hash` or `body_hash`. Rows are kept. Pinned by
> `crates/nexus-core/tests/fact_invalidation.rs`.

Memory is what makes the second scan cheaper than the first and the tenth scan smarter than
the second. It lives in SQLite at `.nexus/nexus.db` and survives across scans,
sessions, agents and machines.

**It is not conversation history.** A chat transcript is unqueryable, unbounded, unverifiable
and provider-specific. Everything BugHunter learns is converted into structured rows before
it is stored, and anything that cannot be converted is not stored.

---

## 1. Four layers

| Layer | Tables | Lifetime |
|---|---|---|
| **Project knowledge** — what this system is | `project_profile`, `external_deps`, module/service symbols | replaced when detection re-runs |
| **Code knowledge** — what the code contains | `files`, `symbols`, `symbol_edges`, `symbol_aliases`, `tests`, `test_coverage` | current state, soft-deleted |
| **Historical knowledge** — what has happened | `scans`, `commits`, `changes`, `test_runs`, `audit_events` | immutable, append-only |
| **Bug knowledge** — what has been wrong | `bugs`, `bug_occurrences`, `bug_verifications`, `bug_relations` | summary mutable, evidence immutable |

Plus `facts` — the layer that holds what does not fit in any of the above.

---

## 2. Facts: turning discoveries into structure

An agent investigating a change discovers things that are true about the project but are not
symbols, edges or bugs:

> "Payments are made idempotent by the caller-supplied `Idempotency-Key` header, checked in
> `PaymentController`, not in `PaymentService`."

That is expensive to rediscover and cheap to store. It becomes a row:

```json
{
  "fact_key":   "arch.payment.idempotency",
  "scope":      "module",
  "subject":    "mn.pay",
  "claim":      "Idempotency is enforced at PaymentController via the Idempotency-Key header, not in PaymentService.",
  "source":     "ai",
  "evidence":   [{"file":"src/main/java/mn/pay/PaymentController.java","line":48}],
  "confidence": 0.8
}
```

### Rules

1. **A fact must carry evidence or be marked `human`.** An `ai`-sourced fact with an empty
   `evidence_json` is rejected at the boundary. A memory of an unfounded guess is worse than
   no memory: it will be retrieved later and treated as established.
2. **Facts are never edited.** A new fact with the same `fact_key` supersedes the old one via
   `superseded_by`. You can always ask what BugHunter believed at scan 12 and what changed
   its mind.
3. **Facts are invalidated by change, not by age.** When a scan changes a symbol referenced
   in a fact's `evidence_json`, that fact gets `invalidated_at` set and is excluded from
   retrieval until re-established. A fact about code that no longer exists is a trap.
4. **`source` is always recorded** — `deterministic` (derived by BugHunter itself),
   `ai` (proposed by a model, evidence-checked), `human` (entered via
   `bughunter fact add`). Retrieval ranks `human` > `deterministic` > `ai`, and the CLI shows
   the provenance. Constraint 14 in memory form.

### Fact key namespace

Flat, dotted, greppable:

```
arch.<module>.<topic>          arch.payment.idempotency
convention.<topic>             convention.error-handling
invariant.<subject>            invariant.payment.status-transitions
risk.<subject>                 risk.payment.no-optimistic-locking
decision.<topic>               decision.storage.mongo-over-postgres
```

---

## 3. Retrieval

`ContextBuilder` (in `nexus-core::context`) assembles what an agent or provider sees. Facts are
selected by relevance to the current question, never dumped:

```
relevance(fact) =
      subject_match(fact.subject, focus_symbols)     -- exact fqn 1.0, module 0.6, project 0.3
    × source_weight(fact.source)                     -- human 1.0, deterministic 0.9, ai 0.7
    × fact.confidence
    × recency_decay(fact.created_scan_id)            -- gentle; old facts are usually still true
    , excluding invalidated_at IS NOT NULL
```

Top-K under a token budget, K defaulting to 12. Everything in a context bundle is chosen by
this function; nothing is included "just in case".

---

## 4. What memory buys, concretely

```
scan 1     index 42 k symbols · 3 facts recorded · 2 bugs SUSPECTED
scan 2     4 files changed → 17 symbols → 11 affected → hunt over 11, not 42 000
           BUG-104 fingerprint matches scan 1 → occurrence appended, no duplicate
scan 3     BUG-104 verification test passes → FIXED, fixed_commit recorded
scan 9     the same test fails again → REGRESSED, with the full history of both
           the original introduction and the fix attached
```

Scan 9's conclusion is only possible because scans 1, 3 and 9 all still exist unedited. That
is the immutability doctrine in [data-model.md](data-model.md) §2 paying for itself.

---

## 5. Portability

Memory is local by default and never leaves the machine. Two escape hatches:

- `bughunter export --bugs > bugs.json` — fingerprints, statuses, verification history and
  evidence *references* (paths and line numbers, never source text). Safe to commit or share
  with a team.
- `bughunter import bugs.json` — merges by fingerprint. A teammate's `VERIFIED` becomes your
  `VERIFIED`; a status conflict is reported, not silently resolved.

The code index is deliberately **not** exportable. It is a derived artifact that a `scan`
rebuilds in seconds, and shipping it around would invite stale indexes that disagree with
the checkout — the class of bug that is hardest to notice and hardest to explain.
