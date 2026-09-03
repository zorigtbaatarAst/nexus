//! TypeScript and TSX analyzer.
//!
//! Beyond symbols, this crate carries the frontend half of the cross-stack seam: `gql`
//! documents become operation symbols, and a component's use of a generated
//! `<Name>Document` becomes an edge to the operation it names. Resolution then joins
//! those operations to the backend resolvers that serve their root fields.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod graphql;

use nexus_lang::{LangError, LanguageAnalyzer, ParsedFile, RawEdge, RawSymbol, SourceFile};
use nexus_types::{EdgeType, Language, SymbolKind};
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

/// JavaScript, by the same parser.
///
/// A separate analyzer rather than four more extensions on the TypeScript one, because
/// `language()` is per-analyzer: a `.js` file claimed by `TypeScriptAnalyzer` would be
/// reported as TypeScript in the profile, and a JavaScript project would be described as
/// something it is not. Everything else — symbols, the `gql` seam, module ids — is shared,
/// since the extraction is identical once the grammar is chosen.
pub struct JavaScriptAnalyzer;

impl Default for JavaScriptAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl JavaScriptAnalyzer {
    pub fn new() -> Self {
        JavaScriptAnalyzer
    }
}

impl LanguageAnalyzer for JavaScriptAnalyzer {
    fn language(&self) -> Language {
        Language::JavaScript
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["js", "jsx", "mjs", "cjs"]
    }

    fn grammar_version(&self) -> &'static str {
        "tree-sitter-typescript/0.23+extract1"
    }

    fn parse(&self, src: &SourceFile<'_>) -> Result<ParsedFile, LangError> {
        parse_with(src, Language::JavaScript)
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
        parse_with(src, Language::TypeScript)
    }
}

/// Which grammar a path needs.
///
/// TSX and TS need different grammars: `<T>x` is a type assertion in one and an element in
/// the other, and using the wrong one silently mangles every generic.
///
/// Every JavaScript extension takes the TSX grammar, `.js` included. A React project puts JSX
/// in `.js` constantly, and the ambiguity that forces the split does not exist here — `<T>x`
/// as a type assertion is TypeScript-only syntax, so nothing is lost by reading it as an
/// element in a file that cannot contain one.
fn needs_tsx(path: &str) -> bool {
    [".tsx", ".jsx", ".js", ".mjs", ".cjs"]
        .iter()
        .any(|e| path.ends_with(e))
}

