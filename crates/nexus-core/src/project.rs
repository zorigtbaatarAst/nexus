//! The prepared project snapshot a capability is handed.
//!
//! `ProjectContext` is the whole index as plain data: symbols, edges, files, what changed in
//! the scan under analysis, and the detected profile. A capability reads it and returns
//! findings; it never touches storage, git or the CLI, which is what lets `nexus-core` decide
//! whether a finding is new, recurring, fixed or regressed without every capability
//! re-implementing the answer.
//!
//! `Scoped` is that snapshot narrowed to what was asked for. Narrowing happens once, here,
//! rather than in each rule: a rule that reaches past `scoped` to `ctx` is doing something
//! deliberate, and one that forgets to narrow makes a targeted analysis quietly cost what a
//! full one costs.

use nexus_types::ChangeKind;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SymbolFacts {
    pub fqn: String,
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub visibility: Option<String>,
    pub parent_fqn: Option<String>,
    pub annotations: Vec<String>,
}

impl SymbolFacts {
    pub fn has_annotation(&self, name: &str) -> bool {
        self.annotations.iter().any(|a| {
            let bare = a.trim_start_matches('@');
            bare == name || bare.starts_with(&format!("{name}("))
        })
    }

    /// The class or module this belongs to, used as the `component` half of an identity.
    pub fn component(&self) -> String {
        let owner = self.parent_fqn.as_deref().unwrap_or(&self.fqn);
        owner.rsplit('.').next().unwrap_or(owner).to_string()
    }
}

#[derive(Debug, Clone)]
pub struct EdgeFacts {
    pub src_fqn: String,
    pub dst_fqn: Option<String>,
    pub dst_hint: Option<String>,
    pub edge_type: String,
    pub resolution: String,
    pub line: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct FileFacts {
    pub path: String,
    pub lang: Option<String>,
}

/// A symbol that moved in the scan being analyzed, and how.
#[derive(Debug, Clone)]
pub struct ChangedSymbol {
    pub fqn: String,
    pub path: String,
    pub kind: ChangeKind,
}

/// The project narrowed to what a capability was asked to look at.
pub struct Scoped<'a> {
    pub symbols: Vec<&'a SymbolFacts>,
    pub files: Vec<&'a FileFacts>,
}

pub struct ProjectContext<'a> {
    pub root: &'a Path,
    pub symbols: &'a [SymbolFacts],
    pub edges: &'a [EdgeFacts],
    pub files: &'a [FileFacts],
    /// What moved in the scan under analysis. Empty for a full analysis; the point of
    /// carrying it is that a capability can narrow its own work the way `Scope` asks it to.
    pub changed: &'a [ChangedSymbol],
    /// The commit this snapshot describes, when the project is under version control.
    pub commit: Option<&'a str>,
    /// What the platform already worked out about this project: languages, frameworks,
    /// build system, datastores, containers — each with the file and line that proved it.
    /// Carried so a capability does not re-derive what `detect` has already established;
    /// `None` when no profile has been saved yet, which a rule must tolerate rather than
    /// assume away.
    pub profile: Option<&'a crate::report::Profile>,
    /// Indexed lookup, built once: a capability that scans the symbol list per edge turns a
    /// linear pass into a quadratic one, and the graph is the biggest thing here.
    pub by_fqn: BTreeMap<&'a str, &'a SymbolFacts>,
    /// Symbols a test run actually reached (roadmap 4.5).
    ///
    /// Empty until something has been verified, and empty is *not* the same as uncovered — a
    /// rule must check [`Self::has_coverage_evidence`] before reading a conclusion into it.
    /// This is what turns "nothing tests this" from a filename heuristic into evidence, and
    /// keeping the distinction visible is the whole point of carrying it separately.
    pub covered: std::collections::BTreeSet<String>,
}

