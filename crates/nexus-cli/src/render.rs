//! Human rendering.
//!
//! Results go to stdout; diagnostics go to stderr. This is not a style preference:
//! `bughunter rescan --json | jq` must work while `-v` is on, and a progress line on
//! stdout would corrupt the JSON.
//!
//! Colour is removed under `NO_COLOR`, on a pipe, and in CI. Severity is a word as well as
//! a colour, because a format that loses meaning without colour is broken by default.

use nexus_core::context::{ContextItem, ContextPackage, Decision, ItemKind};
use nexus_core::nexus_store;
use nexus_core::report::*;
use nexus_core::tuning::WeightsReport;
use nexus_types::Health;
use std::io::{IsTerminal, Write};

pub struct Style {
    color: bool,
}

impl Style {
    pub fn detect(no_color: bool) -> Self {
        Style {
            color: !no_color
                && std::env::var_os("NO_COLOR").is_none()
                && std::io::stdout().is_terminal(),
        }
    }
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn head(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn good(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn warn(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn bad(&self, s: &str) -> String {
        self.wrap("31", s)
    }
}

const RULE: &str = "────────────────────────────────────────";

/// The product name this invocation is wearing.
///
/// One binary under two names: which one the user typed is the only thing that should
/// differ, so it is read from argv[0] rather than compiled in twice.
/// The command the user actually typed, for messages that tell them what to run next.
/// Derived from the product name so the two names cannot drift apart.
pub fn binary_name() -> &'static str {
    if product_name() == "BugHunter" {
        "bughunter"
    } else {
        "nexus"
    }
}

pub fn product_name() -> &'static str {
    static NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let invoked = std::env::args()
            .next()
            .map(std::path::PathBuf::from)
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();
        if invoked.starts_with("bughunter") {
            "BugHunter"
        } else {
            "Nexus"
        }
    })
}

pub fn banner(w: &mut impl Write, st: &Style) -> std::io::Result<()> {
    writeln!(w, "{}", st.head(product_name()))?;
    writeln!(w, "{}", st.dim(RULE))?;
    writeln!(w)
}

/// The session package, in the shape `07-agent-integration.md` §4 specifies.
///
/// Every line here is a query result. Nothing is inferred and no token was spent producing
/// it, which is the whole claim the package makes.
pub fn context(w: &mut impl Write, st: &Style, p: &ContextPackage) -> std::io::Result<()> {
    match &p.project.profile {
        Some(prof) => profile(w, st, prof)?,
        None => writeln!(w, "Project: {}", st.head(&p.project.name))?,
    }

    let findings: Vec<&ContextItem> = p
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Finding)
        .collect();
    if !findings.is_empty() {
        writeln!(w)?;
        writeln!(
            w,
            "{}",
            st.head(&format!("Open findings ({})", findings.len()))
        )?;
        for i in &findings {
            writeln!(w, "  {}", i.text)?;
        }
    }

    // A task package is mostly symbols; a session package has none. Rendering both here
    // rather than in two functions keeps one definition of what a package looks like.
    let symbols: Vec<&ContextItem> = p
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Symbol)
        .collect();
    if !symbols.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.head(&format!("Code ({})", symbols.len())))?;
        for i in &symbols {
            writeln!(
                w,
                "  {:<52} {}",
                i.text,
                st.dim(&format!("{}:{} · {}", i.anchor.file, i.anchor.line, i.why))
            )?;
        }
    }

    let facts: Vec<&ContextItem> = p
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Fact)
        .collect();
    if !facts.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.head(&format!("Known ({})", facts.len())))?;
        for i in &facts {
            writeln!(w, "  {}", i.text)?;
        }
    }

    if let Some(warning) = &p.project.scope_warning {
        writeln!(w)?;
        writeln!(w, "{} {}", st.warn("Scope warning:"), warning)?;
    }

    writeln!(w)?;
    let excluded = p.items_considered.saturating_sub(p.items_included);
    writeln!(
        w,
        "{}",
        st.dim(&format!(
            "considered {} · included {} · excluded {} · {} of {} tokens",
            p.items_considered, p.items_included, excluded, p.tokens_estimated, p.budget_tokens
        ))
    )?;
    Ok(())
}

