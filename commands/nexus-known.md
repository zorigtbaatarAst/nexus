---
description: What is already known about this code
argument-hint: <file, symbol, or component>
---

1. Call `nexus_get_known` with `target: $ARGUMENTS`.
2. Report the findings recorded there before — their status matters as much as their title:
   a `REGRESSED` finding means this broke, was fixed, and broke again.
3. Report the facts, and say who recorded each: a `human` fact outranks an `ai` one.
4. If nothing is known, say so plainly. That is useful information, not an empty result.
