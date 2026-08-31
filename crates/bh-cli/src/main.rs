//! `bughunter` — the composition root.
//!
//! Parse flags, open the store, build the analyzer registry, construct the `Engine`,
//! dispatch. This is the only place in the workspace that knows about all of those at once,
//! and the only place `anyhow` is used: a library that returns `anyhow::Error` has told its
//! caller nothing.

#![forbid(unsafe_code)]

mod render;

use bh_core::impact::{Direction, ImpactQuery};
use bh_core::report::Resolved;
use bh_core::{Engine, EngineError};
use clap::{Parser, Subcommand};
use render::Style;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "bughunter",
    version,
    about = "Change-aware code intelligence: what changed, what it touches, and what broke",
    disable_help_subcommand = true
)]
struct Cli {
    /// Machine-readable output on stdout, and nothing else on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// Errors only; the exit code carries the result.
    #[arg(long, global = true)]
    quiet: bool,
    /// Progress and timings to stderr.
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Operate on a project other than the working directory.
    #[arg(long, global = true, value_name = "PATH")]
    project: Option<PathBuf>,
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Detect the project, create .bughunter/, migrate the database
    Init,
    /// Full scan: index files and symbols, and establish the baseline
    Scan,
    /// Incremental: diff against the baseline down to changed symbols
    Rescan,
    /// Baseline, drift and index size
    Status,
    /// What changed in the current baseline scan
    Changes {
        /// file | symbol | dependency | config | test
        #[arg(long)]
        entity: Option<String>,
    },
    /// Blast radius of a symbol, a file or a name
    Impact {
        /// An FQN, an FQN suffix, a bare name, or a repo-relative file path
        target: String,
        /// What this reaches, instead of who depends on it
        #[arg(long)]
        forward: bool,
        #[arg(long, default_value_t = 5)]
        depth: usize,
        #[arg(long, default_value_t = 0.15)]
        min_score: f64,
        /// Only follow edges a body-only change can travel along
        #[arg(long)]
        body_only: bool,
        /// Show the edge chain that reached each symbol
        #[arg(long)]
        paths: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Dependency graph size and how much of it resolved
    Graph,
    /// Diagnose the environment and configuration
    Doctor,
    /// Run as an MCP server on stdio, for Claude Code, Codex, Copilot or any MCP client
    Mcp,
}

/// Exit codes are part of the interface. Discovering a change is a success, not an error —
/// a tool that exits non-zero for doing its job gets removed from the pipeline in a week.
mod exit {
    pub const OK: u8 = 0;
    pub const RUNTIME: u8 = 1;
    pub const USAGE: u8 = 2;
    pub const NO_BASELINE: u8 = 5;
    /// The target matched several symbols. The caller chooses; BugHunter does not guess.
    pub const AMBIGUOUS: u8 = 6;
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            // `bughunter rescan | head` closes stdout early. That is a normal way to use a
            // terminal tool, not a failure, so it exits quietly like every other Unix
            // command rather than printing an error the user did not cause.
            if is_broken_pipe(e.as_ref()) {
                return ExitCode::from(exit::OK);
            }
            // Diagnostics on stderr, always, so `--json | jq` never sees them.
            eprintln!("bughunter: {e}");
            let mut src = std::error::Error::source(&*e);
            while let Some(s) = src {
                eprintln!("  caused by: {s}");
                src = s.source();
            }
            ExitCode::from(exit::RUNTIME)
        }
    }
}

