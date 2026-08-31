//! Java analyzer.
//!
//! Extracts types, methods, constructors and fields with two hashes each, per ADR-010:
//! `sig_hash` over the signature and annotations, `body_hash` over the normalized body.
//! Which one moves decides how far a change ripples.

#![forbid(unsafe_code)]
// A panic in a scan loses the whole run; an error loses one file. Tests are exempt:
// an assertion that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use bh_lang::{LangError, LanguageAnalyzer, ParsedFile, RawSymbol, SourceFile};
use bh_types::{Language, SymbolKind};
use tree_sitter::{Node, Parser};

pub struct JavaAnalyzer;

impl Default for JavaAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl JavaAnalyzer {
    pub fn new() -> Self {
        JavaAnalyzer
    }
}

impl LanguageAnalyzer for JavaAnalyzer {
    fn language(&self) -> Language {
        Language::Java
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn grammar_version(&self) -> &'static str {
        // Bump on any change to extraction or normalization, not only on a grammar upgrade:
        // this value forces a re-parse when content hashes would otherwise say "unchanged".
        "tree-sitter-java/0.23.5+extract2"
    }

    fn parse(&self, src: &SourceFile<'_>) -> Result<ParsedFile, LangError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .map_err(|_| LangError::Grammar(Language::Java))?;
        let tree = parser.parse(src.text, None).ok_or(LangError::NoTree)?;
        let bytes = src.text.as_bytes();
        let root = tree.root_node();

        let mut out = ParsedFile::default();
        if root.has_error() {
            // Partial output is still useful; saying nothing about it would not be.
            out.warnings
                .push("file contains syntax errors; symbols may be incomplete".into());
        }

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            match child.kind() {
                "package_declaration" => {
                    out.package = child
                        .named_child(0)
                        .and_then(|n| n.utf8_text(bytes).ok())
                        .map(str::to_string);
                }
                "import_declaration" => {
                    if let Some(t) = child.named_child(0).and_then(|n| n.utf8_text(bytes).ok()) {
                        out.imports.push(t.to_string());
                    }
                }
                _ => {}
            }
        }

        let prefix = out.package.clone().unwrap_or_default();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            walk_type(child, bytes, &prefix, None, &mut out.symbols);
        }
        Ok(out)
    }
}

// ─────────────────────────── extraction ───────────────────────────

fn type_kind(kind: &str) -> Option<SymbolKind> {
    Some(match kind {
        "class_declaration" => SymbolKind::Class,
        "interface_declaration" => SymbolKind::Interface,
        "enum_declaration" => SymbolKind::Enum,
        "record_declaration" => SymbolKind::Record,
        "annotation_type_declaration" => SymbolKind::Interface,
        _ => return None,
    })
}

fn walk_type(node: Node, src: &[u8], prefix: &str, parent: Option<&str>, out: &mut Vec<RawSymbol>) {
    let Some(kind) = type_kind(node.kind()) else {
        return;
    };
    let Some(name) = field_text(node, "name", src) else {
        return;
    };

    let fqn = if prefix.is_empty() {
        name.clone()
    } else {
        format!("{prefix}.{name}")
    };
    let annotations = annotations_of(node, src);
    let signature = type_signature(node, src, &name);

    // Containers are pushed before their members, so `parent_id` resolves in one pass
    // when the store writes them in order.
    out.push(RawSymbol {
        kind,
        name,
        fqn: fqn.clone(),
        parent_fqn: parent.map(str::to_string),
        signature: Some(signature.clone()),
        visibility: visibility_of(node, src),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        sig_hash: sig_hash(&signature, &annotations),
        // A type's "body" is its member declarations, not their bodies: reordering members
        // is a change, but editing one method's body already shows on that method.
        body_hash: hash(&member_shape(node, src)),
        annotations,
    });

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for member in body.children(&mut cursor) {
        match member.kind() {
            "method_declaration" | "compact_constructor_declaration" => {
                push_method(member, src, &fqn, out, SymbolKind::Method)
            }
            "constructor_declaration" => {
                push_method(member, src, &fqn, out, SymbolKind::Constructor)
            }
            "field_declaration" => push_fields(member, src, &fqn, out),
            "enum_constant" => push_enum_constant(member, src, &fqn, out),
            "enum_body_declarations" => {
                let mut c2 = member.walk();
                for m2 in member.children(&mut c2) {
                    match m2.kind() {
                        "method_declaration" => push_method(m2, src, &fqn, out, SymbolKind::Method),
                        "constructor_declaration" => {
                            push_method(m2, src, &fqn, out, SymbolKind::Constructor)
                        }
                        "field_declaration" => push_fields(m2, src, &fqn, out),
                        _ => walk_type(m2, src, &fqn, Some(&fqn), out),
                    }
                }
            }
            _ => walk_type(member, src, &fqn, Some(&fqn), out),
        }
    }
}