pub fn profile(w: &mut impl Write, st: &Style, p: &Profile) -> std::io::Result<()> {
    writeln!(w, "Project: {}", st.head(&p.name))?;
    let langs: Vec<String> = p
        .languages
        .iter()
        .take(4)
        .map(|l| {
            let s = format!("{} ({})", l.lang, l.files);
            if l.analyzed {
                s
            } else {
                st.dim(&format!("{s} not analyzed"))
            }
        })
        .collect();
    if !langs.is_empty() {
        writeln!(w, "  languages    {}", langs.join(", "))?;
    }
    if !p.frameworks.is_empty() {
        let f: Vec<String> = p
            .frameworks
            .iter()
            .map(|f| match &f.version {
                Some(v) => format!("{} {v} {}", f.name, st.dim(&f.evidence)),
                None => format!("{} {}", f.name, st.dim(&f.evidence)),
            })
            .collect();
        writeln!(w, "  frameworks   {}", f.join(", "))?;
    }
    if let Some(b) = &p.build_system {
        let pm = p.package_manager.as_deref().unwrap_or("-");
        writeln!(w, "  build        {b} · {pm}")?;
    }
    if !p.databases.is_empty() {
        let d: Vec<String> = p
            .databases
            .iter()
            .map(|d| format!("{} {}", d.kind, st.dim(&d.evidence)))
            .collect();
        writeln!(w, "  databases    {}", d.join(", "))?;
    }
    if !p.containers.is_empty() {
        writeln!(w, "  containers   {}", p.containers.len())?;
    }
    Ok(())
}

pub fn scan(w: &mut impl Write, st: &Style, r: &ScanReport) -> std::io::Result<()> {
    writeln!(w, "{}", st.head("Scan"))?;
    writeln!(w, "  {} · {}", r.scan_uid, r.kind)?;
    if let Some(c) = &r.commit {
        writeln!(
            w,
            "  commit       {}{}",
            &c[..7.min(c.len())],
            if r.dirty { " (dirty)" } else { "" }
        )?;
    }
    writeln!(w, "  files        {}", r.files_scanned)?;
    writeln!(w, "  symbols      {}", r.symbols_indexed)?;
    if r.facts_invalidated > 0 {
        writeln!(
            w,
            "  facts        {} invalidated — evidence moved",
            r.facts_invalidated
        )?;
    }
    if r.edges_total > 0 {
        // The honest denominator excludes third-party libraries — an edge to
        // org.springframework is correctly not in this index, not a resolution failure.
        // It does NOT exclude sibling modules: that is code this project owns, and
        // discounting it made the rate rise as coverage fell.
        let in_scope = r.edges_total.saturating_sub(r.edges_external);
        let pct = if in_scope > 0 {
            (r.edges_resolved as f64 / in_scope as f64) * 100.0
        } else {
            100.0
        };
        // "coverage", not "resolved": this is the share of call sites that found *a*
        // destination, and nothing here checks that it is the right one. See
        // docs/superpowers/specs/2026-09-03-resolution-accuracy-harness-design.md.
        writeln!(
            w,
            "  call sites   {} {}",
            r.edges_total,
            st.dim(&format!(
                "({pct:.0}% coverage of {in_scope} in-project, {} external)",
                r.edges_external
            ))
        )?;
        // Reported only when it means something. The graph breakdown always shows the raw
        // count; this line exists to interrupt someone, and interrupting over one edge is
        // how a warning becomes noise people learn to scroll past.
        if r.edges_sibling >= nexus_core::SIBLING_WARN_FLOOR {
            writeln!(
                w,
                "  {}      {} {}",
                st.warn("sibling"),
                r.edges_sibling,
                st.dim("edges point at code you own that was not scanned")
            )?;
        }
    }
    if r.files_skipped > 0 {
        writeln!(
            w,
            "  skipped      {} {}",
            r.files_skipped,
            st.dim("(no analyzer for this language)")
        )?;
    }
    writeln!(w, "  took         {} ms", r.duration_ms)?;
    health(w, st, r.health, r.files_failed, &r.warnings)
}

