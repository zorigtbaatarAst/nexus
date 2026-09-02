//! `nexus fixture` — build the benchmark corpus.
//!
//! Test infrastructure behind a product binary, which is a fair question to ask about. The
//! answer is that the corpus is only useful if it is trivial to rebuild: a fixture somebody
//! has to remember how to regenerate is a fixture that goes stale, and a stale corpus makes
//! every measurement taken against it a measurement of the corpus.

use nexus_fixtures::spec::Spec;
use nexus_fixtures::{generate, Options};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Build fixture repositories from their specifications
    Generate {
        /// Build one fixture rather than the whole corpus
        #[arg(long, value_name = "NAME")]
        fixture: Option<String>,
        /// Where the specifications live
        #[arg(long, value_name = "DIR", default_value = nexus_fixtures::DEFAULT_SPEC_DIR)]
        spec_dir: PathBuf,
        /// Where to write. Under target/ by default, because that is already git-ignored
        #[arg(long, value_name = "DIR", default_value = nexus_fixtures::DEFAULT_OUT_DIR)]
        out: PathBuf,
        /// Replace an existing fixture
        #[arg(long)]
        force: bool,
        /// Also write one task file per task, with its commit resolved to a sha
        #[arg(long, value_name = "DIR")]
        emit_tasks: Option<PathBuf>,
    },
    /// Generate twice and prove the histories agree. The CI determinism gate
    Verify {
        #[arg(long, value_name = "NAME")]
        fixture: Option<String>,
        #[arg(long, value_name = "DIR", default_value = nexus_fixtures::DEFAULT_SPEC_DIR)]
        spec_dir: PathBuf,
    },
    /// What the corpus contains
    List {
        #[arg(long, value_name = "DIR", default_value = nexus_fixtures::DEFAULT_SPEC_DIR)]
        spec_dir: PathBuf,
    },
}

