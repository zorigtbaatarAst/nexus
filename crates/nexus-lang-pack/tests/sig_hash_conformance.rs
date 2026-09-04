//! The signature-hash contract, held against every analyzer this build ships.
//!
//! `RawSymbol::sig_hash` states a contract: the signature plus the annotations, sorted. It
//! was four independent implementations that disagreed with it and with each other — Java
//! sorted and Rust and Python did not, so swapping two attributes read as an API break, and
//! TypeScript omitted annotations entirely, so a decorator appearing or vanishing rippled
//! nowhere. One analyzer had a test for any of this.
//!
//! These run off `Registry::analyzers()`, so a new analyzer joins this suite by being
//! registered. Without a case below it fails `every_registered_analyzer_has_a_conformance_case`
//! rather than joining quietly.

use nexus_lang::{LanguageAnalyzer, SourceFile};
use nexus_lang_pack::default_registry;

/// One language's three shapes: two annotations, the same two swapped, and one of them gone.
struct Case {
    path: &'static str,
    /// The symbol whose annotations move.
    symbol: &'static str,
    base: &'static str,
    reordered: &'static str,
    removed: &'static str,
}

const CASES: &[Case] = &[
    Case {
        path: "src/main/java/p/C.java",
        symbol: "go",
        base: "package p;\nclass C {\n  @A @B\n  public void go(String s) {}\n}\n",
        reordered: "package p;\nclass C {\n  @B @A\n  public void go(String s) {}\n}\n",
        removed: "package p;\nclass C {\n  @A\n  public void go(String s) {}\n}\n",
    },
    Case {
        path: "src/c.ts",
        symbol: "go",
        base: "export class C {\n  @A() @B()\n  go(s: string) {}\n}\n",
        reordered: "export class C {\n  @B() @A()\n  go(s: string) {}\n}\n",
        removed: "export class C {\n  @A()\n  go(s: string) {}\n}\n",
    },
    Case {
        path: "src/c.js",
        symbol: "go",
        base: "export class C {\n  @A() @B()\n  go(s) {}\n}\n",
        reordered: "export class C {\n  @B() @A()\n  go(s) {}\n}\n",
        removed: "export class C {\n  @A()\n  go(s) {}\n}\n",
    },
    Case {
        path: "src/schema.graphqls",
        symbol: "go",
        base: "type Query {\n  go(id: ID!): String @a @b\n}\n",
        reordered: "type Query {\n  go(id: ID!): String @b @a\n}\n",
        removed: "type Query {\n  go(id: ID!): String @a\n}\n",
    },
    Case {
        path: "src/m.rs",
        symbol: "go",
        base: "#[a]\n#[b]\npub fn go(x: u32) -> u32 { x }\n",
        reordered: "#[b]\n#[a]\npub fn go(x: u32) -> u32 { x }\n",
        removed: "#[a]\npub fn go(x: u32) -> u32 { x }\n",
    },
    Case {
        path: "src/m.py",
        symbol: "go",
        base: "@a\n@b\ndef go(x):\n    return x\n",
        reordered: "@b\n@a\ndef go(x):\n    return x\n",
        removed: "@a\ndef go(x):\n    return x\n",
    },
];

/// Sources for the structural property alone, covering symbol shapes the cases above do not
/// produce. Every analyzer has a symbol or two built outside its main path — a Spring for
/// GraphQL route, a `gql` operation — and those were the hash sites most likely to be left
/// hashing something of their own.
const EXTRA_SHAPES: &[(&str, &str)] = &[
    (
        "src/main/java/p/R.java",
        "package p;\nclass R {\n  @QueryMapping\n  public String vehicles(String id) { return id; }\n}\n",
    ),
    (
        "src/q.ts",
        "export const Doc = gql`query Sales { vehicles { id } stats }`;\n",
    ),
    (
        "src/main/java/p/E.java",
        "package p;\npublic record Dto(String plate) {}\nenum Status { NEW, SOLD }\n",
    ),
];

/// Every sig_hash of every symbol, keyed by name, from one source.
fn hashes(path: &str, text: &str) -> Vec<(String, String)> {
    let registry = default_registry();
    let analyzer = registry
        .for_path(path)
        .unwrap_or_else(|| panic!("{path} is claimed by no analyzer"));
    let parsed = analyzer
        .parse(&SourceFile { path, text })
        .unwrap_or_else(|e| panic!("{path} did not parse: {e}"));
    assert!(
        !parsed.symbols.is_empty(),
        "{path} produced no symbols; the case proves nothing"
    );
    parsed
        .symbols
        .iter()
        .map(|s| (s.name.clone(), s.sig_hash.clone()))
        .collect()
}

