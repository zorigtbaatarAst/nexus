//! The capability extension point.
//!
//! Nexus understands the project; capabilities use that understanding. A capability is
//! handed a prepared view of the index and a scope, and returns findings — Nexus owns
//! identity, lifecycle, storage and presentation.
//!
//! This is deliberately the *only* new abstraction in the platform. There is no plugin
//! loader, manifest format, dynamic dispatch over dynamic libraries, event bus, or
//! capability dependency graph, because none of those solve a problem that exists. What
//! does exist is one capability's logic sitting in the core, and this is the smallest thing
//! that gets it out.

use crate::findings::Finding;
use crate::project::ProjectContext;

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("{0}")]
    Failed(String),
}

/// What a capability was asked to look at.
///
/// This is what makes "do not re-analyze what Nexus already understands" a parameter rather
/// than an aspiration: `Changed` is fed straight from the rescan cascade, which already
/// knows exactly which symbols moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Everything,
    /// Only what changed since a scan, plus what that change reaches.
    Changed {
        since_scan: i64,
    },
    Symbols(Vec<String>),
    Files(Vec<String>),
}

impl Scope {
    /// Whether a file is in scope. `Everything` admits all; the narrow forms admit only
    /// what they name, which is where the saving comes from.
    pub fn admits_file(&self, path: &str) -> bool {
        match self {
            Scope::Everything | Scope::Changed { .. } | Scope::Symbols(_) => true,
            Scope::Files(fs) => fs.iter().any(|f| f == path),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Scope::Everything => "the whole project".into(),
            Scope::Changed { since_scan } => format!("what changed since scan {since_scan}"),
            Scope::Symbols(s) => format!("{} symbols", s.len()),
            Scope::Files(f) => format!("{} files", f.len()),
        }
    }
}

pub trait Capability: Send + Sync {
    /// Stable identifier, stored on every finding this capability produces. Renaming it
    /// orphans its findings, so it is chosen once.
    fn id(&self) -> &'static str;

    /// Display-id prefix — `BUG`, `SEC`, `REV`. A developer should never have to ask which
    /// subsystem a number came from.
    fn finding_prefix(&self) -> &'static str;

    fn describe(&self) -> &'static str;

    /// Examine the project and report what is wrong with it.
    ///
    /// A capability never touches storage, git or the CLI: it reads a snapshot and returns
    /// findings. That is what lets Nexus decide whether a finding is new, recurring, fixed
    /// or regressed without every capability re-implementing the answer.
    fn analyze(
        &self,
        ctx: &ProjectContext<'_>,
        scope: &Scope,
    ) -> Result<Vec<Finding>, CapabilityError>;
}

/// The capabilities available to this build.
#[derive(Default)]
pub struct Registry {
    capabilities: Vec<Box<dyn Capability>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, c: Box<dyn Capability>) -> &mut Self {
        self.capabilities.push(c);
        self
    }

    pub fn get(&self, id: &str) -> Option<&dyn Capability> {
        self.capabilities
            .iter()
            .find(|c| c.id() == id)
            .map(AsRef::as_ref)
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.capabilities.iter().map(|c| c.id()).collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &dyn Capability> {
        self.capabilities.iter().map(AsRef::as_ref)
    }

    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub(&'static str);
    impl Capability for Stub {
        fn id(&self) -> &'static str {
            self.0
        }
        fn finding_prefix(&self) -> &'static str {
            "STB"
        }
        fn describe(&self) -> &'static str {
            "stub"
        }
        fn analyze(
            &self,
            _: &ProjectContext<'_>,
            _: &Scope,
        ) -> Result<Vec<Finding>, CapabilityError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn the_registry_holds_more_than_one_capability() {
        let mut r = Registry::new();
        r.register(Box::new(Stub("bughunter")))
            .register(Box::new(Stub("security")));
        assert_eq!(r.ids(), vec!["bughunter", "security"]);
        assert!(r.get("security").is_some());
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn a_narrow_scope_admits_only_what_it_names() {
        let s = Scope::Files(vec!["a.java".into()]);
        assert!(s.admits_file("a.java"));
        assert!(
            !s.admits_file("b.java"),
            "otherwise the scope saves nothing"
        );
        assert!(Scope::Everything.admits_file("b.java"));
    }
}
