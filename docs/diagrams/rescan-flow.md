# Rescan Flow — the tiered cascade

The everyday command. Each tier's job is to make the next tier's input smaller, so cost is
proportional to *what changed* rather than to how much code exists.

```mermaid
flowchart TD
    START(["bughunter rescan"]) --> T0{"Tier 0 — repo gate<br/>commit == baseline AND clean?"}
    T0 -->|yes| NOOP(["no changes since scan-NNN<br/>exit 0 — under 50 ms"])
    T0 -->|no| T1

    subgraph tier1["Tier 1 — file set"]
        T1["candidates:<br/>git diff baseline..HEAD<br/>+ git status --porcelain"]
        T1 --> REACH{"baseline commit<br/>reachable?"}
        REACH -->|no| FULL["fall back to full walk<br/>record kind = full AND SAY SO"]
        REACH -->|yes| STAT{"size + mtime_ns<br/>unchanged?"}
        FULL --> STAT
        STAT -->|yes| SKIPF["skip — no hashing"]
        STAT -->|no| BL["blake3 the bytes"]
        BL --> CMP{"content_hash<br/>differs?"}
        CMP -->|no| TOUCH["touched, not changed<br/>refresh mtime"]
        CMP -->|yes| CHANGED["CHANGED"]
        CHANGED --> REN{"same hash,<br/>new path?"}
        REN -->|yes| RENAME["RENAMED<br/>carry symbol identity<br/>write symbol_aliases"]
    end

    CHANGED --> T2
    RENAME --> T2

    subgraph tier2["Tier 2 — symbol set (changed files only)"]
        T2["re-parse changed files"]
        T2 --> DIFF{"diff by FQN"}
        DIFF -->|"sig_hash differs"| API["API_CHANGED"]
        DIFF -->|"body_hash differs"| BODY["BODY_CHANGED"]
        DIFF -->|"annotations differ"| CONTRACT["CONTRACT_CHANGED"]
        DIFF -->|"new FQN"| ADDED["ADDED"]
        DIFF -->|"missing FQN"| DELETED["DELETED"]
    end

    API --> T3
    BODY --> T3
    CONTRACT --> T3
    ADDED --> T3
    DELETED --> T3

    T3["Tier 3 — re-resolve edges<br/>indexed lookup per changed FQN<br/>not a graph rebuild"]
    T3 --> IMPACT["impact analysis<br/>weighted reverse BFS"]
    IMPACT --> HUNT["hunt over the AFFECTED REGION ONLY"]
    HUNT --> FP["fingerprint each finding"]
    FP --> LIFE["lifecycle: new bug | same bug | regression"]
    LIFE --> WRITE[("scans + changes + bug_occurrences")]
    WRITE --> ADV[("advance baselines pointer")]
    ADV --> DONE(["RescanReport"])

    classDef fast fill:#166534,stroke:#14532d,color:#f0fdf4
    classDef store fill:#0f766e,stroke:#134e4a,color:#ecfdf5
    class NOOP,SKIPF fast
    class WRITE,ADV store
```

## Why the change kind decides the ripple

The `sig_hash` / `body_hash` split exists so that a one-line edit does not fan out to the
entire reverse-reachable set.

```mermaid
flowchart LR
    subgraph api["API_CHANGED · CONTRACT_CHANGED · DELETED"]
        A1["changed symbol"] --> A2["ALL reverse edge types<br/>calls · implements · extends<br/>injects · routes · persists · reads · writes"]
    end

    subgraph body["BODY_CHANGED"]
        B1["changed symbol"] --> B2["data and effect edges only<br/>reads · writes · persists · emits"]
    end
```

A body-only edit cannot break a caller's compilation. It can only reach one through shared
state or an observable effect — so those are the only edges worth traversing, and filtering
there is what keeps an impact report to eleven symbols instead of four hundred.

## Worked example

```mermaid
flowchart TD
    F["PaymentService.java<br/>content_hash changed"]
    F --> M1["createPayment(String, Money)<br/>BODY_CHANGED"]
    F --> M2["refund(String)<br/>API_CHANGED — parameter added"]
    F --> M3["audit(Payment)<br/>DELETED"]
    F --> M4["PaymentService class<br/>CONTRACT_CHANGED — @Transactional removed"]

    M1 --> I1["reads/writes edges<br/>→ 2 symbols"]
    M2 --> I2["all reverse edges<br/>→ 6 symbols"]
    M3 --> I3["all reverse edges, full weight<br/>→ 1 symbol"]
    M4 --> I4["framework expansion<br/>whole tx call subtree → 4 symbols"]

    I1 --> U["union, deduplicated by best score<br/>11 affected symbols · 8 related tests"]
    I2 --> U
    I3 --> U
    I4 --> U
```

One file, four different ripple rules. A file-granular tool reports "PaymentService changed"
and hands a model the whole class.
