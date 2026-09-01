//! Java analyzer.
//!
//! Extracts types, methods, constructors and fields with two hashes each, per ADR-010:
//! `sig_hash` over the signature and annotations, `body_hash` over the normalized body.
//! Which one moves decides how far a change ripples.

#![forbid(unsafe_code)]
// A panic in a scan loses the whole run; an error loses one file. Tests are exempt:
// an assertion that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use nexus_lang::{LangError, LanguageAnalyzer, ParsedFile, RawEdge, RawSymbol, SourceFile};
use nexus_types::{EdgeType, Language, SymbolKind};
use std::collections::HashMap;
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
        "tree-sitter-java/0.23.5+extract7"
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

        let mut static_imports: Vec<(String, bool)> = Vec::new();
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
                    let Ok(text) = child.utf8_text(bytes) else {
                        continue;
                    };
                    let Some(path) = child.named_child(0).and_then(|n| n.utf8_text(bytes).ok())
                    else {
                        continue;
                    };
                    if text.contains("import static") {
                        // `import static org.mockito.Mockito.when;` makes `when(...)` a call
                        // on Mockito, not on the enclosing class. Attributing it to the
                        // enclosing class is how a test file invents hundreds of edges to
                        // methods that do not exist.
                        let wildcard = text.trim_end().ends_with("*;") || text.contains(".*");
                        static_imports.push((path.to_string(), wildcard));
                    } else {
                        out.imports.push(path.to_string());
                    }
                }
                _ => {}
            }
        }

        let prefix = out.package.clone().unwrap_or_default();
        // Imports let the heuristic tier turn `PaymentRepository` into a fully-qualified
        // name instead of a guess, which is most of the difference between a usable call
        // graph and a noisy one.
        let import_map: HashMap<String, String> = out
            .imports
            .iter()
            .filter_map(|i| {
                i.rsplit('.')
                    .next()
                    .map(|last| (last.to_string(), i.clone()))
            })
            .collect();

        // method name -> declaring type, for named static imports. A wildcard static
        // import cannot be attributed to a method name, so those only suppress the
        // enclosing-class guess rather than redirecting it.
        let mut static_map: HashMap<String, String> = HashMap::new();
        let mut static_wildcards: Vec<String> = Vec::new();
        for (path, wildcard) in &static_imports {
            let trimmed = path.trim_end_matches(".*");
            if *wildcard {
                static_wildcards.push(trimmed.to_string());
            } else if let Some((owner, member)) = trimmed.rsplit_once('.') {
                static_map.insert(member.to_string(), owner.to_string());
            }
        }
        let ctx = Ctx {
            imports: &import_map,
            package: &prefix,
            statics: &static_map,
            static_wildcards: &static_wildcards,
        };

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            walk_type(child, bytes, &prefix, None, &ctx, &mut out);
        }
        Ok(out)
    }
}

// ─────────────────────────── extraction ───────────────────────────

/// Everything file-level that name resolution needs. Passing one struct rather than four
/// arguments keeps the recursive walkers readable.
struct Ctx<'a> {
    imports: &'a HashMap<String, String>,
    package: &'a str,
    statics: &'a HashMap<String, String>,
    static_wildcards: &'a [String],
}

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

