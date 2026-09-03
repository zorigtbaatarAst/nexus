//! Installing the deterministic invocation tier (ADR-024).
//!
//! This lives in the CLI, not in `nexus-core`, and that is deliberate. `.claude/settings.json`
//! is one agent's format; `07-agent-integration.md` §3 says adding an agent is a shim and
//! never a change under the core. Keeping the format here means the binary's analysis path
//! stays agent-agnostic, which is the property the boundary tests exist to protect.
//!
//! A hook contains no logic. It is one command and one timeout, so a hook regression costs
//! the automatic path and nothing else.

use serde_json::{json, Map, Value};
use std::path::Path;

/// The `SessionStart` command, budget 800 tokens (ADR-024).
///
/// Fail-open is in the string itself rather than in a wrapper script: `|| true` survives a
/// missing binary, a missing baseline (exit 5) and any runtime error, and `2>/dev/null`
/// keeps a diagnostic from being read as context. Removing `nexus` from `PATH` mid-session
/// must leave the harness fully working, and a test asserts that this exact string does.
pub const SESSION_START_COMMAND: &str = "nexus context --session --budget 800 2>/dev/null || true";

/// The `UserPromptSubmit` command, budget 4000 tokens (ADR-024).
///
/// This is the one the design calls the product: the context package for the task the
/// developer just described. It is also the one on the critical path of every prompt, which
/// is why it is opt-in and why its latency is measured before anyone is asked to enable it.
///
/// The prompt arrives on stdin as JSON, and a hook that tried to parse it would be logic in a
/// hook — the thing ADR-024 forbids, because then a hook regression costs more than the
/// automatic path. The harness substitutes the variable; if it is empty the command still
/// exits 0 and Nexus reports that it anchored nothing.
pub const USER_PROMPT_COMMAND: &str =
    "nexus context --task \"$CLAUDE_USER_PROMPT\" --budget 4000 2>/dev/null || true";

/// Keep the index warm after an edit (ADR-024). A no-op rescan is the fast path, so this is
/// the cheapest hook in the set and the one that makes the others cheap.
pub const POST_TOOL_USE_COMMAND: &str = "nexus rescan --quiet 2>/dev/null || true";

/// The gate. "Done" gets checked before the turn ends.
///
/// It exits 0 whatever the verdict, because a hook's exit code is not the channel: the verdict
/// is on stdout, where the agent reads it. Making a failing gate fail the hook would stop the
/// turn rather than inform it.
pub const STOP_COMMAND: &str = "nexus verify --changed 2>/dev/null || true";

/// Seconds. A ceiling, not a target: the budgets are 400 ms and 150 ms for the context hooks.
const TIMEOUT_SECONDS: u64 = 5;

/// The gate runs a real build. Its budget is seconds, not milliseconds.
const VERIFY_TIMEOUT_SECONDS: u64 = 600;

pub enum Outcome {
    Installed,
    AlreadyPresent,
}

/// Add the `SessionStart` hook to `<root>/.claude/settings.json`, preserving everything else.
///
/// Never clobbers: an existing file is parsed, added to, and written back. A file that is not
/// valid JSON is an error rather than a thing to overwrite — someone's configuration is not
/// ours to discard because we could not read it.
pub fn install(root: &Path) -> std::io::Result<Outcome> {
    let mut any = false;
    for (event, command, timeout) in [
        ("SessionStart", SESSION_START_COMMAND, TIMEOUT_SECONDS),
        ("UserPromptSubmit", USER_PROMPT_COMMAND, TIMEOUT_SECONDS),
        ("PostToolUse", POST_TOOL_USE_COMMAND, TIMEOUT_SECONDS),
        ("Stop", STOP_COMMAND, VERIFY_TIMEOUT_SECONDS),
    ] {
        if matches!(
            install_one(root, event, command, timeout)?,
            Outcome::Installed
        ) {
            any = true;
        }
    }
    Ok(if any {
        Outcome::Installed
    } else {
        Outcome::AlreadyPresent
    })
}

fn install_one(root: &Path, event: &str, command: &str, timeout: u64) -> std::io::Result<Outcome> {
    let dir = root.join(".claude");
    let path = dir.join("settings.json");

    let mut settings: Value = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} is not valid JSON ({e}) — fix or move it first",
                    path.display()
                ),
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(e),
    };

    let not_an_object = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} in {} is not the expected shape", what, path.display()),
        )
    };

    let entries = settings
        .as_object_mut()
        .ok_or_else(|| not_an_object("the top level"))?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| not_an_object("hooks"))?
        .entry(event.to_string())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| not_an_object(event))?;

    // Idempotent on the command, not on exact equality: someone may have edited the timeout,
    // and rewriting their choice is not what "install" was asked to do.
    let present = entries.iter().any(|e| {
        e["hooks"]
            .as_array()
            .is_some_and(|hs| hs.iter().any(|h| h["command"] == json!(command)))
    });
    if present {
        return Ok(Outcome::AlreadyPresent);
    }

    entries.push(json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
        }]
    }));

    std::fs::create_dir_all(&dir)?;
    let mut body = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    body.push('\n');
    std::fs::write(&path, body)?;
    Ok(Outcome::Installed)
}
