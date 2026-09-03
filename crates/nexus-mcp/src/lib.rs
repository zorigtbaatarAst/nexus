//! The MCP server.
//!
//! Boundary rule, enforced by `tests/boundaries.rs`: this crate must not depend on
//! `nexus-store`, `nexus-lang*` or `nexus-verify`. It reaches capabilities only through `nexus-core`,
//! so a handler physically cannot grow logic the CLI lacks.
//!
//! Every handler is the same three steps — deserialize, one `Engine` call, serialize. If a
//! handler ever needs two `Engine` calls to do its job, the missing method belongs in
//! `nexus-core`, because the CLI needs it too.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod budget;

use cap_architect::Architect as ArchitectCapability;
use cap_bughunter::BugHunter as BugHunterCapability;
use cap_review::Review as ReviewCapability;
use nexus_core::capability::Scope;
use nexus_core::impact::{Direction, ImpactQuery};
use nexus_core::Engine;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ─────────────────────────── parameters ───────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NoArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImpactArgs {
    /// A fully-qualified name, an FQN suffix, a bare method name, or a repo-relative path.
    pub target: String,
    /// What this reaches, instead of who depends on it. Defaults to who depends on it.
    #[serde(default)]
    pub forward: bool,
    /// How many hops to follow. Defaults to 5.
    #[serde(default)]
    pub depth: Option<usize>,
    /// Drop results scoring below this. Defaults to 0.15; raise it to narrow.
    #[serde(default)]
    pub min_score: Option<f64>,
    /// Only follow edges a body-only change can travel along.
    #[serde(default)]
    pub body_only: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SymbolArgs {
    /// A fully-qualified name, an FQN suffix, or a bare name.
    pub target: String,
    /// Include a capped source excerpt. Off by default: source is expensive context.
    #[serde(default)]
    pub with_source: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BugsArgs {
    /// SUSPECTED | UNVERIFIED | VERIFIED | FIXED | REGRESSED | IGNORED
    #[serde(default)]
    pub status: Option<String>,
    /// critical | high | medium | low | info
    #[serde(default)]
    pub severity: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BugArgs {
    /// e.g. BUG-3
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TargetArgs {
    /// A file path, a fully-qualified symbol name, or a component name.
    pub target: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordFindingArgs {
    /// Which capability this belongs to. Defaults to `agent` — findings you reasoned out
    /// rather than a rule produced.
    #[serde(default)]
    pub capability: Option<String>,
    pub finding: nexus_core::findings::Finding,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContextArgs {
    /// What is being worked on, in the developer's own words. Intent is read from it by a
    /// verb table, never by a model.
    pub task: String,
    /// Anchors you already have. A caller editing a file knows, and an explicit anchor is
    /// not a guess.
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Token ceiling. The package is selected to fit, never truncated to fit.
    pub budget: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FactArgs {
    /// Dotted and greppable: `arch.payment.idempotency`, `invariant.order.status`.
    pub key: String,
    /// The fact itself, in one sentence.
    pub claim: String,
    /// The symbol or module it is about, when it is about one.
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub evidence: Vec<nexus_core::findings::CodeRef>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangesArgs {
    /// file | symbol | dependency | config | test
    #[serde(default)]
    pub entity: Option<String>,
}

// ─────────────────────────── server ───────────────────────────

#[derive(Clone)]
pub struct Nexus {
    root: PathBuf,
    // `Engine` owns a rusqlite Connection, which is Send but not Sync, so it is reached
    // through a mutex. No lock is ever held across an await: every Engine call is
    // synchronous, and the slow ones run on a blocking thread.
    engine: Arc<Mutex<Option<Engine>>>,
    // Read by the code `#[tool_handler]` generates, which dead-code analysis cannot see.
    #[allow(dead_code)]
    tool_router: ToolRouter<Nexus>,
}

impl Nexus {
    pub fn new(root: PathBuf) -> Self {
        Nexus {
            root,
            engine: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    /// Run one synchronous `Engine` call on a blocking thread.
    ///
    /// A scan takes hundreds of milliseconds on a normal project and minutes on a monorepo.
    /// Doing that on the async runtime would stall every other request, which from the
    /// user's point of view is an outage.
    async fn with_engine<T, F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&mut Engine) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let engine = Arc::clone(&self.engine);
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = engine
                .lock()
                .map_err(|_| "engine lock poisoned".to_string())?;
            if guard.is_none() {
                // `scan` on a fresh checkout should just work over MCP too, so the project
                // is initialized on first use rather than erroring.
                let (mut e, _) = Engine::open_or_init(&root).map_err(|e| e.to_string())?;
                // The composition root: Nexus is handed its capabilities here rather than
                // compiling them in, which is what lets BugHunter ship separately.
                e.register_capability(Box::new(BugHunterCapability::new()));
                e.register_capability(Box::new(ArchitectCapability::new()));
                e.register_capability(Box::new(ReviewCapability::new()));
                *guard = Some(e);
            }
            let e = guard
                .as_mut()
                .ok_or_else(|| "engine unavailable".to_string())?;
            f(e)
        })
        .await
        .map_err(|e| format!("task failed: {e}"))?
    }
}

/// Domain failures are results, not protocol errors. An agent can act on a result; a
/// JSON-RPC error just makes it retry.
fn failure(kind: &str, message: String, next: &[&str]) -> CallToolResult {
    ok(json!({
        "status": "error",
        "kind": kind,
        "message": message,
        "recoverable": !next.is_empty(),
        "next": next,
    }))
}

fn ok(value: Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into());
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(value);
    result
}

#[tool_router]
impl Nexus {
    #[tool(
        description = "What kind of project this is: languages, frameworks, build system, \
                       databases, the current baseline, and how far it has drifted. Call this \
                       first — it is cheap and it tells you what the other tools can answer."
    )]
    async fn nexus_get_project_context(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.status().map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(budget::fit(&r, "warnings", "no narrowing needed"))),
            Err(m) => Ok(failure("no_project", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "Index the project and set a baseline. Needed once before rescan or \
                       impact will work. Initializes the project if it has never been set up."
    )]
    async fn nexus_scan(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.scan().map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(budget::fit(
                &r,
                "warnings",
                "use --verbose on the CLI for all warnings",
            ))),
            Err(m) => Ok(failure("scan_failed", m, &["nexus_doctor"])),
        }
    }

    #[tool(
        description = "What changed since the baseline, down to the symbol, and advance the \
                       baseline. Reports API_CHANGED, BODY_CHANGED, CONTRACT_CHANGED, ADDED, \
                       DELETED and RENAMED per symbol — not just which files differ."
    )]
    async fn nexus_rescan(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.rescan().map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(budget::fit(
                &r,
                "items",
                "call nexus_get_changes with an entity filter",
            ))),
            Err(m) => Ok(failure("no_baseline", m, &["nexus_scan"])),
        }
    }

    #[tool(description = "The changes recorded by the current baseline scan.")]
    async fn nexus_get_changes(
        &self,
        Parameters(a): Parameters<ChangesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let entity = a.entity.clone();
        match self
            .with_engine(move |e| e.changes(entity.as_deref()).map_err(|e| e.to_string()))
            .await
        {
            Ok(rows) => {
                let items: Vec<Value> = rows
                    .into_iter()
                    .map(|(entity, change_type, target, detail)| {
                        json!({"entity": entity, "change_type": change_type,
                               "target": target, "detail": detail})
                    })
                    .collect();
                Ok(ok(budget::fit(
                    &json!({"status": "ok", "items": items}),
                    "items",
                    "filter with entity: \"symbol\"",
                )))
            }
            Err(m) => Ok(failure("no_baseline", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "Blast radius: who breaks if this symbol changes, or with forward=true \
                       what it reaches. Crosses the frontend/backend seam, so a backend service \
                       method reaches the UI components that render it. Every result carries the \
                       edge chain that produced it and the weakest confidence along that chain. \
                       `uncovered: true` means no test in the index reaches this symbol while \
                       other code depends on it — a change here will not fail loudly, so treat \
                       it as the strongest reason to be careful that this tool can give you."
    )]
    async fn nexus_get_impact(
        &self,
        Parameters(a): Parameters<ImpactArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let q = ImpactQuery {
            target: a.target.clone(),
            direction: if a.forward {
                Direction::Forward
            } else {
                Direction::Reverse
            },
            max_depth: a.depth.unwrap_or(5).clamp(1, 12),
            min_score: a.min_score.unwrap_or(0.15).clamp(0.0, 1.0),
            ..Default::default()
        };
        let body_only = a.body_only;
        let q2 = ImpactQuery { body_only, ..q };
        match self
            .with_engine(move |e| e.impact(&q2).map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(budget::fit(
                &r,
                "items",
                "raise min_score, or lower depth",
            ))),
            Err(m) => Ok(failure("no_baseline", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "One symbol in detail: what it depends on, what depends on it, and \
                       optionally a capped source excerpt. Follows renames, so an old name \
                       still resolves."
    )]
    async fn nexus_get_symbol(
        &self,
        Parameters(a): Parameters<SymbolArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let target = a.target.clone();
        let with_source = a.with_source;
        match self
            .with_engine(move |e| e.symbol(&target).map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => {
                let mut v = serde_json::to_value(&r).unwrap_or(json!({}));
                if !with_source {
                    if let Some(obj) = v.as_object_mut() {
                        obj.remove("source");
                    }
                }
                Ok(ok(budget::fit(
                    &v,
                    "depended_on_by",
                    "ask for the specific caller instead",
                )))
            }
            Err(m) => Ok(failure("unknown_symbol", m, &["nexus_get_impact"])),
        }
    }

    #[tool(
        description = "Run the deterministic detectors and reconcile the results with what is \
                       already known. Findings are recognized across scans by fingerprint, so a \
                       bug seen again is not reported twice; one that stops firing is closed; \
                       one that returns after a fix is a regression. No model is involved, so \
                       these confidences are not model estimates."
    )]
    async fn bughunter_analyze(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| {
                e.analyze("bughunter", Scope::Everything)
                    .map_err(|e| e.to_string())
            })
            .await
        {
            Ok(r) => Ok(ok(budget::fit(
                &r,
                "findings",
                "filter with nexus_get_findings",
            ))),
            Err(m) => Ok(failure("no_baseline", m, &["nexus_scan"])),
        }
    }

    #[tool(description = "List findings, optionally filtered by status or severity.")]
    async fn nexus_get_findings(
        &self,
        Parameters(a): Parameters<BugsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (status, severity) = (a.status.clone(), a.severity.clone());
        match self
            .with_engine(move |e| {
                e.findings(None, status.as_deref(), severity.as_deref())
                    .map_err(|e| e.to_string())
            })
            .await
        {
            Ok(findings) => Ok(ok(budget::fit(
                &json!({"status": "ok", "findings": findings}),
                "findings",
                "filter by severity",
            ))),
            Err(m) => Ok(failure("no_project", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "One finding in full: its evidence with file and line, and every scan \
                       that saw it. The history is what distinguishes a regression from a new \
                       bug."
    )]
    async fn nexus_get_finding(
        &self,
        Parameters(a): Parameters<BugArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = a.id.clone();
        match self
            .with_engine(move |e| e.finding(&id).map_err(|e| e.to_string()))
            .await
        {
            Ok(Some(d)) => Ok(ok(serde_json::to_value(&d).unwrap_or(json!({})))),
            Ok(None) => Ok(failure(
                "unknown_bug",
                format!("no finding {}", a.id),
                &["nexus_get_findings"],
            )),
            Err(m) => Ok(failure("no_project", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "Dismiss a finding. A human decision is sticky: later scans will not \
                       re-open it. Only call this when a person has decided it is not a bug."
    )]
    async fn nexus_ignore_finding(
        &self,
        Parameters(a): Parameters<BugArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = a.id.clone();
        match self
            .with_engine(move |e| e.ignore_finding(&id).map_err(|e| e.to_string()))
            .await
        {
            Ok(true) => Ok(ok(
                json!({"status": "ok", "id": a.id, "new_status": "IGNORED"}),
            )),
            Ok(false) => Ok(failure(
                "unknown_bug",
                format!("no finding {}", a.id),
                &["nexus_get_findings"],
            )),
            Err(m) => Ok(failure("no_project", m, &[])),
        }
    }

    #[tool(
        description = "Dependency graph size and how much of it resolved, broken down by tier. \
                       Use it to judge how much to trust an impact result on this project."
    )]
    async fn nexus_get_graph(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.graph().map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(serde_json::to_value(&r).unwrap_or(json!({})))),
            Err(m) => Ok(failure("no_project", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "Record a finding you reasoned out yourself. This is how a model \
                       contributes to Nexus: it gets the same identity, lifecycle and history \
                       as a rule-produced one, so the same observation next session is \
                       recognized rather than duplicated. Every finding needs at least one \
                       file:line of evidence — a claim nobody can check is rejected, not \
                       stored — and model confidence is capped at 0.75, because reproduction, \
                       not assertion, is what makes a finding certain."
    )]
    async fn nexus_record_finding(
        &self,
        Parameters(a): Parameters<RecordFindingArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let capability = a.capability.clone().unwrap_or_else(|| "agent".into());
        let finding = a.finding.clone();
        match self
            .with_engine(move |e| {
                e.record_finding(&capability, finding)
                    .map_err(|e| e.to_string())
            })
            .await
        {
            Ok(r) => Ok(ok(serde_json::to_value(&r).unwrap_or(json!({})))),
            Err(m) => Ok(failure("rejected", m, &["nexus_get_symbol"])),
        }
    }

    #[tool(
        description = "What is already known about this file, symbol or component: findings \
                       recorded here before, and facts learned about it. Ask before changing \
                       something — the answer is what a previous session already worked out."
    )]
    async fn nexus_get_known(
        &self,
        Parameters(a): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (t1, t2) = (a.target.clone(), a.target.clone());
        let findings = self
            .with_engine(move |e| e.findings_for(&t1).map_err(|e| e.to_string()))
            .await;
        let facts = self
            .with_engine(move |e| e.facts(Some(&t2)).map_err(|e| e.to_string()))
            .await;
        match (findings, facts) {
            (Ok(f), Ok(k)) => Ok(ok(budget::fit(
                &json!({"status": "ok", "target": a.target, "findings": f, "facts": k}),
                "findings",
                "narrow the target",
            ))),
            (Err(m), _) | (_, Err(m)) => Ok(failure("no_project", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "The context for a task: the code it reaches, what is known about that \
                       code, and why each item is here — ranked and fitted to a token budget \
                       in one call. Every item carries a file:line anchor rather than file \
                       contents, so read what you need and pay for nothing you do not. \
                       Deterministic: no model runs anywhere in this pipeline."
    )]
    async fn nexus_get_context(
        &self,
        Parameters(a): Parameters<ContextArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = nexus_core::TaskRequest {
            text: a.task,
            files: a.files,
            symbols: a.symbols,
            budget_tokens: a.budget.unwrap_or(nexus_core::context::TASK_BUDGET_TOKENS),
            purpose: nexus_core::Purpose::Task,
        };
        match self
            .with_engine(move |e| e.context(&request).map_err(|e| e.to_string()))
            .await
        {
            Ok(p) => Ok(ok(budget::fit(
                &serde_json::to_value(&p).unwrap_or(json!({})),
                "items",
                "lower the budget",
            ))),
            Err(m) => Ok(failure("no_baseline", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "What to look at next: the changed symbols this scan reports, ranked. \
                       This has existed in the CLI since the first release and has never been \
                       reachable by an agent."
    )]
    async fn nexus_what_next(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| {
                e.ask(&nexus_core::Question::Next)
                    .map_err(|err| err.to_string())
            })
            .await
        {
            Ok(answer) => Ok(ok(budget::fit(
                &serde_json::to_value(&answer).unwrap_or(json!({})),
                "items",
                "run nexus_rescan first",
            ))),
            Err(m) => Ok(failure("no_baseline", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "Remember something about this project that is not a symbol or a \
                       finding — an invariant, a convention, a decision. It survives every \
                       later session and every later model, which is the point: expensive \
                       conclusions should be reached once."
    )]
    async fn nexus_record_fact(
        &self,
        Parameters(a): Parameters<FactArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = nexus_core::FactInput {
            key: a.key.clone(),
            scope: if a.subject.is_some() {
                "module".into()
            } else {
                "project".into()
            },
            subject: a.subject.clone(),
            claim: a.claim.clone(),
            source: "ai".into(),
            evidence: a.evidence.clone(),
            confidence: a.confidence.unwrap_or(0.8),
        };
        match self
            .with_engine(move |e| e.record_fact(input).map_err(|e| e.to_string()))
            .await
        {
            Ok(()) => Ok(ok(json!({"status": "ok", "key": a.key}))),
            Err(m) => Ok(failure("no_baseline", m, &["nexus_scan"])),
        }
    }

    #[tool(description = "Which capabilities this build can run.")]
    async fn nexus_capabilities(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.with_engine(|e| Ok(e.capability_list())).await {
            Ok(c) => Ok(ok(json!({"status": "ok", "capabilities": c}))),
            Err(m) => Ok(failure("no_project", m, &["nexus_scan"])),
        }
    }

    #[tool(
        description = "Diagnose the environment and configuration. Each check names what it \
                          found and the command that fixes it."
    )]
    async fn nexus_doctor(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.doctor().map_err(|e| e.to_string()))
            .await
        {
            Ok(checks) => Ok(ok(json!({"status": "ok", "checks": checks}))),
            Err(m) => Ok(failure("no_project", m, &[])),
        }
    }
}

#[tool_handler]
impl ServerHandler for Nexus {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        // Without this the server advertises no capabilities, and a client is entitled to
        // never call tools/list. It worked against a hand-driven probe, which is exactly
        // the kind of thing a probe does not catch.
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(
            "Nexus is persistent code intelligence. It understands the project once and \
             remembers; capabilities use that understanding.\n\
             \n\
             Call nexus_get_project_context first. If there is no baseline, nexus_scan once; \
             afterwards nexus_rescan is cheap and reports changes down to the symbol.\n\
             \n\
             Before changing code, call nexus_get_known on it — findings recorded there \
             before and facts a previous session worked out. That is what the persistence is \
             for: expensive conclusions should be reached once.\n\
             \n\
             nexus_get_impact crosses the frontend/backend seam, so a change to a backend \
             service method reaches the UI components that render it. Every result carries \
             the edge chain that produced it and the weakest confidence along that chain — \
             treat a low min_confidence as a lead, not a fact.\n\
             \n\
             bughunter_analyze runs deterministic rules only: Spring proxy mistakes, GraphQL \
             fields no resolver serves, credentials in source. Their confidences are not \
             model estimates, so do not discount them as such.\n\
             \n\
             What the rules cannot do is reason about business logic, races or data \
             consistency. That is yours — and nexus_record_finding is how you contribute it, \
             with the same identity and history a rule-produced finding gets, so the same \
             observation next session is recognized rather than duplicated. Every finding \
             needs file:line evidence, and model confidence is capped at 0.75: nothing here \
             runs tests, so nothing is verified by reproduction yet."
                .into(),
        );
        info
    }
}

/// Serve on stdio until the client disconnects.
pub async fn serve(root: PathBuf) -> anyhow::Result<()> {
    let server = Nexus::new(root);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
