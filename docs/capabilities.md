# Nexus — Capabilities

> Nexus understands the project; capabilities use that understanding.

A capability is handed a prepared view of the index and a scope, and returns findings. Nexus
owns identity, lifecycle, storage and presentation — so a capability never asks whether a
finding is new, whether it has been seen before, or how to display it.

This is the whole extension point. There is no plugin loader, manifest format or lifecycle
protocol, because none of those solve a problem that exists yet.
See [ADR-019](architecture-decisions.md#adr-019--one-capability-abstraction-and-no-plugin-system).

---

## The contract

```rust
pub trait Capability: Send + Sync {
    fn id(&self) -> &'static str;              // "bughunter" — stored on every finding
    fn finding_prefix(&self) -> &'static str;  // "BUG"       — findings are numbered BUG-n
    fn describe(&self) -> &'static str;
    fn analyze(&self, ctx: &ProjectContext<'_>, scope: &Scope) -> Result<Vec<Finding>, CapabilityError>;
}
```

`id` is stored on every finding it produces. **Renaming it orphans them**, so it is chosen
once.

## What a capability is given

`ProjectContext` is a snapshot, not the store. That keeps capabilities pure and testable, and
it keeps SQL in `nexus-store` where boundary rule 3 says it belongs — a capability that could
reach the database would grow queries the CLI cannot answer.

| Field | What it holds |
|---|---|
| `symbols` | every indexed symbol: fqn, kind, file, line, visibility, parent, annotations |
| `edges` | the dependency graph, each edge with its resolution tier and confidence |
| `files` | indexed files and their language |
| `changed` | what moved in the scan under analysis |
| `commit` | the revision this snapshot describes |
| `by_fqn` | an index, built once — scanning the symbol list per edge turns a linear pass quadratic |

## Scope, and why narrowing is not optional

```rust
pub enum Scope { Everything, Changed { since_scan }, Symbols(Vec<String>), Files(Vec<String>) }
```

`ctx.scoped(scope)` returns the narrowed view. **Iterate `scoped.symbols` and `scoped.files`,
not `ctx.symbols`** — a rule that reaches past the scope makes a targeted analysis cost what a
full one costs, silently.

Reaching past it is sometimes right: BugHunter's self-invocation rule needs the *callee's*
annotations even when the callee is out of scope, because the finding is anchored on the
caller. That is a deliberate exception, and it is the reason `ctx` is passed alongside
`scoped` rather than replaced by it.

Two rules follow from a narrowed run:

- It may report findings only for what it examined.
- **It may not close what it did not look at.** `Engine::analyze` enforces this — the
  fixed-sweep runs only under `Scope::Everything`. Absence is evidence only when something
  actually looked.

## Writing one

```rust
impl Capability for TodoHunter {
    fn id(&self) -> &'static str { "todo" }
    fn finding_prefix(&self) -> &'static str { "TODO" }
    fn describe(&self) -> &'static str { "counts TODO comments" }

    fn analyze(&self, ctx: &ProjectContext<'_>, scope: &Scope) -> Result<Vec<Finding>, CapabilityError> {
        let scoped = ctx.scoped(scope);
        let mut out = Vec::new();
        for f in &scoped.files {
            let Ok(text) = std::fs::read_to_string(ctx.root.join(&f.path)) else { continue };
            for (i, line) in text.lines().enumerate() {
                if !line.contains("TODO") { continue }
                out.push(Finding { /* … */ });
            }
        }
        Ok(out)
    }
}
```

Register it at the composition root — `nexus-cli::open` and `nexus-mcp`:

```rust
engine.register_capability(Box::new(TodoHunter));
```

That is the entire integration. It appears in `nexus capabilities`, runs under
`nexus analyze todo`, its findings are numbered `TODO-1`, they filter with
`nexus findings --capability todo`, and they get recurrence, regression and closure for free.

A working example lives in `crates/cap-bughunter/tests/capability_contract.rs` — forty lines,
kept as a test rather than shipped, to prove the registry takes more than one.

## The rules a finding must satisfy

**Evidence is not optional.** A finding with no `file:line` is rejected at the boundary rather
than down-ranked, whether it came from a rule or a model. An assertion nobody can check is not
a finding, and storing one lets the next reader mistake it for one. Rejections are counted and
reported, because a silently discarded finding is indistinguishable from finding nothing.

**`structural_key` is what separates two findings in the same place.** It is the capability's
own normalization of what the finding is *about* — the shared state involved, the endpoint,
the field. A sloppy key produces sloppy identity: too vague and two real problems merge, too
specific and a formatting change invents a new one.

**Identity excludes what changes without the finding changing**: file path, line numbers,
commit, wording, severity, confidence. It includes the anchor's *shape* — package, type and
member with generics and parameter types normalized away — so a parameter rename does not
invent a duplicate while a move to another type does change identity.
[ADR-007](architecture-decisions.md#adr-007--composite-hash-fingerprint-for-bug-identity).

## Boundaries, enforced by tests

| Rule | Why |
|---|---|
| `cap-* ↛ nexus-cli`, `cap-* ↛ nexus-mcp` | a capability that could reach a UI would drag one with it wherever it went |
| `cap-* ↛ nexus-store` | a capability reads a snapshot, never the database |
| `nexus-core ↛ cap-*` | capabilities are registered into the platform, never compiled into it — otherwise "add Code Review later" is a core change |

`crates/nexus-cli/tests/boundaries.rs` walks `cargo metadata` and fails the build on any of
them. They are not conventions.

## Capabilities that do not exist yet

Code Review, Security Analysis, Test Generation, Refactoring, Dependency Analysis and
Architecture Analysis are all shaped like BugHunter: read the index, return findings. None is
built, and the foundation exists so that building one touches nothing outside its own crate
and two lines at each composition root.

The one that will need more than this contract is verification — running a generated test to
prove a finding real. That needs sandboxing and execution policy, which is a platform concern
rather than a capability's, and it is designed in
[verification-engine.md](verification-engine.md).