#[derive(Serialize)]
pub struct Built {
    pub name: String,
    pub repo: String,
    pub manifest: String,
    pub commits: usize,
    pub tasks: usize,
    pub patches: usize,
    pub head: String,
    pub spec_digest: String,
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Report {
    Generated {
        out: String,
        fixtures: Vec<Built>,
    },
    Verified {
        fixtures: Vec<Verified>,
        /// False when any fixture disagreed with itself. The exit code follows this.
        deterministic: bool,
    },
    Listed {
        fixtures: Vec<Listed>,
    },
}

#[derive(Serialize)]
pub struct Verified {
    pub name: String,
    pub deterministic: bool,
    pub commits: usize,
    pub head: String,
    /// The first commit id whose sha moved between runs, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diverged_at: Option<String>,
}

#[derive(Serialize)]
pub struct Listed {
    pub name: String,
    pub role: String,
    pub description: String,
    pub stack: Vec<String>,
    pub commits: usize,
    pub tasks: usize,
}

/// Load the corpus, or the single fixture that was named.
fn load(spec_dir: &Path, one: Option<&String>) -> Result<Vec<Spec>, Box<dyn std::error::Error>> {
    match one {
        Some(name) => Ok(vec![Spec::load(&spec_dir.join(name))?]),
        None => Ok(Spec::load_all(spec_dir)?),
    }
}

pub fn run(
    cmd: &Cmd,
    out: &mut impl Write,
    json: bool,
    quiet: bool,
) -> Result<(Report, u8), Box<dyn std::error::Error>> {
    match cmd {
        Cmd::Generate {
            fixture,
            spec_dir,
            out: out_dir,
            force,
            emit_tasks,
        } => {
            let specs = load(spec_dir, fixture.as_ref())?;
            let opts = Options {
                force: *force,
                emit_tasks: emit_tasks.clone(),
            };
            let mut built = Vec::new();
            for s in &specs {
                if !json && !quiet {
                    writeln!(out, "building {}", s.name())?;
                }
                let g = generate(s, out_dir, &opts)?;
                built.push(Built {
                    name: g.name.clone(),
                    repo: g.repo.display().to_string(),
                    manifest: g.manifest_path.display().to_string(),
                    commits: g.manifest.commits.len(),
                    tasks: g.manifest.tasks.len(),
                    patches: g.manifest.patches.len(),
                    head: g
                        .manifest
                        .commits
                        .last()
                        .map(|c| c.sha[..12].to_string())
                        .unwrap_or_default(),
                    spec_digest: g.manifest.spec_digest[..12].to_string(),
                });
            }
            Ok((
                Report::Generated {
                    out: out_dir.display().to_string(),
                    fixtures: built,
                },
                0,
            ))
        }

        Cmd::Verify { fixture, spec_dir } => {
            let specs = load(spec_dir, fixture.as_ref())?;
            // Two throwaway directories rather than one rebuilt in place: generating over a
            // previous run could pass by reusing something, and the point is to prove the
            // spec determines the history and nothing else does.
            let tmp =
                std::env::temp_dir().join(format!("nexus-fixture-verify-{}", std::process::id()));
            let mut results = Vec::new();
            let mut all = true;
            let opts = Options {
                force: true,
                ..Default::default()
            };
            for s in &specs {
                let a = generate(s, &tmp.join("a"), &opts)?;
                let b = generate(s, &tmp.join("b"), &opts)?;
                let diverged = a
                    .manifest
                    .commits
                    .iter()
                    .zip(b.manifest.commits.iter())
                    .find(|(x, y)| x.sha != y.sha)
                    .map(|(x, _)| x.id.clone());
                let ok = diverged.is_none()
                    && a.manifest.commits.len() == b.manifest.commits.len()
                    && a.manifest.spec_digest == b.manifest.spec_digest;
                all &= ok;
                results.push(Verified {
                    name: s.name().to_string(),
                    deterministic: ok,
                    commits: a.manifest.commits.len(),
                    head: a
                        .manifest
                        .commits
                        .last()
                        .map(|c| c.sha[..12].to_string())
                        .unwrap_or_default(),
                    diverged_at: diverged,
                });
            }
            let _ = std::fs::remove_dir_all(&tmp);
            Ok((
                Report::Verified {
                    fixtures: results,
                    deterministic: all,
                },
                // Non-zero on divergence: this runs in CI, and a determinism failure that
                // exits 0 is a determinism failure nobody hears about.
                if all { 0 } else { 1 },
            ))
        }

        Cmd::List { spec_dir } => {
            let specs = Spec::load_all(spec_dir)?;
            Ok((
                Report::Listed {
                    fixtures: specs
                        .iter()
                        .map(|s| Listed {
                            name: s.name().to_string(),
                            role: s.manifest.fixture.role.clone(),
                            description: s.manifest.fixture.description.clone(),
                            stack: s.manifest.fixture.stack.clone(),
                            commits: s.manifest.commit.len(),
                            tasks: s.manifest.task.len(),
                        })
                        .collect(),
                },
                0,
            ))
        }
    }
}

pub fn render(out: &mut impl Write, report: &Report) -> std::io::Result<()> {
    match report {
        Report::Generated { out: dir, fixtures } => {
            writeln!(out)?;
            for f in fixtures {
                writeln!(
                    out,
                    "  {:<18} {} commits · {} tasks · {} patches · head {}",
                    f.name, f.commits, f.tasks, f.patches, f.head
                )?;
            }
            writeln!(out)?;
            writeln!(out, "{} fixtures in {dir}", fixtures.len())?;
        }
        Report::Verified {
            fixtures,
            deterministic,
        } => {
            for f in fixtures {
                match &f.diverged_at {
                    None => writeln!(
                        out,
                        "  ok        {:<18} {} commits, head {}",
                        f.name, f.commits, f.head
                    )?,
                    Some(id) => writeln!(
                        out,
                        "  DIVERGED  {:<18} two runs disagree from commit `{id}` onward",
                        f.name
                    )?,
                }
            }
            writeln!(out)?;
            if *deterministic {
                writeln!(out, "{} fixtures are reproducible", fixtures.len())?;
            } else {
                writeln!(
                    out,
                    "not reproducible — a benchmark run against these would be measuring the corpus"
                )?;
            }
        }
        Report::Listed { fixtures } => {
            for f in fixtures {
                writeln!(out, "  {:<18} {:<20} {}", f.name, f.role, f.description)?;
                writeln!(
                    out,
                    "  {:<18} {} commits · {} tasks · {}",
                    "",
                    f.commits,
                    f.tasks,
                    f.stack.join(", ")
                )?;
                writeln!(out)?;
            }
        }
    }
    Ok(())
}
