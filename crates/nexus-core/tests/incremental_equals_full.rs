//! `docs/testing-strategy.md` §4 calls this "the central invariant ... worth more than the
//! rest of the property suite combined", and it did not exist:
//!
//! ```text
//! full_scan(repo)  ≡  scan(commit_1) then rescan through to commit_N
//! ```
//!
//! It is the seam that guards every attempt to make the incremental path do less work. The
//! two cases below are the two halves of that guard — the work a rescan may skip, and the
//! work it may not.

use nexus_core::Engine;
use nexus_store::Store;
use std::fs;
use std::path::{Path, PathBuf};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

fn empty_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-inc-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    root
}

/// The normalized index: what the two paths must agree on, with the row ids and scan ids
/// that legitimately differ between them left out. The resolution histogram is the part
/// that matters here — an edge a rescan failed to resolve is not missing, it is sitting in
/// the index as `unresolved`, so only a comparison that counts *by resolution* sees it.
fn fingerprint(root: &Path) -> String {
    let store = Store::open(&root.join(".nexus/nexus.db")).expect("open store");
    let pid = store
        .project_id(&root.to_string_lossy())
        .expect("project id");
    let (files, symbols) = store.index_counts(pid).expect("counts");
    let mut by_res = store.edges_by_resolution(pid).expect("edges");
    by_res.sort();
    format!("files={files} symbols={symbols} edges={by_res:?}")
}

fn caller(body: &str) -> String {
    format!(
        r#"
package mn.app;

public class Caller {{
    private final Helper helper = new Helper();

    public String run() {{
{body}
        return helper.greet();
    }}
}}
"#
    )
}

const HELPER: &str = r#"
package mn.app;

public class Helper {
    public String greet() {
        return "hi";
    }
}
"#;

/// The case a rescan is allowed to take shortcuts on: a body-only edit adds no symbol,
/// renames none and removes none, so nothing that was unresolvable before can have become
/// resolvable. Whatever the incremental path skips here, the index must come out identical
/// to a repository scanned cold in that state.
#[test]
fn a_body_only_rescan_leaves_the_index_identical_to_a_full_scan() {
    let incremental = empty_root("body-inc");
    write(
        &incremental,
        "src/main/java/mn/app/Caller.java",
        &caller(""),
    );
    write(&incremental, "src/main/java/mn/app/Helper.java", HELPER);
    let (mut engine, _) =
        Engine::init(&incremental, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");

    let edit = "        int unused = 1 + 1;";
    write(
        &incremental,
        "src/main/java/mn/app/Caller.java",
        &caller(edit),
    );
    let report = engine.rescan().expect("rescan");
    assert_eq!(report.files_changed, 1, "the edit should be seen");

    // The same tree, scanned cold. This is the reference the incremental path must match.
    let full = empty_root("body-full");
    write(&full, "src/main/java/mn/app/Caller.java", &caller(edit));
    write(&full, "src/main/java/mn/app/Helper.java", HELPER);
    let (mut cold, _) = Engine::init(&full, nexus_lang_pack::default_registry()).expect("init");
    cold.scan().expect("scan");

    assert_eq!(
        fingerprint(&incremental),
        fingerprint(&full),
        "scan-then-rescan diverged from a cold scan of the same tree"
    );

    let _ = fs::remove_dir_all(&incremental);
    let _ = fs::remove_dir_all(&full);
}

/// The case a rescan may *not* take shortcuts on, and the reason `resolve_edges` re-runs
/// over the whole unresolved set: `Caller.java` is not touched by the second scan, yet the
/// symbol added in a different file is what makes its edge resolvable. A rescan that only
/// looked at edges belonging to changed files would leave this one unresolved forever.
#[test]
fn a_symbol_added_elsewhere_resolves_an_edge_in_a_file_that_did_not_change() {
    let root = empty_root("late-helper");
    write(&root, "src/main/java/mn/app/Caller.java", &caller(""));
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");

    let before = fingerprint(&root);

    // Helper appears. Caller is untouched — its bytes, its mtime and its hash are the same.
    write(&root, "src/main/java/mn/app/Helper.java", HELPER);
    let report = engine.rescan().expect("rescan");
    assert_eq!(
        report.files_changed, 1,
        "only Helper.java is new; Caller.java must not be re-parsed"
    );

    let after = fingerprint(&root);
    // Not merely "the fingerprint moved" — adding a file moves the counts on its own, so
    // that would pass with resolution untouched. The claim is specifically that edges in
    // the *unchanged* file stopped being unresolved.
    assert!(
        before.contains("unresolved"),
        "the fixture must start with Caller's edges unresolved: {before}"
    );
    assert!(
        !after.contains("unresolved"),
        "the symbol appeared, so Caller's edges must have resolved: {after}"
    );

    let full = empty_root("late-helper-full");
    write(&full, "src/main/java/mn/app/Caller.java", &caller(""));
    write(&full, "src/main/java/mn/app/Helper.java", HELPER);
    let (mut cold, _) = Engine::init(&full, nexus_lang_pack::default_registry()).expect("init");
    cold.scan().expect("scan");

    assert_eq!(
        after,
        fingerprint(&full),
        "the edge into Helper resolves on a cold scan but not through a rescan"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&full);
}
