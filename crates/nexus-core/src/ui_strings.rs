//! What a person sees on the screen, indexed back to the code that produces it (roadmap 5.5).
//!
//! This is the strongest signal an investigation has. Someone reports "the Confirm button
//! does nothing" and names no file, no symbol and no endpoint — but the words on the button
//! are in the repository, and from there the component is one edge away.
//!
//! # Every locale's values, not only the keys
//!
//! The screenshot may be in Mongolian while the source holds an English key. Indexing keys
//! alone makes a non-English interface unanchorable by text, which removes the signal
//! entirely for exactly the projects that need it most. So both sides of a translation file
//! are indexed: the key, and the value in whatever language it is written in.
//!
//! # Extraction is deliberately shallow
//!
//! No parsing of framework-specific markup, no evaluation of template expressions. A string
//! literal in a component, a key and a value in a translation file. A cleverer extractor
//! guesses, and a guessed screen string points an investigation at the wrong component with
//! full confidence.

use serde_json::Value;

/// One indexed string, and what kind of thing it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiString {
    pub text: String,
    /// `i18n_key` · `i18n_value` · `literal` · `test_id` · `aria_label` · `placeholder`
    pub kind: &'static str,
    /// The locale a value is written in, inferred from the file's name or directory. `None`
    /// for a key, which belongs to no language.
    pub locale: Option<String>,
    pub line: u32,
}

/// Whether this file is worth extracting from at all.
pub fn is_candidate(path: &str) -> bool {
    let p = path.to_lowercase();
    if p.contains("node_modules") || p.contains("/target/") || p.contains("/build/") {
        return false;
    }
    is_translation_file(&p)
        || p.ends_with(".tsx")
        || p.ends_with(".jsx")
        || p.ends_with(".vue")
        || p.ends_with(".svelte")
}

fn is_translation_file(lower: &str) -> bool {
    let named = lower.contains("/i18n/")
        || lower.contains("/locales/")
        || lower.contains("/locale/")
        || lower.contains("/lang/")
        || lower.contains("messages")
        || lower.contains("translation");
    named && (lower.ends_with(".json") || lower.ends_with(".properties"))
}

/// The locale a translation file is for, from its own name or its directory.
///
/// `locales/mn/common.json`, `messages_mn.properties`, `mn.json` all give `mn`. Absent rather
/// than guessed when nothing in the path says: a wrong locale label is worse than none,
/// because retrieval would filter on it.
pub fn locale_of(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    let stem = lower.rsplit('/').next()?;
    let stem = stem.split('.').next()?;
    if let Some((_, tail)) = stem.rsplit_once('_') {
        if is_locale_tag(tail) {
            return Some(tail.to_string());
        }
    }
    if is_locale_tag(stem) {
        return Some(stem.to_string());
    }
    // `locales/mn/common.json` — the directory names the language.
    lower
        .split('/')
        .rev()
        .nth(1)
        .filter(|d| is_locale_tag(d))
        .map(str::to_string)
}

fn is_locale_tag(s: &str) -> bool {
    let core = s.split(['-', '_']).next().unwrap_or(s);
    core.len() == 2 && core.chars().all(|c| c.is_ascii_lowercase())
}

/// Extract from one file.
pub fn extract(path: &str, text: &str) -> Vec<UiString> {
    if is_translation_file(&path.to_lowercase()) {
        return translations(path, text);
    }
    component_strings(text)
}

/// Keys and values from a translation file, both sides indexed.
fn translations(path: &str, text: &str) -> Vec<UiString> {
    let locale = locale_of(path);
    let mut out = Vec::new();
    if path.to_lowercase().ends_with(".properties") {
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                push(&mut out, key.trim(), "i18n_key", None, i as u32 + 1);
                push(
                    &mut out,
                    value.trim(),
                    "i18n_value",
                    locale.clone(),
                    i as u32 + 1,
                );
            }
        }
        return out;
    }
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return out;
    };
    flatten(&json, &mut out, &locale);
    out
}

/// Nested translation objects are flattened: `{"cart": {"confirm": "Батлах"}}` indexes the
/// key `cart.confirm` and the value, because that is how the key appears in the source.
fn flatten(v: &Value, out: &mut Vec<UiString>, locale: &Option<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                push(out, k, "i18n_key", None, 0);
                flatten(child, out, locale);
            }
        }
        Value::Array(items) => {
            for child in items {
                flatten(child, out, locale);
            }
        }
        Value::String(s) => push(out, s, "i18n_value", locale.clone(), 0),
        _ => {}
    }
}

/// Strings a component shows: literals, `data-testid`, `aria-label`, `placeholder`.
fn component_strings(text: &str) -> Vec<UiString> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line_no = i as u32 + 1;
        for (attr, kind) in [
            ("data-testid", "test_id"),
            ("aria-label", "aria_label"),
            ("placeholder", "placeholder"),
        ] {
            if let Some(v) = attribute_value(line, attr) {
                push(&mut out, &v, kind, None, line_no);
            }
        }
        for literal in quoted(line) {
            push(&mut out, &literal, "literal", None, line_no);
        }
    }
    out
}

