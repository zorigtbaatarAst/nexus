---
description: Baseline, drift, and how much of the graph resolved
---

1. Call `nexus_get_project_context` and `nexus_get_graph`.
2. Report the baseline, how far it has drifted, and the index size.
3. Report the share of in-project **call sites** that resolved. This is *coverage*, not
   accuracy: it says how much of the graph exists, never how much of it is correct, because
   nothing verifies that a bound destination is the right one. Low coverage means impact
   results are missing callers, which is worth saying before anyone relies on them.
4. If anything looks wrong, call `nexus_doctor` and relay the remedies verbatim.
