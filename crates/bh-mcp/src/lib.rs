//! The MCP server.
//!
//! Boundary rule, enforced by `tests/boundaries.rs`: this crate must not depend on
//! `bh-store`, `bh-lang*` or `bh-verify`. It reaches capabilities only through `bh-core`,
//! so a handler physically cannot grow logic the CLI lacks.
//!
//! Every handler is the same three steps — deserialize, one `Engine` call, serialize. If a
//! handler ever needs two `Engine` calls to do its job, the missing method belongs in
//! `bh-core`, because the CLI needs it too.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod budget;

use bh_core::impact::{Direction, ImpactQuery};
use bh_core::Engine;
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
pub struct ChangesArgs {
    /// file | symbol | dependency | config | test
    #[serde(default)]
    pub entity: Option<String>,
}

// ─────────────────────────── server ───────────────────────────

#[derive(Clone)]
pub struct BugHunter {
    root: PathBuf,
    // `Engine` owns a rusqlite Connection, which is Send but not Sync, so it is reached
    // through a mutex. No lock is ever held across an await: every Engine call is
    // synchronous, and the slow ones run on a blocking thread.
    engine: Arc<Mutex<Option<Engine>>>,
    // Read by the code `#[tool_handler]` generates, which dead-code analysis cannot see.
    #[allow(dead_code)]
    tool_router: ToolRouter<BugHunter>,
}

