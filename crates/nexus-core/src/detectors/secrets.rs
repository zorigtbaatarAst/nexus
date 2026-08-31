//! Hardcoded credentials.
//!
//! Finding a secret and protecting it are the same pass: docs/security.md §5 already needs
//! this scan to keep credentials out of anything sent to a model, so reporting what it finds
//! costs nothing extra.
//!
//! The detector reports the *location*, never the value. A bug record that quotes the secret
//! recreates, inside BugHunter's own database, exactly the exposure the redactor exists to
//! prevent — and that database is not covered by the deny-list protecting the repository.

use super::{DetectContext, Detector};
use crate::bugs::{BugCandidate, CodeRef};
use nexus_types::{BugType, Severity};

/// `(label, prefix)` — shapes specific enough that a match is a credential, not a word.
const PREFIXES: &[(&str, &str)] = &[
    ("aws_access_key", "AKIA"),
    ("aws_access_key", "ASIA"),
    ("github_pat", "ghp_"),
    ("github_oauth", "gho_"),
    ("github_app", "ghs_"),
    ("slack_token", "xoxb-"),
    ("slack_token", "xoxp-"),
    ("stripe_secret", "sk_live_"),
    ("stripe_secret", "rk_live_"),
    ("openai_key", "sk-proj-"),
    ("google_api_key", "AIza"),
    ("private_key", "-----BEGIN RSA PRIVATE KEY"),
    ("private_key", "-----BEGIN OPENSSH PRIVATE KEY"),
    ("private_key", "-----BEGIN PRIVATE KEY"),
];

/// Assignment targets whose value is a credential by definition.
const ASSIGNED: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "api_key",
    "apikey",
    "access_token",
    "private_key",
];

pub struct HardcodedSecret;

impl Detector for HardcodedSecret {
    fn id(&self) -> &'static str {
        "secret:hardcoded"
    }

    fn describe(&self) -> &'static str {
        "a credential committed to source"
    }

    fn run(&self, ctx: &DetectContext<'_>) -> Vec<BugCandidate> {
        let mut out = Vec::new();
        for f in ctx.files {
            if !is_scannable(&f.path) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(ctx.root.join(&f.path)) else {
                continue;
            };
            // A file large enough to be a data dump is not source, and scanning it costs
            // more than it finds.
            if text.len() > 1_000_000 {
                continue;
            }
            for (i, line) in text.lines().enumerate() {
                let Some((label, why)) = classify(line) else {
                    continue;
                };
                let component = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
                out.push(BugCandidate {
                    bug_type: BugType::Security,
                    title: format!("{label} committed to {component}"),
                    component: component.clone(),
                    anchor_fqn: None,
                    severity: Severity::Critical,
                    confidence: 0.9,
                    detector: self.id().to_string(),
                    // The path is part of what this bug *is* — the same key in two files is
                    // two things to revoke — but the line number is not, so it stays out.
                    structural_key: format!("{}:{label}", f.path),
                    slug: format!("secret-{}", label.replace('_', "-")),
                    evidence: vec![CodeRef {
                        file: f.path.clone(),
                        line: i as u32 + 1,
                        // The value is deliberately absent.
                        note: format!("{why}. Rotate the credential, then remove it from history."),
                    }],
                });
            }
        }
        out
    }
}

fn is_scannable(path: &str) -> bool {
    const SKIP: &[&str] = &[
        ".lock", ".min.js", ".map", ".svg", ".png", ".jpg", ".pdf", ".woff", ".woff2",
    ];
    if SKIP.iter().any(|e| path.ends_with(e)) {
        return false;
    }
    // Test fixtures are full of deliberate fake credentials, and reporting those trains
    // people to ignore the rule.
    // Test files are full of deliberate fake credentials, and reporting those trains people
    // to ignore the rule. The suffix forms matter as much as the directory ones: a real
    // project had its only "critical" finding in registerRequest.test.ts.
    let is_test = path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("__tests__")
        || path.contains("__fixtures__")
        || path.contains("/e2e/")
        || [
            ".test.ts",
            ".test.tsx",
            ".test.js",
            ".spec.ts",
            ".spec.tsx",
            ".spec.js",
            "Test.java",
            "Tests.java",
            "IT.java",
        ]
        .iter()
        .any(|suffix| path.ends_with(suffix));
    !is_test
}

fn classify(line: &str) -> Option<(&'static str, String)> {
    if line.len() > 4000 {
        return None;
    }
    for (label, prefix) in PREFIXES {
        if line.contains(prefix) {
            return Some((
                label,
                format!("a {} appears on this line", label.replace('_', " ")),
            ));
        }
    }

    let lower = line.to_lowercase();
    for key in ASSIGNED {
        let Some(pos) = lower.find(key) else { continue };
        let rest = &line[pos + key.len()..];
        let Some(value) = assigned_literal(rest) else {
            continue;
        };
        if is_placeholder(&value) || value.len() < 8 {
            continue;
        }
        return Some((
            "credential",
            format!(
                "`{key}` is assigned a literal value {} characters long",
                value.len()
            ),
        ));
    }
    None
}

/// The string literal on the right of `=` or `:`, if there is one.
fn assigned_literal(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix('=')
        .or_else(|| rest.strip_prefix(':'))?
        .trim_start();
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let body = &rest[1..];
    let end = body.find(quote)?;
    Some(body[..end].to_string())
}

/// Templates, environment lookups and obvious dummies are configuration, not credentials.
fn is_placeholder(v: &str) -> bool {
    let lower = v.to_lowercase();
    v.contains("${")
        || v.contains("{{")
        || v.starts_with("env.")
        || v.starts_with("process.env")
        || lower.contains("changeme")
        || lower.contains("example")
        || lower.contains("your-")
        || lower.contains("placeholder")
        || lower.contains("xxxx")
        || v.chars().all(|c| c == '*' || c == 'x' || c == 'X')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_credential_shapes_are_recognized() {
        assert!(classify(r#"  aws_key = "AKIAIOSFODNN7EXAMPLE""#).is_some());
        assert!(classify("token: ghp_abcdefghijklmnopqrstuvwxyz0123456789").is_some());
        assert!(classify("-----BEGIN RSA PRIVATE KEY-----").is_some());
    }

    #[test]
    fn an_assigned_password_literal_is_recognized() {
        let (label, why) =
            classify(r#"spring.datasource.password="hunter2isnotgreat""#).expect("hit");
        assert_eq!(label, "credential");
        assert!(
            !why.contains("hunter2"),
            "the value must never appear in the record: {why}"
        );
    }

    #[test]
    fn templates_and_env_lookups_are_configuration_not_credentials() {
        assert!(classify(r#"password: "${DB_PASSWORD}""#).is_none());
        assert!(classify(r#"password = "changeme-please""#).is_none());
        assert!(classify(r#"apiKey: "your-api-key-here""#).is_none());
        assert!(classify(r#"password = "xxxxxxxxxx""#).is_none());
    }

    #[test]
    fn a_short_value_is_not_a_credential() {
        assert!(classify(r#"password = "abc""#).is_none());
    }

    #[test]
    fn test_fixtures_are_skipped() {
        // Otherwise every fake credential in a test suite is a critical finding, and the
        // rule teaches people to ignore it.
        assert!(!is_scannable("src/test/resources/application.properties"));
        assert!(is_scannable("src/main/resources/application.properties"));
    }
}
