//! Rust analyzer (roadmap 5.2).
//!
//! Until this existed, `nexus scan` on this repository reported 113 files, zero symbols and
//! zero edges: the tool could describe every project except the one it is. That is the
//! dogfooding gap R6 names, and closing it is the phase's acceptance test.
//!
//! # The two hashes, and why the body one is the dangerous function
//!
//! `sig_hash` covers the signature and the attributes: when it moves, the contract moved and
//! every caller is affected. `body_hash` covers the normalized body alone: when only that
//! moves, behaviour changed but the API did not, and the change ripples along data and effect
//! edges rather than to every caller. Collapsing them makes impact analysis noise.
//!
//! [`normalize_body`] is pinned by tests in both directions, because it is the function that
//! decides how much of a repository a reformat appears to change. A whole-file reformat must
//! produce zero symbol changes; a one-line edit must produce exactly one.
//!
//! # Naming
//!
//! `module::path::Type#method` — `::` between modules and types, `#` before a member, which
//! is the shape the rest of the system already uses for Java (`mn.pay.Service#pay`). The
//! store's member lookup keys off that `#`, so a different convention here would silently
//! stop a class seed from reaching its methods.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use nexus_lang::{LangError, LanguageAnalyzer, ParsedFile, RawEdge, RawSymbol, SourceFile};
use nexus_types::{EdgeType, Language, SymbolKind};
use tree_sitter::{Node, Parser};

pub struct RustAnalyzer;

impl Default for RustAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl RustAnalyzer {
    pub fn new() -> Self {
        RustAnalyzer
    }
}

impl LanguageAnalyzer for RustAnalyzer {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn grammar_version(&self) -> &'static str {
        // Bumped with the grammar. `scans.tool_versions_json` carries it, and a change forces
        // a re-parse even where every content hash still matches — otherwise upgrading a
        // grammar silently keeps the symbols the old one produced, forever, with no error.
        "tree-sitter-rust 0.24"
    }

    fn parse(&self, src: &SourceFile<'_>) -> Result<ParsedFile, LangError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|_| LangError::Grammar(Language::Rust))?;
        let tree = parser.parse(src.text, None).ok_or(LangError::NoTree)?;
        let bytes = src.text.as_bytes();

        let mut out = ParsedFile::default();
        if tree.root_node().has_error() {
            // Partial output is still useful; saying nothing about it would not be.
            out.warnings
                .push("file contains syntax errors; symbols may be incomplete".into());
        }
        let module = module_path(src.path);
        out.package = Some(module.clone());

        walk(tree.root_node(), bytes, &module, None, &mut out);
        Ok(out)
    }
}

/// The module path a file declares, from its location.
///
/// `src/lib.rs` and `src/main.rs` are the crate root; `src/a/mod.rs` is `a`; `src/a/b.rs` is
/// `a::b`. Derived from the path rather than from `mod` declarations because an analyzer sees
/// one file and the declarations are in another — the same reason a Java analyzer reads the
/// package statement rather than the directory.
pub fn module_path(path: &str) -> String {
    let p = path.strip_prefix("./").unwrap_or(path);
    let p = p.rsplit_once("src/").map_or(p, |(_, tail)| tail);
    let p = p.strip_suffix(".rs").unwrap_or(p);
    let parts: Vec<&str> = p
        .split('/')
        .filter(|s| !s.is_empty() && *s != "mod" && *s != "lib" && *s != "main")
        .collect();
    parts.join("::")
}

fn child_text<'a>(node: Node<'_>, field: &str, bytes: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name(field)
        .and_then(|n| n.utf8_text(bytes).ok())
}

fn is_pub(node: Node<'_>, bytes: &[u8]) -> bool {
    let mut c = node.walk();
    let found = node.children(&mut c).any(|ch| {
        ch.kind() == "visibility_modifier"
            && ch.utf8_text(bytes).is_ok_and(|t| t.starts_with("pub"))
    });
    found
}

