//! `.nexus/policy.toml` — committed team intent, read at request time.
//!
//! Ranking weights live here rather than in code (§6: *weights are data, not code*), so
//! tuning is a config change and a re-run instead of a release. That is not a convenience:
//! it is what makes the first real tuning, in Phase 5.7, an edit backed by ledger evidence
//! rather than a patch backed by an argument.
//!
//! A missing or malformed file yields the documented defaults and a note. Failing a context
//! request because a config file has a typo would put the ranker on the critical path of a
//! per-prompt hook for no gain — the defaults are always a valid answer.

use serde::Deserialize;

/// §6's weighted sum, one weight per term.
///
/// The defaults are argued, not fitted, and the roadmap forbids tuning them in Phase 2: ship
/// the ledger, gather evidence, then tune. Seeds dominate because an explicitly named symbol
/// is not a guess. History is next because a regression is the single most useful thing to
/// know before editing. Cost is real but never decisive alone, or the package fills with
/// cheap trivia.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Weights {
    pub seed: f64,
    pub graph: f64,
    pub churn: f64,
    pub recency: f64,
    pub history: f64,
    pub fact: f64,
    pub test: f64,
    pub arch: f64,
    pub cost: f64,
    /// Items scoring below this are excluded even with budget remaining (§7).
    pub min_score: f64,
    /// At most this many items from one file before another component gets a turn (§7).
    pub max_per_component: usize,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            seed: 1.0,
            graph: 0.8,
            churn: 0.3,
            recency: 0.2,
            history: 0.6,
            fact: 0.5,
            test: 0.3,
            arch: 0.3,
            cost: 0.4,
            min_score: 0.15,
            max_per_component: 3,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct PolicyFile {
    #[serde(default)]
    context: ContextSection,
}

#[derive(Debug, Default, Deserialize)]
struct ContextSection {
    #[serde(default)]
    weights: Option<Weights>,
}

/// What was loaded, and whether it came from the file.
#[derive(Debug, Clone)]
pub struct LoadedWeights {
    pub weights: Weights,
    pub note: Option<String>,
}

impl Weights {
    /// A stable identity for this weight set, for the package cache key (§11).
    ///
    /// Formatted rather than bit-hashed so that two runs that parsed the same file agree, and
    /// so that a human reading a cache filename can tell two policies apart.
    pub fn hash(&self) -> String {
        let s = format!(
            "{:.4}|{:.4}|{:.4}|{:.4}|{:.4}|{:.4}|{:.4}|{:.4}|{:.4}|{:.4}|{}",
            self.seed,
            self.graph,
            self.churn,
            self.recency,
            self.history,
            self.fact,
            self.test,
            self.arch,
            self.cost,
            self.min_score,
            self.max_per_component
        );
        blake3::hash(s.as_bytes()).to_hex()[..16].to_string()
    }
}

/// Read `[context.weights]` from a policy file. Never fails.
pub fn load(policy_toml: &std::path::Path) -> LoadedWeights {
    let raw = match std::fs::read_to_string(policy_toml) {
        Ok(raw) => raw,
        Err(_) => {
            return LoadedWeights {
                weights: Weights::default(),
                note: None, // A project that never wrote one is the normal case, not a problem.
            };
        }
    };
    match toml::from_str::<PolicyFile>(&raw) {
        Ok(p) => LoadedWeights {
            weights: p.context.weights.unwrap_or_default(),
            note: None,
        },
        Err(e) => LoadedWeights {
            weights: Weights::default(),
            note: Some(format!(
                "{} could not be parsed ({e}), so the default ranking weights were used",
                policy_toml.display()
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(body: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "nexus-policy-{}-{}.toml",
            std::process::id(),
            body.len()
        ));
        std::fs::write(&p, body).expect("write");
        p
    }

    #[test]
    fn a_missing_file_yields_the_documented_defaults_without_complaint() {
        let got = load(std::path::Path::new("/nonexistent/policy.toml"));
        assert_eq!(got.weights, Weights::default());
        assert!(got.note.is_none(), "not writing one is the normal case");
    }

    #[test]
    fn a_weight_in_the_file_overrides_the_default_without_recompiling() {
        let p = write("[context.weights]\nchurn = 0.9\n");
        let got = load(&p);
        assert_eq!(got.weights.churn, 0.9);
        assert_eq!(
            got.weights.seed,
            Weights::default().seed,
            "an unset weight keeps its default"
        );
    }

    #[test]
    fn a_malformed_file_is_a_note_and_the_defaults_never_an_error() {
        // A context request is on a per-prompt hook. Failing it over a config typo would put
        // the ranker on the critical path for no gain: the defaults are always valid.
        let p = write("[context.weights]\nchurn = \"very high\"\n");
        let got = load(&p);
        assert_eq!(got.weights, Weights::default());
        assert!(got.note.is_some(), "the typo is reported, not swallowed");
    }

    #[test]
    fn a_policy_with_no_context_section_is_not_an_error() {
        let p = write("[permissions]\nexecute = \"none\"\n");
        let got = load(&p);
        assert_eq!(got.weights, Weights::default());
        assert!(got.note.is_none());
    }

    #[test]
    fn the_hash_changes_when_a_weight_changes_and_not_otherwise() {
        let a = Weights::default();
        let mut b = Weights::default();
        assert_eq!(a.hash(), b.hash());
        b.churn = 0.31;
        assert_ne!(
            a.hash(),
            b.hash(),
            "the cache key must notice a re-weighting"
        );
    }
}

/// What the committed policy permits an execution to do.
#[derive(Debug, Clone)]
pub struct Execution {
    /// `docker` | `host` | `none`. Default `none`, and the default is the point: a freshly
    /// initialized project can index, diff and analyze but cannot run anything until someone
    /// commits a change saying otherwise.
    pub execute: String,
    pub timeout_seconds: u64,
}

impl Default for Execution {
    fn default() -> Self {
        Execution {
            execute: "none".into(),
            timeout_seconds: 600,
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct ExecutionFile {
    #[serde(default)]
    permissions: PermissionsSection,
    #[serde(default)]
    execute: ExecuteSection,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PermissionsSection {
    #[serde(default)]
    execute: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ExecuteSection {
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

/// Read `[permissions] execute` and `[execute] timeout_seconds`.
///
/// An unreadable or malformed file yields the safe default rather than an error. Failing
/// closed here means "we could not read the permission, so we did not run anything", which is
/// the only safe way for a permission check to fail.
pub fn load_execution(policy_toml: &std::path::Path) -> Execution {
    let Ok(raw) = std::fs::read_to_string(policy_toml) else {
        return Execution::default();
    };
    let Ok(f) = toml::from_str::<ExecutionFile>(&raw) else {
        return Execution::default();
    };
    let execute = f.permissions.execute.unwrap_or_else(|| "none".into());
    Execution {
        // Anything unrecognised is treated as "none". A typo must not be a grant.
        execute: match execute.as_str() {
            "host" | "docker" => execute,
            _ => "none".into(),
        },
        timeout_seconds: f.execute.timeout_seconds.unwrap_or(600).clamp(1, 3600),
    }
}