fn parse_with(src: &SourceFile<'_>, lang: Language) -> Result<ParsedFile, LangError> {
    {
        let mut parser = Parser::new();
        let language = if needs_tsx(src.path) {
            tree_sitter_typescript::LANGUAGE_TSX
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT
        };
        parser
            .set_language(&language.into())
            .map_err(|_| LangError::Grammar(lang))?;
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
    for ext in [".tsx", ".ts", ".mts", ".cts", ".jsx", ".js", ".mjs", ".cjs"] {
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
            let member = format!("{fqn}.{mname}");
            collect_graphql_usage(b, src, &member, out);
            calls(b, src, &member, out);
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
        calls(b, src, &fqn, out);
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
        calls(v, src, &fqn, out);
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
/// Call edges out of one body.
///
/// Until this existed the TypeScript analyzer emitted `CallsGraphql` and nothing else, so a
/// TypeScript or JavaScript project had symbols and no roads between them: `nexus impact` on
/// a component could only traverse the GraphQL seam. Express indexed 532 symbols and 0 edges.
///
/// The hint is the best shape one file affords — the callee's own name. Binding it to a
/// symbol is resolution's job in `nexus-core`, once every symbol is known.
fn calls(body: Node, src: &[u8], src_fqn: &str, out: &mut ParsedFile) {
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if matches!(n.kind(), "call_expression" | "new_expression") {
            if let Some(hint) = n
                .child_by_field_name("function")
                .and_then(|f| call_hint(f, src))
            {
                out.edges.push(RawEdge {
                    src_fqn: src_fqn.to_string(),
                    dst_hint: hint,
                    edge_type: EdgeType::Calls,
                    site_line: (n.start_position().row + 1) as u32,
                });
            }
        }
        let mut c = n.walk();
        for child in n.children(&mut c) {
            stack.push(child);
        }
    }
}

/// The callee's name, or `None` when naming it would be worse than saying nothing.
fn call_hint(func: Node, src: &[u8]) -> Option<String> {
    let name = match func.kind() {
        "identifier" => text(func, src)?,
        // `a.b.c()` and `this.x()` — the property is the callee.
        "member_expression" => text(func.child_by_field_name("property")?, src)?,
        _ => return None,
    };
    if name.len() < 3 || UBIQUITOUS.contains(&name.as_str()) {
        return None;
    }
    Some(name)
}

/// Names carried by every object in the language, not by this project.
///
/// The Rust analyzer needed the same list for the same reason: a bare `map` hint matches every
/// `map` in the index, so emitting it produces a *wrong* edge rather than a missing one. There
/// it was 459 `#clone` edges burying the real ones. Deliberately short and boring — builtins
/// and near-universal methods, not a style guide.
const UBIQUITOUS: &[&str] = &[
    // Module and object plumbing.
    "require",
    "define",
    "assign",
    "keys",
    "values",
    "entries",
    "freeze",
    "create",
    // Array and iterable.
    "map",
    "filter",
    "forEach",
    "reduce",
    "find",
    "findIndex",
    "some",
    "every",
    "includes",
    "indexOf",
    "push",
    "pop",
    "shift",
    "unshift",
    "slice",
    "splice",
    "concat",
    "join",
    "sort",
    "reverse",
    "flat",
    "flatMap",
    "fill",
    // String.
    "split",
    "trim",
    "replace",
    "replaceAll",
    "substring",
    "substr",
    "toLowerCase",
    "toUpperCase",
    "startsWith",
    "endsWith",
    "padStart",
    "padEnd",
    "repeat",
    "match",
    // Promise and async.
    "then",
    "catch",
    "finally",
    "resolve",
    "reject",
    "all",
    "race",
    "race",
    "allSettled",
    // Serialisation and logging.
    "stringify",
    "parse",
    "log",
    "warn",
    "error",
    "info",
    "debug",
    "trace",
    // Object protocol.
    "toString",
    "valueOf",
    "hasOwnProperty",
    "call",
    "apply",
    "bind",
    // Timers and events.
    "setTimeout",
    "setInterval",
    "clearTimeout",
    "clearInterval",
    "emit",
    "once",
];

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

#[cfg(test)]
mod javascript_tests {
    use super::*;

    fn js(path: &str, text: &str) -> ParsedFile {
        JavaScriptAnalyzer::new()
            .parse(&SourceFile { path, text })
            .expect("parse")
    }

    #[test]
    fn javascript_is_claimed_and_reported_as_javascript() {
        let a = JavaScriptAnalyzer::new();
        assert_eq!(a.language(), Language::JavaScript);
        for e in ["js", "jsx", "mjs", "cjs"] {
            assert!(a.extensions().contains(&e), "{e} is unclaimed");
        }
        // Not TypeScript. A `.js` file claimed by the TypeScript analyzer would describe a
        // JavaScript project as something it is not, which is why this is a second analyzer
        // rather than four more extensions on the first.
        assert_ne!(
            TypeScriptAnalyzer::new().language(),
            JavaScriptAnalyzer::new().language()
        );
    }

    #[test]
    fn jsx_inside_a_plain_js_file_still_parses() {
        // React projects put JSX in `.js` constantly. Parsed with the TypeScript grammar this
        // is a syntax error and every symbol below the first element is lost.
        let p = js(
            "src/App.js",
            r#"
            export function App({ user }) {
              return <div className="app">{user.name}</div>;
            }
            export function Footer() { return <footer />; }
            "#,
        );
        assert!(
            p.warnings.is_empty(),
            "JSX in a .js file must not be a syntax error: {:?}",
            p.warnings
        );
        let names: Vec<&str> = p.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"App") && names.contains(&"Footer"),
            "{names:?}"
        );
    }

    #[test]
    fn a_call_becomes_an_edge_and_a_builtin_does_not() {
        // Before this the analyzer emitted `CallsGraphql` and nothing else, so a JavaScript
        // project had symbols and no roads between them — Express indexed 532 symbols and
        // 0 edges, and `nexus impact` could answer nothing.
        let p = js(
            "lib/router.js",
            r#"
            function handleRequest(req) {
              const parsed = parseUrl(req.url);
              items.map(x => x);
              console.log(parsed);
              return dispatchRoute(parsed);
            }
            "#,
        );
        let hints: Vec<&str> = p
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::Calls)
            .map(|e| e.dst_hint.as_str())
            .collect();
        assert!(hints.contains(&"parseUrl"), "{hints:?}");
        assert!(hints.contains(&"dispatchRoute"), "{hints:?}");
        // `map` and `log` are carried by every object in the language. Emitting them produces
        // a *wrong* edge rather than a missing one, which is the lesson the Rust analyzer's
        // PRELUDE list already recorded.
        assert!(
            !hints.contains(&"map"),
            "a builtin is not a call edge: {hints:?}"
        );
        assert!(
            !hints.contains(&"log"),
            "a builtin is not a call edge: {hints:?}"
        );
    }

    #[test]
    fn typescript_gains_call_edges_too() {
        let p = TypeScriptAnalyzer::new()
            .parse(&SourceFile {
                path: "src/svc.ts",
                text: "export function a(): void { helperFunction(); }",
            })
            .expect("parse");
        assert!(p
            .edges
            .iter()
            .any(|e| e.edge_type == EdgeType::Calls && e.dst_hint == "helperFunction"));
    }
}
