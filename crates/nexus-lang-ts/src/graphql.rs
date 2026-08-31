//! A deliberately small GraphQL document reader.
//!
//! It answers exactly two questions: which operations does this document declare, and
//! which root fields does each select. That is the whole of what the seam needs — the
//! join key is a schema coordinate, not a parse tree — so a full GraphQL grammar would be
//! a dependency bought for nothing.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// `Query`, `Mutation` or `Subscription`.
    pub op_type: String,
    /// The operation name, when it has one. Anonymous operations still carry fields.
    pub name: Option<String>,
    /// Root selection fields, which are exactly the schema coordinates this document hits.
    pub fields: Vec<String>,
}

/// Read every operation in a document. Fragments are skipped: they select on a type, not
/// on the root, so they name no schema coordinate the backend serves at the top level.
pub fn operations(doc: &str) -> Vec<Operation> {
    let text = strip_comments(doc);
    let bytes: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let Some((op_type, start)) = next_keyword(&bytes, i) else {
            break;
        };
        let mut j = start;

        // operation name, then variable definitions, then the selection set
        let name = read_ident(&bytes, &mut j);
        skip_ws(&bytes, &mut j);
        if bytes.get(j) == Some(&'(') {
            skip_balanced(&bytes, &mut j, '(', ')');
        }
        skip_ws(&bytes, &mut j);
        // Directives on the operation itself.
        while bytes.get(j) == Some(&'@') {
            j += 1;
            let _ = read_ident(&bytes, &mut j);
            skip_ws(&bytes, &mut j);
            if bytes.get(j) == Some(&'(') {
                skip_balanced(&bytes, &mut j, '(', ')');
            }
            skip_ws(&bytes, &mut j);
        }
        if bytes.get(j) != Some(&'{') {
            i = j.max(start + 1);
            continue;
        }
        let body_start = j;
        skip_balanced(&bytes, &mut j, '{', '}');
        let body: String = bytes[body_start + 1..j.saturating_sub(1)].iter().collect();
        out.push(Operation {
            op_type,
            name,
            fields: root_fields(&body),
        });
        i = j;
    }
    out
}

/// Field names selected at depth zero of a selection set, with aliases resolved to the
/// real field: `total: cartTotal` names `cartTotal`, which is what the backend serves.
pub fn root_fields(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    let mut depth = 0i32;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' => {
                depth += 1;
                i += 1;
            }
            '}' => {
                depth -= 1;
                i += 1;
            }
            '(' if depth == 0 => skip_balanced(&chars, &mut i, '(', ')'),
            '.' if depth == 0 => {
                // `...FragmentName` — a spread, not a root field.
                while i < chars.len()
                    && (chars[i] == '.' || chars[i].is_alphanumeric() || chars[i] == '_')
                {
                    i += 1;
                }
            }
            '@' if depth == 0 => {
                i += 1;
                let mut j = i;
                let _ = read_ident(&chars, &mut j);
                i = j;
                skip_ws(&chars, &mut i);
                if chars.get(i) == Some(&'(') {
                    skip_balanced(&chars, &mut i, '(', ')');
                }
            }
            _ if depth == 0 && (c.is_alphabetic() || c == '_') => {
                let mut j = i;
                let Some(word) = read_ident(&chars, &mut j) else {
                    i += 1;
                    continue;
                };
                i = j;
                skip_ws(&chars, &mut i);
                if chars.get(i) == Some(&':') {
                    // An alias: the name after the colon is the real field.
                    i += 1;
                    skip_ws(&chars, &mut i);
                    let mut k = i;
                    if let Some(real) = read_ident(&chars, &mut k) {
                        out.push(real);
                        i = k;
                        continue;
                    }
                }
                out.push(word);
            }
            _ => i += 1,
        }
    }
    // Introspection meta-fields are valid on every type by spec and are declared in no
    // schema. Emitting them as selections makes every document that asks for __typename —
    // which Apollo adds automatically — look like it hits a field nobody serves.
    out.retain(|f| !f.starts_with("__"));
    out.dedup();
    out
}