fn walk_type(
    node: Node,
    src: &[u8],
    prefix: &str,
    parent: Option<&str>,
    ctx: &Ctx<'_>,
    out: &mut ParsedFile,
) {
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
    // Captured before `annotations` is moved into the symbol below.
    let (lombok_getters, lombok_setters) = lombok_accessors(&annotations);
    let signature = type_signature(node, src, &name);

    // Containers are pushed before their members, so `parent_id` resolves in one pass
    // when the store writes them in order.
    out.symbols.push(RawSymbol {
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

    // extends / implements
    for (field, edge) in [
        ("superclass", EdgeType::Extends),
        ("interfaces", EdgeType::Implements),
    ] {
        let Some(n) = node.child_by_field_name(field) else {
            continue;
        };
        for name in type_names(n, src) {
            out.edges.push(RawEdge {
                src_fqn: fqn.clone(),
                dst_hint: qualify(&name, ctx),
                edge_type: edge,
                site_line: n.start_position().row as u32 + 1,
            });
        }
    }

    // A record's components are implicit accessor methods. Without them every
    // `dto.orderStatus()` in the codebase is an unresolvable call to a method that,
    // as far as the index is concerned, does not exist.
    if kind == SymbolKind::Record {
        if let Some(params) = node.child_by_field_name("parameters") {
            let mut c = params.walk();
            for param in params.children(&mut c) {
                if param.kind() != "formal_parameter" {
                    continue;
                }
                let (Some(ty), Some(pname)) = (
                    field_text(param, "type", src),
                    field_text(param, "name", src),
                ) else {
                    continue;
                };
                let sig = format!("public {ty} {pname}()");
                let line = param.start_position().row as u32 + 1;
                out.symbols.push(RawSymbol {
                    kind: SymbolKind::Method,
                    name: pname.clone(),
                    fqn: format!("{fqn}#{pname}()"),
                    parent_fqn: Some(fqn.clone()),
                    signature: Some(sig.clone()),
                    visibility: Some("public".into()),
                    start_line: line,
                    end_line: line,
                    sig_hash: sig_hash(&sig, &[]),
                    body_hash: hash(""),
                    annotations: Vec::new(),
                });
            }
        }
    }

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };

    // The type environment for this class: field name -> declared type. Resolving
    // `repo.save(x)` to `PaymentRepository#save` needs it, and a receiver whose type is
    // unknown yields no edge rather than a wrong one.
    let mut env: HashMap<String, String> = HashMap::new();
    collect_field_types(body, src, &mut env);

    // Emitted after the field environment exists, because an accessor's signature is the
    // field's declared type and nothing else knows it.
    if lombok_getters || lombok_setters {
        let mut fc = body.walk();
        for m in body.children(&mut fc) {
            if m.kind() != "field_declaration" {
                continue;
            }
            // A static field gets no instance accessor, and a constant certainly not.
            if modifier_words(m, src)
                .split_whitespace()
                .any(|w| w == "static")
            {
                continue;
            }
            let Some(ty) = field_text(m, "type", src) else {
                continue;
            };
            let simple = simplify_type(&ty);
            let mut dc = m.walk();
            for d in m.children(&mut dc) {
                if d.kind() != "variable_declarator" {
                    continue;
                }
                let Some(fname) = field_text(d, "name", src) else {
                    continue;
                };
                let line = d.start_position().row as u32 + 1;
                let mut emit = |name: String, sig: String| {
                    out.symbols.push(RawSymbol {
                        kind: SymbolKind::Method,
                        name: name.clone(),
                        fqn: format!("{fqn}#{name}()"),
                        parent_fqn: Some(fqn.clone()),
                        signature: Some(sig.clone()),
                        visibility: Some("public".into()),
                        start_line: line,
                        end_line: line,
                        sig_hash: sig_hash(&sig, &[]),
                        body_hash: hash(""),
                        annotations: Vec::new(),
                    });
                };
                if lombok_getters {
                    let n = accessor_name("get", &fname, &simple);
                    emit(n.clone(), format!("public {simple} {n}()"));
                }
                if lombok_setters {
                    let n = accessor_name("set", &fname, &simple);
                    emit(n.clone(), format!("public void {n}({simple})"));
                }
            }
        }
    }

    let mut cursor = body.walk();
    for member in body.children(&mut cursor) {
        match member.kind() {
            "method_declaration" | "compact_constructor_declaration" => {
                push_method(member, src, &fqn, ctx, &env, out, SymbolKind::Method)
            }
            "constructor_declaration" => {
                collect_injections(member, src, &fqn, ctx, out);
                push_method(member, src, &fqn, ctx, &env, out, SymbolKind::Constructor)
            }
            "field_declaration" => push_fields(member, src, &fqn, ctx, out),
            "enum_constant" => push_enum_constant(member, src, &fqn, out),
            "enum_body_declarations" => {
                let mut c2 = member.walk();
                for m2 in member.children(&mut c2) {
                    match m2.kind() {
                        "method_declaration" => {
                            push_method(m2, src, &fqn, ctx, &env, out, SymbolKind::Method)
                        }
                        "constructor_declaration" => {
                            push_method(m2, src, &fqn, ctx, &env, out, SymbolKind::Constructor)
                        }
                        "field_declaration" => push_fields(m2, src, &fqn, ctx, out),
                        _ => walk_type(m2, src, &fqn, Some(&fqn), ctx, out),
                    }
                }
            }
            _ => walk_type(member, src, &fqn, Some(&fqn), ctx, out),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_method(
    node: Node,
    src: &[u8],
    owner: &str,
    ctx: &Ctx<'_>,
    class_env: &HashMap<String, String>,
    out: &mut ParsedFile,
    kind: SymbolKind,
) {
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

    let line = node.start_position().row as u32 + 1;
    out.symbols.push(RawSymbol {
        kind,
        name: name.clone(),
        fqn: fqn.clone(),
        parent_fqn: Some(owner.to_string()),
        signature: Some(signature.clone()),
        visibility: visibility_of(node, src),
        start_line: line,
        end_line: node.end_position().row as u32 + 1,
        sig_hash: sig_hash(&signature, &annotations),
        body_hash,
        annotations: annotations.clone(),
    });

    // Spring for GraphQL: the handler becomes reachable from a schema field, which is the
    // join key the frontend also points at. See docs/investigation.md §3.
    if let Some(field) = graphql_field(&annotations, &name, params, src) {
        out.symbols.push(RawSymbol {
            kind: SymbolKind::Route,
            name: field.clone(),
            fqn: format!("graphql:{field}"),
            parent_fqn: Some(owner.to_string()),
            signature: Some(format!("graphql {field}")),
            visibility: Some("public".into()),
            start_line: line,
            end_line: line,
            sig_hash: hash(&format!("graphql:{field}")),
            body_hash: hash(""),
            annotations: annotations.clone(),
        });
        out.edges.push(RawEdge {
            src_fqn: format!("graphql:{field}"),
            dst_hint: fqn.clone(),
            edge_type: EdgeType::Routes,
            site_line: line,
        });
    }

    // Local type environment: class fields, then parameters, then declared locals.
    let mut env = class_env.clone();
    if let Some(p) = params {
        collect_param_types(p, src, &mut env);
    }
    if let Some(body) = node.child_by_field_name("body") {
        collect_local_types(body, src, &mut env);
        collect_calls(body, src, &fqn, &env, ctx, out);
    }
}

fn push_fields(node: Node, src: &[u8], owner: &str, ctx: &Ctx<'_>, out: &mut ParsedFile) {
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
        // A final field of a project type in a Spring bean is constructor injection by
        // Lombok's @RequiredArgsConstructor, which is how most of this codebase wires up.
        if mods.contains("final") || !annotations.is_empty() {
            if let Some(qualified) = project_type(&simplify_type(&ty), ctx) {
                out.edges.push(RawEdge {
                    src_fqn: owner.to_string(),
                    dst_hint: qualified,
                    edge_type: EdgeType::Injects,
                    site_line: d.start_position().row as u32 + 1,
                });
            }
        }
        out.symbols.push(RawSymbol {
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

fn push_enum_constant(node: Node, src: &[u8], owner: &str, out: &mut ParsedFile) {
    let Some(name) = field_text(node, "name", src) else {
        return;
    };
    let signature = format!("enum constant {name}");
    let annotations = annotations_of(node, src);
    out.symbols.push(RawSymbol {
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

// ─────────────────────────── edges ───────────────────────────

/// Type names inside a `superclass` or `interfaces` clause.
fn type_names(node: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        match c.kind() {
            "type_identifier" | "scoped_type_identifier" => {
                if let Ok(t) = c.utf8_text(src) {
                    out.push(simplify_type(t));
                }
            }
            "type_list" | "generic_type" => out.extend(type_names(c, src)),
            _ => {}
        }
    }
    out
}

/// Lombok's accessors are real methods at every call site and in no source file.
///
/// This is the record-component problem with a different generator. `@Data` on an entity
/// means `record.setStatus(x)` compiles, runs, and resolves to a method the index does not
/// contain — so the edge dangles and the symbol it should have reached looks unused.
/// Measured on a six-service Spring codebase: **8,633 of 10,367 unresolved in-project
/// edges — 83 % — were Lombok getters and setters.**
///
/// Only the annotations that generate accessors are honoured. `@Builder` is left alone
/// (0.4 % of the same population) because a builder is a nested type with its own methods,
/// and inventing a shape that Lombok may not have produced is worse than an unresolved
/// edge: a wrong symbol resolves *other* calls to the wrong place.
fn lombok_accessors(class_annotations: &[String]) -> (bool, bool) {
    let has = |name: &str| {
        class_annotations
            .iter()
            .any(|a| a == name || a.starts_with(&format!("{name}(")))
    };
    // @Value is @Getter plus immutability: getters, never setters.
    let getters = has("@Data") || has("@Getter") || has("@Value");
    let setters = has("@Data") || has("@Setter");
    (getters, setters)
}

/// Lombok's own rule, which is not "prepend get": a primitive `boolean` yields `isActive`,
/// while a boxed `Boolean` yields `getActive`. Emitting the wrong one leaves the call
/// unresolved and adds a symbol nothing calls.
fn accessor_name(prefix: &str, field: &str, ty: &str) -> String {
    let mut chars = field.chars();
    let capitalized = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => return field.to_string(),
    };
    if prefix == "get" && ty == "boolean" {
        format!("is{capitalized}")
    } else {
        format!("{prefix}{capitalized}")
    }
}

fn collect_field_types(body: Node, src: &[u8], env: &mut HashMap<String, String>) {
    let mut cursor = body.walk();
    for m in body.children(&mut cursor) {
        if m.kind() != "field_declaration" {
            continue;
        }
        let Some(ty) = field_text(m, "type", src) else {
            continue;
        };
        let mut c2 = m.walk();
        for d in m.children(&mut c2) {
            if d.kind() == "variable_declarator" {
                if let Some(n) = field_text(d, "name", src) {
                    env.insert(n, simplify_type(&ty));
                }
            }
        }
    }
}

fn collect_param_types(params: Node, src: &[u8], env: &mut HashMap<String, String>) {
    let mut cursor = params.walk();
    for p in params.children(&mut cursor) {
        if !matches!(p.kind(), "formal_parameter" | "spread_parameter") {
            continue;
        }
        if let (Some(ty), Some(name)) = (field_text(p, "type", src), field_text(p, "name", src)) {
            env.insert(name, simplify_type(&ty));
        }
    }
}

fn collect_local_types(node: Node, src: &[u8], env: &mut HashMap<String, String>) {
    if node.kind() == "local_variable_declaration" {
        if let Some(ty) = field_text(node, "type", src) {
            let simple = simplify_type(&ty);
            let mut c = node.walk();
            for d in node.children(&mut c) {
                if d.kind() == "variable_declarator" {
                    if let Some(n) = field_text(d, "name", src) {
                        // `var x = ...` tells us nothing without inference; recording "var"
                        // as a type would produce confident nonsense downstream.
                        if simple != "var" {
                            env.insert(n, simple.clone());
                        }
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        collect_local_types(c, src, env);
    }
}

/// Constructor injection: every project-typed parameter of a constructor is a dependency
/// of the class, which is how Spring wiring becomes visible to a call graph that has no
/// idea what a bean is.
fn collect_injections(ctor: Node, src: &[u8], owner: &str, ctx: &Ctx<'_>, out: &mut ParsedFile) {
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for p in params.children(&mut cursor) {
        if p.kind() != "formal_parameter" {
            continue;
        }
        let Some(ty) = field_text(p, "type", src) else {
            continue;
        };
        let Some(qualified) = project_type(&simplify_type(&ty), ctx) else {
            continue;
        };
        out.edges.push(RawEdge {
            src_fqn: owner.to_string(),
            dst_hint: qualified,
            edge_type: EdgeType::Injects,
            site_line: p.start_position().row as u32 + 1,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_calls(
    node: Node,
    src: &[u8],
    src_fqn: &str,
    env: &HashMap<String, String>,
    ctx: &Ctx<'_>,
    out: &mut ParsedFile,
) {
    match node.kind() {
        "method_invocation" => {
            if let Some(name) = field_text(node, "name", src) {
                let line = node.start_position().row as u32 + 1;
                let owner_type = match node.child_by_field_name("object") {
                    // An unqualified call is a call on the enclosing type, which is already
                    // fully qualified — pass it through rather than re-qualifying it.
                    None => {
                        if let Some(owner) = ctx.statics.get(&name) {
                            // A named static import: the call belongs to the declaring type.
                            out.edges.push(RawEdge {
                                src_fqn: src_fqn.to_string(),
                                dst_hint: format!("{owner}#{name}"),
                                edge_type: EdgeType::Calls,
                                site_line: line,
                            });
                        } else if ctx.static_wildcards.is_empty() {
                            // No wildcard static import in scope, so an unqualified call is
                            // a call on the enclosing type.
                            out.edges.push(RawEdge {
                                src_fqn: src_fqn.to_string(),
                                dst_hint: format!("{}#{name}", owner_of(src_fqn)),
                                edge_type: EdgeType::Calls,
                                site_line: line,
                            });
                        }
                        // With a wildcard static import in scope the name could belong to
                        // either the enclosing type or the imported one, and no single file
                        // can tell. Emitting nothing beats emitting a guess.
                        None
                    }
                    Some(obj) => receiver_type(obj, src, env),
                };
                // Resolution in nexus-core turns the hint into a symbol id, or leaves it
                // unresolved and says so. A receiver of unknown or platform type yields no
                // edge at all — not a wrong one.
                let owner_type = match owner_type {
                    Some(t) if t == SELF_RECEIVER => Some(owner_of(src_fqn).to_string()),
                    other => other,
                };
                if let Some(qualified) = owner_type.and_then(|t| project_type(&t, ctx)) {
                    out.edges.push(RawEdge {
                        src_fqn: src_fqn.to_string(),
                        dst_hint: format!("{qualified}#{name}"),
                        edge_type: EdgeType::Calls,
                        site_line: line,
                    });
                }
            }
        }
        "object_creation_expression" => {
            if let Some(ty) = field_text(node, "type", src) {
                if let Some(qualified) = project_type(&simplify_type(&ty), ctx) {
                    out.edges.push(RawEdge {
                        src_fqn: src_fqn.to_string(),
                        dst_hint: qualified,
                        edge_type: EdgeType::Calls,
                        site_line: node.start_position().row as u32 + 1,
                    });
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        collect_calls(c, src, src_fqn, env, ctx, out);
    }
}

/// The declared type of a call receiver, or `None` when it cannot be known from one file.
fn receiver_type(obj: Node, src: &[u8], env: &HashMap<String, String>) -> Option<String> {
    match obj.kind() {
        "identifier" => {
            let name = obj.utf8_text(src).ok()?;
            match env.get(name) {
                Some(ty) => Some(ty.clone()),
                // An unknown lowercase identifier is a local we failed to type; an
                // uppercase one is a static call on a type.
                None if name.starts_with(char::is_uppercase) => Some(name.to_string()),
                None => None,
            }
        }
        "field_access" => {
            let field = obj.child_by_field_name("field")?.utf8_text(src).ok()?;
            env.get(field).cloned()
        }
        // `this.foo()` is the enclosing type, and it is the exact shape of a Spring
        // self-invocation bug — the proxy is bypassed, so @Transactional on `foo` does
        // nothing. Returning None here made that class of bug invisible.
        "this" => Some(SELF_RECEIVER.to_string()),
        _ => None,
    }
}

/// Sentinel for a `this` receiver, replaced with the enclosing type at the call site.
pub(crate) const SELF_RECEIVER: &str = "\u{0}self";

fn owner_of(fqn: &str) -> &str {
    fqn.split('#').next().unwrap_or(fqn)
}

/// The fully-qualified name of a type this project is likely to define, or `None` for a
/// platform type.
///
/// The filter runs on the *qualified* name, not the simple one: `java.util.ArrayList`
/// simplifies to `ArrayList`, which no hand-written list of builtins will ever fully cover.
/// Resolving through the import table first and then excluding platform packages catches
/// both `String` and anything a library brings in.
///
/// Erring toward exclusion is deliberate: an edge to `java.lang.String` is noise in every
/// impact report, and a missing edge to a JDK type costs nothing.
fn project_type(simple: &str, ctx: &Ctx<'_>) -> Option<String> {
    const BUILTIN: &[&str] = &[
        "String",
        "Integer",
        "Long",
        "Double",
        "Float",
        "Boolean",
        "Byte",
        "Short",
        "Character",
        "Object",
        "List",
        "ArrayList",
        "LinkedList",
        "Map",
        "HashMap",
        "LinkedHashMap",
        "TreeMap",
        "Set",
        "HashSet",
        "LinkedHashSet",
        "TreeSet",
        "Collection",
        "Collections",
        "Arrays",
        "Optional",
        "Stream",
        "Objects",
        "Math",
        "System",
        "Thread",
        "BigDecimal",
        "BigInteger",
        "LocalDate",
        "LocalDateTime",
        "LocalTime",
        "Instant",
        "Duration",
        "Period",
        "ZonedDateTime",
        "UUID",
        "Class",
        "Exception",
        "RuntimeException",
        "Throwable",
        "Comparable",
        "Iterable",
        "Number",
        "CharSequence",
        "StringBuilder",
        "Pattern",
        "Matcher",
        "int",
        "long",
        "double",
        "float",
        "boolean",
        "byte",
        "short",
        "char",
        "void",
        "var",
    ];
    const PLATFORM: &[&str] = &["java.", "javax.", "jakarta.", "sun.", "com.sun.", "kotlin."];

    // An already-qualified project name (the enclosing type, for a `this` or unqualified
    // call) passes straight through: the checks below are about *simple* names.
    if simple.contains('.') && !PLATFORM.iter().any(|p| simple.starts_with(p)) {
        return Some(simple.to_string());
    }
    if simple.is_empty() || simple.ends_with("[]") || !simple.starts_with(char::is_uppercase) {
        return None;
    }
    // A bare single uppercase letter is a type parameter, not a type.
    if simple.len() <= 2 && simple.chars().all(char::is_uppercase) {
        return None;
    }
    if BUILTIN.contains(&simple) {
        return None;
    }
    let qualified = qualify(simple, ctx);
    if PLATFORM.iter().any(|p| qualified.starts_with(p)) {
        return None;
    }
    Some(qualified)
}

/// Turn a simple name into a fully-qualified one using the file's imports, falling back to
/// the file's own package. This is the whole of the heuristic tier's precision.
fn qualify(simple: &str, ctx: &Ctx<'_>) -> String {
    if simple.contains('.') {
        return simple.to_string();
    }
    if let Some(full) = ctx.imports.get(simple) {
        return full.clone();
    }
    if ctx.package.is_empty() {
        simple.to_string()
    } else {
        format!("{}.{simple}", ctx.package)
    }
}

/// The schema coordinate a Spring for GraphQL handler serves: `Query.vehicles`,
/// `Mutation.createDeal`, `Vehicle.owner`.
fn graphql_field(
    annotations: &[String],
    method_name: &str,
    params: Option<Node>,
    src: &[u8],
) -> Option<String> {
    for a in annotations {
        let (type_name, default_field) = if a.starts_with("@QueryMapping") {
            ("Query".to_string(), method_name.to_string())
        } else if a.starts_with("@MutationMapping") {
            ("Mutation".to_string(), method_name.to_string())
        } else if a.starts_with("@SubscriptionMapping") {
            ("Subscription".to_string(), method_name.to_string())
        } else if a.starts_with("@SchemaMapping") {
            // Spring defaults the type name to the first parameter's type.
            let inferred = params
                .and_then(|p| {
                    let mut c = p.walk();
                    let first = p.children(&mut c).find(|n| n.kind() == "formal_parameter");
                    first
                })
                .and_then(|f| field_text(f, "type", src))
                .map(|t| simplify_type(&t))
                .unwrap_or_default();
            (inferred, method_name.to_string())
        } else {
            continue;
        };

        let type_name = annotation_arg(a, "typeName").unwrap_or(type_name);
        let field = annotation_arg(a, "field")
            .or_else(|| annotation_arg(a, "name"))
            .or_else(|| annotation_arg(a, "value"))
            .unwrap_or(default_field);
        if type_name.is_empty() || field.is_empty() {
            return None;
        }
        return Some(format!("{type_name}.{field}"));
    }
    None
}

/// `@SchemaMapping(typeName="Vehicle",field="owner")` -> the value of one named argument.
fn annotation_arg(annotation: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let start = annotation.find(&needle)? + needle.len();
    let rest = &annotation[start..];
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
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
    use nexus_lang::LanguageAnalyzer;

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

    fn edges(p: &ParsedFile, t: EdgeType) -> Vec<(String, String)> {
        p.edges
            .iter()
            .filter(|e| e.edge_type == t)
            .map(|e| (e.src_fqn.clone(), e.dst_hint.clone()))
            .collect()
    }

    const WIRED: &str = r#"
        package mn.sales.vehicle.api;

        import mn.sales.vehicle.service.VehicleService;
        import mn.sales.vehicle.dto.VehicleDto;

        @Controller
        public class VehicleGraphQLController extends BaseController implements Auditable {
            private final VehicleService vehicleService;

            @QueryMapping
            @PreAuthorize("hasRole('sales')")
            public AntPage<VehicleDto> vehicles(@Argument AntPageable pagination) {
                return vehicleService.list(pagination);
            }

            @MutationMapping
            public VehicleDto createVehicle(@Argument VehicleInput input) {
                VehicleDto dto = vehicleService.create(input);
                return dto;
            }

            @SchemaMapping(typeName = "Vehicle", field = "owner")
            public OwnerDto owner(VehicleDto vehicle) { return vehicleService.owner(vehicle); }
        }
    "#;

    #[test]
    fn extracts_extends_implements_and_injection() {
        let p = parse(WIRED);
        let owner = "mn.sales.vehicle.api.VehicleGraphQLController";
        assert!(edges(&p, EdgeType::Extends)
            .contains(&(owner.into(), "mn.sales.vehicle.api.BaseController".into())));
        assert!(edges(&p, EdgeType::Implements)
            .contains(&(owner.into(), "mn.sales.vehicle.api.Auditable".into())));
        // The final field is Lombok constructor injection, and imports qualify the type.
        assert!(
            edges(&p, EdgeType::Injects).contains(&(
                owner.into(),
                "mn.sales.vehicle.service.VehicleService".into()
            )),
            "{:?}",
            edges(&p, EdgeType::Injects)
        );
    }

    #[test]
    fn resolves_a_call_through_the_field_type_and_the_import_table() {
        let p = parse(WIRED);
        let calls = edges(&p, EdgeType::Calls);
        assert!(
            calls
                .iter()
                .any(|(s, d)| s.ends_with("#vehicles(AntPageable)")
                    && d == "mn.sales.vehicle.service.VehicleService#list"),
            "{calls:?}"
        );
    }

    #[test]
    fn does_not_invent_edges_to_jdk_types() {
        let p = parse(
            r#"package p; class C { void go() { String s = "x"; s.trim(); new java.util.ArrayList<>(); } }"#,
        );
        let calls = edges(&p, EdgeType::Calls);
        assert!(
            !calls
                .iter()
                .any(|(_, d)| d.contains("String") || d.contains("ArrayList")),
            "an edge to a JDK type is noise in every impact report: {calls:?}"
        );
    }

    #[test]
    fn a_receiver_of_unknown_type_yields_no_edge_rather_than_a_wrong_one() {
        let p = parse(r#"package p; class C { void go(Object o) { unknownLocal.doThing(); } }"#);
        let calls = edges(&p, EdgeType::Calls);
        assert!(
            !calls.iter().any(|(_, d)| d.contains("doThing")),
            "guessing a receiver type produces a confidently wrong trace: {calls:?}"
        );
    }

    #[test]
    fn graphql_mappings_become_route_symbols_the_frontend_can_join_to() {
        let p = parse(WIRED);
        let routes: Vec<&str> = p
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Route)
            .map(|s| s.fqn.as_str())
            .collect();
        assert!(routes.contains(&"graphql:Query.vehicles"), "{routes:?}");
        assert!(
            routes.contains(&"graphql:Mutation.createVehicle"),
            "{routes:?}"
        );
        // typeName and field come from the annotation, not from the method name.
        assert!(routes.contains(&"graphql:Vehicle.owner"), "{routes:?}");

        // The route points at its handler, so reverse traversal from a service reaches the
        // schema field, and from there the frontend that selects it.
        assert!(edges(&p, EdgeType::Routes)
            .iter()
            .any(|(s, d)| s == "graphql:Query.vehicles" && d.ends_with("#vehicles(AntPageable)")));
    }

    #[test]
    fn simplify_type_keeps_arrays_and_drops_generics_and_packages() {
        assert_eq!(simplify_type("java.util.List<String>"), "List");
        assert_eq!(simplify_type("String[]"), "String[]");
        assert_eq!(simplify_type("Map<String, List<Integer>>"), "Map");
        assert_eq!(simplify_type("int"), "int");
    }
}

#[cfg(test)]
mod lombok_tests {
    use super::*;

    fn parse(src: &str) -> ParsedFile {
        JavaAnalyzer::new()
            .parse(&SourceFile {
                path: "T.java",
                text: src,
            })
            .expect("parse")
    }

    fn method_fqns(p: &ParsedFile) -> Vec<String> {
        p.symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Method)
            .map(|s| s.fqn.clone())
            .collect()
    }

    #[test]
    fn data_generates_both_halves_of_every_accessor() {
        // 83% of unresolved in-project edges on a real Spring codebase were these.
        let p = parse(
            r#"
package mn.a;
@Data
public class Rec {
    private String requestKey;
    private int status;
}
"#,
        );
        let m = method_fqns(&p);
        for want in [
            "mn.a.Rec#getRequestKey()",
            "mn.a.Rec#setRequestKey()",
            "mn.a.Rec#getStatus()",
            "mn.a.Rec#setStatus()",
        ] {
            assert!(m.contains(&want.to_string()), "missing {want} in {m:?}");
        }
    }

    #[test]
    fn value_is_getters_only_and_getter_is_not_a_setter() {
        // @Value is immutable. Emitting setters for it invents methods that do not exist,
        // and a wrong symbol resolves other calls to the wrong place.
        let p = parse("package mn.a;\n@Value\npublic class V { private String name; }\n");
        let m = method_fqns(&p);
        assert!(m.contains(&"mn.a.V#getName()".to_string()), "{m:?}");
        assert!(!m.iter().any(|f| f.contains("setName")), "{m:?}");

        let p = parse("package mn.a;\n@Getter\npublic class G { private String name; }\n");
        let m = method_fqns(&p);
        assert!(m.contains(&"mn.a.G#getName()".to_string()), "{m:?}");
        assert!(!m.iter().any(|f| f.contains("setName")), "{m:?}");
    }

    #[test]
    fn a_primitive_boolean_is_is_and_a_boxed_one_is_get() {
        // Lombok's own rule. The wrong one leaves the call unresolved *and* adds a symbol
        // nothing calls, which is worse than emitting neither.
        let p = parse(
            "package mn.a;\n@Data\npublic class B { private boolean active; private Boolean flagged; }\n",
        );
        let m = method_fqns(&p);
        assert!(m.contains(&"mn.a.B#isActive()".to_string()), "{m:?}");
        assert!(m.contains(&"mn.a.B#getFlagged()".to_string()), "{m:?}");
        assert!(!m.iter().any(|f| f.contains("getActive")), "{m:?}");
    }

    #[test]
    fn a_class_without_lombok_gains_nothing() {
        // The guard that keeps this from inventing methods across the whole codebase.
        let p = parse("package mn.a;\npublic class Plain { private String name; }\n");
        assert!(method_fqns(&p).is_empty(), "{:?}", method_fqns(&p));
    }

    #[test]
    fn a_static_field_gets_no_instance_accessor() {
        let p = parse(
            "package mn.a;\n@Data\npublic class C { private static final String KIND = \"x\"; private String id; }\n",
        );
        let m = method_fqns(&p);
        assert!(m.contains(&"mn.a.C#getId()".to_string()), "{m:?}");
        assert!(
            !m.iter().any(|f| f.contains("KIND") || f.contains("Kind")),
            "{m:?}"
        );
    }
}
