//! Project detection: language, framework, build system, databases, containers.
//!
//! Everything here is evidence-based — a detection carries the file and line that produced
//! it, so `bughunter status` can be argued with rather than merely believed.

use crate::report::{Detected, Framework, LanguageShare, Profile};
use std::collections::BTreeMap;
use std::path::Path;

pub struct Detector<'a> {
    pub root: &'a Path,
    pub paths: &'a [String],
}

impl<'a> Detector<'a> {
    pub fn run(&self, name: String, vcs: &str, analyzed: &[&str]) -> Profile {
        Profile {
            name,
            languages: self.languages(analyzed),
            frameworks: self.frameworks(),
            build_system: self.build_system(),
            package_manager: self.package_manager(),
            databases: self.databases(),
            containers: self.containers(),
            vcs: vcs.to_string(),
        }
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok()
    }

    fn has(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    fn languages(&self, analyzed: &[&str]) -> Vec<LanguageShare> {
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for p in self.paths {
            let Some(ext) = p.rsplit('.').next() else {
                continue;
            };
            let lang = match ext {
                "java" => "java",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" | "mjs" => "javascript",
                "py" => "python",
                "rs" => "rust",
                "go" => "go",
                "kt" => "kotlin",
                _ => continue,
            };
            *counts.entry(lang).or_default() += 1;
        }
        let mut v: Vec<LanguageShare> = counts
            .into_iter()
            .map(|(lang, files)| LanguageShare {
                analyzed: analyzed.contains(&lang),
                lang: lang.to_string(),
                files,
            })
            .collect();
        v.sort_by_key(|l| std::cmp::Reverse(l.files));
        v
    }

    fn frameworks(&self) -> Vec<Framework> {
        let mut out = Vec::new();
        let build_files = [
            "build.gradle",
            "build.gradle.kts",
            "pom.xml",
            "package.json",
            "pyproject.toml",
            "requirements.txt",
            "Cargo.toml",
        ];
        // (marker, framework name) — checked against every build file that exists.
        let markers = [
            ("spring-boot", "spring-boot"),
            ("spring-cloud", "spring-cloud"),
            ("quarkus", "quarkus"),
            ("micronaut", "micronaut"),
            ("\"next\"", "next.js"),
            ("\"react\"", "react"),
            ("@nestjs/core", "nestjs"),
            ("\"vue\"", "vue"),
            ("@angular/core", "angular"),
            ("fastapi", "fastapi"),
            ("django", "django"),
            ("flask", "flask"),
            ("axum", "axum"),
            ("actix-web", "actix-web"),
        ];
        for bf in build_files {
            let Some(content) = self.read(bf) else {
                continue;
            };
            let lower = content.to_lowercase();
            for (marker, name) in markers {
                if !lower.contains(marker) {
                    continue;
                }
                if out.iter().any(|f: &Framework| f.name == name) {
                    continue;
                }
                let (line_no, version) = find_line(&content, marker);
                out.push(Framework {
                    name: name.to_string(),
                    version,
                    evidence: format!("{bf}:{line_no}"),
                });
            }
        }
        out
    }

    fn build_system(&self) -> Option<String> {
        if self.has("gradlew") || self.has("build.gradle") || self.has("build.gradle.kts") {
            Some("gradle".into())
        } else if self.has("pom.xml") {
            Some("maven".into())
        } else if self.has("Cargo.toml") {
            Some("cargo".into())
        } else if self.has("package.json") {
            Some("npm".into())
        } else if self.has("pyproject.toml") {
            Some("poetry".into())
        } else {
            None
        }
    }

    fn package_manager(&self) -> Option<String> {
        for (f, pm) in [
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("bun.lockb", "bun"),
            ("package-lock.json", "npm"),
            ("poetry.lock", "poetry"),
            ("uv.lock", "uv"),
            ("Cargo.lock", "cargo"),
        ] {
            if self.has(f) {
                return Some(pm.to_string());
            }
        }
        self.build_system()
    }

    fn databases(&self) -> Vec<Detected> {
        let mut out = Vec::new();
        let candidates: Vec<String> = self
            .paths
            .iter()
            .filter(|p| {
                let l = p.to_lowercase();
                l.contains("docker-compose")
                    || l.ends_with("compose.yml")
                    || l.ends_with("compose.yaml")
                    || l.ends_with("application.yml")
                    || l.ends_with("application.yaml")
                    || l.ends_with("application.properties")
                    || l.ends_with(".env.example")
            })
            .cloned()
            .collect();

        for rel in candidates {
            let Some(content) = self.read(&rel) else {
                continue;
            };
            let lower = content.to_lowercase();
            for (marker, kind) in [
                ("mongo", "mongodb"),
                ("postgres", "postgresql"),
                ("mysql", "mysql"),
                ("mariadb", "mariadb"),
                ("redis", "redis"),
                ("elasticsearch", "elasticsearch"),
                ("clickhouse", "clickhouse"),
            ] {
                if lower.contains(marker) && !out.iter().any(|d: &Detected| d.kind == kind) {
                    let (line_no, _) = find_line(&content, marker);
                    out.push(Detected {
                        kind: kind.to_string(),
                        evidence: format!("{rel}:{line_no}"),
                    });
                }
            }
        }
        out
    }

    fn containers(&self) -> Vec<String> {
        self.paths
            .iter()
            .filter(|p| {
                let l = p.to_lowercase();
                let base = l.rsplit('/').next().unwrap_or(&l).to_string();
                base.starts_with("dockerfile")
                    || base.starts_with("docker-compose")
                    || base == "compose.yml"
                    || base == "compose.yaml"
            })
            .cloned()
            .collect()
    }
}

/// The 1-based line a marker appears on, plus a version-looking token from that line.
fn find_line(content: &str, marker: &str) -> (usize, Option<String>) {
    for (i, line) in content.lines().enumerate() {
        if line.to_lowercase().contains(marker) {
            return (i + 1, extract_version(line));
        }
    }
    (0, None)
}

fn extract_version(line: &str) -> Option<String> {
    let mut best: Option<String> = None;
    let mut cur = String::new();
    for ch in line.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            cur.push(ch);
        } else {
            if cur.contains('.')
                && cur.chars().next().is_some_and(|c| c.is_ascii_digit())
                && best.is_none()
            {
                best = Some(cur.trim_end_matches('.').to_string());
            }
            cur.clear();
        }
    }
    if best.is_none() && cur.contains('.') {
        best = Some(cur.trim_end_matches('.').to_string());
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_version_from_a_dependency_line() {
        assert_eq!(
            extract_version(
                "  implementation 'org.springframework.boot:spring-boot-starter:3.5.0'"
            ),
            Some("3.5.0".to_string())
        );
        assert_eq!(extract_version("plugins { id 'java' }"), None);
    }
}
