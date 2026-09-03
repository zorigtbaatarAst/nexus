//! Python analyzer (roadmap 5.3).
//!
//! The same contract as every other analyzer: source text in, a `ParsedFile` out, no
//! knowledge of scans or storage. What is specific to Python is where the structure hides.
//!
//! # Decorators are the structure
//!
//! In Java a route is an annotation on a method and a repository is an interface extending a
//! framework type. Python puts both in decorators — `@app.get("/payments")`,
//! `@router.post(...)` — and a Python analyzer that reads only `def` and `class` sees a file
//! full of functions with no idea which of them serve HTTP. So decorators are captured as
//! annotations, which is exactly how the Java pack treats `@GetMapping`, and the route shape
//! is derived from them (ADR-012: a framework pack is a separate axis from a language).
//!
//! # Indentation is syntax, and the body hash must not care
//!
//! Reformatting Python moves whitespace that the grammar treats as tokens. [`normalize_body`]
//! drops the layout tokens the grammar emits, so a reformat produces no symbol change — the
//! same invariant every other analyzer here is pinned to, and the one that decides how much
//! of a repository a formatting pass appears to change.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use nexus_lang::{LangError, LanguageAnalyzer, ParsedFile, RawEdge, RawSymbol, SourceFile};
use nexus_types::{EdgeType, Language, SymbolKind};
use tree_sitter::{Node, Parser};

pub struct PythonAnalyzer;

impl Default for PythonAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonAnalyzer {
    pub fn new() -> Self {
        PythonAnalyzer
    }
}

impl LanguageAnalyzer for PythonAnalyzer {
    fn language(&self) -> Language {
        Language::Python
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py"]
    }

    fn grammar_version(&self) -> &'static str {
        "tree-sitter-python 0.25"
    }

    fn parse(&self, src: &SourceFile<'_>) -> Result<ParsedFile, LangError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .map_err(|_| LangError::Grammar(Language::Python))?;
        let tree = parser.parse(src.text, None).ok_or(LangError::NoTree)?;
        let bytes = src.text.as_bytes();

        let mut out = ParsedFile::default();
        if tree.root_node().has_error() {
            out.warnings
                .push("file contains syntax errors; symbols may be incomplete".into());
        }
        let module = module_path(src.path);
        out.package = Some(module.clone());
        walk(tree.root_node(), bytes, &module, None, &mut out);
        Ok(out)
    }
}

/// The importable module path of a file.
///
/// `pay/services.py` is `pay.services`; `pay/__init__.py` is `pay`. Dots, because that is how
/// Python names a module and how an import in another file will refer to it — a resolver
/// matching hints against these has to see the same string the source writes.
pub fn module_path(path: &str) -> String {
    let p = path.strip_prefix("./").unwrap_or(path);
    let p = p.strip_suffix(".py").unwrap_or(p);
    let parts: Vec<&str> = p
        .split('/')
        .filter(|s| !s.is_empty() && *s != "__init__" && *s != "src")
        .collect();
    parts.join(".")
}

fn child_text<'a>(node: Node<'_>, field: &str, bytes: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name(field)
        .and_then(|n| n.utf8_text(bytes).ok())
}