/// Attributes on an item: `#[derive(...)]`, `#[test]`, `#[tokio::main]`.
///
/// Carried into `sig_hash` for the same reason Java carries annotations: `#[test]` and
/// `#[serde(rename)]` change what a symbol *is* to the rest of the system, and a change to
/// one is a contract change even when the signature is untouched.
fn attributes(node: Node<'_>, bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut sibling = node.prev_named_sibling();
    while let Some(s) = sibling {
        if s.kind() != "attribute_item" {
            break;
        }
        if let Ok(t) = s.utf8_text(bytes) {
            out.push(t.trim().to_string());
        }
        sibling = s.prev_named_sibling();
    }
    out.reverse();
    out
}

fn hash(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    h.finalize().to_hex()[..32].to_string()
}

/// Tokens of a node, comments and whitespace dropped, single-spaced.
///
/// **The most consequential function in this crate.** It decides how much of a repository a
/// reformat appears to change: too literal and every formatting pass rewrites the index and
/// buries the real change; too loose and a real edit hashes the same as the code before it.
/// Dropping comments is deliberate — a doc comment is not behaviour — and is what makes
/// "document this function" a zero-impact change.
pub fn normalize_body(node: Node<'_>, src: &[u8]) -> String {
    let mut out = String::with_capacity(node.byte_range().len());
    collect_tokens(node, src, &mut out);
    out
}

fn is_comment(kind: &str) -> bool {
    matches!(kind, "line_comment" | "block_comment" | "doc_comment")
}

fn collect_tokens(node: Node<'_>, src: &[u8], out: &mut String) {
    if is_comment(node.kind()) {
        return;
    }
    if node.child_count() == 0 {
        if let Ok(text) = node.utf8_text(src) {
            if text.trim().is_empty() {
                return;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(text);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tokens(child, src, out);
    }
}

/// The signature of an item: every token before the body.
///
/// Tokenized rather than sliced, for the same reason the body is. `fn f(a:i32)->i32` and
/// `fn f(a: i32) -> i32` are the same contract, and a signature hash that disagreed would
/// report an API break on every formatting pass — which ripples to every caller and is the
/// loudest possible way to be wrong.
fn signature(node: Node<'_>, bytes: &[u8]) -> String {
    let body = node.child_by_field_name("body").map(|b| b.id());
    let mut out = String::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if Some(child.id()) == body {
            break;
        }
        collect_tokens(child, bytes, &mut out);
    }
    out
}

fn line(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn push_symbol(
    out: &mut ParsedFile,
    kind: SymbolKind,
    name: &str,
    fqn: String,
    parent: Option<&str>,
    node: Node<'_>,
    bytes: &[u8],
) {
    let attrs = attributes(node, bytes);
    let sig = signature(node, bytes);
    let body = node
        .child_by_field_name("body")
        .map(|b| normalize_body(b, bytes))
        .unwrap_or_default();
    out.symbols.push(RawSymbol {
        kind,
        name: name.to_string(),
        fqn,
        parent_fqn: parent.map(str::to_string),
        signature: Some(sig.clone()),
        visibility: Some(
            if is_pub(node, bytes) {
                "public"
            } else {
                "private"
            }
            .into(),
        ),
        start_line: line(node),
        end_line: node.end_position().row as u32 + 1,
        sig_hash: hash(&[&sig, &attrs.join(",")]),
        body_hash: hash(&[&body]),
        annotations: attrs,
    });
}

/// Walk one scope, emitting symbols and the edges they contain.
fn walk(node: Node<'_>, bytes: &[u8], scope: &str, parent: Option<&str>, out: &mut ParsedFile) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "use_declaration" => {
                if let Ok(t) = child.utf8_text(bytes) {
                    out.imports.push(
                        t.trim_start_matches("pub ")
                            .trim_start_matches("use ")
                            .trim_end_matches(';')
                            .trim()
                            .to_string(),
                    );
                }
            }
            "mod_item" => {
                let Some(name) = child_text(child, "name", bytes) else {
                    continue;
                };
                let fqn = join(scope, name);
                push_symbol(
                    out,
                    SymbolKind::Module,
                    name,
                    fqn.clone(),
                    parent,
                    child,
                    bytes,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, bytes, &fqn, Some(&fqn), out);
                }
            }
            "struct_item" | "union_item" => emit_type(child, bytes, scope, SymbolKind::Class, out),
            "enum_item" => emit_type(child, bytes, scope, SymbolKind::Enum, out),
            "trait_item" => {
                let Some(name) = child_text(child, "name", bytes) else {
                    continue;
                };
                let fqn = join(scope, name);
                push_symbol(
                    out,
                    SymbolKind::Trait,
                    name,
                    fqn.clone(),
                    None,
                    child,
                    bytes,
                );
                if let Some(body) = child.child_by_field_name("body") {
                    members(body, bytes, &fqn, out);
                }
            }
            "impl_item" => emit_impl(child, bytes, scope, out),
            "function_item" => {
                let Some(name) = child_text(child, "name", bytes) else {
                    continue;
                };
                // A free function belongs to a module, not to a type, so it is named with
                // `::` like the module it sits in. `#` is reserved for a member of a type,
                // which is what every other analyzer here means by it.
                let fqn = join(scope, name);
                push_symbol(
                    out,
                    SymbolKind::Function,
                    name,
                    fqn.clone(),
                    parent,
                    child,
                    bytes,
                );
                calls(child, bytes, &fqn, out);
            }
            "const_item" | "static_item" => {
                if let Some(name) = child_text(child, "name", bytes) {
                    let fqn = join(scope, name);
                    push_symbol(out, SymbolKind::Field, name, fqn, parent, child, bytes);
                }
            }
            _ => {}
        }
    }
}