fn attribute_value(line: &str, attr: &str) -> Option<String> {
    let at = line.find(attr)?;
    let rest = &line[at + attr.len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    rest[1..].split(quote).next().map(str::to_string)
}

/// Quoted runs on a line, filtered to what could plausibly be shown to a person.
///
/// Import paths, class names and identifiers are quoted too, and indexing them would fill the
/// table with noise that matches every search. The filter is crude on purpose: it must be
/// explainable, because a string that fails it is a string an investigation cannot find.
fn quoted(line: &str) -> Vec<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("import ") || trimmed.starts_with("//") || trimmed.starts_with('*') {
        return Vec::new();
    }
    let mut out = Vec::new();
    for quote in ['"', '\''] {
        let parts: Vec<&str> = line.split(quote).collect();
        for (i, part) in parts.iter().enumerate() {
            if i % 2 == 1 && looks_visible(part) {
                out.push((*part).to_string());
            }
        }
    }
    out
}

fn looks_visible(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 2 || t.len() > 200 {
        return false;
    }
    // A path, an identifier or a MIME type is not something a person reads on a screen.
    if t.contains('/') || t.contains('\\') || t.starts_with('@') || t.starts_with('#') {
        return false;
    }
    // At least one letter, and a space or a capital — the shape of a phrase rather than a
    // token. `Confirm` and `Are you sure?` pass; `useState` and `px-4` do not.
    t.chars().any(char::is_alphabetic)
        && (t.contains(' ') || t.chars().next().is_some_and(char::is_uppercase))
        && !t
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
}

fn push(
    out: &mut Vec<UiString>,
    text: &str,
    kind: &'static str,
    locale: Option<String>,
    line: u32,
) {
    let text = text.trim();
    if text.is_empty() || text.len() > 400 {
        return;
    }
    let candidate = UiString {
        text: text.to_string(),
        kind,
        locale,
        line,
    };
    if !out.contains(&candidate) {
        out.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_translation_file_indexes_both_the_key_and_the_value() {
        // The screenshot may be in Mongolian while the source holds an English key. Indexing
        // keys alone makes a non-English interface unanchorable by text.
        let out = extract(
            "src/locales/mn/common.json",
            r#"{"cart": {"confirm": "Батлах"}}"#,
        );
        assert!(out
            .iter()
            .any(|s| s.text == "confirm" && s.kind == "i18n_key"));
        let value = out
            .iter()
            .find(|s| s.text == "Батлах")
            .expect("the value is indexed");
        assert_eq!(value.kind, "i18n_value");
        assert_eq!(value.locale.as_deref(), Some("mn"));
    }

    #[test]
    fn a_properties_file_works_too() {
        let out = extract(
            "src/i18n/messages_mn.properties",
            "cart.confirm=Батлах\n# a comment\n",
        );
        assert!(out.iter().any(|s| s.text == "cart.confirm"));
        let v = out.iter().find(|s| s.text == "Батлах").expect("value");
        assert_eq!(v.locale.as_deref(), Some("mn"));
    }

    #[test]
    fn a_locale_is_absent_rather_than_guessed() {
        // A wrong locale label is worse than none: retrieval would filter on it.
        assert_eq!(
            locale_of("src/locales/mn/common.json").as_deref(),
            Some("mn")
        );
        assert_eq!(
            locale_of("src/i18n/messages_en.properties").as_deref(),
            Some("en")
        );
        assert_eq!(locale_of("src/i18n/translations.json"), None);
    }

    #[test]
    fn a_component_yields_its_visible_strings_and_test_ids() {
        let out = extract(
            "src/Cart.tsx",
            "import x from './y';\n<button data-testid=\"confirm-btn\" aria-label=\"Confirm order\">Are you sure?</button>",
        );
        assert!(out
            .iter()
            .any(|s| s.text == "confirm-btn" && s.kind == "test_id"));
        assert!(out
            .iter()
            .any(|s| s.text == "Confirm order" && s.kind == "aria_label"));
    }

    #[test]
    fn identifiers_and_paths_are_not_screen_strings() {
        // A table full of import paths matches every search and helps nobody.
        let out = extract("src/Cart.tsx", "import { useState } from 'react';\nconst c = 'px-4 py-2';\nconst p = './utils/helpers';");
        assert!(
            out.iter().all(|s| !s.text.contains('/')),
            "{:?}",
            out.iter().map(|s| &s.text).collect::<Vec<_>>()
        );
        assert!(out.iter().all(|s| s.text != "react"));
    }

    #[test]
    fn only_files_that_could_hold_screen_strings_are_read() {
        assert!(is_candidate("src/Cart.tsx"));
        assert!(is_candidate("src/locales/mn/common.json"));
        assert!(!is_candidate("src/main.rs"));
        assert!(!is_candidate("node_modules/x/locales/en.json"));
    }
}