pub fn rescan(w: &mut impl Write, st: &Style, r: &RescanReport) -> std::io::Result<()> {
    let base = r.baseline.commit.as_deref().unwrap_or("-");
    let cur = r.current.commit.as_deref().unwrap_or("-");
    writeln!(
        w,
        "Baseline: {} {}",
        &base[..7.min(base.len())],
        st.dim(r.baseline.scan_uid.as_deref().unwrap_or(""))
    )?;
    writeln!(
        w,
        "Current:  {}{}",
        &cur[..7.min(cur.len())],
        if r.current.dirty { " (dirty)" } else { "" }
    )?;
    writeln!(w)?;

    if let Some(reason) = &r.forced_full {
        writeln!(w, "{} {}", st.warn("Full rescan forced:"), reason)?;
        writeln!(w)?;
    }

    if r.unchanged {
        writeln!(w, "{}", st.good("No changes since the baseline."))?;
        writeln!(w, "{}", st.dim(&format!("  {} ms", r.duration_ms)))?;
        return Ok(());
    }

    writeln!(w, "{}", st.head("Changes"))?;
    writeln!(w, "  {} files", r.files_changed + r.files_deleted)?;
    writeln!(w, "  {} symbols", r.symbols_changed)?;
    if r.facts_invalidated > 0 {
        writeln!(
            w,
            "  {} facts invalidated — their evidence moved",
            r.facts_invalidated
        )?;
    }
    writeln!(w)?;

    let mut shown = 0usize;
    for item in r.items.iter().filter(|i| i.entity == "symbol") {
        if shown == 12 {
            let total = r.items.iter().filter(|i| i.entity == "symbol").count();
            writeln!(
                w,
                "  {}",
                st.dim(&format!("… showing 12 of {total} — use --json for all"))
            )?;
            break;
        }
        let kind = item.kind.unwrap_or("");
        let tag = match kind {
            "API_CHANGED" | "API_AND_BODY_CHANGED" => st.warn(kind),
            "CONTRACT_CHANGED" => st.warn(kind),
            "DELETED" => st.bad(kind),
            "ADDED" => st.good(kind),
            "RENAMED" => st.dim(kind),
            _ => st.dim(kind),
        };
        match &item.from_fqn {
            // A rename is one fact about one symbol, so it prints as one line.
            Some(from) => writeln!(
                w,
                "  {tag:<28} {}\n  {:<28} {}",
                item.fqn.as_deref().unwrap_or("-"),
                "",
                st.dim(&format!("was {from}"))
            )?,
            None => writeln!(w, "  {tag:<28} {}", item.fqn.as_deref().unwrap_or("-"))?,
        }
        shown += 1;
    }
    if shown > 0 {
        writeln!(w)?;
    }
    writeln!(w, "{}", st.dim(&format!("  {} ms", r.duration_ms)))?;
    health(w, st, r.health, r.files_failed, &r.warnings)
}

pub fn status(w: &mut impl Write, st: &Style, s: &StatusReport) -> std::io::Result<()> {
    if let Some(p) = &s.profile {
        profile(w, st, p)?;
        writeln!(w)?;
    }
    match &s.baseline {
        None => {
            writeln!(w, "{}", st.warn("No baseline."))?;
            writeln!(w, "  run: bughunter scan")?;
        }
        Some(b) => {
            let c = b.commit.as_deref().unwrap_or("-");
            writeln!(
                w,
                "Baseline: {} {}",
                &c[..7.min(c.len())],
                st.dim(b.scan_uid.as_deref().unwrap_or(""))
            )?;
            let cur = s.current.commit.as_deref().unwrap_or("-");
            writeln!(
                w,
                "Current:  {}{}",
                &cur[..7.min(cur.len())],
                if s.current.dirty { " (dirty)" } else { "" }
            )?;
            writeln!(w)?;
            writeln!(w, "Index")?;
            writeln!(w, "  {} files", s.files)?;
            writeln!(w, "  {} symbols", s.symbols)?;
            writeln!(w, "  {} scans", s.scans)?;
            writeln!(w)?;
            if s.drifted {
                let behind = s.commits_behind.unwrap_or(0);
                let detail = if behind > 0 {
                    format!("baseline is {behind} commits behind HEAD")
                } else {
                    "working tree has uncommitted changes".into()
                };
                writeln!(w, "{} {}", st.warn("Drifted:"), detail)?;
                writeln!(w, "  run: bughunter rescan")?;
            } else {
                writeln!(w, "{}", st.good("Up to date."))?;
            }
        }
    }
    Ok(())
}

