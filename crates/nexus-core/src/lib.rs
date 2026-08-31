//! BugHunter Core — all business logic.
//!
//! Boundary rules, enforced by `tests/boundaries.rs`:
//!   * must not depend on `nexus-mcp` or `nexus-cli` — MCP is an adapter, not the core;
//!   * must not depend on any concrete AI provider — AI is optional, and the
//!     deterministic build carries no HTTP client at all.

#![forbid(unsafe_code)]
// A panic in a scan loses the whole run; an error loses one file. Tests are exempt:
// an assertion that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod bugs;
pub mod detect;
pub mod detectors;
pub mod engine;
pub mod impact;
pub mod report;
pub mod walk;

pub use engine::{Engine, EngineError, Result, DB_FILE, NEXUS_DIR};
pub use report::*;

/// Re-exported so adapters can name storage row aliases without taking a dependency on
/// `nexus-store` themselves — boundary rule 3 keeps SQL in one crate, and this keeps the
/// *types* reachable without widening that.
pub use nexus_store;