fn strip_comments(doc: &str) -> String {
    doc.lines()
        .map(|l| match l.find('#') {
            Some(p) => &l[..p],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn next_keyword(chars: &[char], from: usize) -> Option<(String, usize)> {
    let text: String = chars[from..].iter().collect();
    let mut best: Option<(usize, &str, &str)> = None;
    for (kw, ty) in [
        ("query", "Query"),
        ("mutation", "Mutation"),
        ("subscription", "Subscription"),
    ] {
        let mut search = 0usize;
        while let Some(p) = text[search..].find(kw) {
            let abs = search + p;
            let before_ok = abs == 0
                || !text[..abs]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after = text[abs + kw.len()..].chars().next();
            let after_ok = after.is_none_or(|c| c.is_whitespace() || c == '{' || c == '(');
            if before_ok && after_ok {
                if best.is_none_or(|(b, _, _)| abs < b) {
                    best = Some((abs, kw, ty));
                }
                break;
            }
            search = abs + kw.len();
        }
    }
    let (pos, kw, ty) = best?;
    // Count chars, not bytes: `text` may hold non-ASCII, and the caller indexes by char.
    let char_pos = text[..pos].chars().count();
    Some((ty.to_string(), from + char_pos + kw.chars().count()))
}

fn read_ident(chars: &[char], i: &mut usize) -> Option<String> {
    skip_ws(chars, i);
    let start = *i;
    while *i < chars.len() && (chars[*i].is_alphanumeric() || chars[*i] == '_') {
        *i += 1;
    }
    (*i > start).then(|| chars[start..*i].iter().collect())
}

fn skip_ws(chars: &[char], i: &mut usize) {
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }
}

fn skip_balanced(chars: &[char], i: &mut usize, open: char, close: char) {
    let mut depth = 0i32;
    while *i < chars.len() {
        if chars[*i] == open {
            depth += 1;
        } else if chars[*i] == close {
            depth -= 1;
            if depth == 0 {
                *i += 1;
                return;
            }
        }
        *i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_named_query_and_its_root_fields() {
        let ops = operations(
            r#"
            query MySalary($period: String) {
              mySalary(period: $period) { id netSalary }
              salaryStats { total }
            }"#,
        );
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].op_type, "Query");
        assert_eq!(ops[0].name.as_deref(), Some("MySalary"));
        assert_eq!(ops[0].fields, vec!["mySalary", "salaryStats"]);
    }

    #[test]
    fn nested_selections_are_not_root_fields() {
        let ops = operations("query Q { vehicles { id owner { name } } }");
        assert_eq!(
            ops[0].fields,
            vec!["vehicles"],
            "only depth-zero fields name a schema coordinate"
        );
    }

    #[test]
    fn an_alias_resolves_to_the_real_field() {
        let ops = operations("query Q { total: cartTotal { amount } }");
        assert_eq!(
            ops[0].fields,
            vec!["cartTotal"],
            "the backend serves the field, not the alias"
        );
    }

    #[test]
    fn fragment_spreads_are_skipped_but_sibling_fields_survive() {
        let ops = operations("query Q { ...SalaryFields  mySalary { id } }");
        assert_eq!(ops[0].fields, vec!["mySalary"]);
    }

    #[test]
    fn introspection_meta_fields_are_not_selections() {
        let ops = operations("query Ping { __typename }");
        assert!(
            ops[0].fields.is_empty(),
            "__typename is valid on every type: {:?}",
            ops[0].fields
        );
        let mixed = operations("query Q { __typename vehicles { id } }");
        assert_eq!(mixed[0].fields, vec!["vehicles"]);
    }

    #[test]
    fn a_fragment_definition_declares_no_operation() {
        assert!(operations("fragment SalaryFields on Salary { id netSalary }").is_empty());
    }

    #[test]
    fn handles_mutations_and_several_operations_in_one_document() {
        let ops = operations(
            "query A { one } mutation B($i: In!) { two(input: $i) { id } } subscription C { three }",
        );
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].op_type, "Query");
        assert_eq!(ops[1].op_type, "Mutation");
        assert_eq!(ops[1].name.as_deref(), Some("B"));
        assert_eq!(ops[1].fields, vec!["two"]);
        assert_eq!(ops[2].op_type, "Subscription");
    }

    #[test]
    fn comments_and_an_anonymous_operation() {
        let ops = operations("# a comment mentioning query Fake\nquery { vehicles { id } }");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, None);
        assert_eq!(ops[0].fields, vec!["vehicles"]);
    }
}
