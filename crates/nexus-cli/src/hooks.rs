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
/// `--brief` because this runs on every prompt. Without it the package repeats the project
/// profile the `SessionStart` hook already sent — 234-256 tokens a turn, which is half of a
/// small package and all of an empty one — and prints that header even when the prompt named
/// nothing and nothing was selected.
pub const USER_PROMPT_COMMAND: &str =
    "nexus context --task \"$CLAUDE_USER_PROMPT\" --budget 4000 --brief 2>/dev/null || true";

/// Keep the index warm after an edit (ADR-024). A no-op rescan is the fast path, so this is
/// the cheapest hook in the set and the one that makes the others cheap.
pub const POST_TOOL_USE_COMMAND: &str = "nexus rescan --quiet 2>/dev/null || true";

/// Which tools change files. `PostToolUse` fires for every tool the agent calls, so without
/// a matcher the index is rescanned after every `Read`, `Grep` and `Bash` — work that cannot
/// change the answer, on the developer's critical path. ADR-024's table always said
/// `PostToolUse (Edit|Write)`; the installer just never said it to the harness.
pub const EDIT_TOOLS: &str = "Edit|Write|MultiEdit|NotebookEdit";

/// The gate. "Done" gets checked before the turn ends.
///
/// It exits 0 whatever the verdict, because a hook's exit code is not the channel: the verdict
/// is on stdout, where the agent reads it. Making a failing gate fail the hook would stop the
/// turn rather than inform it.
///
/// **Not installed by `--hooks`.** `--changed` is currently accepted and ignored (`main.rs`
/// calls it "reserved for scoping a future run"), so this runs a *full* build and test of the
/// whole project at the end of every turn. On a Gradle project that is minutes. It is a real
/// gate and worth having deliberately, which is what `--hooks --verify` is for; it is not
/// worth acquiring as a side effect of turning on a context hook. When `--changed` scopes,
/// this moves back into the default set.
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
pub fn install(root: &Path, verify: bool) -> std::io::Result<Outcome> {
    let mut any = false;
    let mut set = vec![
        ("SessionStart", SESSION_START_COMMAND, TIMEOUT_SECONDS, None),
        (
            "UserPromptSubmit",
            USER_PROMPT_COMMAND,
            TIMEOUT_SECONDS,
            None,
        ),
        (
            "PostToolUse",
            POST_TOOL_USE_COMMAND,
            TIMEOUT_SECONDS,
            Some(EDIT_TOOLS),
        ),
    ];
    // The gate runs a real build and its scoping flag does nothing yet. Deliberate, or not
    // at all — see `STOP_COMMAND`.
    if verify {
        set.push(("Stop", STOP_COMMAND, VERIFY_TIMEOUT_SECONDS, None));
    }
    for (event, command, timeout, matcher) in set {
        if matches!(
            install_one(root, event, command, timeout, matcher)?,
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

fn install_one(
    root: &Path,
    event: &str,
    command: &str,
    timeout: u64,
    matcher: Option<&str>,
) -> std::io::Result<Outcome> {
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

    let mut entry = json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
        }]
    });
    // Omitted rather than written as "" for the events that have no tool to match on: an
    // empty matcher is a value, and the harness is entitled to read it as one.
    if let Some(m) = matcher {
        entry["matcher"] = json!(m);
    }
    entries.push(entry);

    std::fs::create_dir_all(&dir)?;
    let mut body = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    body.push('\n');
    std::fs::write(&path, body)?;
    Ok(Outcome::Installed)
}

/// What `doctor` reports about the hooks.
///
/// This lives in the CLI beside `install`, not in `nexus-core`, for the reason the module
/// doc gives: `.claude/settings.json` is one agent's format, and the core's analysis path
/// stays agent-agnostic. `Engine::doctor` returns a `Vec<Check>` and the CLI appends this
/// one, which keeps the format knowledge on this side of the boundary.
///
/// **Why this check has to exist at all.** Every hook command ends `2>/dev/null || true`, so
/// a hook that cannot run looks exactly like a hook that ran and found nothing. ADR-024
/// accepts that trade and names `doctor` as the compensating control; without this function
/// the control was documented and not built.
///
/// It checks presence and reachability rather than executing the hooks. Running them would
/// be a side effect — `nexus rescan` writes to the database — and the dominant field failure
/// is not a hook that errors but a hook whose binary is not on `PATH`, which presence alone
/// cannot see.
pub fn health(root: &Path) -> nexus_core::report::Check {
    let path = root.join(".claude").join("settings.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        // Not installed is a supported state, not a fault: ADR-024 ships them off.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return check(
                "ok",
                "not installed (hooks are opt-in)".to_string(),
                Some(format!("{} init --hooks", crate::render::binary_name())),
            )
        }
        Err(e) => {
            return check("warn", format!("cannot read {}: {e}", path.display()), None);
        }
    };
    let settings: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return check(
                "error",
                format!("{} is not valid JSON ({e})", path.display()),
                Some("fix or move the file; nexus will not overwrite it".into()),
            )
        }
    };

    let mut found = Vec::new();
    let mut missing = Vec::new();
    for (event, command) in [
        ("SessionStart", SESSION_START_COMMAND),
        ("UserPromptSubmit", USER_PROMPT_COMMAND),
        ("PostToolUse", POST_TOOL_USE_COMMAND),
    ] {
        if has_command(&settings, event, command) {
            found.push(event);
        } else {
            missing.push(event);
        }
    }
    let gate = has_command(&settings, "Stop", STOP_COMMAND);

    if found.is_empty() {
        return check(
            "ok",
            "not installed (hooks are opt-in)".to_string(),
            Some(format!("{} init --hooks", crate::render::binary_name())),
        );
    }

    // The failure fail-open is built to hide: the hook fires, the shell cannot find `nexus`,
    // `|| true` swallows it, and the session looks normal forever.
    if !on_path() {
        return check(
            "error",
            format!(
                "{} hook(s) installed but `nexus` is not on PATH — every one silently does nothing",
                found.len()
            ),
            Some(
                "install nexus to a directory on PATH, or edit the commands to an absolute path"
                    .into(),
            ),
        );
    }

    let mut detail = format!("{} installed", found.join(", "));
    if gate {
        detail.push_str(", Stop (runs a full build each turn)");
    }
    if missing.is_empty() {
        check("ok", detail, None)
    } else {
        check(
            "warn",
            format!("{detail}; {} not installed", missing.join(", ")),
            Some(format!("{} init --hooks", crate::render::binary_name())),
        )
    }
}

fn check(level: &'static str, detail: String, remedy: Option<String>) -> nexus_core::report::Check {
    nexus_core::report::Check {
        name: "hooks",
        level,
        detail,
        remedy,
    }
}

fn has_command(settings: &Value, event: &str, command: &str) -> bool {
    settings["hooks"][event].as_array().is_some_and(|entries| {
        entries.iter().any(|e| {
            e["hooks"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(|h| h["command"] == json!(command)))
        })
    })
}

/// Is `nexus` resolvable the way a hook's shell would resolve it?
fn on_path() -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join("nexus").is_file())
}