fn line(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn hash(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    h.finalize().to_hex()[..32].to_string()
}

fn is_comment(kind: &str) -> bool {
    matches!(kind, "comment")
}

/// Layout tokens the Python grammar emits. Dropping them is what makes a reformat a no-op.
fn is_layout(kind: &str) -> bool {
    matches!(kind, "indent" | "dedent" | "newline" | "line_continuation")
}

/// Tokens of a node, comments and layout dropped, single-spaced.
///
/// Indentation is syntax in Python, so the grammar emits it as tokens. Keeping them would
/// make re-indenting a block a change to every symbol in it, and a formatting pass would
/// rewrite the index and bury the real change.
pub fn normalize_body(node: Node<'_>, src: &[u8]) -> String {
    let mut out = String::new();
    collect_tokens(node, src, &mut out);
    out
}

fn collect_tokens(node: Node<'_>, src: &[u8], out: &mut String) {
    if is_comment(node.kind()) || is_layout(node.kind()) {
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

/// Decorators attached to a definition.
///
/// Carried as annotations because that is what they are to the rest of the system: `@app.get`
/// says this function serves HTTP exactly as `@GetMapping` does in Java, and a change to one
/// is a contract change even when the signature is untouched.
fn decorators(node: Node<'_>, bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(parent) = node.parent() else {
        return out;
    };
    if parent.kind() != "decorated_definition" {
        return out;
    }
    let mut cursor = parent.walk();
    for child in parent.children(&mut cursor) {
        if child.kind() == "decorator" {
            if let Ok(t) = child.utf8_text(bytes) {
                out.push(t.trim().to_string());
            }
        }
    }
    out
}

/// The signature: every token before the body.
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

/// A route path from a web-framework decorator, and the method it serves.
///
/// FastAPI and Flask both write `@thing.verb("/path")`; Django writes routes in a URL
/// configuration instead, which this does not read — a route table is a different shape and
/// guessing at it from a decorator would invent endpoints.
pub fn route_of(decorator: &str) -> Option<(String, String)> {
    let d = decorator.trim_start_matches('@');
    let (head, rest) = d.split_once('(')?;
    let verb = head.rsplit('.').next()?.to_ascii_uppercase();
    if !matches!(
        verb.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "ROUTE"
    ) {
        return None;
    }
    let path = rest
        .trim_start()
        .trim_start_matches(['"', '\''])
        .split(['"', '\''])
        .next()?
        .to_string();
    if !path.starts_with('/') {
        return None;
    }
    Some((verb, path))
}

#[allow(clippy::too_many_arguments)]
fn push_symbol(
    out: &mut ParsedFile,
    kind: SymbolKind,
    name: &str,
    fqn: String,
    parent: Option<&str>,
    node: Node<'_>,
    bytes: &[u8],
    annotations: Vec<String>,
) {
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
        // Python has no keyword for it; a leading underscore is the whole convention.
        visibility: Some(
            if name.starts_with('_') {
                "private"
            } else {
                "public"
            }
            .into(),
        ),
        start_line: line(node),
        end_line: node.end_position().row as u32 + 1,
        sig_hash: hash(&[&sig, &annotations.join(",")]),
        body_hash: hash(&[&body]),
        annotations,
    });
}

fn walk(node: Node<'_>, bytes: &[u8], scope: &str, parent: Option<&str>, out: &mut ParsedFile) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "decorated_definition" => {
                // The decorators belong to the definition inside; recursing here reaches it
                // with `decorators()` able to see them through the parent link.
                walk(child, bytes, scope, parent, out);
            }
            "import_statement" | "import_from_statement" => {
                if let Ok(t) = child.utf8_text(bytes) {
                    out.imports.push(t.trim().to_string());
                }
            }
            "class_definition" => {
                let Some(name) = child_text(child, "name", bytes) else {
                    continue;
                };
                let fqn = join(scope, name);
                let decs = decorators(child, bytes);
                push_symbol(
                    out,
                    SymbolKind::Class,
                    name,
                    fqn.clone(),
                    None,
                    child,
                    bytes,
                    decs,
                );
                // Base classes are `extends` edges: `class Payment(models.Model)` is how
                // Django says this is a table, and it is the only place it says so.
                if let Some(args) = child.child_by_field_name("superclasses") {
                    let mut c = args.walk();
                    for base in args.named_children(&mut c) {
                        if let Ok(t) = base.utf8_text(bytes) {
                            out.edges.push(RawEdge {
                                src_fqn: fqn.clone(),
                                dst_hint: t.trim().to_string(),
                                edge_type: EdgeType::Extends,
                                site_line: line(child),
                            });
                        }
                    }
                }
                if let Some(body) = child.child_by_field_name("body") {
                    walk(body, bytes, &fqn, Some(&fqn), out);
                }
            }
            "function_definition" => {
                let Some(name) = child_text(child, "name", bytes) else {
                    continue;
                };
                // A method is a member of its class; a module-level function is a member of
                // its module. `#` before a member matches every other analyzer here, and the
                // store's member lookup keys off it.
                let fqn = match parent {
                    Some(_) => format!("{scope}#{name}"),
                    None => join(scope, name),
                };
                let decs = decorators(child, bytes);
                let kind = if parent.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                for d in &decs {
                    if let Some((verb, path)) = route_of(d) {
                        out.edges.push(RawEdge {
                            src_fqn: fqn.clone(),
                            dst_hint: format!("http:{verb} {path}"),
                            edge_type: EdgeType::Routes,
                            site_line: line(child),
                        });
                    }
                }
                push_symbol(out, kind, name, fqn.clone(), parent, child, bytes, decs);
                calls(child, bytes, &fqn, out);
            }
            _ => {}
        }
    }
}

