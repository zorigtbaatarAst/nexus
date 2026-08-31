---
description: Index this project with BugHunter and report what it is
---

1. Call `bughunter_get_project_context`. If it reports a baseline already, say so and stop —
   `/bughunter:rescan` is the cheaper command.
2. Otherwise call `bughunter_scan`.
3. Report the detected languages, frameworks, build system and databases, the symbol count,
   and the share of in-project edges that resolved.
4. If `health` is `degraded`, say which files failed to parse. Do not round that off.
