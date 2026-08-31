//! TypeScript and TSX analyzer.
//!
//! Beyond symbols, this crate carries the frontend half of the cross-stack seam: `gql`
//! documents become operation symbols, and a component's use of a generated
//! `<Name>Document` becomes an edge to the operation it names. Resolution then joins
//! those operations to the backend resolvers that serve their root fields.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod graphql;

use bh_lang::{LangError, LanguageAnalyzer, ParsedFile, RawEdge, RawSymbol, SourceFile};
use bh_types::{EdgeType, Language, SymbolKind};
use tree_sitter::{Node, Parser};

pub struct TypeScriptAnalyzer;

impl Default for TypeScriptAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeScriptAnalyzer {
    pub fn new() -> Self {
        TypeScriptAnalyzer
    }
}

impl LanguageAnalyzer for TypeScriptAnalyzer {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx", "mts", "cts"]
    }

    fn grammar_version(&self) -> &'static str {
        "tree-sitter-typescript/0.23+extract1"
    }

    fn parse(&self, src: &SourceFile<'_>) -> Result<ParsedFile, LangError> {
        let mut parser = Parser::new();
        // TSX and TS need different grammars: `<T>` is a type assertion in one and an
        // element in the other, and using the wrong one silently mangles every generic.
        let language = if src.path.ends_with(".tsx") {
            tree_sitter_typescript::LANGUAGE_TSX
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT
        };
        parser
            .set_language(&language.into())
            .map_err(|_| LangError::Grammar(Language::TypeScript))?;
        let tree = parser.parse(src.text, None).ok_or(LangError::NoTree)?;
        let bytes = src.text.as_bytes();
        let root = tree.root_node();

        let mut out = ParsedFile::default();
        if root.has_error() {
            out.warnings
                .push("file contains syntax errors; symbols may be incomplete".into());
        }

        // TypeScript has no packages, so the module path is the namespace.
        let module = module_id(src.path);
        out.package = Some(module.clone());

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            walk_top(child, bytes, &module, &mut out);
        }
        Ok(out)
    }
}

/// `src/lib/graphql/salary.ts` -> `src/lib/graphql/salary`.
fn module_id(path: &str) -> String {
    for ext in [".tsx", ".ts", ".mts", ".cts"] {
        if let Some(stripped) = path.strip_suffix(ext) {
            return stripped.to_string();
        }
    }
    path.to_string()
}

// ─────────────────────────── symbols ───────────────────────────

fn walk_top(node: Node, src: &[u8], module: &str, out: &mut ParsedFile) {
    match node.kind() {
        "import_statement" => {
            if let Some(s) = node
                .child_by_field_name("source")
                .and_then(|n| text(n, src))
            {
                out.imports.push(s.trim_matches(['"', '\'']).to_string());
            }
        }
        "export_statement" => {
            let mut cursor = node.walk();
            for c in node.children(&mut cursor) {
                walk_top(c, src, module, out);
            }
        }
        "function_declaration" | "generator_function_declaration" => {
            push_function(node, src, module, None, out)
        }
        "class_declaration" | "abstract_class_declaration" => push_class(node, src, module, out),
        "interface_declaration" => push_named(node, src, module, SymbolKind::Interface, out),
        "type_alias_declaration" => push_named(node, src, module, SymbolKind::Config, out),
        "enum_declaration" => push_named(node, src, module, SymbolKind::Enum, out),
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            for d in node.children(&mut cursor) {
                if d.kind() == "variable_declarator" {
                    push_declarator(d, src, module, out);
                }
            }
        }
        _ => {}
    }
}

fn push_named(node: Node, src: &[u8], module: &str, kind: SymbolKind, out: &mut ParsedFile) {
    let Some(name) = field_text(node, "name", src) else {
        return;
    };
    let sig = format!("{} {name}", node.kind().replace("_declaration", ""));
    out.symbols.push(symbol(
        kind,
        &name,
        &format!("{module}#{name}"),
        Some(module),
        &sig,
        node,
        src,
    ));
}

