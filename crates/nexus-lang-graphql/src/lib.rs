//! The GraphQL schema, indexed as the contract it is.
//!
//! Resolvers were originally taken from Java annotations alone, which quietly assumed that
//! every field a backend serves has a `@QueryMapping` this analyzer recognizes. On a real
//! Spring for GraphQL project that is false — a field can be served by a controller shape
//! not yet extracted, or resolved from a property — and the result was thirteen confident
//! reports that a field "no resolver serves" was missing when the schema declared it plainly.
//!
//! The `.graphqls` file is what graphql-codegen generates the frontend types from, so it is
//! the same contract both sides already agree on. Indexing it makes the join authoritative
//! rather than inferential.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use nexus_lang::{LangError, LanguageAnalyzer, ParsedFile, RawSymbol, SourceFile};
use nexus_types::{Language, SymbolKind};

pub struct GraphQlSchemaAnalyzer;

impl Default for GraphQlSchemaAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphQlSchemaAnalyzer {
    pub fn new() -> Self {
        GraphQlSchemaAnalyzer
    }
}

impl LanguageAnalyzer for GraphQlSchemaAnalyzer {
    fn language(&self) -> Language {
        // The schema is not a programming language, but it is a source of symbols, and the
        // registry dispatches on extension. Reporting it as TypeScript would be a lie in
        // `bughunter status`; reporting it as its own language needs no new machinery.
        Language::TypeScript
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["graphqls", "graphql", "gql"]
    }

    fn grammar_version(&self) -> &'static str {
        "graphql-schema/1"
    }

    fn parse(&self, src: &SourceFile<'_>) -> Result<ParsedFile, LangError> {
        let mut out = ParsedFile::default();
        for def in root_fields(src.text) {
            let fqn = format!("graphql:{}.{}", def.type_name, def.field);
            out.symbols.push(RawSymbol {
                kind: SymbolKind::Route,
                name: def.field.clone(),
                fqn: fqn.clone(),
                parent_fqn: None,
                signature: Some(def.signature.clone()),
                visibility: Some("public".into()),
                start_line: def.line,
                end_line: def.line,
                // The signature is the contract: a changed argument list or return type is
                // an API change on this field, which is exactly what sig_hash should catch.
                sig_hash: hash(&def.signature),
                body_hash: hash(""),
                annotations: Vec::new(),
            });
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub type_name: String,
    pub field: String,
    pub signature: String,
    pub line: u32,
}

/// Fields declared on `Query`, `Mutation` and `Subscription`, including `extend type` blocks.
///
/// A hand-written reader rather than a grammar: the join key is a schema coordinate, and a
/// full GraphQL parser is a dependency bought for one line of information per field.
pub fn root_fields(text: &str) -> Vec<FieldDef> {
    const ROOTS: [&str; 3] = ["Query", "Mutation", "Subscription"];
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut depth = 0i32;

    for (i, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }

        if current.is_none() {
            if let Some(name) = type_header(&line) {
                if ROOTS.contains(&name.as_str()) {
                    current = Some(name);
                    depth = line.matches('{').count() as i32 - line.matches('}').count() as i32;
                    continue;
                }
            }
            continue;
        }

        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        if depth <= 0 {
            current = None;
            continue;
        }

        let Some(type_name) = current.clone() else {
            continue;
        };
        let Some(field) = field_name(&line) else {
            continue;
        };
        out.push(FieldDef {
            type_name,
            field,
            signature: line.clone(),
            line: i as u32 + 1,
        });
    }
    out
}

/// `type Query {`, `extend type Mutation {`, `type Query @directive {` → the type name.
fn type_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("extend ").unwrap_or(line);
    let rest = rest.strip_prefix("type ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The field name from a declaration line, ignoring directives and argument blocks.
fn field_name(line: &str) -> Option<String> {
    let first = line.chars().next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    let name: String = line
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    // A declaration always has a type, so a colon or an argument list must follow.
    let rest = &line[name.len()..];
    let next = rest.trim_start().chars().next()?;
    (!name.is_empty() && (next == ':' || next == '(')).then_some(name)
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(p) => &line[..p],
        None => line,
    }
}

fn hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"
# The root types.
type Query {
  vehicles(pagination: AntPageable, status: VehicleStatus): AntPage!
  vehicleStats: VehicleStatsDto!
}

type Mutation {
  createUser(input: CreateUserInput!): User!
  changePassword(currentPassword: String!, newPassword: String!): Boolean!
}

type Vehicle {
  id: ID!
  plate: String
}

extend type Query {
  reports: [Report!]!
}
"#;

    fn names(text: &str) -> Vec<String> {
        root_fields(text)
            .into_iter()
            .map(|f| format!("{}.{}", f.type_name, f.field))
            .collect()
    }

    #[test]
    fn reads_query_and_mutation_fields() {
        let n = names(SCHEMA);
        assert!(n.contains(&"Query.vehicles".to_string()), "{n:?}");
        assert!(n.contains(&"Query.vehicleStats".to_string()), "{n:?}");
        assert!(n.contains(&"Mutation.createUser".to_string()), "{n:?}");
        assert!(n.contains(&"Mutation.changePassword".to_string()), "{n:?}");
    }

    #[test]
    fn extend_type_query_contributes_fields_too() {
        assert!(names(SCHEMA).contains(&"Query.reports".to_string()));
    }

    #[test]
    fn non_root_types_are_not_endpoints() {
        // `Vehicle.id` is a field, not something a client can ask for at the root.
        let n = names(SCHEMA);
        assert!(!n.iter().any(|f| f.starts_with("Vehicle.")), "{n:?}");
    }

    #[test]
    fn comments_do_not_become_fields() {
        let n = names("type Query {\n  # createUser was removed\n  vehicles: [V!]!\n}\n");
        assert_eq!(n, vec!["Query.vehicles"]);
    }

    #[test]
    fn each_field_becomes_a_route_symbol_the_frontend_can_join_to() {
        let parsed = GraphQlSchemaAnalyzer::new()
            .parse(&SourceFile {
                path: "schema.graphqls",
                text: SCHEMA,
            })
            .expect("parse");
        let fqns: Vec<&str> = parsed.symbols.iter().map(|s| s.fqn.as_str()).collect();
        assert!(fqns.contains(&"graphql:Mutation.createUser"), "{fqns:?}");
        assert!(parsed.symbols.iter().all(|s| s.kind == SymbolKind::Route));
    }

    #[test]
    fn changing_a_fields_arguments_changes_its_signature_hash() {
        let a = GraphQlSchemaAnalyzer::new()
            .parse(&SourceFile {
                path: "s.graphqls",
                text: "type Query {\n  v(a: Int): T\n}",
            })
            .expect("a");
        let b = GraphQlSchemaAnalyzer::new()
            .parse(&SourceFile {
                path: "s.graphqls",
                text: "type Query {\n  v(a: Int, b: Int): T\n}",
            })
            .expect("b");
        assert_ne!(a.symbols[0].sig_hash, b.symbols[0].sig_hash);
    }
}
