//! Deterministic generation of benchmark fixture repositories.
//!
//! ```text
//! fixture specification  →  deterministic generator  →  git repository  →  benchmark fixture
//! ```
//!
//! [`docs/architecture/13-evaluation.md`] describes a corpus of repositories with scripted
//! histories — planted bugs at known commits, a reformat that must move no symbol, a rename
//! that must not duplicate a finding. Those properties are worth more than realism, and they
//! cannot be scraped: they have to be built on purpose.
//!
//! This crate builds them, and builds the *same* ones every time. Determinism is the whole
//! point: a benchmark whose fixtures drift measures the fixtures.
//!
//! **Boundary.** `nexus-fixtures` depends on `git2` and nothing of Nexus. It creates
//! repositories; it does not index them, and it must not learn how. A generator that could
//! ask the engine what it had just produced would be marking its own work, and the
//! `expect` fields it records exist precisely so that something else can do the checking.
//! `nexus-cli/tests/boundaries.rs` enforces this.

#![forbid(unsafe_code)]
// A panic here aborts a corpus build with no useful message. Tests are exempt: an assertion
// that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod generate;
pub mod manifest;
pub mod spec;

pub use generate::{generate, GenError, Generated, Options};
pub use manifest::Manifest;
pub use spec::{Spec, SpecError};

/// Where specifications live by default, relative to the repository root.
pub const DEFAULT_SPEC_DIR: &str = "tests/fixtures/specs";

/// Where fixtures are written by default.
///
/// Under `target/` because it is already git-ignored: a generated repository inside the
/// working tree would be walked by Nexus's own scan of this project, and a fixture that
/// plants a bug on purpose has no business appearing in its author's findings.
pub const DEFAULT_OUT_DIR: &str = "target/fixtures";
