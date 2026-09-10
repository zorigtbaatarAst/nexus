---
name: nexus
description: "Answers what the source text cannot: what a change breaks across the frontend/backend seam, what moved since the last scan down to the symbol, what has already gone wrong in a file, and which Spring proxy mistakes or orphaned GraphQL fields exist. Use it whenever the question is what something affects, touches or depends on; whenever you are about to edit code you have not read; and after finishing an edit, before calling it done. Prefer it over grep and reading files to trace a dependency — nothing in the text connects fetch('/api/x') to @QueryMapping, but the index does. For style opinions and for running tests, look elsewhere."
metadata:
  version: "0.3.0"
  user-invocable: "true"
---

# Nexus

Persistent code intelligence. Nexus indexed this project once and remembers its structure,
its git history, its dependency graph, and every finding recorded in it. **It knows things
this session does not.** Ask before deriving.

## When it earns its keep

**Before editing code you have not read.** `nexus_get_known` answers "what has gone wrong
here before, and what did a previous session work out about it" — a `REGRESSED` finding means
this exact thing broke, was fixed, and broke again, which is worth knowing before you touch it.

**When asked what a change affects.** `nexus_get_impact` traverses the real dependency graph
and crosses the GraphQL seam, so a change to a backend service method reaches the React
components that render it. Reading files cannot answer this: nothing in the source text
connects `fetch('/api/x')` to `@QueryMapping`.

**When you need to know what moved.** `nexus_rescan` reports changes down to the symbol and
distinguishes `API_CHANGED` from `BODY_CHANGED` from `CONTRACT_CHANGED` — the last being an
annotation change a compiler never notices and that, in Spring, often matters most.

## Three capabilities, three moments

Nexus understands the project once; each capability uses that understanding at a different
point in the work. Reach for the one that matches the moment rather than running all three.

| Moment | Capability | The question it answers |
|---|---|---|
| Starting in an unfamiliar project | **Architect** | What is this, and what does working in it need that is missing? |
| An edit is finished, before it is accepted | **Review** | What does this change reach, and what covers it? |
| A bug is suspected or reported | **BugHunter** | Where is it, and what proves it? |

**Architect** runs automatically with the first `nexus_scan`. It reports what the project is
built from and what an agent working in it lacks — a datastore with no MCP server configured
to reach it, no CI, or a scan that is looking at one module of something larger. That last one
matters most: it means every impact answer here is understated, and you should say so rather
than trusting the number.

**Review** runs on what changed and nothing else. Call it after editing, before telling anyone
the work is done. It reports a change no test reaches, a contract change that reaches frontend
code nobody touched, and a signature whose callers did not move with it — none of which the
diff shows, because none of them are in the files you edited.

**BugHunter** is for a suspected defect, not for a routine check. Its rules are deterministic:
Spring proxy mistakes, and GraphQL fields no resolver serves.

All three run through one tool, `bughunter_analyze` — the name is historical, the `capability`
argument is what picks. It defaults to bughunter, so pass `capability` explicitly for the other
two, and pass `changed: true` to scope the run to the edit instead of the whole index.

## A normal sequence

```
nexus_get_project_context     what is this, and has the baseline drifted
nexus_rescan                  what changed since it was last indexed
nexus_get_impact <symbol>     what breaks, including on the other side of the stack
nexus_get_known <file>        what is already known about this code
  … make the change …
nexus_rescan                  what the edit moved
bughunter_analyze             capability: review, changed: true — what the edit reaches,
                              and what covers it
```

## Reading the answers honestly

**Confidence is not decoration.** Every impact result carries the edge chain that produced it
and `min_confidence`, the weakest link along that chain. Below 0.7 the path went through a
heuristic hop: report it as a lead, not a fact.

**A deterministic finding is not a model estimate.** `bughunter_analyze` runs rules — both
sides of every claim are in the index and comparing them is a query. Do not hedge those the
way you would hedge your own inference.

**Truncation is stated.** A result carrying `truncated: true` gives the true total and a way
to narrow. Use it rather than reasoning from a partial answer.

## Contributing what you work out

Nexus finds what rules can express. Business-logic errors, races, transaction boundaries and
data-consistency problems are yours — and `nexus_record_finding` is how they survive the
session. A recorded finding gets the same identity and history a rule's does, so the same
observation next week is recognized rather than repeated.

Two rules, and both are enforced rather than advisory:

- **Every finding needs `file:line` evidence.** Evidence pointing at a file outside the index
  is rejected, not stored. A claim nobody can check is not a finding.
- **Your confidence is capped at 0.75.** Nothing here runs tests, so nothing is proven by
  reproduction. Do not present a recorded finding as certain.

`nexus_record_fact` is for what is true about the project but is not a symbol, an edge or a
finding — an invariant, a convention, a decision and its reason. Expensive conclusions should
be reached once.

## Knowledge someone already extracted

If the project has a `graphify-out/graph.json`, call `nexus_import_knowledge` once. graphify's
semantic pass already read the design documents and produced claims about this project;
importing them turns each into a fact anchored on the symbol it names, so it surfaces while
that symbol is being edited instead of sitting in a document nobody opens.

It costs nothing per request afterwards. The budget is a ceiling, so more knowledge changes
*which* items a package carries, never how many tokens it is: on the Nexus repository, going
from 0 to 2,171 facts moved a package from 468 to 712 tokens and no further, while the 19
documents those claims came from are about 79,900 tokens to read.

Imported claims are `ai` facts at confidence 0.5. Nothing has checked them against the code,
so treat one as a lead worth confirming, not as a settled fact.

## What it does not do

It runs nothing unless the project's committed `.nexus/policy.toml` allows it: `nexus verify`
defaults to `execute = "none"` and returns a result rather than an execution. Even where it
does run, it runs the project's own build, test and lint — no finding is proven by a generated
reproduction yet, which is why model confidence stays capped at 0.75.

And it does not reason: the index, the graph and the history are evidence, and what they mean
is your job.

Review in particular has no opinion about how code is written. It will not comment on naming,
formatting or structure, and it is not trying to — those are taste, and taste is what the tool
deliberately stays out of. Every rule it has reports something the index can prove.
