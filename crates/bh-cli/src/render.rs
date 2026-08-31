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
            _ => st.dim(kind),
        };
        writeln!(w, "  {tag:<28} {}", item.fqn.as_deref().unwrap_or("-"))?;
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
