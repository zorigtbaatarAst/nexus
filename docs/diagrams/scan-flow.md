# Scan Flow — the first scan

`bughunter init` followed by `bughunter scan`. This is the only run that reads the whole
repository. Everything afterwards is incremental.

```mermaid
flowchart TD
    START(["bughunter scan"]) --> DETECT["detect<br/>language · framework · build system<br/>package manager · databases · containers"]
    DETECT --> PROFILE[("project_profile")]
    DETECT --> WALK["walk<br/>ignore-aware traversal"]

    WALK --> HASH["hash<br/>blake3, parallel via rayon"]
    HASH --> FILES[("files")]

    HASH --> PARSE{"language<br/>supported?"}
    PARSE -->|no| SKIP["parse_status = skipped"]
    PARSE -->|yes| TS["tree-sitter parse<br/>per LanguageAnalyzer"]

    TS -->|error| FAIL["parse_status = failed<br/>record error, CONTINUE"]
    TS -->|ok| SYM["extract symbols<br/>sig_hash + body_hash"]

    SYM --> ENRICH["FrameworkPack.enrich<br/>routes · entities · beans"]
    ENRICH --> SYMBOLS[("symbols")]

    SYMBOLS --> RESOLVE["resolve edges<br/>tier 0 exact → 1 heuristic → 2 framework"]
    RESOLVE --> EDGES[("symbol_edges")]

    SYMBOLS --> TESTS["discover tests<br/>frameworks + naming + static calls"]
    TESTS --> TESTTBL[("tests + test_coverage")]

    EDGES --> DETECT2["deterministic detectors<br/>compiler · secrets · Semgrep"]
    DETECT2 --> BUGS[("bugs — SUSPECTED / UNVERIFIED")]

    BUGS --> SCAN[("scans<br/>commit · working_tree_hash · tool_versions")]
    SCAN --> BASE[("baselines → this scan")]
    BASE --> REPORT(["ScanReport: Ok | Degraded"])
    FAIL --> SCAN
    SKIP --> SCAN

    classDef store fill:#0f766e,stroke:#134e4a,color:#ecfdf5
    classDef bad fill:#7c2d12,stroke:#9a3412,color:#fff7ed
    class PROFILE,FILES,SYMBOLS,EDGES,TESTTBL,BUGS,SCAN,BASE store
    class FAIL bad
```

## Two things worth noticing

**A parse failure does not stop the scan.** It sets `files.parse_status = 'failed'`, records
the error, and the run continues, finishing with status `Degraded` and a visible
`2 files failed to parse` line. Aborting would make one bad generated file fatal; skipping
silently would make the index quietly wrong, which is worse.

**Edge resolution runs after every symbol exists.** Resolving an FQN needs the complete
symbol table, so it is a second pass over an immutable in-memory map — which is also why it
parallelizes cleanly.

## Phases and their cost

```mermaid
pie showData
    title Full scan — where the time goes
    "parse" : 55
    "hash" : 20
    "extract symbols" : 10
    "resolve edges" : 8
    "walk + stat" : 5
    "db writes" : 2
```

Parsing dominates, which is why the entire incremental design exists to avoid it.