fn run(cli: &Cli) -> Result<u8, Box<dyn std::error::Error>> {
    let root = cli.project.clone().unwrap_or(std::env::current_dir()?);
    let st = Style::detect(cli.no_color || cli.json);
    let mut out = std::io::stdout().lock();

    macro_rules! emit {
        ($value:expr, $render:expr) => {{
            if cli.json {
                writeln!(out, "{}", envelope(cli, $value)?)?;
            } else if !cli.quiet {
                $render;
            }
        }};
    }

    match &cli.command {
        Command::Init => {
            let (_engine, profile) = Engine::init(&root)?;
            emit!(&profile, {
                render::banner(&mut out, &st)?;
                render::profile(&mut out, &st, &profile)?;
                writeln!(out)?;
                writeln!(out, "Initialized {}/.bughunter", root.display())?;
                writeln!(out, "  next: bughunter scan")?;
            });
        }

        Command::Scan => {
            // `scan` on a fresh checkout should just work. Requiring `init` first is a step
            // whose only outcome is the error "you forgot to run init".
            let (mut engine, initialized) = Engine::open_or_init(&root)?;
            if initialized && !cli.quiet && !cli.json {
                eprintln!("initialized {}/{}", root.display(), bh_core::BH_DIR);
            }
            if cli.verbose > 0 {
                eprintln!("scanning {}", engine.root().display());
            }
            let report = engine.scan()?;
            emit!(&report, {
                render::banner(&mut out, &st)?;
                render::scan(&mut out, &st, &report)?;
            });
        }

        Command::Rescan => {
            let (mut engine, initialized) = Engine::open_or_init(&root)?;
            if initialized {
                // Nothing to diff against yet, so a rescan on a fresh project is a scan.
                let report = engine.scan()?;
                emit!(&report, {
                    render::banner(&mut out, &st)?;
                    writeln!(out, "{}", st.dim("No baseline yet — ran a full scan."))?;
                    writeln!(out)?;
                    render::scan(&mut out, &st, &report)?;
                });
                return Ok(exit::OK);
            }
            match engine.rescan() {
                Ok(report) => {
                    emit!(&report, {
                        render::banner(&mut out, &st)?;
                        render::rescan(&mut out, &st, &report)?;
                    });
                }
                Err(EngineError::NoBaseline) => {
                    eprintln!("bughunter: no baseline for this project");
                    eprintln!("  run: bughunter scan");
                    return Ok(exit::NO_BASELINE);
                }
                Err(e) => return Err(Box::new(e)),
            }
        }

        Command::Status => {
            let engine = open(&root)?;
            let report = engine.status()?;
            emit!(&report, {
                render::banner(&mut out, &st)?;
                render::status(&mut out, &st, &report)?;
            });
        }

        Command::Changes { entity } => {
            let engine = open(&root)?;
            match engine.changes(entity.as_deref()) {
                Ok(rows) => {
                    let items: Vec<ChangeOut> = rows
                        .iter()
                        .map(|(e, c, t, d)| ChangeOut {
                            entity: e.clone(),
                            change_type: c.clone(),
                            target: t.clone(),
                            detail: d.clone(),
                        })
                        .collect();
                    emit!(&items, {
                        render::banner(&mut out, &st)?;
                        render::changes(&mut out, &st, &rows)?;
                    });
                }
                Err(EngineError::NoBaseline) => {
                    eprintln!("bughunter: no baseline for this project");
                    eprintln!("  run: bughunter scan");
                    return Ok(exit::NO_BASELINE);
                }
                Err(e) => return Err(Box::new(e)),
            }
        }

        Command::Impact {
            target,
            forward,
            depth,
            min_score,
            body_only,
            paths,
            limit,
        } => {
            let engine = open(&root)?;
            let q = ImpactQuery {
                target: target.clone(),
                direction: if *forward {
                    Direction::Forward
                } else {
                    Direction::Reverse
                },
                max_depth: *depth,
                min_score: *min_score,
                body_only: *body_only,
                limit: *limit,
                ..Default::default()
            };
            match engine.impact(&q)? {
                Resolved::One(report) => {
                    emit!(&Resolved::One(report.clone()), {
                        render::banner(&mut out, &st)?;
                        render::impact(&mut out, &st, &report, *paths)?;
                    });
                }
                r @ Resolved::Ambiguous(_) => {
                    // Not an error and not a guess: hand back the candidates so the caller
                    // can choose. The CLI's form of `clarification_required`.
                    let Resolved::Ambiguous(cands) = &r else {
                        unreachable!()
                    };
                    emit!(&r, {
                        render::banner(&mut out, &st)?;
                        render::ambiguous(&mut out, &st, target, cands)?;
                    });
                    return Ok(exit::AMBIGUOUS);
                }
                r @ Resolved::NotFound { .. } => {
                    emit!(&r, {
                        eprintln!("bughunter: no symbol matches '{target}'");
                        eprintln!(
                            "  try a fully-qualified name, a bare method name, or a file path"
                        );
                    });
                    return Ok(exit::USAGE);
                }
            }
        }

        Command::Graph => {
            let engine = open(&root)?;
            let report = engine.graph()?;
            emit!(&report, {
                render::banner(&mut out, &st)?;
                render::graph(&mut out, &st, &report)?;
            });
        }

        Command::Mcp => {
            // stdout is the MCP transport here, so the renderer's lock on it must go first.
            // Holding it deadlocks the server the moment it tries to answer, and the
            // symptom is a process that reads happily and never replies.
            drop(out);
            let root = root.clone();
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(bh_mcp::serve(root))?;
        }

        Command::Doctor => {
            let engine = open(&root)?;
            let checks = engine.doctor()?;
            let worst = checks.iter().any(|c| c.level == "error");
            emit!(&checks, {
                writeln!(out, "{}", st.head("BugHunter doctor"))?;
                writeln!(
                    out,
                    "{}",
                    st.dim("────────────────────────────────────────")
                )?;
                render::doctor(&mut out, &st, &checks)?;
            });
            if worst {
                return Ok(exit::RUNTIME);
            }
        }
    }
    Ok(exit::OK)
}

/// Walks the source chain: the io::Error is wrapped by the time it reaches main.
fn is_broken_pipe(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::BrokenPipe {
                return true;
            }
        }
        cur = err.source();
    }
    false
}

fn open(root: &std::path::Path) -> Result<Engine, EngineError> {
    Engine::open(root)
}

#[derive(Serialize)]
struct ChangeOut {
    entity: String,
    change_type: String,
    target: Option<String>,
    detail: Option<String>,
}

/// One versioned envelope for every command, so a script written today keeps working.
/// `warnings` is always present: a consumer must never have to tell "no warnings" apart
/// from "an older version that did not report them".
#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    bughunter: &'a str,
    schema: u32,
    command: &'a str,
    result: T,
}

fn envelope<T: Serialize>(cli: &Cli, value: T) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&Envelope {
        bughunter: env!("CARGO_PKG_VERSION"),
        schema: 1,
        command: match &cli.command {
            Command::Init => "init",
            Command::Scan => "scan",
            Command::Rescan => "rescan",
            Command::Status => "status",
            Command::Changes { .. } => "changes",
            Command::Impact { .. } => "impact",
            Command::Graph => "graph",
            Command::Doctor => "doctor",
            Command::Mcp => "mcp",
        },
        result: value,
    })
}