fn join(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}::{name}")
    }
}

fn emit_type(node: Node<'_>, bytes: &[u8], scope: &str, kind: SymbolKind, out: &mut ParsedFile) {
    let Some(name) = child_text(node, "name", bytes) else {
        return;
    };
    let fqn = join(scope, name);
    push_symbol(out, kind, name, fqn, None, node, bytes);
}

/// `impl Type` and `impl Trait for Type`.
///
/// The impl block itself is not a symbol — nothing calls an impl. Its methods belong to the
/// type, which is what a caller names, and `impl Trait for Type` is an `implements` edge:
/// the single most useful structural fact Rust has, because it is how the language expresses
/// what an interface expresses elsewhere.
fn emit_impl(node: Node<'_>, bytes: &[u8], scope: &str, out: &mut ParsedFile) {
    let Some(type_name) = child_text(node, "type", bytes) else {
        return;
    };
    let owner = join(scope, base_name(type_name));
    if let Some(trait_name) = child_text(node, "trait", bytes) {
        out.edges.push(RawEdge {
            src_fqn: owner.clone(),
            dst_hint: base_name(trait_name).to_string(),
            edge_type: EdgeType::Implements,
            site_line: line(node),
        });
    }
    if let Some(body) = node.child_by_field_name("body") {
        members(body, bytes, &owner, out);
    }
}

/// Strip generics and references so `Vec<Foo>` and `&Foo` both name `Foo`.
fn base_name(t: &str) -> &str {
    let t = t.trim_start_matches(['&', '*']).trim();
    let t = t.split('<').next().unwrap_or(t).trim();
    t.rsplit("::").next().unwrap_or(t)
}

