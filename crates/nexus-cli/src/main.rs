//! `bughunter` — the composition root.
//!
//! Parse flags, open the store, build the analyzer registry, construct the `Engine`,
//! dispatch. This is the only place in the workspace that knows about all of those at once,
//! and the only place `anyhow` is used: a library that returns `anyhow::Error` has told its
//! caller nothing.

#![forbid(unsafe_code)]

mod ask;
mod fixture;
mod hooks;
mod plugin;
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
enum MemoryCommand {
    /// Write one Markdown file per namespace. Generated, never read back.
    Export {
        /// Where to write. Created if absent.
        #[arg(long, default_value = "docs/knowledge")]
        markdown: PathBuf,
    },
    /// Read an external knowledge graph's claims into project memory
    ///
    /// graphify's structural pass already reaches Nexus as edges. Its semantic pass costs
    /// model calls and produces claims about the project; this is how they arrive, as
    /// ordinary facts ranked and budgeted with everything else.
    Import {
        /// The graph to read.
        #[arg(long, default_value = "graphify-out/graph.json")]
        from: PathBuf,
    },
}

#[derive(Subcommand)]
enum ShareCommand {
    /// Write facts and findings to one JSON document, safe to commit
    Export {
        /// Where to write. Defaults to stdout.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
    /// Merge a document written by `export`. Conflicts are reported, never resolved.
    Import { file: PathBuf },
}

#[derive(Subcommand)]
enum Command {
    /// Detect the project, create .nexus/, migrate the database
    Init {
        /// Also install the Claude Code hooks. Off by default: a hook whose latency has not
        /// been measured on this project is not turned on uninvited.
        #[arg(long)]
        hooks: bool,
        /// Additionally install the `Stop` verification gate. Separate from `--hooks`
        /// because it runs a real build at the end of every turn — `verify --changed` does
        /// not scope yet, so the run is the whole project, and on a Gradle build that is
        /// minutes. Worth having deliberately; not worth acquiring by accident.
        #[arg(long, requires = "hooks")]
        verify: bool,
    },
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
    Graph {
        /// Also write every edge row to this path as NDJSON, for `nexus-eval`.
        /// Not stdout: `--json` is exactly one document, and an edge list is not it.
        #[arg(long, value_name = "PATH")]
        edges: Option<PathBuf>,
        /// Also write every indexed file path to this path, one per line. The accuracy
        /// harness's coverage denominator, which the edge dump cannot supply: a file with
        /// no edges is still a file the oracle was supposed to index.
        #[arg(long, value_name = "PATH")]
        files: Option<PathBuf>,
    },
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
    /// Run this project's own build, test and lint, and judge the result
    Verify {
        /// Reserved for scoping a future run to the changed set. Accepted now so the Stop
        /// hook's command string does not change when scoping lands.
        #[arg(long)]
        changed: bool,
        /// Write a reproduction scaffold for this finding instead of running anything.
        ///
        /// It does not reproduce the defect: it names it, quotes its evidence and fails until
        /// somebody writes the assertion. Written only inside .nexus/generated-tests.
        #[arg(long, value_name = "UID")]
        reproduce: Option<String>,
    },
    /// Move findings and facts between machines, over a file rather than a server
    Share {
        #[command(subcommand)]
        cmd: ShareCommand,
    },
    /// Project memory: what has been learned, as files a person can read
    Memory {
        #[command(subcommand)]
        cmd: MemoryCommand,
    },
    /// What an agent should know before it reads a file
    Context {
        /// The session package: what this project is, what is open, what is known
        #[arg(long)]
        session: bool,
        /// The package for one task, ranked and budgeted
        #[arg(long, value_name = "TEXT")]
        task: Option<String>,
        /// Token ceiling. The package is selected to fit, never truncated to fit.
        #[arg(long, value_name = "TOKENS")]
        budget: Option<usize>,
        /// Anchors the caller already has — a hook editing a file knows
        #[arg(long, value_name = "PATH")]
        file: Vec<String>,
        #[arg(long, value_name = "FQN")]
        symbol: Vec<String>,
        /// Why every candidate is in or out, with the score terms that decided it
        #[arg(long)]
        explain: bool,
        /// Counts only: considered, included, tokens
        #[arg(long)]
        stats: bool,
        /// Anchors from the previous package in this conversation. The harness has the
        /// conversation; Nexus does not and will not.
        #[arg(long, value_name = "FQN", value_delimiter = ',')]
        carry_seeds: Vec<String>,
        /// The previous user message. Reaches intent classification and nothing else — it is
        /// never stored, never indexed, and never reaches the database.
        #[arg(long, value_name = "TEXT")]
        recent: Option<String>,
        /// What the packages built so far say about the ranking weights.
        ///
        /// Reports rather than tunes, and refuses to recommend anything until there are
        /// enough packages to be about the project rather than about one session.
        #[arg(long)]
        weights: bool,
    },
    /// Record something learned about this project, so the next session starts with it
    Fact {
        /// e.g. arch.payment.idempotency. The namespace is one of arch, constraint,
        /// convention, decision, discovery, failure, incident, invariant, pattern, risk.
        key: String,
        /// One sentence
        claim: String,
        #[arg(long)]
        subject: Option<String>,
        /// Where in the code this is true, as PATH:LINE. Repeatable.
        ///
        /// Without it the fact is remembered but unanchored: nothing can check it against a
        /// later scan, so it is never invalidated when the code moves and never appears in a
        /// context package, which requires a file:line on every item.
        #[arg(long, value_name = "PATH:LINE")]
        evidence: Vec<String>,
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
    /// Build the benchmark fixture corpus from its specifications
    Fixture {
        #[command(subcommand)]
        cmd: fixture::Cmd,
    },
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
            eprintln!(
                "{}: {}",
                render::binary_name(),
                render::with_binary(&e.to_string())
            );
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
        // No project, no store, no engine: this builds repositories rather than reading one,
        // so it is dispatched before anything tries to open `.nexus/`.
        Command::Fixture { cmd } => {
            let (report, code) = fixture::run(cmd, &mut out, cli.json, cli.quiet)?;
            if cli.json {
                writeln!(out, "{}", envelope(cli, &report)?)?;
            } else if !cli.quiet {
                fixture::render(&mut out, &report)?;
            }
            return Ok(code);
        }

        Command::Init { hooks, verify } => {
            let (_engine, profile) = Engine::init(&root, nexus_lang_pack::default_registry())?;
            let installed = if *hooks {
                Some(hooks::install(&root, *verify)?)
            } else {
                None
            };
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
                match installed {
                    // Naming them is the point: a hook the developer did not know they had
                    // is the thing ADR-024's off-by-default exists to prevent, and saying
                    // "the SessionStart hook" while installing three said the wrong number.
                    Some(hooks::Outcome::Installed) => {
                        writeln!(
                            out,
                            "Installed hooks in .claude/settings.json: SessionStart, \
                             UserPromptSubmit, PostToolUse ({})",
                            hooks::EDIT_TOOLS
                        )?;
                        if *verify {
                            writeln!(out, "  and Stop — a full build runs at the end of a turn")?;
                        }
                    }
                    Some(hooks::Outcome::AlreadyPresent) => {
                        writeln!(out, "The hooks were already installed")?;
                    }
                    None => {}
                }
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
            let mut report = engine.scan()?;

            // The first scan is when someone learns what this tool thinks their project is,
            // and it is the only moment they are certainly paying attention. Architect runs
            // here rather than at `init` because two of its three rules need an index —
            // before one exists it could only report the datastore rule, and its
            // missing-scaffolding rule would have no indexed build file to point at.
            //
            // Failure is not fatal: a capability that cannot run must not cost someone their
            // scan, which is the expensive part.
            //
            // Its result goes *into* the scan report and is emitted with it. Emitting it
            // separately printed a second JSON document on stdout, which parses as neither —
            // and every consumer broke on it, including this project's own CI smoke check.
            report.architect = engine
                .analyze("architect", Scope::Everything)
                .ok()
                .filter(|a| !a.findings.is_empty());

            emit!(&report, {
                render::banner(&mut out, &st)?;
                render::scan(&mut out, &st, &report)?;
                if let Some(arc) = &report.architect {
                    writeln!(out)?;
                    render::analyze(&mut out, &st, arc)?;
                }
            });
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

        Command::Graph { edges, files } => {
            let engine = open(&root)?;
            if let Some(path) = edges {
                // One JSON object per line rather than one document: this is the only output
                // whose size is proportional to the repository rather than to the answer,
                // and a consumer should be able to read it a line at a time.
                let mut w = create(path)?;
                for rec in engine.edge_records()? {
                    writeln!(w, "{}", serde_json::to_string(&rec)?)?;
                }
                w.flush()?;
            }
            if let Some(path) = files {
                let mut w = create(path)?;
                for f in engine.indexed_files()? {
                    writeln!(w, "{f}")?;
                }
                w.flush()?;
            }
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

        Command::Verify {
            changed: _,
            reproduce,
        } => {
            let mut engine = open(&root)?;
            if let Some(uid) = reproduce {
                let path = engine.reproduce(uid)?;
                if !cli.quiet {
                    writeln!(out, "wrote {path}")?;
                    writeln!(
                        out,
                        "  {}",
                        st.dim("it fails until you write the assertion — that is the point")
                    )?;
                }
                return Ok(exit::OK);
            }
            let report = engine.verify()?;
            let failed = report.verdict == "failed";
            emit!(&report, {
                render::verify(&mut out, &st, &report)?;
            });
            // Finding a problem is the tool doing its job, so it is not a runtime error. An
            // inconclusive verdict is not a failure either: nothing was concluded, and
            // exiting non-zero for that is how a gate gets switched off.
            if failed {
                return Ok(exit::FINDINGS);
            }
        }

        Command::Share { cmd } => match cmd {
            ShareCommand::Export { out: dest } => {
                let engine = open(&root)?;
                let doc = engine.export_portable()?;
                let body = serde_json::to_string_pretty(&doc)?;
                match dest {
                    Some(path) => {
                        std::fs::write(path, format!("{body}\n"))?;
                        if !cli.quiet {
                            writeln!(
                                out,
                                "wrote {} fact(s) and {} finding(s) to {}",
                                doc.facts.len(),
                                doc.findings.len(),
                                path.display()
                            )?;
                        }
                    }
                    // stdout is results: the document *is* the result, so it goes there whole
                    // and nothing else does.
                    None => writeln!(out, "{body}")?,
                }
            }
            ShareCommand::Import { file } => {
                let raw = std::fs::read_to_string(file)?;
                let doc: nexus_core::portable::Portable = serde_json::from_str(&raw)?;
                let mut engine = open(&root)?;
                let report = engine.import_portable(&doc)?;
                emit!(&report, {
                    writeln!(
                        out,
                        "{} fact(s) added, {} already here",
                        report.facts_added, report.facts_unchanged
                    )?;
                    if !report.conflicts.is_empty() {
                        writeln!(out)?;
                        writeln!(
                            out,
                            "{}",
                            st.warn(&format!(
                                "{} conflict(s), none applied:",
                                report.conflicts.len()
                            ))
                        )?;
                        for c in &report.conflicts {
                            writeln!(out, "  {c}")?;
                        }
                    }
                });
            }
        },

        Command::Memory {
            cmd: MemoryCommand::Import { from },
        } => {
            let mut engine = open(&root)?;
            let path = if from.is_absolute() {
                from.clone()
            } else {
                root.join(from)
            };
            let r = engine.import_graphify(&path)?;
            emit!(&r, {
                writeln!(out, "{}", st.head("Imported"))?;
                writeln!(
                    out,
                    "  {} claim(s) read, {} recorded, {} anchored on code, {} not a claim, {} skipped",
                    r.concepts_read, r.facts_recorded, r.anchored_on_code, r.skipped_not_a_claim, r.skipped
                )?;
                for w in &r.warnings {
                    writeln!(out, "  {}", st.dim(w))?;
                }
                writeln!(
                    out,
                    "\n{}",
                    st.dim(
                        "Recorded as `ai` facts at confidence 0.5 — a model wrote them and \
                         nothing has verified them against the code."
                    )
                )?;
            });
        }

        Command::Memory {
            cmd: MemoryCommand::Export { markdown },
        } => {
            let engine = open(&root)?;
            let grouped = engine.memory_export()?;
            // Relative paths land in the project, not in whatever directory the terminal
            // happens to be in — this is a project artefact and `--project` says which one.
            let dir = if markdown.is_absolute() {
                markdown.clone()
            } else {
                root.join(markdown)
            };
            std::fs::create_dir_all(&dir)?;
            let mut written = Vec::new();
            for (namespace, facts) in &grouped {
                let path = dir.join(format!("{namespace}.md"));
                std::fs::write(&path, nexus_core::memory::to_markdown(namespace, facts))?;
                written.push((path, facts.len()));
            }
            emit!(
                &grouped
                    .iter()
                    .map(|(n, f)| serde_json::json!({"namespace": n, "facts": f.len()}))
                    .collect::<Vec<_>>(),
                {
                    if written.is_empty() {
                        writeln!(out, "{}", st.dim("No facts to export yet."))?;
                    } else {
                        writeln!(out, "{}", st.head("Exported"))?;
                        for (path, n) in &written {
                            writeln!(out, "  {:<40} {n} fact(s)", path.display())?;
                        }
                        writeln!(
                            out,
                            "\n{}",
                            st.dim(
                                "Generated. Nexus never reads these back — to add \
                                    knowledge use `nexus fact`."
                            )
                        )?;
                    }
                }
            );
        }

        Command::Context {
            session,
            task,
            budget,
            file,
            symbol,
            explain,
            stats,
            carry_seeds,
            recent,
            weights,
        } => {
            if *weights {
                let report =
                    nexus_core::tuning::report(&root.join(nexus_core::NEXUS_DIR).join("cache"));
                emit!(&report, {
                    render::weights(&mut out, &st, &report)?;
                });
                return Ok(exit::OK);
            }
            // Exactly one shape per invocation. Defaulting to one of them would make a bare
            // `nexus context` mean something different depending on which flags exist.
            let request = match (session, task) {
                (true, Some(_)) => {
                    eprintln!("nexus context: --session and --task ask different questions");
                    return Ok(exit::USAGE);
                }
                (false, None) => {
                    eprintln!("nexus context: one of --session or --task is required");
                    return Ok(exit::USAGE);
                }
                (true, None) => {
                    let mut r = nexus_core::TaskRequest::session(
                        budget.unwrap_or(nexus_core::context::SESSION_BUDGET_TOKENS),
                    );
                    r.explain = *explain;
                    r
                }
                (false, Some(text)) => nexus_core::TaskRequest {
                    text: text.clone(),
                    files: file.clone(),
                    symbols: symbol.clone(),
                    budget_tokens: budget.unwrap_or(nexus_core::context::TASK_BUDGET_TOKENS),
                    purpose: nexus_core::Purpose::Task,
                    explain: *explain,
                    carry_seeds: carry_seeds.clone(),
                    recent: recent.clone(),
                },
            };
            let engine = open(&root)?;
            match engine.context(&request) {
                Ok(pkg) => {
                    emit!(&pkg, {
                        if *stats {
                            render::context_stats(&mut out, &pkg)?;
                        } else {
                            render::context(&mut out, &st, &pkg)?;
                            if *explain {
                                render::context_explain(&mut out, &st, &pkg)?;
                            }
                        }
                    });
                }
                Err(nexus_core::EngineError::NoBaseline) => {
                    // Not an error worth a stack trace: the project simply has not been
                    // scanned. The exit code carries it; the hook ignores the code.
                    if !cli.quiet && !cli.json {
                        writeln!(out, "No baseline — run `{} scan`.", render::binary_name())?;
                    }
                    return Ok(exit::NO_BASELINE);
                }
                Err(e) => return Err(e.into()),
            }
        }

        Command::Fact {
            key,
            claim,
            subject,
            evidence,
        } => {
            let mut refs = Vec::new();
            for e in evidence {
                let Some((path, line)) = e.rsplit_once(':') else {
                    eprintln!("nexus fact: --evidence wants PATH:LINE, got '{e}'");
                    return Ok(exit::USAGE);
                };
                let Ok(line) = line.parse::<u32>() else {
                    eprintln!("nexus fact: '{line}' in '{e}' is not a line number");
                    return Ok(exit::USAGE);
                };
                refs.push(nexus_core::findings::CodeRef {
                    file: path.to_string(),
                    line,
                    note: String::new(),
                });
            }
            let anchored = !refs.is_empty();
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
                evidence: refs,
                confidence: 1.0,
            })?;
            if !cli.quiet {
                writeln!(out, "remembered: {key}")?;
                if !anchored {
                    // Said once, plainly, rather than left to be discovered when the fact
                    // never turns up in a package.
                    writeln!(
                        out,
                        "  {}",
                        st.dim(
                            "no --evidence, so nothing can check this against a later scan and \
                             it will not appear in a context package"
                        )
                    )?;
                }
            }
        }

        Command::Doctor => {
            let engine = open(&root)?;
            let mut checks = engine.doctor()?;
            // Appended here rather than produced by the core: `.claude/settings.json` is one
            // agent's format, and the core must not learn it. ADR-024 names `doctor` as the
            // compensating control for fail-open hiding hook failures by construction.
            checks.push(hooks::health(&root));
            checks.push(plugin::health());
            let worst = checks.iter().any(|c| c.level == "error");
            emit!(&checks, {
                writeln!(
                    out,
                    "{}",
                    st.head(&format!("{} doctor", render::product_name()))
                )?;
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

/// The composition root: this is the one place in the CLI that knows both the platform and
/// which capabilities exist. Nexus never compiles a capability in; it is handed them here.
///
/// `nexus-mcp` is the other root and keeps its own list, because it must — a handler cannot
/// reach into the CLI. `tests/boundaries.rs` asserts the two agree, which is what makes two
/// lists safe rather than merely tolerated.
fn register_capabilities(engine: &mut Engine) {
    engine.register_capability(Box::new(BugHunter::new()));
    engine.register_capability(Box::new(Architect::new()));
    engine.register_capability(Box::new(Review::new()));
}

/// Create a file for one of `graph`'s side outputs, naming the path when it cannot.
///
/// `std::fs::File::create` alone reports "No such file or directory (os error 2)" and leaves
/// the reader to guess which of two `--` flags they mistyped.
fn create(path: &std::path::Path) -> anyhow::Result<std::io::BufWriter<std::fs::File>> {
    let file = std::fs::File::create(path)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
    Ok(std::io::BufWriter::new(file))
}

fn open(root: &std::path::Path) -> Result<Engine, EngineError> {
    let mut engine = Engine::open(root, nexus_lang_pack::default_registry())?;
    register_capabilities(&mut engine);
    Ok(engine)
}

fn open_or_init(root: &std::path::Path) -> Result<(Engine, bool), EngineError> {
    let (mut engine, fresh) = Engine::open_or_init(root, nexus_lang_pack::default_registry)?;
    register_capabilities(&mut engine);
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
            Command::Init { .. } => "init",
            Command::Scan => "scan",
            Command::Rescan => "rescan",
            Command::Status => "status",
            Command::Changes { .. } => "changes",
            Command::Impact { .. } => "impact",
            Command::Graph { .. } => "graph",
            Command::Findings { .. } => "findings",
            Command::Finding { .. } => "finding",
            Command::Analyze { .. } => "analyze",
            Command::Capabilities => "capabilities",
            Command::Ask { .. } => "ask",
            Command::Context { .. } => "context",
            Command::Memory { .. } => "memory",
            Command::Share { .. } => "share",
            Command::Verify { .. } => "verify",
            Command::Fact { .. } => "fact",
            Command::Ignore { .. } => "ignore",
            Command::Doctor => "doctor",
            Command::Mcp => "mcp",
            Command::Fixture { .. } => "fixture",
        },
        result: value,
    })
}
