//! Human rendering.
//!
//! Results go to stdout; diagnostics go to stderr. This is not a style preference:
//! `bughunter rescan --json | jq` must work while `-v` is on, and a progress line on
//! stdout would corrupt the JSON.
//!
//! Colour is removed under `NO_COLOR`, on a pipe, and in CI. Severity is a word as well as
//! a colour, because a format that loses meaning without colour is broken by default.

use bh_core::bh_store;
use bh_core::report::*;
use bh_types::Health;
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

pub fn banner(w: &mut impl Write, st: &Style) -> std::io::Result<()> {
    writeln!(w, "{}", st.head("BugHunter"))?;
    writeln!(w, "{}", st.dim(RULE))?;
    writeln!(w)
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
    if r.edges_total > 0 {
        // The honest denominator excludes edges pointing outside the project: an edge to
        // org.springframework is correctly not in this index, not a resolution failure.
        let in_scope = r.edges_total.saturating_sub(r.edges_external);
        let pct = if in_scope > 0 {
            (r.edges_resolved as f64 / in_scope as f64) * 100.0
        } else {
            100.0
        };
        writeln!(
            w,
            "  edges        {} {}",
            r.edges_total,
            st.dim(&format!(
                "({pct:.0}% of {in_scope} in-project resolved, {} external)",
                r.edges_external
            ))
        )?;
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
    rows: &[bh_store::ChangeRow],
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
    writeln!(w, "  {} related tests", r.tests.len())?;
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
    writeln!(w, "  {} edges total", g.edges_total)?;
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
        st.good(&format!("({pct:.0}%)"))
    )?;
    writeln!(
        w,
        "  {} external {}",
        g.edges_external,
        st.dim("(libraries, unscanned sibling modules)")
    )?;
    if !g.by_resolution.is_empty() {
        writeln!(w)?;
        for (res, n) in &g.by_resolution {
            writeln!(w, "  {:<12} {n}", st.dim(res))?;
        }
    }
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
pub fn breaches(bugs: &[BugSummary], threshold: &str) -> bool {
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

pub fn hunt(w: &mut impl Write, st: &Style, r: &HuntReport) -> std::io::Result<()> {
    writeln!(w, "{}", st.head("Analysis"))?;
    writeln!(w, "  {} detectors", r.detectors_run.len())?;
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
    if r.bugs.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.good("Nothing found."))?;
        return Ok(());
    }
    writeln!(w)?;
    bugs(w, st, &r.bugs)
}

pub fn bugs(w: &mut impl Write, st: &Style, list: &[BugSummary]) -> std::io::Result<()> {
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
            st.dim(&b.bug_type)
        )?;
        if let (Some(f), Some(l)) = (&b.file, b.line) {
            writeln!(w, "     {}", st.dim(&format!("{f}:{l}")))?;
        }
        writeln!(w)?;
    }
    writeln!(
        w,
        "{}",
        st.dim(&format!(
            "  {} findings — see one with: bughunter bug <id>",
            list.len()
        ))
    )
}

pub fn bug(w: &mut impl Write, st: &Style, d: &BugDetail) -> std::io::Result<()> {
    let b = &d.summary;
    writeln!(w, "{} {}", glyph(st, &b.severity), st.head(&b.uid))?;
    writeln!(w, "{}", b.title)?;
    writeln!(w)?;
    writeln!(w, "Severity:   {}", b.severity)?;
    writeln!(w, "Confidence: {}%", (b.confidence * 100.0).round() as u32)?;
    writeln!(w, "Status:     {}", b.status)?;
    writeln!(w, "Type:       {}", b.bug_type)?;
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
