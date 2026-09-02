# The problem

What is actually wrong today, stated so that a solution can be checked against it.

---

## 1. The failure is not intelligence. It is retrieval and grounding.

Modern coding agents reason well. Given the right five files and the right three facts, Claude
Code produces work a competent engineer would sign off on.

The failure happens *before* the reasoning: the agent does not know which five files, does not
know the three facts, and has no way to find out except by reading — linearly, expensively, and
into a context window it then has to think inside of.

Every symptom below is a consequence of that one thing.

---

## 2. Six concrete failures

### F1 — Every session starts at zero

The agent that spent forty minutes last Tuesday working out that idempotency is enforced in the
controller, not the service, knows nothing about it today. That conclusion cost real tokens and
real wall-clock time, and it evaporated when the session ended.

Nothing in the developer's toolchain persists a conclusion. Files persist code; git persists
diffs; neither persists *what was learned*.

### F2 — Reading is the wrong instrument for most questions

An agent asked "what breaks if I change this method?" opens files. But the answer is not in any
file — it is in the *relationships between* files, which no single file states.

Measured on this repository: 44 Rust files, ~594 KB, **~149,000 tokens to read all of it**, and
~3,400 tokens for an average file. Ten files to chase one question is ~34,000 tokens — and at
the end of it, the reverse-dependency answer is still not written down anywhere, so the agent
has inferred it, which means it might be wrong.

The same question answered from an index: a resolved `ImpactItem` with its full edge chain and
confidence serializes to **~73 tokens**. Twenty affected symbols — a large blast radius —
is **~1,500 tokens**.

> **~34,000 tokens for an inferred answer, versus ~1,500 for a proven one.** That ratio is the
> product.

### F3 — The cross-stack answer is invisible in the source text

Nothing in a TypeScript file connects `fetch('/api/payments')` to a Java method annotated
`@QueryMapping`. No amount of reading either side reveals the edge, because the edge lives in
the `.graphqls` schema that both sides were generated against.

An agent that changes a backend DTO field and reports "done" has no way to know it just broke
three React components. Neither does the compiler.

### F4 — History is invisible

"This module broke in July, was fixed in August, and the fix was a workaround" is not in the
code. It is in commits, in issues, in someone's memory. The agent sees the current state and
treats every file as though it has always looked like this.

The single most useful sentence before editing a function — *this exact thing broke, was fixed,
and broke again* — is unavailable at exactly the moment it would prevent the third occurrence.

### F5 — "Done" is a claim, and it is accepted

An agent finishes an edit and reports completion. Nothing compiles it, runs it, or lints it. The
developer discovers the truth later, in CI or in production, at which point the cheap fix has
become an expensive one.

The agent is not lying. It genuinely cannot tell — it has no verification channel and never had
one.

### F6 — More context makes all of this worse, not better

The obvious response to F1–F4 is to send more: the whole module, the whole service, the whole
repository. This fails three ways at once.

- **Linear cost.** Tokens are money and latency, and the bill scales with what you send, not
  with what was useful.
- **Attention dilution.** Three relevant lines buried in four hundred irrelevant ones are
  measurably harder for a model to use than three lines alone.
- **It still misses F3 and F4.** The seam and the history are not in the files at *any* volume.
  Sending ten times more of the wrong thing does not eventually contain the right thing.

This is why the core principle is a *negation*:

> **Do not give AI more context. Give AI better context.**

---

## 3. Why existing tools do not solve it

| Tool | What it does | Why it is not the answer |
|---|---|---|
| **grep / ripgrep** | Finds text | Finds text. Cannot answer "who calls this", cannot cross the seam, has no history |
| **LSP** | Exact symbol resolution | Per-language sidecars, no persistence, no history, no findings, no cross-stack view. Genuinely useful *as a resolution tier* — which is why it is on the roadmap behind a recall trigger, not as a substitute |
| **RAG over a vector index** | Semantic similarity retrieval | Retrieves things that *read* similar. Dependency, impact and regression are structural facts, not semantic ones. A caller often shares no vocabulary with its callee |
| **The agent's own file reading** | Ground truth | Correct, and the most expensive possible way to obtain it. F2's numbers |
| **CI** | Verification | Right answer, wrong latency. Minutes-to-hours after the agent has moved on |
| **A bigger context window** | More room | Addresses volume; F1, F3 and F4 are not volume problems |

None of these is wrong. Each is a good tool for its own question. The gap is that no tool
**owns the project's accumulated understanding** and serves it, ranked, on demand.

---

## 4. The shape of a solution

Any adequate solution has to satisfy all six constraints simultaneously. That is what makes
this a real design problem rather than a feature request:

| Constraint | Because |
|---|---|
| **Persistent** | F1 — a conclusion must be reached once |
| **Structural, not textual** | F2, F3 — the answer lives in relationships |
| **Historical** | F4 — the ledger must survive, unedited |
| **Selective, not exhaustive** | F6 — the budget is the mechanism, not an afterthought |
| **Deterministic** | Trust and cost — a probabilistic retriever cannot be debugged, and paying a model to answer what a join answers is paying to become less accurate |
| **Invisible to the developer** | Adoption — a tool that changes how someone works is a tool they evaluate; a tool that does not is a tool they keep |

---

## 5. Who has the problem

**Primary:** a developer using Claude Code or Codex on a real production codebase — large enough
that no one holds it in their head, old enough to have history, and crossing at least one
language boundary. This describes almost every commercial system and almost no tutorial.

**Not the target:** greenfield projects small enough to fit in a context window, single-file
scripts, and anyone whose codebase the agent can read entirely for less than the cost of
indexing it. Nexus has negative value there, and saying so is what keeps its scope honest.

---

## 6. How we will know the problem is solved

Falsifiable, measurable, and each one is an acceptance test somewhere in
[`10-roadmap.md`](10-roadmap.md):

1. An agent's first action in a session is not a `Read` of a file it will discard.
2. "What does this change affect?" is answered in one call, across the seam, with the edge chain
   that proves it — for under 2,000 tokens.
3. A conclusion reached expensively last month is retrieved this month for free, and stops being
   retrieved when the code it describes moves.
4. "Done" is followed by a compile, a test run and a verdict.
5. **The total token cost of a task drops, measurably, against the same task without Nexus.**

Point 5 is the only one that matters. The first four are mechanisms; that one is the product,
and if it is not true then the mechanisms were built for nothing.
