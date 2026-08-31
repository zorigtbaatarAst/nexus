//! Response budgeting.
//!
//! An agent's context is the scarcest resource in the system. A tool that returns two
//! megabytes of JSON has not helped it — it has destroyed its ability to think about
//! anything else.
//!
//! Truncation is measured on the serialized payload rather than guessed from item counts,
//! and it is never silent: the result carries the true total and a concrete way to narrow.
//! An agent that does not know it received a partial answer will reason confidently from it.

use serde::Serialize;
use serde_json::{json, Value};

/// Roughly 8k tokens at ~3.5 bytes per token. The exact figure matters less than the
/// existence of a ceiling that cannot be widened by a caller.
pub const MAX_BYTES: usize = 28_000;

/// Serialize a value, trimming the named array until the whole payload fits.
///
/// `narrow_with` is advice the agent can act on, which is the difference between a useful
/// truncation notice and an apology.
pub fn fit<T: Serialize>(value: &T, list_field: &str, narrow_with: &str) -> Value {
    let Ok(mut root) = serde_json::to_value(value) else {
        return json!({"status": "error", "kind": "serialize_failed"});
    };
    if measure(&root) <= MAX_BYTES {
        return root;
    }

    let Some(items) = root.get(list_field).and_then(Value::as_array).cloned() else {
        // Nothing to trim: say so rather than returning an oversized payload silently.
        return json!({
            "status": "error",
            "kind": "response_too_large",
            "bytes": measure(&root),
            "hint": narrow_with,
        });
    };

    let total = items.len();
    let mut keep = total;
    while keep > 0 {
        keep = keep * 3 / 4;
        let trimmed: Vec<Value> = items.iter().take(keep).cloned().collect();
        root[list_field] = Value::Array(trimmed);
        if measure(&root) <= MAX_BYTES {
            break;
        }
    }

    root["truncated"] = json!(true);
    root["total"] = json!(total);
    root["showing"] = json!(keep);
    root["note"] = json!(format!(
        "showing the {keep} highest-ranked of {total}. {narrow_with}"
    ));
    root
}

fn measure(v: &Value) -> usize {
    serde_json::to_string(v)
        .map(|s| s.len())
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Page {
        items: Vec<String>,
    }

    #[test]
    fn a_small_payload_passes_through_untouched() {
        let p = Page {
            items: vec!["a".into(), "b".into()],
        };
        let v = fit(&p, "items", "use --limit");
        assert!(v.get("truncated").is_none());
        assert_eq!(v["items"].as_array().expect("array").len(), 2);
    }

    #[test]
    fn a_large_payload_is_trimmed_and_says_so() {
        let p = Page {
            items: (0..20_000).map(|i| format!("symbol-number-{i}")).collect(),
        };
        let v = fit(&p, "items", "narrow with min_score");
        assert_eq!(v["truncated"], json!(true));
        assert_eq!(v["total"], json!(20_000));
        assert!(v["showing"].as_u64().expect("showing") < 20_000);
        assert!(serde_json::to_string(&v).expect("json").len() <= MAX_BYTES);
        // The notice has to be actionable, not just present.
        assert!(v["note"]
            .as_str()
            .expect("note")
            .contains("narrow with min_score"));
    }

    #[test]
    fn an_untrimmable_oversized_payload_reports_rather_than_returns() {
        #[derive(Serialize)]
        struct Blob {
            text: String,
        }
        let b = Blob {
            text: "x".repeat(MAX_BYTES * 2),
        };
        let v = fit(&b, "items", "ask for less");
        assert_eq!(v["kind"], json!("response_too_large"));
    }
}
