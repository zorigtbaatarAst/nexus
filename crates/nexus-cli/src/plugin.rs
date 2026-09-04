//! Is the installed Claude Code plugin the same version as this binary?
//!
//! The binary and the plugin are separate artefacts that update separately, and when they
//! drift the symptom is silence. A 0.2.0 plugin advertises the MCP tools 0.2.0 had, so an
//! agent asking for `nexus_get_context` is told no such tool exists — while the CLI on the
//! same machine has had it for a release. Nothing reports the mismatch, because from either
//! side alone there is nothing wrong.
//!
//! `doctor` is where that gets said out loud. It is the same argument ADR-024 makes for
//! hook health: fail-open is correct, and correct fail-open needs a compensating check
//! somewhere, or a broken thing is indistinguishable from a quiet one.
//!
//! **Why the plugin's declared version and not the running server's tool count.** `doctor`
//! runs in the CLI. There is no MCP session in front of it to interrogate, and starting one
//! to count its tools would make a diagnostic command spawn a server. The manifest on disk
//! is what the agent will load next time, which is the thing worth checking.

use nexus_core::report::Check;
use std::path::{Path, PathBuf};

/// Where Claude Code keeps plugin manifests, in the order they are consulted.
fn manifest_paths(home: &Path) -> Vec<PathBuf> {
    let base = home.join(".claude").join("plugins");
    vec![
        base.join("marketplaces")
            .join("nexus")
            .join(".claude-plugin")
            .join("plugin.json"),
        base.join("cache")
            .join("nexus")
            .join(".claude-plugin")
            .join("plugin.json"),
    ]
}

/// The version the installed plugin declares, if one is installed and readable.
pub fn installed_version(home: &Path) -> Option<String> {
    for path in manifest_paths(home) {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if let Some(version) = v.get("version").and_then(|v| v.as_str()) {
            return Some(version.to_string());
        }
    }
    None
}

/// Compare the two, and say which of the three states this is.
///
/// Kept pure so the three states are testable without a home directory: agreement, skew, and
/// "no plugin here", which is a normal state and not a fault — the CLI is usable on its own.
pub fn skew(binary: &str, installed: Option<&str>) -> Check {
    match installed {
        // Not installed is not agreement. Saying "ok, versions match" when there is no
        // plugin would be the report inventing a fact it never checked.
        None => Check {
            name: "plugin",
            level: "ok",
            detail: "no Claude Code plugin installed".into(),
            remedy: None,
        },
        Some(v) if v == binary => Check {
            name: "plugin",
            level: "ok",
            detail: format!("Claude Code plugin {v} matches this binary"),
            remedy: None,
        },
        Some(v) => Check {
            name: "plugin",
            level: "warn",
            detail: format!(
                "Claude Code plugin is {v} but this binary is {binary}; the MCP tools an agent \
                 can call are the plugin's, not this binary's"
            ),
            remedy: Some("/plugin marketplace update nexus".into()),
        },
    }
}

/// The check as `doctor` runs it, against the real home directory.
pub fn health() -> Check {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let installed = home.as_deref().and_then(installed_version);
    skew(env!("CARGO_PKG_VERSION"), installed.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_versions_are_quiet() {
        let c = skew("0.3.0", Some("0.3.0"));
        assert_eq!(c.level, "ok");
        assert!(c.remedy.is_none());
    }

    #[test]
    fn a_stale_plugin_warns_and_says_how_to_fix_it() {
        // The case that cost a whole session: binary 0.3.0, plugin 0.2.0, and an agent told
        // that `nexus_get_context` does not exist.
        let c = skew("0.3.0", Some("0.2.0"));
        assert_eq!(c.level, "warn");
        assert!(
            c.detail.contains("0.2.0") && c.detail.contains("0.3.0"),
            "{}",
            c.detail
        );
        assert_eq!(
            c.remedy.as_deref(),
            Some("/plugin marketplace update nexus"),
            "the remedy must be the command that fixes it"
        );
    }

    #[test]
    fn a_plugin_ahead_of_the_binary_also_warns() {
        // Skew in either direction is skew. Installing the plugin and forgetting the binary
        // is the more likely order, since one is a slash command and the other is a build.
        assert_eq!(skew("0.2.0", Some("0.3.0")).level, "warn");
    }

    #[test]
    fn no_plugin_is_reported_as_no_plugin_not_as_agreement() {
        // "ok" here means "nothing to compare", and the detail has to say so: a report that
        // says versions match when it never found a version is worse than no report.
        let c = skew("0.3.0", None);
        assert_eq!(c.level, "ok");
        assert!(c.detail.contains("no Claude Code plugin"), "{}", c.detail);
    }

    #[test]
    fn a_manifest_without_a_version_is_not_a_version() {
        let dir = std::env::temp_dir().join(format!("nexus-plugin-{}", std::process::id()));
        let manifest = dir
            .join(".claude")
            .join("plugins")
            .join("marketplaces")
            .join("nexus")
            .join(".claude-plugin");
        std::fs::create_dir_all(&manifest).expect("mkdir");
        std::fs::write(manifest.join("plugin.json"), r#"{"name":"nexus"}"#).expect("write");

        assert_eq!(installed_version(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_declared_version_is_read_from_the_manifest() {
        let dir = std::env::temp_dir().join(format!("nexus-plugin-ok-{}", std::process::id()));
        let manifest = dir
            .join(".claude")
            .join("plugins")
            .join("marketplaces")
            .join("nexus")
            .join(".claude-plugin");
        std::fs::create_dir_all(&manifest).expect("mkdir");
        std::fs::write(
            manifest.join("plugin.json"),
            r#"{"name":"nexus","version":"0.2.0"}"#,
        )
        .expect("write");

        assert_eq!(installed_version(&dir).as_deref(), Some("0.2.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
