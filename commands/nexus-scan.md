---
description: Index this project with BugHunter and report what it is
---

1. Call `nexus_get_project_context`. If it reports a baseline already, say so and stop —
   `/nexus:rescan` is the cheaper command.
2. Otherwise call `nexus_scan`.
3. Report the detected languages, frameworks, build system and databases, the symbol count,
   and the share of in-project call sites that resolved — coverage, not accuracy.
4. If `health` is `degraded`, say which files failed to parse. Do not round that off.
