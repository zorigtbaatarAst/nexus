---
description: Run a capability over the project
argument-hint: [capability] [--changed]
---

1. Call `nexus_capabilities` if $ARGUMENTS names none, and say which are available.
2. Call `bughunter_analyze` (or the named capability's analyze tool).
3. Report counts first — new, recurring, regressed, closed — then the findings themselves.
   `regressed` is the one worth leading with: it broke, was fixed, and broke again.
4. These are deterministic rules. Do not describe their confidence as an estimate.
5. What the rules cannot see is business logic, races and data consistency. If you reason
   one out yourself, record it with `nexus_record_finding` and concrete `file:line` evidence
   — a claim without one is refused, and rightly.
