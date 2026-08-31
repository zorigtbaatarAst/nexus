---
description: Explain a symbol and its neighbourhood
argument-hint: <symbol or name>
---

1. Call `nexus_get_symbol` with `target: $ARGUMENTS` and `with_source: true`.
2. If ambiguous, show the candidates and ask. Do not pick.
3. Explain what it depends on and what depends on it, naming the edge types. Say when an edge
   was `heuristic` rather than `exact`.
4. BugHunter has not analyzed behaviour — the reasoning about what the code *does* is yours.
