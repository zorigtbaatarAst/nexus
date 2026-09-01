//! `bughunter` — the composition root.
//!
//! Parse flags, open the store, build the analyzer registry, construct the `Engine`,
//! dispatch. This is the only place in the workspace that knows about all of those at once,
//! and the only place `anyhow` is used: a library that returns `anyhow::Error` has told its
//! caller nothing.

#![forbid(unsafe_code)]

mod ask;
mod render;

use cap_architect::Architect;
use cap_bughunter::BugHunter;
use cap_review::Review;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use nexus_core::capability::Scope;
use nexus_core::impact::{Direction, ImpactQuery};
use nexus_core::report::Resolved;
use nexus_core::{Engine, EngineError};
use render::Style;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "nexus",
    version,
    about = "Nexus — persistent code intelligence. Nexus understands the project; capabilities use that understanding.",
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
    /// Detect the project, create .nexus/, migrate the database
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
    /// Run a capability over the project
    #[command(alias = "hunt")]
    Analyze {
        /// Which capability. `nexus capabilities` lists them.
        #[arg(default_value = "bughunter")]
        capability: String,
        /// Only what changed since the previous scan, instead of everything
        #[arg(long)]
        changed: bool,
        /// Only these files
        #[arg(long, value_name = "PATH")]
        file: Vec<String>,
    },
    /// What this build can analyze
    Capabilities,
    /// Answer a question about the project
    Ask {
        /// changed | affected <target> | uses <target> | known <target> | facts | next
        #[arg(trailing_var_arg = true)]
        question: Vec<String>,
    },
    /// Record something learned about this project, so the next session starts with it
    Fact {
        /// e.g. arch.payment.idempotency
        key: String,
        /// One sentence
        claim: String,
        #[arg(long)]
        subject: Option<String>,
    },
    /// List findings
    #[command(alias = "bugs")]
    Findings {
        /// Only this capability's findings
        #[arg(long)]
        capability: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        severity: Option<String>,
        /// Exit 3 if anything at or above this severity is open — the CI gate
        #[arg(long, value_name = "SEVERITY")]
        fail_on: Option<String>,
    },
    /// One finding in full, with its evidence and history
    #[command(alias = "bug")]
    Finding {
        /// e.g. BUG-3
        id: String,
    },
    /// Dismiss a finding. A human decision is sticky: a later scan will not re-open it
    Ignore { id: String },
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
    /// Findings at or above `--fail-on`. The CI gate.
    pub const FINDINGS: u8 = 3;
}