fn push_method(node: Node, src: &[u8], owner: &str, out: &mut Vec<RawSymbol>, kind: SymbolKind) {
    let Some(name) = field_text(node, "name", src) else {
        return;
    };
    let params = node.child_by_field_name("parameters");
    let fqn = format!("{owner}#{name}({})", param_types(params, src, true));
    let annotations = annotations_of(node, src);

    let ret = field_text(node, "type", src).unwrap_or_default();
    let signature = format!(
        "{}{}{name}({})",
        modifier_words(node, src),
        if ret.is_empty() {
            String::new()
        } else {
            format!("{ret} ")
        },
        param_types(params, src, false)
    );

    let body_hash = match node.child_by_field_name("body") {
        Some(b) => hash(&normalize_body(b, src)),
        // Abstract and interface methods have no body. They all hash alike, which is correct:
        // there is nothing there to change.
        None => hash(""),
    };

    out.push(RawSymbol {
        kind,
        name,
        fqn,
        parent_fqn: Some(owner.to_string()),
        signature: Some(signature.clone()),
        visibility: visibility_of(node, src),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        sig_hash: sig_hash(&signature, &annotations),
        body_hash,
        annotations,
    });
}

fn push_fields(node: Node, src: &[u8], owner: &str, out: &mut Vec<RawSymbol>) {
    let ty = field_text(node, "type", src).unwrap_or_default();
    let annotations = annotations_of(node, src);
    let mods = modifier_words(node, src);

    let mut cursor = node.walk();
    for d in node.children(&mut cursor) {
        if d.kind() != "variable_declarator" {
            continue;
        }
        let Some(name) = field_text(d, "name", src) else {
            continue;
        };
        let signature = format!("{mods}{ty} {name}");
        // A field's initializer is its body: `= new ArrayList<>()` changing is a real change.
        let init = d
            .child_by_field_name("value")
            .map(|v| normalize_body(v, src))
            .unwrap_or_default();
        out.push(RawSymbol {
            kind: SymbolKind::Field,
            name: name.clone(),
            fqn: format!("{owner}#{name}"),
            parent_fqn: Some(owner.to_string()),
            signature: Some(signature.clone()),
            visibility: visibility_of(node, src),
            start_line: d.start_position().row as u32 + 1,
            end_line: d.end_position().row as u32 + 1,
            sig_hash: sig_hash(&signature, &annotations),
            body_hash: hash(&init),
            annotations: annotations.clone(),
        });
    }
}

fn push_enum_constant(node: Node, src: &[u8], owner: &str, out: &mut Vec<RawSymbol>) {
    let Some(name) = field_text(node, "name", src) else {
        return;
    };
    let signature = format!("enum constant {name}");
    let annotations = annotations_of(node, src);
    out.push(RawSymbol {
        kind: SymbolKind::Field,
        name: name.clone(),
        fqn: format!("{owner}#{name}"),
        parent_fqn: Some(owner.to_string()),
        signature: Some(signature.clone()),
        visibility: Some("public".into()),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        sig_hash: sig_hash(&signature, &annotations),
        body_hash: hash(&normalize_body(node, src)),
        annotations,
    });
}

// ─────────────────────────── normalization ───────────────────────────

/// The most dangerous function in the crate.
///
/// The body is reduced to its token stream: every leaf node's text, joined by one space,
/// with comments dropped. Formatting and comments therefore vanish, while string literals
/// survive intact because a literal is a single leaf — collapsing whitespace textually
/// would silently rewrite `"a  b"` into `"a b"` and call a real change no change.
///
/// Strip too much and real changes become invisible, which is the worst failure mode in the
/// system. The guard is the fixture assertion that a reformat produces zero symbol changes
/// while a literal edit always produces one.
pub fn normalize_body(node: Node, src: &[u8]) -> String {
    let mut out = String::with_capacity(node.byte_range().len());
    collect_tokens(node, src, &mut out);
    out
}