pub fn doctor(w: &mut impl Write, st: &Style, checks: &[Check]) -> std::io::Result<()> {
    let mut warns = 0;
    let mut errors = 0;
    for c in checks {
        let (mark, name) = match c.level {
            "ok" => (st.good("✓"), c.name),
            "warn" => {
                warns += 1;
                (st.warn("⚠"), c.name)
            }
            _ => {
                errors += 1;
                (st.bad("✗"), c.name)
            }
        };
        writeln!(w, "  {mark} {name:<16} {}", c.detail)?;
        if let Some(r) = &c.remedy {
            writeln!(w, "    {}", st.dim(&format!("run: {r}")))?;
        }
    }
    writeln!(w)?;
    let summary = format!("{warns} warnings, {errors} errors");
    writeln!(
        w,
        "{}",
        if errors > 0 {
            st.bad(&summary)
        } else if warns > 0 {
            st.warn(&summary)
        } else {
            st.good(&summary)
        }
    )
}

pub fn changes(
    w: &mut impl Write,
    st: &Style,
    rows: &[nexus_store::ChangeRow],
) -> std::io::Result<()> {
    if rows.is_empty() {
        writeln!(
            w,
            "{}",
            st.dim("No changes recorded in the current baseline scan.")
        )?;
        return Ok(());
    }
    for (entity, change_type, target, detail) in rows {
        writeln!(
            w,
            "  {:<8} {:<10} {:<12} {}",
            st.dim(entity),
            change_type,
            detail.as_deref().unwrap_or("-"),
            target.as_deref().unwrap_or("-")
        )?;
    }
    Ok(())
}

/// Partial failure is a first-class outcome, so it is always visible — never a silent `ok`.
fn health(
    w: &mut impl Write,
    st: &Style,
    h: Health,
    failed: usize,
    warnings: &[String],
) -> std::io::Result<()> {
    if h == Health::Ok && warnings.is_empty() {
        return Ok(());
    }
    writeln!(w)?;
    if failed > 0 {
        writeln!(
            w,
            "{} {} files failed to parse",
            st.warn("Degraded:"),
            failed
        )?;
    }
    for warning in warnings.iter().take(5) {
        writeln!(w, "  {}", st.dim(warning))?;
    }
    if warnings.len() > 5 {
        writeln!(
            w,
            "  {}",
            st.dim(&format!("… and {} more (use --json)", warnings.len() - 5))
        )?;
    }
    Ok(())
}

pub fn impact(
    w: &mut impl Write,
    st: &Style,
    r: &ImpactReport,
    show_paths: bool,
) -> std::io::Result<()> {
    let arrow = if r.direction == "reverse" {
        "depends on"
    } else {
        "reaches"
    };
    writeln!(w, "{} {}", st.head("Impact"), st.dim(&format!("({arrow})")))?;
    for s in &r.seeds {
        writeln!(
            w,
            "  {} {}",
            st.head(&s.fqn),
            st.dim(&format!("{}:{}", s.file, s.line))
        )?;
    }
    writeln!(w)?;

    if r.items.is_empty() && r.tests.is_empty() {
        writeln!(w, "{}", st.dim("Nothing else is affected."))?;
        return Ok(());
    }

    writeln!(w, "  {} affected symbols", r.items.len())?;
    // A zero in a list of counts is skimmed past. The whole value of this answer is that
    // it changes what someone does next, so it is stated rather than counted.
    if r.uncovered {
        writeln!(
            w,
            "  {} {}",
            st.warn("no test reaches this"),
            st.dim("— a change here fails silently")
        )?;
    } else {
        writeln!(w, "  {} related tests", r.tests.len())?;
    }
    if r.crossed_seam > 0 {
        writeln!(
            w,
            "  {} crossing the frontend/backend seam",
            st.good(&r.crossed_seam.to_string())
        )?;
    }
    writeln!(w)?;

    for item in &r.items {
        let score = format!("{:.2}", item.score);
        // A path whose weakest link is a heuristic guess is shown as one, never as a fact.
        let conf = if item.min_confidence >= 0.95 {
            st.good("exact")
        } else if item.min_confidence >= 0.7 {
            st.dim("likely")
        } else {
            st.warn("guess")
        };
        writeln!(w, "  {score:>5}  {conf:<14} {}", item.fqn)?;
        if show_paths {
            for hop in &item.path {
                writeln!(
                    w,
                    "         {}",
                    st.dim(&format!("{} --{}--> ", hop.from, hop.edge))
                )?;
            }
        }
    }

    if !r.tests.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.head("Related tests"))?;
        for t in r.tests.iter().take(10) {
            writeln!(w, "  {:>5}  {}", format!("{:.2}", t.score), t.fqn)?;
        }
    }

    if !r.truncated_at.is_empty() {
        writeln!(w)?;
        writeln!(w, "{} fan-out capped at:", st.warn("Truncated:"))?;
        for t in &r.truncated_at {
            writeln!(w, "  {t}")?;
        }
    }
    writeln!(w)?;
    writeln!(
        w,
        "{}",
        st.dim(&format!("  {} visited · {} ms", r.visited, r.duration_ms))
    )
}

