# System Architecture

The layer map. Every arrow points downward: `bh-core` has no knowledge that MCP, the CLI or
any AI provider exists.

```mermaid
flowchart TD
    subgraph agents["AI Coding Agents"]
        CC["Claude Code"]
        CX["OpenAI Codex"]
        CP["GitHub Copilot"]
        OT["Any MCP client"]
    end

    subgraph adapters["Adapters — thin, no business logic"]
        MCP["bh-mcp<br/>rmcp over stdio"]
        CLI["bh-cli<br/>clap + renderers"]
    end

    CORE["bh-core — Engine<br/>all business logic lives here"]

    subgraph caps["Capabilities"]
        VCS["bh-vcs<br/>git2"]
        LANG["bh-lang<br/>LanguageAnalyzer + FrameworkPack"]
        VER["bh-verify<br/>plan · emit · sandbox · judge"]
        AI["bh-ai<br/>AiProvider trait"]
    end

    subgraph langs["Language crates"]
        JAVA["bh-lang-java<br/>+ Spring pack"]
        TS["bh-lang-ts"]
        PY["bh-lang-python"]
        RS["bh-lang-rust"]
    end

    STORE["bh-store — SQLite<br/>the only crate containing SQL"]

    CC --> MCP
    CX --> MCP
    CP --> MCP
    OT --> MCP

    MCP --> CORE
    CLI --> CORE

    CORE --> VCS
    CORE --> LANG
    CORE --> VER
    CORE --> AI
    CORE --> STORE

    LANG --> JAVA
    LANG --> TS
    LANG --> PY
    LANG --> RS

    classDef core fill:#1f2937,stroke:#111827,color:#f9fafb
    classDef store fill:#0f766e,stroke:#134e4a,color:#ecfdf5
    class CORE core
    class STORE store
```

## The intelligence pipeline

What the layers actually produce, in order:

```mermaid
flowchart LR
    G["Git analysis<br/>commits · diffs · blame"]
    C["Code analysis<br/>symbols · edges"]
    T["Test analysis<br/>tests · coverage"]
    M["Project memory<br/>SQLite"]
    B["Bug intelligence<br/>fingerprints · lifecycle"]
    V["Verification<br/>generate · run · judge"]

    G --> M
    C --> M
    T --> M
    M --> B
    B --> V
    V -->|"evidence written back"| M
```

## Boundary rules, as a graph

Dashed red edges are the dependencies a `cargo metadata` test forbids. These are what turn
the brief's constraints 1, 2, 3 and 12 into build failures rather than good intentions.

```mermaid
flowchart LR
    CLI["bh-cli"] --> CORE["bh-core"]
    MCP["bh-mcp"] --> CORE
    CORE --> STORE["bh-store"]
    CORE --> LANG["bh-lang"]
    CORE --> VERI["bh-verify"]
    CORE -->|"default-features = false<br/>trait only, no HTTP"| AI["bh-ai"]

    CORE -.->|forbidden| MCP
    CORE -.->|forbidden| CLI
    MCP  -.->|forbidden| STORE
    MCP  -.->|forbidden| VERI
    LANG -.->|forbidden| STORE

    linkStyle 6,7,8,9,10 stroke:#dc2626,stroke-width:2px,stroke-dasharray:5 5
```

## On-disk state

```mermaid
flowchart TD
    ROOT[".bughunter/"]
    ROOT --> CFG["config.toml<br/>committed"]
    ROOT --> POL["policy.toml<br/>committed"]
    ROOT --> DB["bughunter.db<br/>local · WAL"]
    ROOT --> CACHE["cache/<br/>parse caches + worktrees"]
    ROOT --> GEN["generated-tests/<br/>the ONLY writable path for bh-verify"]
    ROOT --> AUD["audit.log<br/>append-only JSONL"]

    classDef committed fill:#1e3a8a,stroke:#1e40af,color:#eff6ff
    classDef jail fill:#7c2d12,stroke:#9a3412,color:#fff7ed
    class CFG,POL committed
    class GEN jail
```
