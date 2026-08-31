# Bug Verification Flow

The feature that separates a suspicion from a finding. Steps 1 and 2 may involve a model;
steps 3, 4 and 5 are entirely deterministic — and they decide the outcome.

```mermaid
flowchart TD
    BUG(["bug in status UNVERIFIED<br/>confidence 0.71"]) --> POL{"policy.execute"}

    POL -->|none| REFUSE["return permission_required<br/>test still written to disk<br/>exit 4 / structured MCP result"]
    POL -->|docker / host| PLAN

    PLAN["1 · PLAN<br/>hypothesis · target · preconditions<br/>trigger · expected_failure · isolation · repetitions"]
    PLAN --> EMIT["2 · EMIT<br/>SafeWriter jail:<br/>.bughunter/generated-tests/BUG-104/<br/>production code unreachable"]

    EMIT --> RUNNOW["3 · RUN NOW<br/>current revision, in the sandbox"]
    RUNNOW --> RUNBASE["4 · RUN BEFORE<br/>same test, baseline revision<br/>detached read-only git worktree"]

    RUNBASE --> JUDGE{"5 · JUDGE"}

    JUDGE -->|"FAIL now · PASS before"| V1["reproduced — REGRESSION<br/>confidence → 0.97<br/>status VERIFIED"]
    JUDGE -->|"FAIL now · FAIL before"| V2["reproduced_preexisting<br/>confidence → 0.90<br/>NOT introduced by this change"]
    JUDGE -->|"FAIL now · baseline unavailable"| V3["reproduced<br/>confidence → 0.85<br/>capped, and said so"]
    JUDGE -->|"PASS now · PASS before"| V4["not_reproduced<br/>confidence x 0.5<br/>stays UNVERIFIED"]
    JUDGE -->|"PASS now · FAIL before"| V5["inconclusive<br/>confidence unchanged<br/>flag for a human"]
    JUDGE -->|"n-of-m disagree"| V6["flaky<br/>confidence capped at 0.75"]
    JUDGE -->|"compile error / timeout"| V7["error<br/>confidence UNCHANGED<br/>output attached"]

    V1 --> LEDGER[("bug_verifications + test_runs<br/>immutable")]
    V2 --> LEDGER
    V3 --> LEDGER
    V4 --> LEDGER
    V5 --> LEDGER
    V6 --> LEDGER
    V7 --> LEDGER

    classDef good fill:#166534,stroke:#14532d,color:#f0fdf4
    classDef bad fill:#7c2d12,stroke:#9a3412,color:#fff7ed
    classDef store fill:#0f766e,stroke:#134e4a,color:#ecfdf5
    class V1,V2 good
    class REFUSE,V7 bad
    class LEDGER store
```

**Step 4 is not optional polish.** Running the same generated test against the baseline
revision is the only way to tell "this change introduced a bug" from "this suite was already
red" — and therefore the only honest route from 71 % to 97 %.

**Step 7's rule matters as much:** an infrastructure failure leaves confidence *unchanged*.
Lowering it would punish the hypothesis for a broken pipe.

## Bug lifecycle

```mermaid
stateDiagram-v2
    [*] --> SUSPECTED : detector fires

    SUSPECTED --> UNVERIFIED : evidence attached<br/>non-empty CodeRef set
    SUSPECTED --> IGNORED : human dismisses

    UNVERIFIED --> VERIFIED : reproduction test fails now,<br/>passes on baseline
    UNVERIFIED --> UNVERIFIED : not reproduced<br/>confidence x 0.5
    UNVERIFIED --> IGNORED : human dismisses

    VERIFIED --> FIXED : stored reproduction test<br/>PASSES on a later revision
    VERIFIED --> IGNORED : human dismisses

    FIXED --> REGRESSED : the same test fails again
    REGRESSED --> VERIFIED : re-confirmed

    IGNORED --> SUSPECTED : structural_key changed<br/>it is no longer the same finding

    note right of FIXED
        FIXED REQUIRES EVIDENCE.
        Absence from an incremental
        scan means the region was
        not examined — never that
        the bug is gone.
    end note
```

That note is the most important rule in the state machine. Without it, touching an unrelated
file silently closes real bugs, and the whole history becomes untrustworthy.

## Where confidence comes from

```mermaid
flowchart LR
    D["deterministic detector<br/>Semgrep, compiler, secret scan"] -->|0.85 - 0.95| C(("confidence"))
    A["AI candidate<br/>clamped at 0.75, never higher"] -->|"<= 0.75"| C
    C --> VER{"verification run"}
    VER -->|reproduced| UP["0.95 - 0.97"]
    VER -->|not reproduced| DOWN["x 0.5"]
    VER -->|error / timeout| SAME["unchanged"]
```

A model is not permitted to grade its own work: only the verification engine can push a bug
above 0.75, and only by reproducing it.
