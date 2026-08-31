# Investigation Flow — from a screenshot to a suspect

The second entry point. The agent reads the image; BugHunter receives observations and
resolves them against the index it already holds.

```mermaid
flowchart TD
    SHOT(["screenshot + 'this number is wrong'"]) --> AGENT["AI agent reads the image"]
    AGENT --> OBS["SymptomReport<br/>route · visible_text · network · console · since"]
    OBS --> BH["bughunter_investigate"]

    BH --> A["A · route → component<br/>frontend route table"]
    BH --> B["B · visible text → component<br/>FTS over ui_strings + i18n"]
    BH --> C["C · network path → route symbol"]
    BH --> D["D · console frames → file"]

    A --> CONV{"anchors converge?"}
    B --> CONV
    C --> CONV
    D --> CONV

    CONV -->|"one candidate"| TRACE
    CONV -->|"several, or they disagree"| ASK["clarification_required<br/>concrete questions + why<br/>+ what is already resolved"]
    ASK -->|"answers + investigation_id"| TRACE

    TRACE["trace forward across the seam"]
    TRACE --> SEAM["calls_http edge<br/>GET /api/cart/:p"]
    SEAM --> BACK["Controller → Service → Repository → table"]

    BACK --> RANK["rank suspects<br/>on_trace × recency × prior_bugs<br/>× coverage_gap × contract_penalty"]
    BACK --> CONTRACT["contract mismatch detector<br/>DETERMINISTIC — no model"]

    RANK --> OUT(["ranked suspects, each with its path and reason"])
    CONTRACT --> OUT

    classDef ask fill:#78350f,stroke:#92400e,color:#fffbeb
    classDef det fill:#166534,stroke:#14532d,color:#f0fdf4
    class ASK ask
    class CONTRACT det
```

**The image never enters BugHunter.** `SymptomReport.screenshot` holds a path and a hash for
provenance on the bug record; nothing opens it. Vision is reasoning, and reasoning belongs to
the agent.

## The seam

The dependency graph stops at each language boundary. Joining it at the HTTP contract is
what makes a UI symptom reachable to a repository method.

```mermaid
flowchart LR
    subgraph fe["frontend — nexus-lang-ts"]
        C1["TotalsPanel.tsx"] --> C2["useCart()"]
        C2 --> C3["fetch(`${API}/api/cart/${id}`)"]
    end
    subgraph canon["canonical join key"]
        K["GET /api/cart/:p"]
    end
    subgraph be["backend — nexus-lang-java"]
        S1["@GetMapping(&quot;/api/cart/{cartId}&quot;)"] --> S2["CartController#get"]
        S2 --> S3["CartService#totals"]
        S3 --> S4["CartRepository#findItems"]
        S4 --> S5[("cart_items")]
    end
    C3 -->|"dst_fqn_hint"| K
    K -->|"resolution = contract"| S1

    classDef seam fill:#1e3a8a,stroke:#1e40af,color:#eff6ff
    class K seam
```

Path parameters canonicalize positionally — `${id}`, `{id}`, `:id` and `<int:id>` all become
`:p` — so the two sides join without either knowing the other's naming. A frontend call with
no matching route stays `unresolved` and is **reported**, because a silently dropped edge
makes the trace lie by omission.

## Contract mismatch, found without a model

```mermaid
flowchart LR
    DTO["CartDto.java:22<br/>private BigDecimal total_amount"] --> CMP{"compare<br/>serialized shape"}
    USE["TotalsPanel.tsx:34<br/>cart.totalAmount"] --> CMP
    CMP -->|"field name disagrees"| FIND["api-contract finding<br/>detector: contract<br/>confidence 0.90"]
    FIND --> WHY["undefined → renders 0<br/>exactly the reported symptom"]

    classDef det fill:#166534,stroke:#14532d,color:#f0fdf4
    class FIND det
```

Confidence 0.90 is not subject to the 0.75 model clamp, because no model was asked. Both
sides are indexed; comparing them is a join.

## Asking, rather than guessing

```mermaid
sequenceDiagram
    autonumber
    participant H as Human
    participant A as AI agent
    participant B as BugHunter

    H->>A: screenshot + "the total is wrong"
    A->>B: investigate { route, visible_text, network }
    B->>B: anchor → 4 candidate components
    B-->>A: clarification_required<br/>resolved_so_far + 2 questions + why<br/>can_proceed_without: true, confidence 0.35
    A->>H: "Which part — the line items or the summary panel?<br/>They call different endpoints."
    H->>A: "the summary panel at the bottom"
    A->>B: investigate { investigation_id, answers }
    B->>B: resume from stored anchors — no re-interrogation
    B-->>A: trace + ranked suspects + one contract mismatch
```

A question is asked at most once per investigation, and only when the ambiguity was actually
measured. Asking something BugHunter already knows is how a tool teaches people to ignore its
questions.
