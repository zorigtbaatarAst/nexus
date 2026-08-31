---
description: What changed since the baseline, down to the symbol
---

1. Call `nexus_rescan`.
2. If `unchanged` is true, say so in one line and stop.
3. Otherwise report the counts first, then the changed symbols grouped by kind:
   `API_CHANGED` and `CONTRACT_CHANGED` matter most — the second is an annotation change a
   compiler would not notice, and in Spring that often matters more than a signature.
4. Offer `/nexus:impact <symbol>` for anything that changed its API or contract.
