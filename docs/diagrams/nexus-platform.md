# Nexus — the platform shape

Nexus understands the project; capabilities use that understanding. Everything above the line
is the platform; everything below is a capability, and a capability is registered into the
platform rather than compiled into it.

```mermaid
flowchart TD
    subgraph agents["AI coding agents"]
        CC["Claude Code"]
        CX["Codex"]
        CP["Copilot"]
    end
    subgraph adapters["Adapters — thin, no logic"]
        MCP["nexus-mcp"]
        CLI["nexus-cli<br/>nexus · bughunter"]
    end

    CORE["nexus-core — the platform<br/>index · graph · change · impact<br/>findings lifecycle · facts · registry"]

    subgraph understanding["Project understanding"]
        VCS["nexus-vcs<br/>git"]
        LANG["nexus-lang*<br/>Java · TS · GraphQL"]
        STORE[("nexus-store<br/>SQLite")]
    end

    subgraph caps["Capabilities — registered, not compiled in"]
        BH["cap-bughunter"]
        REV["Code Review<br/>(later)"]
        SEC["Security<br/>(later)"]
    end

    CC --> MCP
    CX --> MCP
    CP --> MCP
    MCP --> CORE
    CLI --> CORE
    CORE --> VCS
    CORE --> LANG
    CORE --> STORE
    CORE -->|"hands a ProjectContext<br/>and a Scope"| BH
    BH -->|"returns findings"| CORE
    CORE -.-> REV
    CORE -.-> SEC

    classDef core fill:#1f2937,stroke:#111827,color:#f9fafb
    classDef store fill:#0f766e,stroke:#134e4a,color:#ecfdf5
    classDef later fill:#374151,stroke:#1f2937,color:#9ca3af
    class CORE core
    class STORE store
    class REV,SEC later
```

## Forbidden edges

Dashed red is what `crates/nexus-cli/tests/boundaries.rs` fails the build on. These are not
conventions; the rules that make "add a capability later" true are checked mechanically.

```mermaid
flowchart LR
    CLI["nexus-cli"] --> CORE["nexus-core"]
    MCP["nexus-mcp"] --> CORE
    CLI --> CAP["cap-bughunter"]
    MCP --> CAP
    CAP --> CORE
    CORE --> STORE["nexus-store"]

    CORE -.->|forbidden| CAP
    CORE -.->|forbidden| MCP
    CORE -.->|forbidden| CLI
    CAP  -.->|forbidden| STORE
    CAP  -.->|forbidden| MCP
    CAP  -.->|forbidden| CLI

    linkStyle 6,7,8,9,10,11 stroke:#dc2626,stroke-width:2px,stroke-dasharray:5 5
```

`nexus-core ↛ cap-*` is the one that matters most. The reverse dependency would mean the
platform knows its capabilities, and "add Code Review later" would become a core change —
which is precisely the coupling this split exists to remove.

## Analysis, scoped

```mermaid
sequenceDiagram
    autonumber
    participant U as nexus analyze --changed
    participant E as Engine
    participant S as nexus-store
    participant C as cap-bughunter

    U->>E: analyze("bughunter", Scope::Changed)
    E->>S: symbols, edges, files, and what moved
    Note over E: the rescan cascade already worked out<br/>which symbols changed — this reads it,<br/>it does not recompute it
    E->>C: ProjectContext + Scope
    C->>C: ctx.scoped(scope) — narrow once, centrally
    C-->>E: findings (rules only; no storage, no git)
    E->>E: reject anything with no file:line evidence
    E->>S: upsert by (capability, fingerprint), append occurrence
    Note over E,S: new · recurring · regressed decided here,<br/>so no capability re-implements the answer
    E-->>U: AnalyzeReport, incl. symbols_examined
```

A narrowed run reports only what it examined, and **may not close what it did not look at** —
the fixed-sweep runs under `Scope::Everything` alone. Absence is evidence only when something
actually looked.