pub fn ambiguous(
    w: &mut impl Write,
    st: &Style,
    target: &str,
    cands: &[SeedRef],
) -> std::io::Result<()> {
    writeln!(
        w,
        "{} '{target}' matches {} symbols.",
        st.warn("Ambiguous:"),
        cands.len()
    )?;
    writeln!(
        w,
        "{}",
        st.dim("  Pick one — BugHunter will not guess which you meant.")
    )?;
    writeln!(w)?;
    for c in cands.iter().take(15) {
        writeln!(w, "  {}", c.fqn)?;
        writeln!(w, "    {}", st.dim(&format!("{}:{}", c.file, c.line)))?;
    }
    if cands.len() > 15 {
        writeln!(
            w,
            "  {}",
            st.dim(&format!("… and {} more", cands.len() - 15))
        )?;
    }
    Ok(())
}

pub fn graph(w: &mut impl Write, st: &Style, g: &GraphReport) -> std::io::Result<()> {
    writeln!(w, "{}", st.head("Dependency graph"))?;
    writeln!(w, "  {} call sites total", g.edges_total)?;
    let in_scope = g.edges_total - g.edges_external;
    let pct = if in_scope > 0 {
        (g.edges_resolved as f64 / in_scope as f64) * 100.0
    } else {
        100.0
    };
    writeln!(
        w,
        "  {in_scope} in-project · {} resolved {}",
        g.edges_resolved,
        st.good(&format!("({pct:.0}% coverage)"))
    )?;
    writeln!(
        w,
        "  {} external {}",
        g.edges_external,
        st.dim("(third-party libraries)")
    )?;
    // Kept on its own line rather than folded into `external`, because the two call for
    // opposite reactions: one is correctly outside the index, the other means the index
    // is missing code an edit here can break.
    if g.edges_sibling > 0 {
        writeln!(
            w,
            "  {} {} {}",
            g.edges_sibling,
            st.warn("sibling"),
            st.dim("(code you own, not scanned — scan from the repository root)")
        )?;
    }
    if !g.by_resolution.is_empty() {
        writeln!(w)?;
        for (res, n) in &g.by_resolution {
            writeln!(w, "  {:<12} {n}", st.dim(res))?;
        }
    }
    // Said once, plainly, on the surface that reports the number. Coverage answers "how
    // much of the graph exists", never "how much of it is correct" — and this project's own
    // documents read it as the second for long enough that a decision gate was wired to it.
    writeln!(w)?;
    writeln!(
        w,
        "  {}",
        st.dim("coverage, not accuracy: nothing here checks a destination is the right one")
    )?;
    Ok(())
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// Whether any open finding is at or above the gate.
pub fn breaches(bugs: &[FindingSummary], threshold: &str) -> bool {
    let gate = severity_rank(threshold);
    bugs.iter()
        .filter(|b| b.status != "FIXED" && b.status != "IGNORED")
        .any(|b| severity_rank(&b.severity) >= gate)
}

fn glyph(st: &Style, severity: &str) -> String {
    match severity {
        "critical" => st.bad("🚨"),
        "high" => st.warn("⚠"),
        _ => st.dim("·"),
    }
}

pub fn analyze(w: &mut impl Write, st: &Style, r: &AnalyzeReport) -> std::io::Result<()> {
    writeln!(w, "{}", st.head("Analysis"))?;
    writeln!(w, "  {} · {}", st.head(&r.capability), st.dim(&r.scope))?;
    writeln!(w, "  {} symbols examined", r.symbols_examined)?;
    writeln!(
        w,
        "  {} findings  {}",
        r.found,
        st.dim(&format!(
            "({} new, {} recurring, {} regressed)",
            r.new, r.recurring, r.regressed
        ))
    )?;
    if r.fixed > 0 {
        writeln!(
            w,
            "  {} closed — the rule no longer fires",
            st.good(&r.fixed.to_string())
        )?;
    }
    if r.rejected > 0 {
        // A silently discarded finding is indistinguishable from finding nothing.
        writeln!(
            w,
            "  {} rejected {}",
            r.rejected,
            st.dim("(no checkable evidence)")
        )?;
    }
    writeln!(w, "{}", st.dim(&format!("  {} ms", r.duration_ms)))?;
    if r.findings.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.good("Nothing found."))?;
        return Ok(());
    }
    writeln!(w)?;
    findings(w, st, &r.findings)
}

