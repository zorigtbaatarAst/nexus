//! What the supply costs on prompts that should produce nothing.
//!
//! The other half of the measurement in `nexus-core/tests/debug_supply.rs`, and the half that
//! stops the first one from flattering: a package that always contains the fix files but
//! re-sends the project profile on every "yes" can still be net negative. Supply quality
//! without overhead is not a measurement, it is an advertisement.
//!
//! Measured through the binary rather than the library on purpose. `--brief` is a rendering
//! decision and the renderer lives here; a core-side approximation would measure a different
//! thing and drift from it silently.
//!
//! ## Re-baselining
//!
//! ```text
//! NEXUS_REBASELINE=1 cargo test -p nexus-cli --test overhead
//! git diff crates/nexus-cli/tests/golden/overhead.json
//! ```
//!
//! Expect these numbers to move for reasons unrelated to selection — serialisation changes
//! shift byte counts. That is what re-baselining is for. A golden that never moves is one
//! nobody reads; a golden nobody reads is one that stops catching anything.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prompts that name no code, taken from a real session.
///
/// The last row is the one doing work. `amount` occurs 37 times in this fixture's Java
/// source, so it is ordinary English that the index has genuinely seen — which is the case
/// where accidental seeding would show up. An earlier draft used `"the cache key is wrong"`
/// and a review caught it: `cache` occurs **zero** times in `spring-payments`, so that row
/// pinned nothing at all while reading as though it pinned something. A prompt naming a word
/// the corpus does not contain is not a test of restraint.
///
/// Whatever this row costs is recorded rather than asserted. If it is zero, the seeder
/// declined a word it had seen; if it is not, the golden says what that costs.
const QUIET_PROMPTS: &[&str] = &[
    "yes",
    "park it, decision kept",
    "ask questions one by one",
    "thanks, that works",
    "the amount looks wrong on the receipt",
];

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Overhead {
    prompt: String,
    /// Bytes on stdout under `--brief`, exactly as the `UserPromptSubmit` hook would receive.
    /// Zero is the target: the session package already sent the profile.
    brief_bytes: usize,
    /// What the same prompt costs without the flag, so the saving is visible rather than
    /// asserted.
    plain_bytes: usize,
}

fn nexus() -> PathBuf {
    let mut p = std::env::current_exe().expect("test binary");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("nexus")
}

fn context(root: &Path, args: &[&str]) -> usize {
    let out = Command::new(nexus())
        .arg("context")
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run context");
    assert!(
        out.status.success(),
        "context failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout.len()
}

#[test]
fn a_prompt_that_names_no_code_costs_nothing() {
    let out_root = std::env::temp_dir().join(format!("nexus-overhead-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_root);
    std::fs::create_dir_all(&out_root).expect("mkdir");

    let specs = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(nexus_fixtures::DEFAULT_SPEC_DIR);
    let spec = nexus_fixtures::Spec::load(&specs.join("spring-payments")).expect("spec loads");
    let generated = nexus_fixtures::generate(
        &spec,
        &out_root,
        &nexus_fixtures::Options {
            force: true,
            emit_tasks: None,
        },
    )
    .expect("fixture generates");

    Command::new(nexus())
        .args(["scan", "--project"])
        .arg(&generated.repo)
        .output()
        .expect("scan");

    let got: Vec<Overhead> = QUIET_PROMPTS
        .iter()
        .map(|p| Overhead {
            prompt: (*p).to_string(),
            brief_bytes: context(&generated.repo, &["--task", p, "--brief"]),
            plain_bytes: context(&generated.repo, &["--task", p]),
        })
        .collect();

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("overhead.json");

    if std::env::var("NEXUS_REBASELINE").is_ok() {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let body = serde_json::to_string_pretty(&got).expect("serialize");
        std::fs::write(&path, format!("{body}\n")).expect("write golden");
        let _ = std::fs::remove_dir_all(&out_root);
        return;
    }

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no golden yet — record one:\n  NEXUS_REBASELINE=1 cargo test -p nexus-cli \
             --test overhead"
        )
    });
    let want: Vec<Overhead> = serde_json::from_str(&raw).expect("golden is valid JSON");
    let _ = std::fs::remove_dir_all(&out_root);

    assert_eq!(
        got, want,
        "the cost of saying nothing changed.\n\nIf that was deliberate:\n  \
         NEXUS_REBASELINE=1 cargo test -p nexus-cli --test overhead\n  \
         git diff crates/nexus-cli/tests/golden/overhead.json\n\n\
         A brief package growing past zero on a prompt that named no code is the regression \
         this exists to catch: it is paid on every prompt, by every session, for ever."
    );
}
