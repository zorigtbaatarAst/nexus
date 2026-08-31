---
description: Baseline, drift, and how much of the graph resolved
---

1. Call `bughunter_get_project_context` and `bughunter_get_graph`.
2. Report the baseline, how far it has drifted, and the index size.
3. Report the share of in-project edges that resolved. Below ~80% means impact results are
   missing callers, and that is worth saying before anyone relies on them.
4. If anything looks wrong, call `bughunter_doctor` and relay the remedies verbatim.