pub fn findings(w: &mut impl Write, st: &Style, list: &[FindingSummary]) -> std::io::Result<()> {
    if list.is_empty() {
        writeln!(
            w,
            "{}",
            st.dim("No findings. Run `bughunter hunt` if you have not yet.")
        )?;
        return Ok(());
    }
    for b in list {
        let status = match b.status.as_str() {
            "VERIFIED" | "REGRESSED" => st.bad(&b.status),
            "FIXED" => st.good(&b.status),
            "IGNORED" => st.dim(&b.status),
            _ => st.warn(&b.status),
        };
        writeln!(
            w,
            "{} {}  {}",
            glyph(st, &b.severity),
            st.head(&b.uid),
            b.title
        )?;
        writeln!(
            w,
            "     {:<10} {:>3}%  {:<12} {}",
            b.severity,
            (b.confidence * 100.0).round() as u32,
            status,
            st.dim(&b.finding_type)
        )?;
        if let (Some(f), Some(l)) = (&b.file, b.line) {
            writeln!(w, "     {}", st.dim(&format!("{f}:{l}")))?;
        }
        writeln!(w)?;
    }
    writeln!(
        w,
        "{}",
        // Both halves follow the name the user typed. Under `nexus` the subcommand is
        // `finding`; `bug` is BugHunter's alias for it, and printing the alias to someone
        // running the platform tells them to use the other tool.
        st.dim(&format!(
            "  {} findings — see one with: {} {} <id>",
            list.len(),
            binary_name(),
            if product_name() == "BugHunter" {
                "bug"
            } else {
                "finding"
            }
        ))
    )
}

pub fn finding(w: &mut impl Write, st: &Style, d: &FindingDetail) -> std::io::Result<()> {
    let b = &d.summary;
    writeln!(w, "{} {}", glyph(st, &b.severity), st.head(&b.uid))?;
    writeln!(w, "{}", b.title)?;
    writeln!(w)?;
    writeln!(w, "Severity:   {}", b.severity)?;
    writeln!(w, "Confidence: {}%", (b.confidence * 100.0).round() as u32)?;
    writeln!(w, "Status:     {}", b.status)?;
    writeln!(w, "Type:       {}", b.finding_type)?;
    writeln!(w, "Detector:   {}", b.detector)?;
    if let Some(c) = &b.introduced_commit {
        writeln!(w, "Introduced: {}", &c[..7.min(c.len())])?;
    }
    if let Some(c) = &b.fixed_commit {
        writeln!(w, "Fixed:      {}", &c[..7.min(c.len())])?;
    }
    writeln!(w)?;

    writeln!(w, "{}", st.head("Evidence"))?;
    if d.evidence.is_empty() {
        writeln!(w, "  {}", st.warn("none recorded"))?;
    }
    for e in &d.evidence {
        writeln!(w, "  {}", st.head(&format!("{}:{}", e.file, e.line)))?;
        writeln!(w, "    {}", e.note)?;
    }

    if d.history.len() > 1 {
        writeln!(w)?;
        writeln!(w, "{}", st.head("History"))?;
        for h in &d.history {
            let c = h.commit.as_deref().unwrap_or("-");
            writeln!(
                w,
                "  {:<10} {:<8} {}",
                h.scan_uid,
                &c[..7.min(c.len())],
                h.status
            )?;
        }
    }
    writeln!(w)?;
    writeln!(
        w,
        "{}",
        st.dim("This build does not verify findings. `bughunter verify` lands in V1.")
    )
}