fn join(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}.{name}")
    }
}

/// Names that are the language or its standard library, not this project.
///
/// Same reasoning as the Rust analyzer: a bare `#append` hint matches every append in the
/// index, so emitting it is a wrong edge rather than a missing one.
const BUILTINS: &[&str] = &[
    "print",
    "len",
    "str",
    "int",
    "float",
    "bool",
    "list",
    "dict",
    "set",
    "tuple",
    "range",
    "enumerate",
    "zip",
    "sorted",
    "sum",
    "min",
    "max",
    "abs",
    "isinstance",
    "hasattr",
    "getattr",
    "setattr",
    "open",
    "format",
    "append",
    "extend",
    "insert",
    "remove",
    "pop",
    "get",
    "keys",
    "values",
    "items",
    "join",
    "split",
    "strip",
    "replace",
    "startswith",
    "endswith",
    "lower",
    "upper",
    "super",
    "type",
    "repr",
    "iter",
    "next",
    "any",
    "all",
    "map",
    "filter",
    "round",
];

fn calls(node: Node<'_>, bytes: &[u8], src_fqn: &str, out: &mut ParsedFile) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "call" {
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

fn call_hint(func: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = func.utf8_text(bytes).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    let last = text.rsplit('.').next().unwrap_or(text);
    if BUILTINS.contains(&last) {
        return None;
    }
    match func.kind() {
        // `self.thing()` — the receiver's type is not known from one file, so the member name
        // is the honest hint and resolution decides what it means.
        "attribute" if text.starts_with("self.") => Some(format!("#{last}")),
        "attribute" | "identifier" => Some(text.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(path: &str, text: &str) -> ParsedFile {
        PythonAnalyzer::new()
            .parse(&SourceFile { path, text })
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

    const SERVICE: &str = r#"
from decimal import Decimal

class PaymentService:
    def pay(self, key):
        return self.repo.save(key)

    def _internal(self):
        pass

def helper(n):
    return n + 1
"#;

    #[test]
    fn classes_methods_and_functions_all_become_symbols() {
        let p = parse("pay/services.py", SERVICE);
        assert_eq!(
            find(&p, "pay.services.PaymentService").kind,
            SymbolKind::Class
        );
        assert_eq!(
            find(&p, "pay.services.PaymentService#pay").kind,
            SymbolKind::Method
        );
        assert_eq!(find(&p, "pay.services.helper").kind, SymbolKind::Function);
    }

    #[test]
    fn a_leading_underscore_is_the_whole_visibility_convention() {
        let p = parse("pay/services.py", SERVICE);
        assert_eq!(
            find(&p, "pay.services.PaymentService#_internal")
                .visibility
                .as_deref(),
            Some("private")
        );
    }

    #[test]
    fn a_module_path_is_what_an_import_would_write() {
        assert_eq!(module_path("pay/services.py"), "pay.services");
        assert_eq!(module_path("pay/__init__.py"), "pay");
        assert_eq!(module_path("src/app/main.py"), "app.main");
    }

    #[test]
    fn a_reformat_changes_no_hash() {
        // Indentation is syntax in Python, so the grammar emits it as tokens. Keeping them
        // would make re-indenting a block a change to every symbol in it.
        let tight = "def f(a):\n    b = a + 1\n    return b\n";
        let loose = "def f(a):\n\n        b = a + 1\n\n        return b\n";
        assert_eq!(
            parse("a.py", tight).symbols[0].body_hash,
            parse("a.py", loose).symbols[0].body_hash
        );
    }

    #[test]
    fn a_comment_is_not_behaviour() {
        let bare = "def f():\n    return 1\n";
        let documented = "def f():\n    # obviously\n    return 1\n";
        assert_eq!(
            parse("a.py", bare).symbols[0].body_hash,
            parse("a.py", documented).symbols[0].body_hash
        );
    }

    #[test]
    fn a_one_line_change_moves_the_body_hash_and_not_the_signature() {
        let before = parse("a.py", "def f(a):\n    return a + 1\n");
        let after = parse("a.py", "def f(a):\n    return a + 2\n");
        assert_ne!(before.symbols[0].body_hash, after.symbols[0].body_hash);
        assert_eq!(before.symbols[0].sig_hash, after.symbols[0].sig_hash);
    }

    #[test]
    fn a_decorator_is_part_of_the_contract() {
        // `@app.get` says this function serves HTTP exactly as `@GetMapping` does in Java.
        let plain = parse("a.py", "def f():\n    pass\n");
        let routed = parse("a.py", "@app.get(\"/pay\")\ndef f():\n    pass\n");
        assert_ne!(plain.symbols[0].sig_hash, routed.symbols[0].sig_hash);
        assert_eq!(routed.symbols[0].annotations, ["@app.get(\"/pay\")"]);
    }

    #[test]
    fn a_fastapi_decorator_becomes_a_route_edge() {
        // A Python analyzer that reads only `def` and `class` sees a file full of functions
        // with no idea which of them serve HTTP.
        let p = parse(
            "app/api.py",
            "@router.post(\"/payments\")\ndef create():\n    pass\n",
        );
        let route = p
            .edges
            .iter()
            .find(|e| e.edge_type == EdgeType::Routes)
            .expect("a route edge");
        assert_eq!(route.dst_hint, "http:POST /payments");
    }

    #[test]
    fn a_django_model_base_class_is_an_extends_edge() {
        // `class Payment(models.Model)` is how Django says this is a table, and the only
        // place it says so.
        let p = parse("pay/models.py", "class Payment(models.Model):\n    pass\n");
        assert!(
            p.edges
                .iter()
                .any(|e| e.edge_type == EdgeType::Extends && e.dst_hint == "models.Model"),
            "{:?}",
            p.edges
        );
    }

    #[test]
    fn a_builtin_call_is_not_an_edge_into_this_project() {
        let p = parse("a.py", "def f(xs):\n    print(len(xs))\n    xs.append(1)\n");
        assert!(p.edges.is_empty(), "{:?}", p.edges);
    }

    #[test]
    fn a_self_call_gives_the_member_name_as_its_hint() {
        let p = parse("pay/services.py", SERVICE);
        assert!(
            p.edges
                .iter()
                .any(|e| e.src_fqn == "pay.services.PaymentService#pay" && e.dst_hint == "#save"),
            "{:?}",
            p.edges
        );
    }

    #[test]
    fn a_route_is_only_read_from_a_verb_decorator_with_a_path() {
        assert_eq!(
            route_of("@app.get(\"/x\")"),
            Some(("GET".into(), "/x".into()))
        );
        // Not every decorator is a route, and guessing would invent endpoints.
        assert_eq!(route_of("@staticmethod"), None);
        assert_eq!(route_of("@app.get(name)"), None);
        assert_eq!(route_of("@lru_cache(maxsize=1)"), None);
    }

    #[test]
    fn a_file_that_does_not_parse_still_contributes_what_it_has() {
        let p = parse("a.py", "def ok():\n    pass\ndef broken(:\n");
        assert!(!p.warnings.is_empty());
        assert!(p.symbols.iter().any(|s| s.name == "ok"));
    }

    #[test]
    fn imports_are_recorded() {
        let p = parse("pay/services.py", SERVICE);
        assert!(
            p.imports.iter().any(|i| i.contains("Decimal")),
            "{:?}",
            p.imports
        );
    }
}
