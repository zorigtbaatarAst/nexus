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
pub mod edges;
pub mod matcher;
pub mod metrics;
pub mod oracle;
pub mod report;

/// The CLI: one edge dump, one SCIP index, one report.
///
/// Exit is `Ok` even when the run is `partial` — a degraded oracle says nothing about the
/// resolver, and the caveat travels with the numbers rather than as an exit code. §8.2.
pub fn run(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = clap::Command::new("nexus-eval")
        .about("Measure whether a resolved edge points at the right symbol")
        .arg(clap::arg!(--edges <PATH> "NDJSON from `nexus graph --edges`").required(true))
        .arg(clap::arg!(--scip <PATH> "index.scip from a SCIP indexer").required(true))
        .arg(clap::arg!(--oracle <NAME> "what produced the index, recorded in the report"))
        .arg(clap::arg!(--json "emit the run as JSON"))
        .get_matches_from(args);

    let edges = edges::load(std::path::Path::new(
        m.get_one::<String>("edges").ok_or("--edges")?,
    ))?;
    let oracle = oracle::Oracle::load(std::path::Path::new(
        m.get_one::<String>("scip").ok_or("--scip")?,
    ))?;
    // Every file Nexus indexed, so the coverage cross-check compares like with like.
    let files: Vec<String> = {
        let mut v: Vec<String> = edges.iter().map(|e| e.src_file.clone()).collect();
        v.sort();
        v.dedup();
        v
    };
    let comparison = matcher::compare(&edges, &oracle);
    let name = m.get_one::<String>("oracle").map_or("scip", String::as_str);
    let run = report::build(name, &files, &oracle.files, &comparison);

    if m.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&run)?);
    } else {
        print!("{}", report::render(&run));
    }
    Ok(())
}