pub fn capabilities(
    w: &mut impl Write,
    st: &Style,
    caps: &[CapabilityInfo],
) -> std::io::Result<()> {
    writeln!(w, "{}", st.head("Capabilities"))?;
    if caps.is_empty() {
        writeln!(w, "  {}", st.warn("none registered"))?;
        return Ok(());
    }
    for c in caps {
        writeln!(
            w,
            "  {:<12} {} {}",
            st.head(&c.id),
            c.describes,
            st.dim(&format!("({}-n)", c.finding_prefix))
        )?;
    }
    Ok(())
}

pub fn answer(w: &mut impl Write, st: &Style, a: &crate::ask::Answer) -> std::io::Result<()> {
    use crate::ask::Answer;
    match a {
        Answer::Changed {
            since,
            symbols,
            files,
        } => {
            writeln!(w, "{}", st.head("Changed"))?;
            if let Some(s) = since {
                writeln!(w, "  {}", st.dim(&format!("since {s}")))?;
            }
            writeln!(w, "  {files} files · {} symbols", symbols.len())?;
            for s in symbols.iter().take(15) {
                writeln!(w, "    {s}")?;
            }
            if symbols.len() > 15 {
                writeln!(
                    w,
                    "    {}",
                    st.dim(&format!("… and {} more", symbols.len() - 15))
                )?;
            }
        }
        Answer::Affected {
            target,
            symbols,
            crossed_seam,
        } => {
            writeln!(w, "{} {}", st.head("Affected by"), target)?;
            if *crossed_seam > 0 {
                writeln!(
                    w,
                    "  {} crossing the frontend/backend seam",
                    st.good(&crossed_seam.to_string())
                )?;
            }
            if symbols.is_empty() {
                writeln!(w, "  {}", st.dim("nothing else depends on it"))?;
            }
            for s in symbols.iter().take(20) {
                let tag = if s.min_confidence >= 0.95 {
                    st.good("exact")
                } else {
                    st.dim("likely")
                };
                writeln!(
                    w,
                    "  {:>5}  {:<14} {}",
                    format!("{:.2}", s.score),
                    tag,
                    s.fqn
                )?;
            }
        }
        Answer::Known {
            target,
            findings,
            facts,
        } => {
            writeln!(w, "{} {}", st.head("Already known about"), target)?;
            if findings.is_empty() && facts.is_empty() {
                writeln!(w, "  {}", st.dim("nothing recorded here yet"))?;
            }
            for f in findings {
                writeln!(w, "  {:<8} {:<10} {}", st.head(&f.uid), f.status, f.title)?;
            }
            for f in facts {
                writeln!(w, "  {:<8} {}", st.dim(&f.source), f.claim)?;
            }
        }
        Answer::Facts { facts } => {
            writeln!(w, "{}", st.head("What Nexus remembers"))?;
            if facts.is_empty() {
                writeln!(
                    w,
                    "  {}",
                    st.dim("nothing yet — record one with: nexus fact <key> \"<claim>\"")
                )?;
            }
            for f in facts {
                writeln!(w, "  {} {}", st.head(&f.key), st.dim(&f.source))?;
                writeln!(w, "    {}", f.claim)?;
            }
        }
        Answer::Next { suggestions } => {
            writeln!(w, "{}", st.head("Worth looking at next"))?;
            if suggestions.is_empty() {
                writeln!(w, "  {}", st.dim("nothing changed since the baseline"))?;
            }
            for s in suggestions {
                writeln!(w, "  {:>5}  {}", format!("{:.0}", s.score), s.target)?;
                writeln!(w, "         {}", st.dim(&s.why))?;
            }
        }
        Answer::Unknown { asked, understood } => {
            writeln!(w, "{} {asked:?}", st.warn("Not a question I know:"))?;
            writeln!(w, "  try:")?;
            for u in understood {
                writeln!(w, "    nexus ask {u}")?;
            }
        }
    }
    Ok(())
}

/// `--stats`: the three numbers §7 requires a package to be able to state about itself.
pub fn context_stats(w: &mut impl Write, p: &ContextPackage) -> std::io::Result<()> {
    writeln!(w, "items_considered {}", p.items_considered)?;
    writeln!(w, "items_included   {}", p.items_included)?;
    writeln!(w, "tokens_estimated {}", p.tokens_estimated)?;
    Ok(())
}

