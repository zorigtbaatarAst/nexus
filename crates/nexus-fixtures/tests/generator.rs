//! Generator behaviour, against synthetic specifications written into a temp directory.
//!
//! Synthetic rather than shipped, so a failure here names a mechanism rather than a fixture:
//! `corpus.rs` is what proves the real corpus builds.

use nexus_fixtures::spec::Spec;
use nexus_fixtures::{generate, Options};
use std::path::{Path, PathBuf};

/// A minimal spec directory. `extra` is appended to the manifest.
fn spec_dir(dir: &Path, extra: &str, blobs: &[(&str, &str)]) -> PathBuf {
    let root = dir.join("spec");
    std::fs::create_dir_all(root.join("blobs")).expect("blobs dir");
    for (name, content) in blobs {
        std::fs::write(root.join("blobs").join(name), content).expect("blob");
    }
    let manifest = format!(
        r#"
[fixture]
name = "t"
description = "synthetic"
base_epoch = 1700000000

[author]
name = "T"
email = "t@example.invalid"
{extra}
"#
    );
    std::fs::write(root.join("fixture.toml"), manifest).expect("manifest");
    root
}

fn shas(g: &nexus_fixtures::Manifest) -> Vec<String> {
    g.commits.iter().map(|c| c.sha.clone()).collect()
}

fn read(repo: &Path, rel: &str) -> String {
    std::fs::read_to_string(repo.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_same_spec_produces_the_same_shas_twice() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = spec_dir(
        tmp.path(),
        r#"
[[commit]]
id = "c1"
message = "one"
write = [{ path = "a.txt", blob = "a" }]

[[commit]]
id = "c2"
message = "two"
write = [{ path = "b.txt", content = "inline\n" }]
"#,
        &[("a", "hello\n")],
    );
    let spec = Spec::load(&spec).expect("spec loads");

    let a = generate(&spec, &tmp.path().join("out-a"), &Options::default()).expect("first");
    let b = generate(&spec, &tmp.path().join("out-b"), &Options::default()).expect("second");

    assert_eq!(
        shas(&a.manifest),
        shas(&b.manifest),
        "two runs of one spec must agree on every sha, or the corpus measures itself"
    );
    assert_eq!(a.manifest.spec_digest, b.manifest.spec_digest);
    assert_eq!(a.manifest.commits.len(), 2);
    // Derived from base_epoch, never from a clock.
    assert_eq!(a.manifest.commits[0].timestamp, 1_700_000_000);
    assert_eq!(a.manifest.commits[1].timestamp, 1_700_000_000 + 86_400);
}

#[test]
fn generating_over_an_existing_fixture_needs_force() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = spec_dir(
        tmp.path(),
        r#"
[[commit]]
id = "c1"
message = "one"
write = [{ path = "a.txt", content = "x\n" }]
"#,
        &[],
    );
    let spec = Spec::load(&spec).expect("spec loads");
    let out = tmp.path().join("out");

    let first = generate(&spec, &out, &Options::default()).expect("first");
    let again = generate(&spec, &out, &Options::default());
    assert!(
        again.is_err(),
        "a populated output directory is not silently replaced"
    );

    let forced = generate(
        &spec,
        &out,
        &Options {
            force: true,
            ..Default::default()
        },
    )
    .expect("forced");
    assert_eq!(
        shas(&first.manifest),
        shas(&forced.manifest),
        "regeneration is idempotent: --force rebuilds the same history"
    );
}

#[test]
fn write_move_substitute_transform_and_delete_all_take_effect() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = spec_dir(
        tmp.path(),
        r#"
[[commit]]
id = "c1"
message = "seed"
write = [
  { path = "src/a.java", blob = "a" },
  { path = "src/gone.java", content = "delete me\n" },
  { path = "notes.txt", content = "package mn.pay;\n" },
]

[[commit]]
id = "c2"
message = "everything at once"
move = [{ from = "src/a.java", to = "src/moved/a.java" }]
substitute = [{ extensions = ["java"], find = "mn.pay", replace = "mn.payments" }]
transform = [{ kind = "double-indent", extensions = ["java"] }]
delete = [{ path = "src/gone.java" }]
"#,
        &[("a", "package mn.pay;\n    class A {}\n")],
    );
    let spec = Spec::load(&spec).expect("spec loads");
    let out = tmp.path().join("out");
    let g = generate(&spec, &out, &Options::default()).expect("generated");
    let repo = &g.repo;

    assert!(
        !repo.join("src/a.java").exists(),
        "the source of a move is gone"
    );
    assert!(
        !repo.join("src/gone.java").exists(),
        "a deleted file is gone"
    );
    let moved = read(repo, "src/moved/a.java");
    assert!(
        moved.contains("mn.payments"),
        "substitute reached the moved file"
    );
    assert!(
        moved.contains("        class A {}"),
        "transform doubled the indent: {moved:?}"
    );
    assert_eq!(
        read(repo, "notes.txt"),
        "package mn.pay;\n",
        "a .txt file is outside an extensions = [\"java\"] selector"
    );
    // Ordering is fixed, not declaration-order: move ran before substitute, so the
    // substitution found the file at its new path.
    assert_eq!(
        g.manifest.commits[1].files,
        vec!["notes.txt", "src/moved/a.java"]
    );
}