fn push_class(node: Node, src: &[u8], module: &str, out: &mut ParsedFile) {
    let Some(name) = field_text(node, "name", src) else {
        return;
    };
    let fqn = format!("{module}#{name}");
    let sig = format!("class {name}");
    out.symbols.push(symbol(
        SymbolKind::Class,
        &name,
        &fqn,
        Some(module),
        &sig,
        node,
        src,
    ));

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for m in body.children(&mut cursor) {
        if !matches!(m.kind(), "method_definition" | "public_field_definition") {
            continue;
        }
        let Some(mname) = field_text(m, "name", src) else {
            continue;
        };
        let kind = if m.kind() == "method_definition" {
            SymbolKind::Method
        } else {
            SymbolKind::Field
        };
        let msig = format!(
            "{mname}{}",
            field_text(m, "parameters", src).unwrap_or_default()
        );
        out.symbols.push(symbol(
            kind,
            &mname,
            &format!("{fqn}.{mname}"),
            Some(&fqn),
            &msig,
            m,
            src,
        ));
        if let Some(b) = m.child_by_field_name("body") {
            collect_graphql_usage(b, src, &format!("{fqn}.{mname}"), out);
        }
    }
}

fn push_function(node: Node, src: &[u8], module: &str, parent: Option<&str>, out: &mut ParsedFile) {
    let Some(name) = field_text(node, "name", src) else {
        return;
    };
    let sig = format!(
        "function {name}{}",
        field_text(node, "parameters", src).unwrap_or_default()
    );
    let fqn = format!("{module}#{name}");
    out.symbols.push(symbol(
        SymbolKind::Function,
        &name,
        &fqn,
        parent.or(Some(module)),
        &sig,
        node,
        src,
    ));
    if let Some(b) = node.child_by_field_name("body") {
        collect_graphql_usage(b, src, &fqn, out);
    }
}

/// `const X = ...` — the shape that covers React components, hooks and `gql` documents.
fn push_declarator(node: Node, src: &[u8], module: &str, out: &mut ParsedFile) {
    let Some(name) = field_text(node, "name", src) else {
        return;
    };
    let value = node.child_by_field_name("value");
    let fqn = format!("{module}#{name}");

    // A `gql` document is not a variable worth indexing as one — it is a set of operations,
    // and each operation is a coordinate the backend serves.
    if let Some(v) = value {
        if let Some(doc) = gql_document(v, src) {
            push_operations(&doc, &fqn, module, node, out);
            return;
        }
    }

    let kind = match value.map(|v| v.kind()) {
        Some("arrow_function" | "function_expression") => {
            // A capitalized arrow function returning JSX is a component; the distinction
            // matters because `renders` and `calls` are different edges.
            if name.starts_with(char::is_uppercase) {
                SymbolKind::Class
            } else {
                SymbolKind::Function
            }
        }
        _ => SymbolKind::Field,
    };
    let sig = format!("const {name}");
    out.symbols
        .push(symbol(kind, &name, &fqn, Some(module), &sig, node, src));

    if let Some(v) = value {
        collect_graphql_usage(v, src, &fqn, out);
    }
}

fn symbol(
    kind: SymbolKind,
    name: &str,
    fqn: &str,
    parent: Option<&str>,
    signature: &str,
    node: Node,
    src: &[u8],
) -> RawSymbol {
    RawSymbol {
        kind,
        name: name.to_string(),
        fqn: fqn.to_string(),
        parent_fqn: parent.map(str::to_string),
        signature: Some(signature.to_string()),
        visibility: None,
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        sig_hash: hash(signature),
        body_hash: hash(&normalize_body(node, src)),
        annotations: Vec::new(),
    }
}

// ─────────────────────────── the seam ───────────────────────────

