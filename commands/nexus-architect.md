---
description: Resume architecture work on Nexus itself — orient from the code, then plan or implement one roadmap task
argument-hint: "[<task-id> | plan <topic>]"
---

Nexus is the subject here, not the tool. The **design of record** is `AGENTS.md`, `CLAUDE.md`
and `docs/*.md`: they describe what is built. `docs/architecture/` describes what Nexus should
become, and `10-roadmap.md` there numbers the tasks. A task is built only when the code shows
it, whatever any document says.

## Orient — every run

1. Read `AGENTS.md`, then `docs/architecture/README.md`. Done when you can name which
   document answers a question about boundaries, the roadmap, evaluation, and non-goals.
2. Derive the state from the environment. Run these and record the answers:
   - `git status -sb && git log --oneline -12` — what landed last, what is unpushed.
   - `make check` — green or not, and the test total.
   - For each task in the current phase of `10-roadmap.md`, check the code for what the
     task delivers. A task is done when its acceptance criterion holds.
   Done when every row of the phase table carries one line of **evidence**: a path, a
   command's output, or a query result.
3. Evidence is what the system does, not what its text contains. A schema question is
   answered against a live database (`nexus init` in a temp repo, then
   `sqlite3 .nexus/nexus.db`); a dependency question against `cargo metadata`; a behaviour
   question by running the binary. A grep that returns nothing has shown the text is absent,
   which is a different fact from the thing being dead — `03-current-state.md` P2 records the
   task that confused the two.
4. Report the phase table, the tree state, the next task in the roadmap's own order, and
   every place the roadmap and the code disagree. Where they disagree, the code is the
   evidence and the document is corrected in the same change.

With no argument, stop after the report.

## `<task-id>` — implement one roadmap task

5. The scope is the named task. If the id is not in `10-roadmap.md`, say so and stop. A
   defect found on the way goes in the summary; it is fixed only when it blocks the task.
6. Branch from `main`. Plan with `superpowers:writing-plans`, execute with
   `superpowers:executing-plans`. Every step ends with `make check` green; the task ends
   with one commit per plan task, each naming the roadmap id.
7. `git add` names files. A directory sweeps in untracked work from another task, and the
   commit then depends on files it does not contain — the branch merged on 2026-09-02
   carries four such commits.
8. Done when the task's acceptance criterion holds against the code, `make check` passes
   from a fresh worktree of the tip, and the summary names what was left undone and why.

## `plan <topic>` — plan without touching code

9. `superpowers:writing-plans`, argued from the design of record. Every task in the plan
   cites the document it implements and ends on a checkable criterion. Write it to
   `docs/superpowers/plans/`. Done when the plan file exists and nothing under `crates/`
   changed.
