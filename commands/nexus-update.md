---
description: Check whether Nexus itself is up to date
---

1. Run `nexus --version` and compare it with the latest release at
   https://github.com/zorigtbaatarAst/nexus/releases/latest.
2. If it is behind, tell the user the two commands, and which one they need:
   - the binary: `curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/nexus/main/install.sh | sh`
   - the plugin: `/plugin marketplace update nexus`
   They are separate — the plugin carries prompts, the binary carries the intelligence, and
   updating one does not update the other.
3. Then run `nexus doctor` and relay anything it reports, verbatim including its remedies.
   A schema older than the binary is the one that matters: it is fixed by a rescan, and until
   then the index is stale.