/// The text of a `gql`-tagged template, if this expression is one.
fn gql_document(node: Node, src: &[u8]) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }
    let func = node.child_by_field_name("function")?;
    let tag = text(func, src)?;
    if !matches!(tag.as_str(), "gql" | "graphql") {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    if args.kind() != "template_string" {
        return None;
    }
    text(args, src).map(|t| t.trim_matches('`').to_string())
}

/// One symbol per operation, plus an edge to every root field it selects.
///
/// The operation symbol is what a component points at, and the field edges are what reach
/// the backend — two hops, because a component names an operation while a resolver serves
/// a field, and nothing in either file names the other directly.
fn push_operations(doc: &str, export_fqn: &str, module: &str, node: Node, out: &mut ParsedFile) {
    let line = node.start_position().row as u32 + 1;
    for op in graphql::operations(doc) {
        let op_name = op
            .name
            .clone()
            .unwrap_or_else(|| export_fqn.rsplit('#').next().unwrap_or("anon").to_string());
        let op_fqn = format!("graphql:op:{op_name}");
        out.symbols.push(RawSymbol {
            kind: SymbolKind::Route,
            name: op_name.clone(),
            fqn: op_fqn.clone(),
            parent_fqn: Some(module.to_string()),
            signature: Some(format!("{} {op_name}", op.op_type.to_lowercase())),
            visibility: Some("public".into()),
            start_line: line,
            end_line: node.end_position().row as u32 + 1,
            sig_hash: hash(&format!("{op_fqn}:{}", op.fields.join(","))),
            body_hash: hash(doc),
            annotations: Vec::new(),
        });
        for field in &op.fields {
            out.edges.push(RawEdge {
                src_fqn: op_fqn.clone(),
                // The join key. The backend emits exactly this FQN for its resolver.
                dst_hint: format!("graphql:{}.{field}", op.op_type),
                edge_type: EdgeType::CallsGraphql,
                site_line: line,
            });
        }
    }
}

