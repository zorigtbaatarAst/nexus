---
description: What breaks if this symbol changes, across the whole stack
argument-hint: <symbol, file, or name>
---

1. Call `nexus_get_impact` with `target: $ARGUMENTS`.
2. If the status is `ambiguous`, show the candidates and ask which one. Do not pick.
3. Report affected symbols by score, and say plainly when a result crossed the
   frontend/backend seam — a backend change reaching a UI component is the finding, not a
   detail.
4. Quote each result's `min_confidence` honestly: below 0.7 is a lead, not a fact.
5. If `truncated` is true, say so and use the suggested narrowing rather than guessing.