/// The annotations an analyzer actually extracted for one symbol.
fn annotations_of(path: &str, text: &str, symbol: &str) -> Vec<String> {
    let registry = default_registry();
    let analyzer = registry
        .for_path(path)
        .unwrap_or_else(|| panic!("{path} is claimed by no analyzer"));
    analyzer
        .parse(&SourceFile { path, text })
        .unwrap_or_else(|e| panic!("{path} did not parse: {e}"))
        .symbols
        .iter()
        .find(|s| s.name == symbol)
        .map(|s| s.annotations.clone())
        .unwrap_or_else(|| panic!("{path} produced no symbol named {symbol}"))
}

fn hash_of(path: &str, text: &str, symbol: &str) -> String {
    hashes(path, text)
        .into_iter()
        .find(|(name, _)| name == symbol)
        .unwrap_or_else(|| panic!("{path} produced no symbol named {symbol}"))
        .1
}

#[test]
fn every_analyzer_hashes_through_the_shared_construction() {
    // The strongest form of "one implementation": whatever an analyzer put in `signature`
    // and `annotations`, the hash must be exactly what the shared function makes of them.
    // An analyzer that hashes anything else — a name, an fqn, a joined string of its own —
    // fails here regardless of what its own tests assert.
    let registry = default_registry();
    let from_cases = CASES
        .iter()
        .flat_map(|c| [c.base, c.reordered, c.removed].map(|text| (c.path, text)));
    for (path, text) in from_cases.chain(EXTRA_SHAPES.iter().copied()) {
        let analyzer = registry
            .for_path(path)
            .unwrap_or_else(|| panic!("{path} is claimed by no analyzer"));
        let parsed = analyzer
            .parse(&SourceFile { path, text })
            .unwrap_or_else(|e| panic!("{path} did not parse: {e}"));
        assert!(
            !parsed.symbols.is_empty(),
            "{path} produced no symbols; it proves nothing"
        );
        for symbol in &parsed.symbols {
            let expected = nexus_lang::sig_hash(
                symbol.signature.as_deref().unwrap_or_default(),
                &symbol.annotations,
            );
            assert_eq!(
                symbol.sig_hash, expected,
                "{path} hashes {} outside nexus_lang::sig_hash",
                symbol.fqn
            );
        }
    }
}

#[test]
fn reordering_annotations_is_not_a_change_in_any_language() {
    for case in CASES {
        // Without this the test passes vacuously the moment an analyzer stops extracting
        // annotations at all — which is the exact regression this suite exists to catch.
        assert!(
            annotations_of(case.path, case.base, case.symbol).len() >= 2,
            "{}: {} carries fewer than the two annotations the case swaps",
            case.path,
            case.symbol
        );
        assert_eq!(
            hashes(case.path, case.base),
            hashes(case.path, case.reordered),
            "{}: swapping two annotations moved a signature hash",
            case.path
        );
    }
}

#[test]
fn removing_an_annotation_is_a_change_in_any_language() {
    for case in CASES {
        assert_ne!(
            hash_of(case.path, case.base, case.symbol),
            hash_of(case.path, case.removed, case.symbol),
            "{}: dropping an annotation left the signature hash where it was",
            case.path
        );
    }
}

#[test]
fn every_registered_analyzer_has_a_conformance_case() {
    // The guard that makes the three tests above a contract rather than a sample. A new
    // analyzer that registers without a case fails here; one that registers with a case is
    // held to the same hashing as every other.
    //
    // Coverage is by dispatch, not by matching extension strings: `register` overwrites
    // `by_ext`, so an analyzer registering a claim on `.ts` displaces the TypeScript one
    // from every test above. Asking who `for_path` actually returns is the only question
    // whose answer matches what a scan would do.
    let registry = default_registry();
    for analyzer in registry.analyzers() {
        let covered = CASES
            .iter()
            .map(|c| c.path)
            .chain(EXTRA_SHAPES.iter().map(|(path, _)| *path))
            .any(|path| {
                registry
                    .for_path(path)
                    .is_some_and(|got| same(got, analyzer))
            });
        assert!(
            covered,
            "the {} analyzer ({:?}) is registered but no sig_hash conformance case reaches it",
            analyzer.language(),
            analyzer.extensions()
        );
    }
}

/// Whether two references name the same registered analyzer. Compared as thin pointers:
/// both come from the one registry, so the addresses are the identity.
fn same(a: &dyn LanguageAnalyzer, b: &dyn LanguageAnalyzer) -> bool {
    std::ptr::eq(
        a as *const dyn LanguageAnalyzer as *const (),
        b as *const dyn LanguageAnalyzer as *const (),
    )
}