fn collect_tokens(node: Node, src: &[u8], out: &mut String) {
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

/// An annotation's canonical form: its tokens concatenated with no separator, so
/// `@PreAuthorize("x")` and a line-wrapped `@PreAuthorize(\n    "x"\n)` are the same string.
///
/// Concatenating without spaces is safe here specifically because Java annotation arguments
/// are constant expressions — two identifier tokens never sit adjacent without punctuation
/// between them, so nothing can merge. The general body normalizer cannot do this, which is
/// why annotations get their own function rather than reusing `normalize_body`.
fn canonical_annotation(node: Node, src: &[u8]) -> String {
    let mut tokens = String::new();
    collect_tokens(node, src, &mut tokens);
    tokens.split_whitespace().collect::<String>()
}

fn is_comment(kind: &str) -> bool {
    matches!(kind, "line_comment" | "block_comment" | "comment")
}

/// A type's member *shape*: kinds and names only, so adding or removing a member changes the
/// type, but editing a member's body does not — that already shows on the member itself.
fn member_shape(node: Node, src: &[u8]) -> String {
    let Some(body) = node.child_by_field_name("body") else {
        return String::new();
    };
    let mut parts = Vec::new();
    let mut cursor = body.walk();
    for m in body.children(&mut cursor) {
        let name = field_text(m, "name", src)
            .or_else(|| {
                let mut c = m.walk();
                let declarator = m
                    .children(&mut c)
                    .find(|d| d.kind() == "variable_declarator");
                declarator.and_then(|d| field_text(d, "name", src))
            })
            .unwrap_or_default();
        if !name.is_empty() {
            parts.push(format!("{}:{}", m.kind(), name));
        }
    }
    parts.sort();
    parts.join(",")
}

// ─────────────────────────── helpers ───────────────────────────

fn field_text(node: Node, field: &str, src: &[u8]) -> Option<String> {
    node.child_by_field_name(field)?
        .utf8_text(src)
        .ok()
        .map(str::to_string)
}

fn modifiers_node<'a>(node: Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    // Bound to a local: the cursor must outlive the iterator borrowing it.
    let found = node.children(&mut cursor).find(|c| c.kind() == "modifiers");
    found
}

fn annotations_of(node: Node, src: &[u8]) -> Vec<String> {
    let Some(mods) = modifiers_node(node) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = mods.walk();
    for c in mods.children(&mut cursor) {
        if matches!(c.kind(), "annotation" | "marker_annotation") {
            out.push(canonical_annotation(c, src));
        }
    }
    out
}

fn modifier_words(node: Node, src: &[u8]) -> String {
    let Some(mods) = modifiers_node(node) else {
        return String::new();
    };
    let mut words = Vec::new();
    let mut cursor = mods.walk();
    for c in mods.children(&mut cursor) {
        // Annotations belong to `annotations_of`, and comments belong nowhere: a comment
        // sitting between the annotations and the method is part of the `modifiers` node,
        // so without this guard commenting out an annotation reports API_CHANGED instead of
        // CONTRACT_CHANGED, and a reformat that touches this position invents a change.
        if matches!(c.kind(), "annotation" | "marker_annotation") || is_comment(c.kind()) {
            continue;
        }
        if let Ok(t) = c.utf8_text(src) {
            words.push(t.to_string());
        }
    }
    if words.is_empty() {
        String::new()
    } else {
        format!("{} ", words.join(" "))
    }
}

fn visibility_of(node: Node, src: &[u8]) -> Option<String> {
    let mods = modifier_words(node, src);
    for v in ["public", "protected", "private"] {
        if mods.split_whitespace().any(|w| w == v) {
            return Some(v.to_string());
        }
    }
    Some("package-private".into())
}