impl<'a> ProjectContext<'a> {
    /// Whether a real run has established coverage for this project at all.
    ///
    /// The difference between "no test reaches this" and "nothing has run, so nobody knows"
    /// is the difference between a finding and a guess, and a rule that cannot tell them
    /// apart will state the second as the first.
    pub fn has_coverage_evidence(&self) -> bool {
        !self.covered.is_empty()
    }

    pub fn new(
        root: &'a Path,
        symbols: &'a [SymbolFacts],
        edges: &'a [EdgeFacts],
        files: &'a [FileFacts],
    ) -> Self {
        let by_fqn = symbols.iter().map(|s| (s.fqn.as_str(), s)).collect();
        ProjectContext {
            root,
            symbols,
            edges,
            files,
            changed: &[],
            commit: None,
            profile: None,
            by_fqn,
            covered: Default::default(),
        }
    }

    /// Attach what a real run proved. Called by the engine, which is the only thing that has
    /// both the store and a reason to read it.
    pub fn with_coverage(mut self, covered: std::collections::BTreeSet<String>) -> Self {
        self.covered = covered;
        self
    }

    pub fn with_changes(mut self, changed: &'a [ChangedSymbol], commit: Option<&'a str>) -> Self {
        self.changed = changed;
        self.commit = commit;
        self
    }

    pub fn with_profile(mut self, profile: Option<&'a crate::report::Profile>) -> Self {
        self.profile = profile;
        self
    }

    pub fn symbol(&self, fqn: &str) -> Option<&SymbolFacts> {
        self.by_fqn.get(fqn).copied()
    }

    /// The project narrowed to a scope.
    ///
    /// Narrow inputs by default: a rule that iterates `scoped.symbols` is targeted by
    /// construction, and one that reaches past to `ctx.symbols` is doing something
    /// deliberate — a self-invocation rule needs the callee's annotations even when the
    /// callee is out of scope. Leaving each rule to remember to narrow is how a targeted
    /// analysis quietly comes to cost what a full one costs.
    pub fn scoped(&'a self, scope: &crate::capability::Scope) -> Scoped<'a> {
        let symbols = self.in_scope(scope);
        let paths: std::collections::HashSet<&str> =
            symbols.iter().map(|s| s.file.as_str()).collect();
        use crate::capability::Scope;
        let files = match scope {
            Scope::Everything => self.files.iter().collect(),
            // A file with no symbols — a properties file, a schema — is still in scope when
            // it was named directly. Deriving the file set from symbols alone would make
            // the secret scanner blind under a file scope.
            Scope::Files(named) => self
                .files
                .iter()
                .filter(|f| named.contains(&f.path) || paths.contains(f.path.as_str()))
                .collect(),
            _ => self
                .files
                .iter()
                .filter(|f| paths.contains(f.path.as_str()))
                .collect(),
        };
        Scoped { symbols, files }
    }

    /// Symbols a capability should examine under this scope.
    ///
    /// Living here rather than in each capability means one implementation of "what does
    /// narrow actually mean". A capability that forgot to narrow is why a targeted analysis
    /// would otherwise quietly cost the same as a full one.
    pub fn in_scope(&self, scope: &crate::capability::Scope) -> Vec<&SymbolFacts> {
        use crate::capability::Scope;
        match scope {
            Scope::Everything => self.symbols.iter().collect(),
            Scope::Changed { .. } => {
                let names: std::collections::HashSet<&str> =
                    self.changed.iter().map(|c| c.fqn.as_str()).collect();
                let paths: std::collections::HashSet<&str> =
                    self.changed.iter().map(|c| c.path.as_str()).collect();
                // A changed method drags its file in: a rule about a class needs the class
                // even when only one of its methods moved.
                self.symbols
                    .iter()
                    .filter(|s| names.contains(s.fqn.as_str()) || paths.contains(s.file.as_str()))
                    .collect()
            }
            Scope::Symbols(fqns) => self
                .symbols
                .iter()
                .filter(|s| fqns.contains(&s.fqn))
                .collect(),
            Scope::Files(paths) => self
                .symbols
                .iter()
                .filter(|s| paths.contains(&s.file))
                .collect(),
        }
    }
}
