//! The analyzers this build ships with (roadmap 5.1).
//!
//! One crate whose whole job is knowing the list, so that `nexus-core` does not. Before this,
//! the core named `JavaAnalyzer`, `TypeScriptAnalyzer` and `GraphQlSchemaAnalyzer` directly,
//! which made every new language a core edit — the same inversion the capability split
//! already refused for rules.
//!
//! `LanguageAnalyzer` was always the extension point; the *choice* of analyzers was compiled
//! into the platform anyway. This is where the choice lives now, and `tests/boundaries.rs`
//! asserts the core cannot reach any concrete analyzer.

#![forbid(unsafe_code)]

use nexus_lang::Registry;

/// Every analyzer this build knows.
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register(Box::new(nexus_lang_java::JavaAnalyzer::new()))
        .register(Box::new(nexus_lang_ts::TypeScriptAnalyzer::new()))
        // JavaScript, by the same parser. Without it `nexus scan` on a 141-file Express
        // project indexed 0 symbols and 0 edges, and said nothing about why — an empty
        // index reads as "no dependencies found", which is worse than a shallow one.
        .register(Box::new(nexus_lang_ts::JavaScriptAnalyzer::new()))
        // The schema is indexed as the contract both sides are generated from, so "no
        // resolver serves this" means the field is absent from the schema — not merely that
        // no annotation shape this analyzer knows was found.
        .register(Box::new(nexus_lang_graphql::GraphQlSchemaAnalyzer::new()))
        // The language Nexus is written in. Until this was here, `nexus scan` on this
        // repository reported files and zero symbols: the tool could describe every project
        // except the one it is.
        .register(Box::new(nexus_lang_rust::RustAnalyzer::new()))
        .register(Box::new(nexus_lang_python::PythonAnalyzer::new()));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pack_is_not_empty_and_claims_the_languages_it_says_it_does() {
        let r = default_registry();
        assert!(!r.is_empty());
        for path in [
            "A.java",
            "a.ts",
            "schema.graphqls",
            "lib.rs",
            "app.py",
            "a.js",
            "a.jsx",
            "a.mjs",
            "a.cjs",
        ] {
            assert!(r.for_path(path).is_some(), "{path} is unclaimed");
        }
    }
}
