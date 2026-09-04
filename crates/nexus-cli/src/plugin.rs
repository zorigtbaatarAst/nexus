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
///
/// `config` is the configuration directory, which is `$CLAUDE_CONFIG_DIR` when set and
/// `$HOME/.claude` otherwise. Reading only the second reports "no plugin installed" on a
/// machine that has one, which is the failure this whole check exists to stop.
fn manifest_paths(config: &Path) -> Vec<PathBuf> {
    let base = config.join("plugins");
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

/// What a manifest search found. "Absent" and "unreadable" are different answers, and the
/// first version of this collapsed them — so a corrupt manifest was reported as no plugin at
/// all, which is the report inventing a fact it never checked.
pub enum Installed {
    /// No manifest anywhere it is meant to be. The CLI is usable alone; this is not a fault.
    None,
    Version(String),
    /// A manifest exists and could not be read, parsed, or did not declare a version.
    Unreadable(String),
}

/// The version the installed plugin declares.
pub fn installed_version(config: &Path) -> Installed {
    for path in manifest_paths(config) {
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            // Not being there is the ordinary case, and the only one that is silent.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Installed::Unreadable(format!("cannot read {}: {e}", path.display())),
        };
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return Installed::Unreadable(format!("{} is not valid JSON: {e}", path.display()))
            }
        };
        return match value.get("version").and_then(|v| v.as_str()) {
            Some(version) => Installed::Version(version.to_string()),
            None => Installed::Unreadable(format!("{} declares no version", path.display())),
        };
    }
    Installed::None
}

/// Compare the two, and say which of the three states this is.
///
/// Kept pure so the three states are testable without a home directory: agreement, skew, and
/// "no plugin here", which is a normal state and not a fault — the CLI is usable on its own.
pub fn skew(binary: &str, installed: &Installed) -> Check {
    match installed {
        // Not installed is not agreement. Saying "ok, versions match" when there is no
        // plugin would be the report inventing a fact it never checked.
        Installed::None => Check {
            name: "plugin",
            level: "ok",
            detail: "no Claude Code plugin installed".into(),
            remedy: None,
        },
        // Nor is unreadable. A manifest that is present and broken is a real problem and
        // used to be reported as the absence of a plugin.
        Installed::Unreadable(why) => Check {
            name: "plugin",
            level: "warn",
            detail: format!("cannot tell which plugin version is installed: {why}"),
            remedy: Some("/plugin marketplace update nexus".into()),
        },
        Installed::Version(v) if v == binary => Check {
            name: "plugin",
            level: "ok",
            detail: format!("Claude Code plugin {v} matches this binary"),
            remedy: None,
        },
        Installed::Version(v) => Check {
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

/// Claude Code's configuration directory: `$CLAUDE_CONFIG_DIR`, else `$HOME/.claude`.
fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude"))
}

/// The check as `doctor` runs it.
pub fn health() -> Check {
    match config_dir() {
        Some(dir) => skew(env!("CARGO_PKG_VERSION"), &installed_version(&dir)),
        // No HOME and no CLAUDE_CONFIG_DIR: nowhere to look, which is not the same as
        // nothing being there.
        None => Check {
            name: "plugin",
            level: "ok",
            detail: "no configuration directory to check for a plugin".into(),
            remedy: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(dir: &Path, body: &str) {
        let d = dir
            .join("plugins")
            .join("marketplaces")
            .join("nexus")
            .join(".claude-plugin");
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join("plugin.json"), body).expect("write");
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexus-plugin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }

    #[test]
    fn matching_versions_are_quiet() {
        let c = skew("0.3.0", &Installed::Version("0.3.0".into()));
        assert_eq!(c.level, "ok");
        assert!(c.remedy.is_none());
    }

    #[test]
    fn a_stale_plugin_warns_and_says_how_to_fix_it() {
        // The case that cost a whole session: binary 0.3.0, plugin 0.2.0, and an agent told
        // that `nexus_get_context` does not exist.
        let c = skew("0.3.0", &Installed::Version("0.2.0".into()));
        assert_eq!(c.level, "warn");
        assert!(
            c.detail.contains("0.2.0") && c.detail.contains("0.3.0"),
            "{}",
            c.detail
        );
        assert_eq!(
            c.remedy.as_deref(),
            Some("/plugin marketplace update nexus")
        );
    }

    #[test]
    fn a_plugin_ahead_of_the_binary_also_warns() {
        // Skew in either direction is skew. Installing the plugin and forgetting the binary
        // is the more likely order, since one is a slash command and the other is a build.
        assert_eq!(
            skew("0.2.0", &Installed::Version("0.3.0".into())).level,
            "warn"
        );
    }

    #[test]
    fn no_plugin_is_reported_as_no_plugin_not_as_agreement() {
        let c = skew("0.3.0", &Installed::None);
        assert_eq!(c.level, "ok");
        assert!(c.detail.contains("no Claude Code plugin"), "{}", c.detail);
    }

    #[test]
    fn a_manifest_that_cannot_be_read_is_not_the_absence_of_one() {
        // The first version of this returned None for unreadable, unparseable and
        // version-less manifests alike, so a corrupt install reported "no plugin installed"
        // — the check inventing a fact it never established. A review caught it.
        let c = skew(
            "0.3.0",
            &Installed::Unreadable("plugin.json is not valid JSON".into()),
        );
        assert_eq!(c.level, "warn");
        assert!(c.detail.contains("cannot tell"), "{}", c.detail);
    }

    #[test]
    fn a_manifest_without_a_version_is_unreadable_not_absent() {
        let dir = temp("noversion");
        manifest(&dir, r#"{"name":"nexus"}"#);
        assert!(matches!(installed_version(&dir), Installed::Unreadable(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_json_is_unreadable_not_absent() {
        let dir = temp("brokenjson");
        manifest(&dir, "{ this is not json");
        assert!(matches!(installed_version(&dir), Installed::Unreadable(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_declared_version_is_read_from_the_manifest() {
        let dir = temp("ok");
        manifest(&dir, r#"{"name":"nexus","version":"0.2.0"}"#);
        assert!(matches!(installed_version(&dir), Installed::Version(v) if v == "0.2.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_configuration_directory_has_no_plugin() {
        let dir = temp("empty");
        assert!(matches!(installed_version(&dir), Installed::None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