impl BugHunter {
    pub fn new(root: PathBuf) -> Self {
        BugHunter {
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
                let (e, _) = Engine::open_or_init(&root).map_err(|e| e.to_string())?;
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
impl BugHunter {
    #[tool(
        description = "What kind of project this is: languages, frameworks, build system, \
                       databases, the current baseline, and how far it has drifted. Call this \
                       first — it is cheap and it tells you what the other tools can answer."
    )]
    async fn bughunter_get_project_context(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.status().map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(budget::fit(&r, "warnings", "no narrowing needed"))),
            Err(m) => Ok(failure("no_project", m, &["bughunter_scan"])),
        }
    }

    #[tool(
        description = "Index the project and set a baseline. Needed once before rescan or \
                       impact will work. Initializes the project if it has never been set up."
    )]
    async fn bughunter_scan(
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
            Err(m) => Ok(failure("scan_failed", m, &["bughunter_doctor"])),
        }
    }

    #[tool(
        description = "What changed since the baseline, down to the symbol, and advance the \
                       baseline. Reports API_CHANGED, BODY_CHANGED, CONTRACT_CHANGED, ADDED, \
                       DELETED and RENAMED per symbol — not just which files differ."
    )]
    async fn bughunter_rescan(
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
                "call bughunter_get_changes with an entity filter",
            ))),
            Err(m) => Ok(failure("no_baseline", m, &["bughunter_scan"])),
        }
    }

    #[tool(description = "The changes recorded by the current baseline scan.")]
    async fn bughunter_get_changes(
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
            Err(m) => Ok(failure("no_baseline", m, &["bughunter_scan"])),
        }
    }

    #[tool(
        description = "Blast radius: who breaks if this symbol changes, or with forward=true \
                       what it reaches. Crosses the frontend/backend seam, so a backend service \
                       method reaches the UI components that render it. Every result carries the \
                       edge chain that produced it and the weakest confidence along that chain."
    )]
    async fn bughunter_get_impact(
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
            Err(m) => Ok(failure("no_baseline", m, &["bughunter_scan"])),
        }
    }

    #[tool(
        description = "One symbol in detail: what it depends on, what depends on it, and \
                       optionally a capped source excerpt. Follows renames, so an old name \
                       still resolves."
    )]
    async fn bughunter_get_symbol(
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
            Err(m) => Ok(failure("unknown_symbol", m, &["bughunter_get_impact"])),
        }
    }

    #[tool(
        description = "Run the deterministic detectors and reconcile the results with what is \
                       already known. Findings are recognized across scans by fingerprint, so a \
                       bug seen again is not reported twice; one that stops firing is closed; \
                       one that returns after a fix is a regression. No model is involved, so \
                       these confidences are not model estimates."
    )]
    async fn bughunter_find_bugs(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.hunt().map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(budget::fit(
                &r,
                "bugs",
                "filter with bughunter_get_bugs",
            ))),
            Err(m) => Ok(failure("no_baseline", m, &["bughunter_scan"])),
        }
    }

    #[tool(description = "List findings, optionally filtered by status or severity.")]
    async fn bughunter_get_bugs(
        &self,
        Parameters(a): Parameters<BugsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (status, severity) = (a.status.clone(), a.severity.clone());
        match self
            .with_engine(move |e| {
                e.bugs(status.as_deref(), severity.as_deref())
                    .map_err(|e| e.to_string())
            })
            .await
        {
            Ok(bugs) => Ok(ok(budget::fit(
                &json!({"status": "ok", "bugs": bugs}),
                "bugs",
                "filter by severity",
            ))),
            Err(m) => Ok(failure("no_project", m, &["bughunter_scan"])),
        }
    }

    #[tool(
        description = "One finding in full: its evidence with file and line, and every scan \
                       that saw it. The history is what distinguishes a regression from a new \
                       bug."
    )]
    async fn bughunter_get_bug(
        &self,
        Parameters(a): Parameters<BugArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = a.id.clone();
        match self
            .with_engine(move |e| e.bug(&id).map_err(|e| e.to_string()))
            .await
        {
            Ok(Some(d)) => Ok(ok(serde_json::to_value(&d).unwrap_or(json!({})))),
            Ok(None) => Ok(failure(
                "unknown_bug",
                format!("no finding {}", a.id),
                &["bughunter_get_bugs"],
            )),
            Err(m) => Ok(failure("no_project", m, &["bughunter_scan"])),
        }
    }

    #[tool(
        description = "Dismiss a finding. A human decision is sticky: later scans will not \
                       re-open it. Only call this when a person has decided it is not a bug."
    )]
    async fn bughunter_ignore_bug(
        &self,
        Parameters(a): Parameters<BugArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = a.id.clone();
        match self
            .with_engine(move |e| e.ignore_bug(&id).map_err(|e| e.to_string()))
            .await
        {
            Ok(true) => Ok(ok(
                json!({"status": "ok", "id": a.id, "new_status": "IGNORED"}),
            )),
            Ok(false) => Ok(failure(
                "unknown_bug",
                format!("no finding {}", a.id),
                &["bughunter_get_bugs"],
            )),
            Err(m) => Ok(failure("no_project", m, &[])),
        }
    }

    #[tool(
        description = "Dependency graph size and how much of it resolved, broken down by tier. \
                       Use it to judge how much to trust an impact result on this project."
    )]
    async fn bughunter_get_graph(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .with_engine(|e| e.graph().map_err(|e| e.to_string()))
            .await
        {
            Ok(r) => Ok(ok(serde_json::to_value(&r).unwrap_or(json!({})))),
            Err(m) => Ok(failure("no_project", m, &["bughunter_scan"])),
        }
    }

    #[tool(
        description = "Diagnose the environment and configuration. Each check names what it \
                          found and the command that fixes it."
    )]
    async fn bughunter_doctor(
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
impl ServerHandler for BugHunter {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        // Without this the server advertises no capabilities, and a client is entitled to
        // never call tools/list. It worked against a hand-driven probe, which is exactly
        // the kind of thing a probe does not catch.
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(
            "BugHunter indexes a codebase and answers what changed and what it touches.\n\
             \n\
             Call bughunter_get_project_context first. If there is no baseline, call \
             bughunter_scan once; afterwards bughunter_rescan is cheap and reports changes \
             down to the symbol.\n\
             \n\
             bughunter_get_impact is the useful one: it crosses the frontend/backend seam, \
             so a change to a backend service method reaches the UI components that render \
             it. Every result carries the edge chain that produced it and the weakest \
             confidence along that chain — treat a low min_confidence as a guess, not a fact.\n\
             \n\
             bughunter_find_bugs runs deterministic detectors only — Spring proxy mistakes, \
             GraphQL fields no resolver serves, credentials in source. Their confidences are \
             not model estimates, so do not discount them as such. What it does NOT do is \
             reason about business logic, races or data consistency: that is yours, and there \
             is no way yet to write your findings back.\n\
             \n\
             It also runs no tests, so nothing here is verified by reproduction."
                .into(),
        );
        info
    }
}

/// Serve on stdio until the client disconnects.
pub async fn serve(root: PathBuf) -> anyhow::Result<()> {
    let server = BugHunter::new(root);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