/// `useQuery(MySalaryDocument)` in a component becomes an edge to `graphql:op:MySalary`.
///
/// The `<Name>Document` suffix is graphql-codegen's convention, which is what makes this a
/// contract rather than a guess: the constant is generated, so the name is not a coincidence.
fn collect_graphql_usage(node: Node, src: &[u8], src_fqn: &str, out: &mut ParsedFile) {
    if node.kind() == "identifier" {
        if let Some(name) = text(node, src) {
            if let Some(op) = name.strip_suffix("Document") {
                if !op.is_empty() && op.starts_with(char::is_uppercase) {
                    out.edges.push(RawEdge {
                        src_fqn: src_fqn.to_string(),
                        dst_hint: format!("graphql:op:{op}"),
                        edge_type: EdgeType::CallsGraphql,
                        site_line: node.start_position().row as u32 + 1,
                    });
                }
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        collect_graphql_usage(c, src, src_fqn, out);
    }
}

// ─────────────────────────── helpers ───────────────────────────

fn normalize_body(node: Node, src: &[u8]) -> String {
    let mut out = String::new();
    collect_tokens(node, src, &mut out);
    out
}

fn collect_tokens(node: Node, src: &[u8], out: &mut String) {
    if matches!(node.kind(), "comment" | "html_comment") {
        return;
    }
    if node.child_count() == 0 {
        if let Some(t) = text(node, src) {
            if t.trim().is_empty() {
                return;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&t);
        }
        return;
    }
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        collect_tokens(c, src, out);
    }
}

fn text(node: Node, src: &[u8]) -> Option<String> {
    node.utf8_text(src).ok().map(str::to_string)
}

fn field_text(node: Node, field: &str, src: &[u8]) -> Option<String> {
    text(node.child_by_field_name(field)?, src)
}

fn hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(path: &str, text: &str) -> ParsedFile {
        TypeScriptAnalyzer::new()
            .parse(&SourceFile { path, text })
            .expect("parse")
    }

    fn has_symbol(p: &ParsedFile, fqn: &str) -> bool {
        p.symbols.iter().any(|s| s.fqn == fqn)
    }

    #[test]
    fn extracts_functions_components_classes_and_types() {
        let p = parse(
            "src/lib/util.ts",
            r#"
            export function formatMoney(v: number): string { return String(v); }
            export const VehicleTable = () => { return null; };
            const helper = () => 1;
            export interface VehicleDto { id: string }
            export type Status = 'NEW' | 'SOLD';
            export class Client { send(x: string) {} }
            "#,
        );
        assert!(has_symbol(&p, "src/lib/util#formatMoney"));
        assert!(has_symbol(&p, "src/lib/util#VehicleTable"));
        assert!(has_symbol(&p, "src/lib/util#helper"));
        assert!(has_symbol(&p, "src/lib/util#VehicleDto"));
        assert!(has_symbol(&p, "src/lib/util#Status"));
        assert!(has_symbol(&p, "src/lib/util#Client.send"));
    }

    #[test]
    fn a_gql_document_becomes_operations_that_point_at_schema_coordinates() {
        let p = parse(
            "src/lib/graphql/salary.ts",
            r#"
            import { gql } from '@apollo/client';
            export const MySalary = gql`
              query MySalary($period: String) {
                mySalary(period: $period) { id netSalary }
                salaryStats { total }
              }
            `;
            "#,
        );
        assert!(has_symbol(&p, "graphql:op:MySalary"));
        let hints: Vec<&str> = p.edges.iter().map(|e| e.dst_hint.as_str()).collect();
        assert!(hints.contains(&"graphql:Query.mySalary"), "{hints:?}");
        assert!(hints.contains(&"graphql:Query.salaryStats"), "{hints:?}");
        assert!(p
            .edges
            .iter()
            .all(|e| e.edge_type == EdgeType::CallsGraphql));
    }

    #[test]
    fn a_component_using_a_generated_document_reaches_the_operation() {
        let p = parse(
            "src/app/components/SalaryCard.tsx",
            r#"
            import { useQuery } from '@apollo/client/react';
            import { MySalaryDocument } from '@/types/graphql-generated';
            export const SalaryCard = () => {
              const { data } = useQuery(MySalaryDocument);
              return <div>{data?.mySalary?.netSalary}</div>;
            };
            "#,
        );
        assert!(
            p.edges
                .iter()
                .any(|e| e.src_fqn == "src/app/components/SalaryCard#SalaryCard"
                    && e.dst_hint == "graphql:op:MySalary"
                    && e.edge_type == EdgeType::CallsGraphql),
            "{:?}",
            p.edges
        );
    }

    #[test]
    fn a_fragment_only_document_declares_no_operation() {
        let p = parse(
            "src/lib/graphql/frag.ts",
            "export const F = gql`fragment SalaryFields on Salary { id netSalary }`;",
        );
        assert!(p.symbols.iter().all(|s| s.kind != SymbolKind::Route));
        assert!(p.edges.is_empty());
    }

    #[test]
    fn reformatting_does_not_move_a_hash() {
        let a = parse("a.ts", "export const f = (x: number) => { return x + 1; };");
        let b = parse(
            "a.ts",
            "export const f = (x: number) => {\n  // add one\n  return x + 1;\n};",
        );
        let fa = a.symbols.iter().find(|s| s.fqn == "a#f").expect("a");
        let fb = b.symbols.iter().find(|s| s.fqn == "a#f").expect("b");
        assert_eq!(fa.body_hash, fb.body_hash);
    }

    #[test]
    fn tsx_generics_are_not_mangled_by_the_wrong_grammar() {
        let p = parse(
            "c.tsx",
            "export const List = <T,>(items: T[]) => { return <ul>{items.length}</ul>; };",
        );
        assert!(
            has_symbol(&p, "c#List"),
            "{:?}",
            p.symbols.iter().map(|s| &s.fqn).collect::<Vec<_>>()
        );
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }
}
