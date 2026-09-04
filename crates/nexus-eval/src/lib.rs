//! Does a resolved edge point at the *right* symbol?
//!
//! Nexus reports coverage — the share of call sites that found a destination. Nothing in the
//! product checks that the destination is correct, and the confidence on every edge is a
//! probability claim nobody has ever tested. This crate tests both, against an index produced
//! by a real compiler frontend.
//!
//! **Boundary.** Nothing in the workspace may depend on this crate;
//! `nexus-cli/tests/boundaries.rs` fails the build if anything does. It is the mirror of
//! `nexus-fixtures`, which generates repositories and must never index them: a component that
//! produces a number and also grades it has nothing checking it.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
pub mod oracle;
