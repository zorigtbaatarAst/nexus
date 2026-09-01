//! Scaffolding a project of this shape needs and does not have.
//!
//! Only one rule so far, and deliberately: every candidate for this family is a judgement
//! about how someone should work, and most of them are the taste the review non-goal exists
//! to keep out. Continuous integration survives that test because its absence has a
//! consequence this tool can state precisely — nothing runs the tests except a person who
//! remembers to.

use super::{split_evidence, Rule};
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::project::{ProjectContext, Scoped};
use nexus_types::{FindingType, Severity};

/// Directories whose presence means some CI system is configured. Matched as a path prefix,
/// because each of these holds the workflow files rather than being one.
const CI_MARKERS: &[&str] = &[
    ".github/workflows/",
    ".gitlab-ci.yml",
    ".circleci/",
    "Jenkinsfile",
    ".drone.yml",
    "azure-pipelines.yml",
    ".woodpecker.yml",
    "bitbucket-pipelines.yml",
];

pub struct NoContinuousIntegration;

impl Rule for NoContinuousIntegration {
    fn id(&self) -> &'static str {
        "architect:no-ci"
    }

    fn describe(&self) -> &'static str {
        "no continuous integration is configured, so nothing runs the tests but a person"
    }

    fn run(&self, ctx: &ProjectContext<'_>, _scoped: &Scoped<'_>) -> Vec<Finding> {
        let Some(profile) = ctx.profile else {
            return Vec::new();
        };
        // A project with no build system has nothing to run in CI, and telling it to set CI
        // up would be noise about a directory of scripts.
        let Some(build_system) = profile.build_system.as_deref() else {
            return Vec::new();
        };
        if ctx
            .files
            .iter()
            .any(|f| CI_MARKERS.iter().any(|m| f.path.starts_with(m)))
        {
            return Vec::new();
        }

        // The finding is about something absent, so it anchors on the file that would have
        // driven it. If there is no such file in the index there is nowhere honest to point,
        // and a guess at `README.md` would be evidence naming a file that may not exist —
        // worse than saying nothing. ADR-021.
        let Some(anchor) = build_file(ctx) else {
            return Vec::new();
        };
        let (file, line) = split_evidence(&anchor);

        vec![Finding {
            finding_type: FindingType::Architecture,
            title: format!("no CI is configured for this {build_system} project"),
            component: "build".into(),
            anchor_fqn: None,
            severity: Severity::Medium,
            confidence: 0.85,
            detector: self.id().to_string(),
            structural_key: "no-ci".into(),
            slug: "no-continuous-integration".into(),
            evidence: vec![CodeRef {
                file,
                line,
                note: format!(
                    "this project builds with {build_system} and no CI configuration was \
                     found, so nothing runs its tests except a person who remembers to. \
                     Searched: {}",
                    CI_MARKERS.join(", ")
                ),
            }],
            capability_data: Some(serde_json::json!({
                "kind": "missing_scaffolding",
                "what": "continuous_integration",
                "build_system": build_system,
                "searched": CI_MARKERS,
            })),
        }]
    }
}

/// The build file to hang the finding on: the thing CI would have invoked.
fn build_file(ctx: &ProjectContext<'_>) -> Option<String> {
    const BUILD_FILES: &[&str] = &[
        "build.gradle",
        "build.gradle.kts",
        "pom.xml",
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "Makefile",
    ];
    ctx.files
        .iter()
        .find(|f| BUILD_FILES.contains(&f.path.as_str()))
        .map(|f| f.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::capability::Scope;
    use nexus_core::project::{EdgeFacts, FileFacts, SymbolFacts};
    use nexus_core::report::Profile;
    use std::path::Path;

    fn profile(build: Option<&str>) -> Profile {
        Profile {
            name: "p".into(),
            languages: Vec::new(),
            frameworks: Vec::new(),
            build_system: build.map(str::to_string),
            package_manager: None,
            databases: Vec::new(),
            containers: Vec::new(),
            vcs: "git".into(),
        }
    }

    fn run(files: &[&str], p: &Profile) -> Vec<Finding> {
        let symbols: Vec<SymbolFacts> = Vec::new();
        let edges: Vec<EdgeFacts> = Vec::new();
        let files: Vec<FileFacts> = files
            .iter()
            .map(|p| FileFacts {
                path: (*p).into(),
                lang: None,
            })
            .collect();
        let ctx =
            ProjectContext::new(Path::new("/"), &symbols, &edges, &files).with_profile(Some(p));
        let scoped = ctx.scoped(&Scope::Everything);
        NoContinuousIntegration.run(&ctx, &scoped)
    }

    #[test]
    fn an_absence_anchors_on_the_file_that_would_have_driven_it() {
        let found = run(
            &["build.gradle", "src/main/java/A.java"],
            &profile(Some("gradle")),
        );
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].evidence[0].file, "build.gradle");
        assert!(
            !found[0].evidence.is_empty(),
            "an advisory still needs evidence"
        );
    }

    #[test]
    fn configured_ci_silences_it() {
        let found = run(
            &["build.gradle", ".github/workflows/ci.yml"],
            &profile(Some("gradle")),
        );
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn no_build_file_in_the_index_means_nowhere_to_point() {
        // A build system was detected but its file is not indexed — before the first scan,
        // for instance. Guessing at a filename would be evidence naming a file that may
        // not exist.
        let found = run(&["src/main/java/A.java"], &profile(Some("gradle")));
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_project_with_nothing_to_build_is_left_alone() {
        // Telling a directory of scripts to set up CI is noise.
        let found = run(&["notes.md"], &profile(None));
        assert!(found.is_empty(), "{found:#?}");
    }
}
