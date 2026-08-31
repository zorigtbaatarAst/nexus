# MCP Integration

One server, one tool surface, every MCP-capable agent. No per-agent implementation exists in
the repository and none is planned.

```mermaid
flowchart TD
    CC["Claude Code<br/>.mcp.json"]
    CX["OpenAI Codex<br/>~/.codex/config.toml"]
    CP["GitHub Copilot<br/>.vscode/mcp.json"]
    LO["Future local agent<br/>any MCP client"]

    CC --> S
    CX --> S
    CP --> S
    LO --> S

    S["bughunter mcp<br/>stdio JSON-RPC"]
    S --> H["nexus-mcp handlers<br/>deserialize → ONE Engine call → serialize"]
    H --> E["nexus-core Engine"]
    E --> DB[("SQLite")]

    classDef thin fill:#1e3a8a,stroke:#1e40af,color:#eff6ff
    classDef store fill:#0f766e,stroke:#134e4a,color:#ecfdf5
    class H thin
    class DB store
```

Every client above is configured with the same two tokens: `command: "bughunter"`,
`args: ["mcp"]`. A new agent that supports MCP is supported on the day it ships.

## A typical agent session

```mermaid
sequenceDiagram
    autonumber
    participant A as AI agent
    participant M as bughunter mcp
    participant E as nexus-core Engine
    participant D as SQLite

    A->>M: bughunter_get_project_context
    M->>E: project_context
    E->>D: profile + top facts
    D-->>A: spring-boot 3.5 · gradle · 42k symbols · baseline a81f92c

    A->>M: bughunter_rescan
    M->>E: rescan
    E->>D: tiered cascade, write scan-014
    D-->>A: 4 files · 17 symbols · 2 dependencies

    A->>M: bughunter_get_impact
    E->>D: weighted reverse BFS
    D-->>A: 11 affected symbols with paths · 8 tests · truncated false

    A->>M: bughunter_get_symbol detail full
    D-->>A: signature · annotations · 40-line body · callers · prior bugs

    Note over A: the agent reasons — BugHunter does not

    A->>M: bughunter_record_bug with file:line evidence
    M->>E: record_bug
    E->>E: validate evidence, else REJECT
    E->>D: fingerprint → new bug BUG-104
    D-->>A: BUG-104 · UNVERIFIED · confidence 0.71

    A->>M: bughunter_verify_bug BUG-104
    M->>E: verify_bug
    E->>E: plan · emit · run now · run baseline · judge
    D-->>A: reproduced · regression · VERIFIED · 0.97 · introduced a81f92c
```

No file is uploaded, no repository is traversed by the agent, and BugHunter calls no model.
The agent brought the reasoning; BugHunter brought the evidence, the history and the proof.

## Permission gating

An `execute`-class tool consults `policy.toml` before doing anything, and returns a
*structured refusal* rather than an error or a silent execution.

```mermaid
sequenceDiagram
    participant A as AI agent
    participant M as bughunter mcp
    participant P as policy.toml
    participant H as human

    A->>M: bughunter_verify_bug BUG-104
    M->>P: execute permitted?
    P-->>M: execute = "none"
    M->>M: write the test to disk anyway
    M->>M: append audit_events row
    M-->>A: status permission_required<br/>requested_command · sandbox<br/>to_allow: set execute = "docker"<br/>test_written_to: .nexus/generated-tests/BUG-104/
    A->>H: "May I run this? Here is the exact command and the config line."
```

The brief's rule holds on both halves: never silently, and never without a policy a human
committed to the repository.

## Response budgeting

An agent's context is the scarcest resource in the system, so no tool may flood it.

```mermaid
flowchart LR
    Q["tool call<br/>max_items · cursor · detail"] --> E["Engine returns<br/>412 affected symbols"]
    E --> B{"serialize and measure<br/>under 8k tokens?"}
    B -->|yes| OUT["items + truncated false"]
    B -->|no| CUT["rank by impact score<br/>cut to budget"]
    CUT --> OUT2["items<br/>truncated TRUE<br/>total 412<br/>next_cursor<br/>note: narrow with min_score"]

    classDef warn fill:#78350f,stroke:#92400e,color:#fffbeb
    class OUT2 warn
```

Truncation is measured on the serialized payload, not guessed from item counts — and it is
never silent. An agent that does not know it received a partial answer will reason
confidently from it.

## Tool classes

```mermaid
flowchart TD
    subgraph read["read — always permitted"]
        R1["get_project_context"]
        R2["get_symbol"]
        R3["get_changes"]
        R4["get_impact"]
        R5["get_tests_for"]
        R6["get_bug · get_bug_history · get_regressions"]
        R7["scan_status"]
    end

    subgraph write["write — mutates the local store only"]
        W1["init · scan · rescan"]
        W2["record_bug · record_fact"]
    end

    subgraph exec["execute — policy-gated, audit-logged"]
        X1["verify_bug"]
    end

    subgraph ai["ai — may build an evidence bundle"]
        AI1["find_bugs"]
    end

    classDef danger fill:#7c2d12,stroke:#9a3412,color:#fff7ed
    class X1 danger
```

There is no "run an arbitrary command" tool, and there will not be one in any version. The
allowlist in `policy.toml` is the entire execution surface.