fn main() -> ExitCode {
    // The help and version text carry the name the user typed. One binary image under two
    // names, so `bughunter --version` must not answer "nexus".
    // A &'static str, because clap's builder wants one and the two names are known.
    let name: &'static str = if render::product_name() == "BugHunter" {
        "bughunter"
    } else {
        "nexus"
    };
    let about: &'static str = if name == "bughunter" {
        "BugHunter — deterministic bug detection, the first Nexus capability"
    } else {
        "Nexus — persistent code intelligence. Nexus understands the project; capabilities use that understanding."
    };
    let matches = Cli::command()
        .name(name)
        .bin_name(name)
        .about(about)
        .get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
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
                // The directory is named once, in nexus-core. Spelling it here again is how
                // it came to report a directory the tool has not created since the rename.
                writeln!(
                    out,
                    "Initialized {}/{}",
                    root.display(),
                    nexus_core::NEXUS_DIR
                )?;
                writeln!(out, "  next: {} scan", render::binary_name())?;
            });
        }

        Command::Scan => {
            // `scan` on a fresh checkout should just work. Requiring `init` first is a step
            // whose only outcome is the error "you forgot to run init".
            let (mut engine, initialized) = open_or_init(&root)?;
            if initialized && !cli.quiet && !cli.json {
                eprintln!("initialized {}/{}", root.display(), nexus_core::NEXUS_DIR);
            }
            if cli.verbose > 0 {
                eprintln!("scanning {}", engine.root().display());
            }
            let report = engine.scan()?;
            emit!(&report, {
                render::banner(&mut out, &st)?;
                render::scan(&mut out, &st, &report)?;
            });

            // The first scan is when someone learns what this tool thinks their project is,
            // and it is the only moment they are certainly paying attention. Architect runs
            // here rather than at `init` because two of its three rules need an index —
            // before one exists it could only report the datastore rule, and its
            // missing-scaffolding rule would have no indexed build file to point at.
            //
            // Failure is not fatal: a capability that cannot run must not cost someone their
            // scan, which is the expensive part.
            if let Ok(arc) = engine.analyze("architect", Scope::Everything) {
                if !arc.findings.is_empty() {
                    emit!(&arc, {
                        writeln!(out)?;
                        render::analyze(&mut out, &st, &arc)?;
                    });
                }
            }
        }

        Command::Rescan => {
            let (mut engine, initialized) = open_or_init(&root)?;
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
            rt.block_on(nexus_mcp::serve(root))?;
        }

        Command::Findings {
            capability,
            status,
            severity,
            fail_on,
        } => {
            let engine = open(&root)?;
            // Under the `bughunter` name the tool is one capability, so its findings list is
            // that capability's unless asked otherwise. Under `nexus` it is the platform's.
            let default_cap = (render::product_name() == "BugHunter").then_some("bughunter");
            let bugs = engine.findings(
                capability.as_deref().or(default_cap),
                status.as_deref(),
                severity.as_deref(),
            )?;
            emit!(&bugs, {
                render::banner(&mut out, &st)?;
                render::findings(&mut out, &st, &bugs)?;
            });
            if let Some(threshold) = fail_on {
                // Discovering a bug is a success, not an error. Only an explicit gate makes
                // it fail, or the command gets removed from the pipeline within a week.
                if render::breaches(&bugs, threshold.as_str()) {
                    return Ok(exit::FINDINGS);
                }
            }
        }

        Command::Finding { id } => {
            let engine = open(&root)?;
            match engine.finding(id)? {
                Some(detail) => {
                    emit!(&detail, {
                        render::banner(&mut out, &st)?;
                        render::finding(&mut out, &st, &detail)?;
                    });
                }
                None => {
                    eprintln!("bughunter: no finding {id}");
                    eprintln!("  list them with: bughunter bugs");
                    return Ok(exit::USAGE);
                }
            }
        }

        Command::Ignore { id } => {
            let engine = open(&root)?;
            if engine.ignore_finding(id)? {
                if !cli.quiet {
                    writeln!(
                        out,
                        "{id} ignored. It will not be re-opened by a later scan."
                    )?;
                }
            } else {
                eprintln!("bughunter: no finding {id}");
                return Ok(exit::USAGE);
            }
        }

        Command::Analyze {
            capability,
            changed,
            file,
        } => {
            let mut engine = open(&root)?;
            let scope = if !file.is_empty() {
                Scope::Files(file.clone())
            } else if *changed {
                // The previous scan is what "changed" is measured against, and the rescan
                // cascade already worked out exactly which symbols moved.
                match engine.previous_scan_id()? {
                    Some(id) => Scope::Changed { since_scan: id },
                    None => Scope::Everything,
                }
            } else {
                Scope::Everything
            };
            match engine.analyze(capability, scope) {
                Ok(report) => emit!(&report, {
                    render::banner(&mut out, &st)?;
                    render::analyze(&mut out, &st, &report)?;
                }),
                Err(e @ EngineError::UnknownCapability { .. }) => {
                    eprintln!("nexus: {e}");
                    return Ok(exit::USAGE);
                }
                Err(EngineError::NoBaseline) => {
                    eprintln!("nexus: no baseline for this project");
                    eprintln!("  run: nexus scan");
                    return Ok(exit::NO_BASELINE);
                }
                Err(e) => return Err(Box::new(e)),
            }
        }

        Command::Capabilities => {
            let engine = open(&root)?;
            let caps = engine.capability_list();
            emit!(&caps, {
                render::banner(&mut out, &st)?;
                render::capabilities(&mut out, &st, &caps)?;
            });
        }

        Command::Ask { question } => {
            let engine = open(&root)?;
            let answer = ask::answer(&engine, question)?;
            emit!(&answer, {
                render::banner(&mut out, &st)?;
                render::answer(&mut out, &st, &answer)?;
            });
        }

        Command::Fact {
            key,
            claim,
            subject,
        } => {
            let mut engine = open(&root)?;
            engine.record_fact(nexus_core::FactInput {
                key: key.clone(),
                scope: if subject.is_some() {
                    "module".into()
                } else {
                    "project".into()
                },
                subject: subject.clone(),
                claim: claim.clone(),
                // Entered by a person at a terminal, and ranked above an inferred fact.
                source: "human".into(),
                evidence: Vec::new(),
                confidence: 1.0,
            })?;
            if !cli.quiet {
                writeln!(out, "remembered: {key}")?;
            }
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

/// The composition root: this is the one place that knows both the platform and which
/// capabilities exist. Nexus never compiles a capability in; it is handed them here.
fn open(root: &std::path::Path) -> Result<Engine, EngineError> {
    let mut engine = Engine::open(root)?;
    engine.register_capability(Box::new(BugHunter::new()));
    engine.register_capability(Box::new(Architect::new()));
    engine.register_capability(Box::new(Review::new()));
    Ok(engine)
}

fn open_or_init(root: &std::path::Path) -> Result<(Engine, bool), EngineError> {
    let (mut engine, fresh) = Engine::open_or_init(root)?;
    engine.register_capability(Box::new(BugHunter::new()));
    engine.register_capability(Box::new(Architect::new()));
    engine.register_capability(Box::new(Review::new()));
    Ok((engine, fresh))
}

#[derive(Serialize)]
struct ChangeOut {
    entity: String,
    change_type: String,
    target: Option<String>,
    detail: Option<String>,
}

/// One versioned envelope for every command.
///
/// `schema` is 2: version 1 named the commands `bugs` and `hunt`, and the platform renamed
/// their canonical forms to `findings` and `analyze` (the old names still work as aliases).
/// Findings also gained a `capability` field. Additive changes do not move this number;
/// a renamed value does, because a script matching on it would otherwise fail silently.
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
        schema: 2,
        command: match &cli.command {
            Command::Init => "init",
            Command::Scan => "scan",
            Command::Rescan => "rescan",
            Command::Status => "status",
            Command::Changes { .. } => "changes",
            Command::Impact { .. } => "impact",
            Command::Graph => "graph",
            Command::Findings { .. } => "findings",
            Command::Finding { .. } => "finding",
            Command::Analyze { .. } => "analyze",
            Command::Capabilities => "capabilities",
            Command::Ask { .. } => "ask",
            Command::Fact { .. } => "fact",
            Command::Ignore { .. } => "ignore",
            Command::Doctor => "doctor",
            Command::Mcp => "mcp",
        },
        result: value,
    })
}