#[test]
fn a_branch_forks_from_the_commit_it_names() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = spec_dir(
        tmp.path(),
        r#"
[[branch]]
name = "side"
from = "c1"

[[commit]]
id = "c1"
message = "base"
write = [{ path = "a.txt", content = "a\n" }]

[[commit]]
id = "c2"
message = "on main"
write = [{ path = "main-only.txt", content = "m\n" }]

[[commit]]
id = "c3"
message = "on side"
branch = "side"
write = [{ path = "side-only.txt", content = "s\n" }]
"#,
        &[],
    );
    let spec = Spec::load(&spec).expect("spec loads");
    let out = tmp.path().join("out");
    let g = generate(&spec, &out, &Options::default()).expect("generated");

    let by = |id: &str| {
        g.manifest
            .commits
            .iter()
            .find(|c| c.id == id)
            .expect("commit")
    };
    assert_eq!(by("c3").branch, "side");
    assert!(
        !by("c3").files.contains(&"main-only.txt".to_string()),
        "the side branch forked from c1, so c2's file must not be on it"
    );
    // The fixture is left on its default branch, whatever the last commit was on.
    assert!(g.repo.join("main-only.txt").exists());
    assert!(
        !g.repo.join("side-only.txt").exists(),
        "checking out main must remove a file that only exists on the side branch"
    );
}

#[test]
fn a_patch_that_does_not_apply_fails_generation() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = spec_dir(
        tmp.path(),
        r#"
[[commit]]
id = "c1"
message = "base"
write = [{ path = "a.txt", content = "actual content\n" }]

[[patch]]
id = "rotten"
blob = "rotten.patch"
base = "c1"
"#,
        &[(
            "rotten.patch",
            "diff --git a/a.txt b/a.txt\n\
             --- a/a.txt\n\
             +++ b/a.txt\n\
             @@ -1 +1 @@\n\
             -something else entirely\n\
             +replacement\n",
        )],
    );
    let spec = Spec::load(&spec).expect("spec loads");
    let err = generate(&spec, &tmp.path().join("out"), &Options::default())
        .expect_err("a patch that no longer applies must stop the build");
    let msg = err.to_string();
    assert!(msg.contains("rotten"), "the error names the patch: {msg}");
    assert!(
        msg.contains("does not apply"),
        "and says what went wrong: {msg}"
    );
}

#[test]
fn a_path_that_escapes_the_repository_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    for (n, path) in ["../escape.txt", "/etc/passwd", ".git/config"]
        .into_iter()
        .enumerate()
    {
        let spec = spec_dir(
            tmp.path(),
            &format!(
                r#"
[[commit]]
id = "c1"
message = "hostile"
write = [{{ path = "{path}", content = "x\n" }}]
"#
            ),
            &[],
        );
        let spec = Spec::load(&spec).expect("spec loads");
        let err = generate(
            &spec,
            &tmp.path().join(format!("out{n}")),
            &Options::default(),
        )
        .expect_err("must refuse an escaping path");
        assert!(
            err.to_string().contains("unsafe path"),
            "{path} should be refused as unsafe, got: {err}"
        );
    }
}

#[test]
fn generation_refuses_to_write_over_a_source_tree() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = spec_dir(
        tmp.path(),
        r#"
[[commit]]
id = "c1"
message = "one"
write = [{ path = "a.txt", content = "x\n" }]
"#,
        &[],
    );
    let spec = Spec::load(&spec).expect("spec loads");
    let out = tmp.path().join("out");
    // A mistyped --out pointing at somebody's checkout.
    std::fs::create_dir_all(out.join("t")).expect("dir");
    std::fs::write(out.join("t").join("Cargo.toml"), "[package]\n").expect("marker");

    let err = generate(
        &spec,
        &out,
        &Options {
            force: true,
            ..Default::default()
        },
    )
    .expect_err("a Cargo.toml at the target is a stop sign, force or not");
    assert!(err.to_string().contains("refusing to generate"), "{err}");
}

#[test]
fn tasks_are_emitted_with_their_commit_resolved_to_a_sha() {
    let tmp = tempfile::tempdir().expect("tmp");
    let spec = spec_dir(
        tmp.path(),
        r#"
[[commit]]
id = "c1"
message = "one"
write = [{ path = "a.txt", content = "x\n" }]

[[patch]]
id = "wip"
blob = "wip.patch"
base = "c1"

[[task]]
id = "T1"
family = "A"
commit = "c1"
start_state = { dirty = "wip" }
prompt = "do the thing"
required_sites = ["a.txt"]
"#,
        &[(
            "wip.patch",
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n x\n+wip\n",
        )],
    );
    let spec = Spec::load(&spec).expect("spec loads");
    let out = tmp.path().join("out");
    let tasks = tmp.path().join("tasks");
    let g = generate(
        &spec,
        &out,
        &Options {
            emit_tasks: Some(tasks.clone()),
            ..Default::default()
        },
    )
    .expect("generated");

    let t = &g.manifest.tasks[0];
    assert_eq!(t.commit_id, "c1");
    assert_eq!(
        t.commit, g.manifest.commits[0].sha,
        "the logical id is resolved"
    );
    assert_eq!(
        t.start_state, "dirty:wip",
        "the §3 wire form, not a Rust enum"
    );

    let emitted = std::fs::read_to_string(tasks.join("T1.toml")).expect("task file");
    assert!(
        emitted.contains(&g.manifest.commits[0].sha),
        "the sha is pinned in the file"
    );
    assert!(
        emitted.starts_with("# Generated by"),
        "and it says not to edit it"
    );

    // The patch was proved against the tree it claims to apply to.
    assert!(g.manifest.patches[0].verified);
    assert!(out.join("t.patches/wip.patch").is_file());
}