/// Methods and associated items of a trait or an impl block.
fn members(body: Node<'_>, bytes: &[u8], owner: &str, out: &mut ParsedFile) {
    let mut cursor = body.walk();
    for item in body.children(&mut cursor) {
        match item.kind() {
            "function_item" | "function_signature_item" => {
                let Some(name) = child_text(item, "name", bytes) else {
                    continue;
                };
                let fqn = format!("{owner}#{name}");
                push_symbol(
                    out,
                    SymbolKind::Method,
                    name,
                    fqn.clone(),
                    Some(owner),
                    item,
                    bytes,
                );
                calls(item, bytes, &fqn, out);
            }
            "const_item" => {
                if let Some(name) = child_text(item, "name", bytes) {
                    push_symbol(
                        out,
                        SymbolKind::Field,
                        name,
                        format!("{owner}#{name}"),
                        Some(owner),
                        item,
                        bytes,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Call edges out of one function body.
///
/// The hint is the best shape available from one file: a method call gives the member name,
/// a path call gives the last two segments. Turning a hint into a symbol id is resolution's
/// job in `nexus-core`, once every symbol in the project is known — an analyzer only ever
/// sees one file and cannot do it.
fn calls(node: Node<'_>, bytes: &[u8], src_fqn: &str, out: &mut ParsedFile) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression" {
            if let Some(func) = n.child_by_field_name("function") {
                if let Some(hint) = call_hint(func, bytes) {
                    out.edges.push(RawEdge {
                        src_fqn: src_fqn.to_string(),
                        dst_hint: hint,
                        edge_type: EdgeType::Calls,
                        site_line: line(n),
                    });
                }
            }
        }
        let mut c = n.walk();
        for child in n.children(&mut c) {
            stack.push(child);
        }
    }
}

/// Names that are the standard library or the language, not this project.
///
/// A bare `#method` hint is only useful when the name is distinctive. `#clone` matches every
/// `clone` in the index, so emitting it produces a *wrong* edge rather than a missing one —
/// and 459 of them, on this repository, buried the real edges and made the resolution rate
/// meaningless in the other direction. ADR-017's argument, applied one level down: an edge
/// that is correctly outside the index should not be counted as a failure to resolve.
///
/// Deliberately short and boring. It holds prelude methods and enum constructors, not a
/// general "looks like std" heuristic — a longer list starts hiding real project methods,
/// and that failure is silent.
const PRELUDE: &[&str] = &[
    // Constructors, which are not calls at all.
    "Ok",
    "Err",
    "Some",
    "None",
    // Prelude and near-universal inherent methods.
    "clone",
    "into",
    "from",
    "to_string",
    "to_owned",
    "as_str",
    "as_ref",
    "as_mut",
    "iter",
    "into_iter",
    "iter_mut",
    "collect",
    "map",
    "and_then",
    "unwrap_or",
    "unwrap_or_else",
    "unwrap_or_default",
    "unwrap",
    "expect",
    "len",
    "is_empty",
    "push",
    "push_str",
    "insert",
    "get",
    "contains",
    "contains_key",
    "to_vec",
    "join",
    "split",
    "trim",
    "format",
    "vec",
    "starts_with",
    "ends_with",
    "parse",
    "ok",
    "err",
    "is_some",
    "is_none",
    "cloned",
    "copied",
    "filter",
    "filter_map",
    "next",
    "count",
    "sort",
    "sort_by",
    "extend",
    "entry",
    "or_insert",
    "or_default",
    "min",
    "max",
    "abs",
    "saturating_sub",
    "checked_add",
    "chars",
    "lines",
    "as_deref",
    "take",
    "replace",
    "find",
    "any",
    "all",
    "flatten",
    "unwrap_err",
    "borrow",
];

fn is_prelude(name: &str) -> bool {
    PRELUDE.contains(&name)
}

fn call_hint(func: Node<'_>, bytes: &[u8]) -> Option<String> {
    match func.kind() {
        // `x.method(...)` — the receiver's type is not known from one file, so the member
        // name is the honest hint and resolution decides what it means.
        "field_expression" => child_text(func, "field", bytes)
            .filter(|f| !is_prelude(f))
            .map(|f| format!("#{f}")),
        "scoped_identifier" | "identifier" => {
            let text = func.utf8_text(bytes).ok()?.trim();
            let last = text.rsplit("::").next().unwrap_or(text);
            if text.is_empty()
                || is_prelude(last)
                || text.starts_with("std::")
                || text.starts_with("core::")
                || text.starts_with("alloc::")
            {
                return None;
            }
            Some(text.to_string())
        }
        "generic_function" => func
            .child_by_field_name("function")
            .and_then(|f| call_hint(f, bytes)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ParsedFile {
        RustAnalyzer::new()
            .parse(&SourceFile {
                path: "src/pay/service.rs",
                text,
            })
            .expect("parse")
    }

    fn find<'a>(p: &'a ParsedFile, fqn: &str) -> &'a RawSymbol {
        p.symbols.iter().find(|s| s.fqn == fqn).unwrap_or_else(|| {
            panic!(
                "no {fqn} in {:?}",
                p.symbols.iter().map(|s| &s.fqn).collect::<Vec<_>>()
            )
        })
    }

    const SOURCE: &str = r#"
use std::collections::BTreeMap;

pub struct PaymentService {
    pub repo: Repo,
}

pub trait Settles {
    fn settle(&self, key: &str) -> bool;
}

impl Settles for PaymentService {
    fn settle(&self, key: &str) -> bool {
        self.repo.save(key)
    }
}

impl PaymentService {
    /// Pay, once.
    pub fn pay(&self, key: &str) -> bool {
        self.settle(key)
    }
}

pub fn helper(n: i32) -> i32 {
    n + 1
}
"#;

    #[test]
    fn the_shapes_a_rust_file_declares_all_become_symbols() {
        let p = parse(SOURCE);
        for fqn in [
            "pay::service::PaymentService",
            "pay::service::Settles",
            "pay::service::Settles#settle",
            "pay::service::PaymentService#settle",
            "pay::service::PaymentService#pay",
        ] {
            find(&p, fqn);
        }
        assert_eq!(
            find(&p, "pay::service::PaymentService").kind,
            SymbolKind::Class
        );
        assert_eq!(find(&p, "pay::service::Settles").kind, SymbolKind::Trait);
        assert_eq!(
            find(&p, "pay::service::PaymentService#pay").kind,
            SymbolKind::Method
        );
    }

    #[test]
    fn a_member_is_named_with_a_hash_like_every_other_language() {
        // The store's member lookup keys off `#`. A different convention here would silently
        // stop a type seed from reaching its methods, which is a bug with no error message.
        let p = parse(SOURCE);
        assert!(p
            .symbols
            .iter()
            .any(|s| s.fqn == "pay::service::PaymentService#pay"));
    }

    #[test]
    fn impl_trait_for_type_is_an_implements_edge() {
        // How Rust says what an interface says elsewhere, and the most useful structural
        // fact the language offers.
        let p = parse(SOURCE);
        assert!(
            p.edges.iter().any(|e| e.edge_type == EdgeType::Implements
                && e.src_fqn == "pay::service::PaymentService"
                && e.dst_hint == "Settles"),
            "{:?}",
            p.edges
        );
    }

    #[test]
    fn a_method_call_becomes_an_edge_with_an_honest_hint() {
        let p = parse(SOURCE);
        let call = p
            .edges
            .iter()
            .find(|e| e.src_fqn == "pay::service::PaymentService#pay")
            .expect("pay calls something");
        assert_eq!(call.edge_type, EdgeType::Calls);
        // The receiver's type is not knowable from one file, so the member name is the hint
        // and resolution decides what it means.
        assert_eq!(call.dst_hint, "#settle");
    }

    #[test]
    fn visibility_is_recorded() {
        let p = parse(SOURCE);
        assert_eq!(
            find(&p, "pay::service::helper").visibility.as_deref(),
            Some("public")
        );
    }

    #[test]
    fn a_reformat_changes_no_hash() {
        // The invariant that decides how much of a repository a formatting pass appears to
        // change. If this fails, every `cargo fmt` rewrites the index and buries the real
        // change in it.
        let dense = "pub fn f(a:i32)->i32{let b=a+1;b*2}";
        let spaced = "pub fn f(a: i32) -> i32 {\n    let b = a + 1;\n\n    b * 2\n}\n";
        let a = parse(dense);
        let b = parse(spaced);
        assert_eq!(a.symbols[0].body_hash, b.symbols[0].body_hash);
        assert_eq!(a.symbols[0].sig_hash, b.symbols[0].sig_hash);
    }

    #[test]
    fn a_comment_is_not_behaviour() {
        let bare = "pub fn f() -> i32 { 1 }";
        let documented = "/// Returns one.\npub fn f() -> i32 {\n    // obviously\n    1\n}";
        assert_eq!(
            parse(bare).symbols[0].body_hash,
            parse(documented).symbols[0].body_hash
        );
    }

    #[test]
    fn a_one_line_change_moves_the_body_hash_and_not_the_signature() {
        // The two hashes exist to tell an API break from a behaviour change. Collapsing them
        // makes every body edit ripple to every caller, which is impact noise.
        let before = parse("pub fn f(a: i32) -> i32 { a + 1 }");
        let after = parse("pub fn f(a: i32) -> i32 { a + 2 }");
        assert_ne!(before.symbols[0].body_hash, after.symbols[0].body_hash);
        assert_eq!(before.symbols[0].sig_hash, after.symbols[0].sig_hash);
    }

    #[test]
    fn a_signature_change_moves_the_signature_hash() {
        let before = parse("pub fn f(a: i32) -> i32 { a }");
        let after = parse("pub fn f(a: i64) -> i32 { a }");
        assert_ne!(before.symbols[0].sig_hash, after.symbols[0].sig_hash);
    }

    #[test]
    fn an_attribute_is_part_of_the_contract() {
        // `#[test]` changes what a symbol is to the rest of the system, exactly as an
        // annotation does in Java.
        let plain = parse("fn f() {}");
        let tested = parse("#[test]\nfn f() {}");
        assert_ne!(plain.symbols[0].sig_hash, tested.symbols[0].sig_hash);
        assert_eq!(tested.symbols[0].annotations, ["#[test]"]);
    }

    #[test]
    fn a_module_path_comes_from_the_file_location() {
        assert_eq!(module_path("src/lib.rs"), "");
        assert_eq!(module_path("src/engine/mod.rs"), "engine");
        assert_eq!(module_path("src/context/intent.rs"), "context::intent");
        assert_eq!(module_path("crates/nexus-core/src/memory.rs"), "memory");
    }

    #[test]
    fn a_nested_module_nests_its_symbols() {
        let p = parse("pub mod inner { pub fn deep() {} }");
        find(&p, "pay::service::inner");
        find(&p, "pay::service::inner::deep");
    }

    #[test]
    fn a_file_that_does_not_parse_still_contributes_what_it_has() {
        let p = RustAnalyzer::new()
            .parse(&SourceFile {
                path: "src/a.rs",
                text: "pub fn ok() {} pub fn broken( {",
            })
            .expect("parse");
        assert!(
            !p.warnings.is_empty(),
            "silence would make the index quietly wrong"
        );
        assert!(p.symbols.iter().any(|s| s.name == "ok"));
    }

    #[test]
    fn use_declarations_are_recorded() {
        let p = parse(SOURCE);
        assert!(
            p.imports.iter().any(|i| i.contains("BTreeMap")),
            "{:?}",
            p.imports
        );
    }
}

#[cfg(test)]
mod prelude_tests {
    use super::*;

    fn edges(text: &str) -> Vec<String> {
        RustAnalyzer::new()
            .parse(&SourceFile {
                path: "src/a.rs",
                text,
            })
            .expect("parse")
            .edges
            .into_iter()
            .map(|e| e.dst_hint)
            .collect()
    }

    #[test]
    fn a_standard_library_call_is_not_an_edge_into_this_project() {
        // `#clone` matches every clone in the index, so emitting it is a wrong edge rather
        // than a missing one — and on this repository it produced 459 of them.
        let hints = edges(
            "fn f(v: Vec<String>) { let w = v.clone(); w.iter().count(); Some(1).unwrap(); }",
        );
        assert!(hints.is_empty(), "{hints:?}");
    }

    #[test]
    fn a_project_call_still_produces_an_edge() {
        let hints = edges("fn f(s: &Service) { s.settle_payment(); crate::pay::run(); }");
        assert!(hints.contains(&"#settle_payment".to_string()), "{hints:?}");
        assert!(hints.contains(&"crate::pay::run".to_string()), "{hints:?}");
    }

    #[test]
    fn the_list_is_short_enough_to_read() {
        // A longer list starts hiding real project methods, and that failure is silent. If
        // this ever needs raising, the reason belongs in the commit message.
        assert!(PRELUDE.len() < 100, "{} entries", PRELUDE.len());
    }
}