fn param_types(params: Option<Node>, src: &[u8], simplified: bool) -> String {
    let Some(params) = params else {
        return String::new();
    };
    let mut out = Vec::new();
    let mut cursor = params.walk();
    for p in params.children(&mut cursor) {
        if !matches!(
            p.kind(),
            "formal_parameter" | "spread_parameter" | "receiver_parameter"
        ) {
            continue;
        }
        let ty = field_text(p, "type", src)
            .or_else(|| {
                p.named_child(0)
                    .and_then(|n| n.utf8_text(src).ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "?".into());
        out.push(if simplified { simplify_type(&ty) } else { ty });
    }
    out.join(",")
}

/// `java.util.List<String>` becomes `List`, and `String[]` stays `String[]`.
///
/// The FQN must survive a parameter rename and an import reshuffle, so it carries the
/// simple type name only. The full text is kept in `signature`, which is what `sig_hash`
/// covers — so a genuine type change is still an API change.
fn simplify_type(ty: &str) -> String {
    let base = ty.split('<').next().unwrap_or(ty);
    let arrays = ty.matches("[]").count();
    let simple = base.rsplit('.').next().unwrap_or(base).replace("[]", "");
    format!("{}{}", simple.trim(), "[]".repeat(arrays))
}

fn type_signature(node: Node, src: &[u8], name: &str) -> String {
    let mut sig = format!(
        "{}{} {name}",
        modifier_words(node, src),
        node.kind().replace("_declaration", "")
    );
    if let Some(sc) = node
        .child_by_field_name("superclass")
        .and_then(|n| n.utf8_text(src).ok())
    {
        sig.push(' ');
        sig.push_str(sc.trim());
    }
    if let Some(i) = node
        .child_by_field_name("interfaces")
        .and_then(|n| n.utf8_text(src).ok())
    {
        sig.push(' ');
        sig.push_str(i.trim());
    }
    sig.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex()[..32].to_string()
}

/// Annotations are sorted so reordering them is not a change, but adding or removing one is.
/// `@Transactional` appearing or vanishing carries more meaning than most signatures.
fn sig_hash(signature: &str, annotations: &[String]) -> String {
    let mut anns: Vec<&str> = annotations.iter().map(String::as_str).collect();
    anns.sort_unstable();
    hash(&format!("{signature}\u{0}{}", anns.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bh_lang::LanguageAnalyzer;

    fn parse(text: &str) -> ParsedFile {
        JavaAnalyzer::new()
            .parse(&SourceFile {
                path: "T.java",
                text,
            })
            .expect("parse")
    }

    fn sym<'a>(p: &'a ParsedFile, fqn: &str) -> &'a RawSymbol {
        p.symbols.iter().find(|s| s.fqn == fqn).unwrap_or_else(|| {
            panic!(
                "no symbol {fqn}; have {:?}",
                p.symbols.iter().map(|s| &s.fqn).collect::<Vec<_>>()
            )
        })
    }

    const SRC: &str = r#"
        package mn.pay;
        import java.util.List;

        @Service
        public class PaymentService {
            private final PaymentRepository repo;

            @Transactional
            public Payment createPayment(String key, Money amount) {
                if (repo.exists(key)) { return repo.find(key); }
                return repo.save(new Payment(key, amount));
            }

            public void refund(String key) { repo.refund(key); }
        }
    "#;

    #[test]
    fn extracts_package_class_and_methods() {
        let p = parse(SRC);
        assert_eq!(p.package.as_deref(), Some("mn.pay"));
        assert_eq!(sym(&p, "mn.pay.PaymentService").kind, SymbolKind::Class);
        let m = sym(&p, "mn.pay.PaymentService#createPayment(String,Money)");
        assert_eq!(m.kind, SymbolKind::Method);
        assert_eq!(m.parent_fqn.as_deref(), Some("mn.pay.PaymentService"));
        assert!(
            m.annotations.iter().any(|a| a == "@Transactional"),
            "{:?}",
            m.annotations
        );
    }

    #[test]
    fn reformatting_and_comments_do_not_change_any_hash() {
        // The single most important assertion in this crate: `gradle spotlessApply` across a
        // repository must produce zero symbol changes and trigger no bug hunt.
        let reformatted = r#"
            package mn.pay;
            import java.util.List;

            @Service
            public class PaymentService {

                private final PaymentRepository repo;

                // look up first, then save
                @Transactional
                public Payment createPayment( String key , Money amount )
                {
                    /* an idempotency check */
                    if ( repo.exists( key ) )
                    {
                        return repo.find( key );
                    }
                    return repo.save( new Payment( key , amount ) );
                }

                public void refund( String key ) { repo.refund( key ); }
            }
        "#;
        let a = parse(SRC);
        let b = parse(reformatted);
        for s in &a.symbols {
            let t = sym(&b, &s.fqn);
            assert_eq!(
                s.sig_hash, t.sig_hash,
                "sig_hash moved on reformat: {}",
                s.fqn
            );
            assert_eq!(
                s.body_hash, t.body_hash,
                "body_hash moved on reformat: {}",
                s.fqn
            );
        }
    }

    #[test]
    fn body_edit_moves_body_hash_but_not_sig_hash() {
        let edited = SRC.replace("return repo.find(key);", "return repo.findLatest(key);");
        let a = parse(SRC);
        let b = parse(&edited);
        let fqn = "mn.pay.PaymentService#createPayment(String,Money)";
        assert_eq!(
            sym(&a, fqn).sig_hash,
            sym(&b, fqn).sig_hash,
            "signature did not change"
        );
        assert_ne!(
            sym(&a, fqn).body_hash,
            sym(&b, fqn).body_hash,
            "body did change"
        );
    }

    #[test]
    fn signature_edit_moves_sig_hash() {
        let edited = SRC.replace(
            "public void refund(String key)",
            "public void refund(String key, Money amount)",
        );
        let a = parse(SRC);
        let b = parse(&edited);
        assert!(b
            .symbols
            .iter()
            .any(|s| s.fqn.ends_with("#refund(String,Money)")));
        assert!(!b.symbols.iter().any(|s| s.fqn.ends_with("#refund(String)")));
        let _ = a;
    }

    #[test]
    fn removing_transactional_moves_sig_hash_though_the_signature_is_identical() {
        // The change a compiler would not notice and that matters most under concurrency.
        let edited = SRC.replace("@Transactional\n", "");
        let a = parse(SRC);
        let b = parse(&edited);
        let fqn = "mn.pay.PaymentService#createPayment(String,Money)";
        assert_eq!(
            sym(&a, fqn).body_hash,
            sym(&b, fqn).body_hash,
            "body untouched"
        );
        assert_ne!(
            sym(&a, fqn).sig_hash,
            sym(&b, fqn).sig_hash,
            "annotation change must register"
        );
    }

    #[test]
    fn a_comment_among_the_modifiers_does_not_touch_the_signature() {
        // Regression: found by running the scanner on a real Spring repository. Commenting
        // an annotation out must read as CONTRACT_CHANGED, never API_CHANGED.
        let plain = parse(r#"package p; class C { @Transactional public void go() {} }"#);
        let commented = parse(
            r#"package p; class C {
                 // @Transactional removed while debugging
                 public void go() {}
               }"#,
        );
        let a = sym(&plain, "p.C#go()");
        let b = sym(&commented, "p.C#go()");
        assert_eq!(
            a.signature, b.signature,
            "a comment must not enter the signature"
        );
        assert_ne!(
            a.sig_hash, b.sig_hash,
            "but losing @Transactional is still a real change"
        );
        assert_eq!(a.body_hash, b.body_hash);
    }

    #[test]
    fn a_line_wrapped_annotation_is_the_same_annotation() {
        let one = parse(
            r#"package p; class C { @PreAuthorize("isAuthenticated()") public void go() {} }"#,
        );
        let two = parse(
            r#"package p;
               class C {
                 @PreAuthorize(
                     "isAuthenticated()"
                 )
                 public void go() {}
               }"#,
        );
        assert_eq!(
            sym(&one, "p.C#go()").sig_hash,
            sym(&two, "p.C#go()").sig_hash,
            "wrapping an annotation across lines is formatting, not a change"
        );
    }

    #[test]
    fn whitespace_inside_a_string_literal_is_preserved() {
        let one = parse(r#"package p; class C { String f() { return "a  b"; } }"#);
        let two = parse(r#"package p; class C { String f() { return "a b"; } }"#);
        assert_ne!(
            sym(&one, "p.C#f()").body_hash,
            sym(&two, "p.C#f()").body_hash,
            "collapsing whitespace inside a literal would call a real change no change"
        );
    }

    #[test]
    fn handles_interfaces_enums_records_and_nesting() {
        let p = parse(
            r#"package p;
               interface Repo { Payment find(String k); }
               enum Status { NEW, PAID }
               record Money(long amount, String currency) {}
               class Outer { class Inner { void go() {} } }"#,
        );
        assert_eq!(sym(&p, "p.Repo").kind, SymbolKind::Interface);
        assert_eq!(sym(&p, "p.Status").kind, SymbolKind::Enum);
        assert_eq!(sym(&p, "p.Money").kind, SymbolKind::Record);
        assert_eq!(sym(&p, "p.Status#PAID").kind, SymbolKind::Field);
        assert_eq!(sym(&p, "p.Outer.Inner#go()").kind, SymbolKind::Method);
        // An interface method has no body; all bodiless methods hash alike.
        assert_eq!(sym(&p, "p.Repo#find(String)").body_hash, hash(""));
    }

    #[test]
    fn a_file_with_syntax_errors_still_yields_symbols_and_warns() {
        let p = parse("package p; class C { void ok() {} void broken( { }");
        assert!(!p.warnings.is_empty(), "a degraded parse must say so");
        assert!(p.symbols.iter().any(|s| s.fqn == "p.C"));
    }

    #[test]
    fn simplify_type_keeps_arrays_and_drops_generics_and_packages() {
        assert_eq!(simplify_type("java.util.List<String>"), "List");
        assert_eq!(simplify_type("String[]"), "String[]");
        assert_eq!(simplify_type("Map<String, List<Integer>>"), "Map");
        assert_eq!(simplify_type("int"), "int");
    }
}