/// `--explain`: §8's mandatory account, in the shape the design specifies.
///
/// Both halves, always. A ranker that explains only its inclusions cannot be debugged for the
/// failure that matters most — the right item that never made it in — so every excluded
/// candidate is printed with the rule that refused it, and none is elided.
pub fn context_explain(w: &mut impl Write, st: &Style, p: &ContextPackage) -> std::io::Result<()> {
    writeln!(w)?;
    writeln!(w, "{}", st.head("Why"))?;
    if let Some(intent) = &p.intent {
        let signal = intent.signal.as_deref().unwrap_or("nothing matched");
        writeln!(
            w,
            "  intent {} {}",
            intent.intent.as_str(),
            st.dim(&format!("({signal})"))
        )?;
    }
    writeln!(w, "  {}", st.dim(&p.basis.selection))?;
    writeln!(w)?;

    for row in &p.ledger.rows {
        let mark = match row.decision {
            Decision::Included => st.good("included"),
            Decision::Excluded => st.dim("excluded"),
        };
        writeln!(
            w,
            "  {mark}  {:>5.2}  {:<44} {}",
            row.score,
            row.label,
            st.dim(&row.reason)
        )?;
        if row.decision == Decision::Included {
            if let Some(item) = p.items.iter().find(|i| i.text.contains(&row.label)) {
                let t = &item.terms;
                writeln!(
                    w,
                    "            {}",
                    st.dim(&format!(
                        "seed {:.2} · graph {:.2} · churn {:.2} · recency {:.2} · hist {:.2} · \
                         fact {:.2} · test {:.2} · arch {:.2} · cost {:.2}",
                        t.seed,
                        t.graph,
                        t.churn,
                        t.recency,
                        t.history,
                        t.fact,
                        t.test,
                        t.arch,
                        t.cost
                    ))
                )?;
            }
        }
    }
    if !p.notes.is_empty() {
        writeln!(w)?;
        for note in &p.notes {
            writeln!(w, "  {}", st.dim(note))?;
        }
    }
    Ok(())
}

/// The verdict, and the checks behind it.
pub fn verify(w: &mut impl Write, st: &Style, r: &VerifyReport) -> std::io::Result<()> {
    let headline = match r.verdict.as_str() {
        "verified" => st.good("VERIFIED"),
        "failed" => st.bad("FAILED"),
        "permission_required" => st.warn("PERMISSION REQUIRED"),
        _ => st.warn("INCONCLUSIVE"),
    };
    writeln!(w, "{headline}")?;
    if let Some(why) = &r.why {
        writeln!(w, "  {why}")?;
    }
    if let Some(note) = &r.note {
        writeln!(w, "  {}", st.dim(note))?;
    }
    if !r.checks.is_empty() {
        writeln!(w)?;
        for c in &r.checks {
            let state = match (&c.blocked, c.exit_code) {
                (Some(_), _) => st.warn("blocked"),
                (None, Some(0)) => st.good("ok"),
                (None, _) => st.bad("failed"),
            };
            writeln!(
                w,
                "  {:<8} {:<9} {}",
                c.kind.as_str(),
                state,
                st.dim(&c.argv.join(" "))
            )?;
        }
    }
    if let Some(baseline) = &r.baseline {
        writeln!(w)?;
        writeln!(w, "  {}", st.dim(baseline))?;
    }
    writeln!(w, "  {}", st.dim(&format!("{} ms", r.duration_ms)))?;
    Ok(())
}

/// What the accumulated inclusion ledgers say about the ranking weights.
pub fn weights(w: &mut impl Write, st: &Style, r: &WeightsReport) -> std::io::Result<()> {
    writeln!(
        w,
        "{}",
        st.head(&format!(
            "{} package(s) · {} considered · {} included",
            r.packages, r.items_considered, r.items_included
        ))
    )?;
    if !r.mean_terms.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.head("Mean contribution per included item"))?;
        for (name, value) in &r.mean_terms {
            writeln!(w, "  {name:<9} {value:>7.3}")?;
        }
    }
    if !r.exclusions.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.head("Why candidates were refused"))?;
        for (reason, n) in &r.exclusions {
            writeln!(w, "  {reason:<18} {n}")?;
        }
    }
    writeln!(w)?;
    match &r.insufficient {
        Some(why) => writeln!(w, "{} {}", st.warn("No recommendation:"), why)?,
        None => writeln!(
            w,
            "{}",
            st.dim(
                "A term near zero everywhere is doing no work; a rule refusing nearly \
                 everything is the one to change first. Edit [context.weights] in \
                 .nexus/policy.toml and cite these numbers."
            )
        )?,
    }
    Ok(())
}
