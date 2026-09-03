//! `analyze` — capability dispatch and the finding lifecycle.
//!
//! Nexus owns identity, recurrence, fixed and regressed; a capability owns only rules. This
//! file is that division: everything here is the platform's half.

use super::*;

impl Engine {
    /// Run every deterministic detector and reconcile the results with what is already known.
    ///
    /// No model is asked here, so nothing this produces is subject to the 0.75 clamp that
    /// applies to a model's own confidence: both sides of every claim are in the index and
    /// comparing them is a query.
    /// Run one capability over a scope and reconcile its findings with what is known.
    ///
    /// Nexus owns identity, lifecycle and storage; the capability only says what is wrong.
    /// That split is what lets a second capability be a few hundred lines instead of a
    /// re-argument about when a finding is new, recurring, fixed or regressed.
    pub fn analyze(&mut self, capability_id: &str, scope: Scope) -> Result<AnalyzeReport> {
        let started = Instant::now();
        let (commit, _) = self.head();

        let capability =
            self.capabilities
                .get(capability_id)
                .ok_or_else(|| EngineError::UnknownCapability {
                    asked: capability_id.to_string(),
                    known: self.capabilities.ids().join(", "),
                })?;
        let cap_id = capability.id().to_string();
        let prefix = capability.finding_prefix().to_string();

        let baseline = self
            .store
            .baseline(self.project_id)?
            .ok_or(EngineError::NoBaseline)?;
        let (scan_uid, scan_id) = (baseline.scan_uid.clone(), baseline.scan_id);

        let symbols: Vec<SymbolFacts> = self
            .store
            .symbol_facts(self.project_id)?
            .into_iter()
            .map(|r| SymbolFacts {
                fqn: r.fqn,
                name: r.name,
                kind: r.kind,
                file: r.file,
                line: r.line,
                visibility: r.visibility,
                parent_fqn: r.parent_fqn,
                annotations: r
                    .annotations_json
                    .as_deref()
                    .and_then(|j| serde_json::from_str(j).ok())
                    .unwrap_or_default(),
            })
            .collect();
        let edges: Vec<EdgeFacts> = self
            .store
            .edge_facts(self.project_id)?
            .into_iter()
            .map(|r| EdgeFacts {
                src_fqn: r.src_fqn,
                dst_fqn: r.dst_fqn,
                dst_hint: r.dst_hint,
                edge_type: r.edge_type,
                resolution: r.resolution,
                line: r.line,
            })
            .collect();
        let files: Vec<FileFacts> = self
            .store
            .file_facts(self.project_id)?
            .into_iter()
            .map(|r| FileFacts {
                path: r.path,
                lang: r.lang,
            })
            .collect();

        // What moved, so a `Changed` scope has something to narrow by. Read from the
        // ledger rather than recomputed: the rescan cascade already worked this out.
        let changed: Vec<ChangedSymbol> = match &scope {
            Scope::Changed { since_scan } => self
                .store
                .changes_for_scan(*since_scan, Some("symbol"))?
                .into_iter()
                // The kind is what a capability decides on — an API break ripples to every
                // caller where a body edit does not — and it is already in the ledger.
                // Hardcoding BODY_CHANGED here made every rule that asks "did the contract
                // move?" permanently unreachable, silently, with no test able to see it
                // because no capability asked until now.
                .filter_map(|(_, change_type, target, detail)| {
                    let fqn = target?;
                    let kind = ChangeKind::from_ledger(&change_type, detail.as_deref())?;
                    Some(ChangedSymbol {
                        path: symbols
                            .iter()
                            .find(|s| s.fqn == fqn)
                            .map(|s| s.file.clone())
                            .unwrap_or_default(),
                        fqn,
                        kind,
                    })
                })
                .collect(),
            _ => Vec::new(),
        };

        // Loaded once and handed over: a capability that re-derived the profile would be
        // re-reading build files the platform already read.
        let profile = self.load_profile().ok().flatten();
        let ctx = ProjectContext::new(&self.root, &symbols, &edges, &files)
            .with_changes(&changed, commit.as_deref())
            .with_profile(profile.as_ref())
            .with_coverage(
                self.store
                    .covered_fqns(self.project_id)?
                    .into_iter()
                    .collect(),
            );

        let mut candidates = capability
            .analyze(&ctx, &scope)
            .map_err(|e| EngineError::Capability(e.to_string()))?;
        let examined = ctx.in_scope(&scope).len();

        // A candidate with no checkable evidence is rejected at the boundary rather than
        // down-ranked, whether it came from a rule or from a model. An assertion nobody can
        // verify is not a finding, and storing one lets the next reader mistake it for one.
        let before = candidates.len();
        candidates.retain(|c| !c.evidence.is_empty());
        let rejected = before - candidates.len();

        let mut alternates: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for c in &candidates {
            let Some(anchor) = c.anchor_fqn.as_deref() else {
                continue;
            };
            if alternates.contains_key(anchor) {
                continue;
            }
            alternates.insert(
                anchor.to_string(),
                self.store.old_fqns_for(self.project_id, anchor)?,
            );
        }

        let open_before = self.store.open_findings(self.project_id, &cap_id)?;
        let mut touched: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let (mut new_count, mut recurring, mut regressed) = (0usize, 0usize, 0usize);

        let tx = self.store.transaction()?;
        for c in &candidates {
            let fingerprint = c.fingerprint();
            let alt_fingerprints: Vec<String> = c
                .anchor_fqn
                .as_deref()
                .and_then(|a| alternates.get(a))
                .map(|olds| {
                    olds.iter()
                        .map(|old| {
                            let mut moved = c.clone();
                            moved.anchor_fqn = Some(old.clone());
                            moved.fingerprint()
                        })
                        .collect()
                })
                .unwrap_or_default();

            let status = c.initial_status();
            let anchor_line = c.evidence.first().map(|e| e.line as i64);
            let anchor_file = c.evidence.first().map(|e| e.file.clone());

            let up = Store::upsert_finding(
                &tx,
                self.project_id,
                scan_id,
                &nexus_store::NewFinding {
                    capability: cap_id.clone(),
                    uid_prefix: prefix.clone(),
                    fingerprint,
                    alt_fingerprints,
                    slug: c.slug.clone(),
                    title: c.title.clone(),
                    finding_type: c.finding_type.as_str().to_string(),
                    component: c.component.clone(),
                    severity: c.severity.as_str().to_string(),
                    confidence: c.confidence,
                    status: status.as_str().to_string(),
                    detector: c.detector.clone(),
                    anchor_fqn: c.anchor_fqn.clone(),
                    commit: commit.clone(),
                    capability_data: c.capability_data.as_ref().map(|v| v.to_string()),
                },
            )?;
            touched.insert(up.id);
            if up.is_new {
                new_count += 1;
            } else if up.status == "REGRESSED" {
                regressed += 1;
            } else {
                recurring += 1;
            }

            Store::insert_occurrence(
                &tx,
                up.id,
                scan_id,
                &nexus_store::NewOccurrence {
                    file_path: anchor_file,
                    start_line: anchor_line,
                    status: up.status.clone(),
                    confidence: c.confidence,
                    evidence_json: serde_json::to_string(&c.evidence)?,
                    commit: commit.clone(),
                },
            )?;
        }

        // Closing a finding needs evidence. For a deterministic capability that evidence is:
        // the rule ran again over the same index and did not fire. A narrowed scope did not
        // look everywhere, so it may not close anything it did not examine.
        let mut fixed = 0usize;
        if scope == Scope::Everything {
            for open in &open_before {
                if touched.contains(&open.id) {
                    continue;
                }
                Store::mark_fixed(&tx, open.id, scan_id, commit.as_deref())?;
                fixed += 1;
            }
        }
        tx.commit().map_err(nexus_store::StoreError::from)?;

        let findings = self.findings(Some(&cap_id), None, None)?;
        Ok(AnalyzeReport {
            capability: cap_id,
            scope: scope.describe(),
            scan_uid: Some(scan_uid),
            symbols_examined: examined,
            found: candidates.len(),
            new: new_count,
            recurring,
            regressed,
            fixed,
            rejected,
            findings,
            duration_ms: started.elapsed().as_millis(),
        })
    }
    /// Record a finding produced outside this process.
    ///
    /// This is what LLM independence actually means. Until an agent can write a finding
    /// back, only code compiled into Nexus can produce one — and that, not the absence of
    /// HTTP clients, is what makes a system model-dependent. With this, any model is a
    /// provider and Nexus contains no provider-specific code at all.
    ///
    /// The same evidence rule applies as to a rule-produced finding: a candidate with no
    /// checkable `file:line` is rejected, not down-ranked. A model's confidence is clamped,
    /// because a model may not grade its own work.
    pub fn record_finding(&mut self, capability: &str, mut f: Finding) -> Result<RecordedFinding> {
        if f.evidence.is_empty() {
            return Err(EngineError::NoEvidence);
        }
        let known: Vec<String> = self
            .store
            .live_files(self.project_id)?
            .into_iter()
            .map(|r| r.path)
            .collect();
        // Evidence pointing at a file that is not in the index is not evidence. A model
        // describing a plausible problem in a file that does not exist produces no rows.
        if let Some(bad) = f
            .evidence
            .iter()
            .find(|e| !known.iter().any(|k| k == &e.file))
        {
            return Err(EngineError::UnknownEvidenceFile(bad.file.clone()));
        }
        f.confidence = f.confidence.min(MODEL_CONFIDENCE_CAP);

        let (commit, _) = self.head();
        let baseline = self
            .store
            .baseline(self.project_id)?
            .ok_or(EngineError::NoBaseline)?;
        let prefix = self
            .capabilities
            .get(capability)
            .map(|c| c.finding_prefix().to_string())
            .unwrap_or_else(|| capability.to_uppercase().chars().take(3).collect());

        let tx = self.store.transaction()?;
        let up = Store::upsert_finding(
            &tx,
            self.project_id,
            baseline.scan_id,
            &nexus_store::NewFinding {
                capability: capability.to_string(),
                uid_prefix: prefix,
                fingerprint: f.fingerprint(),
                alt_fingerprints: Vec::new(),
                slug: f.slug.clone(),
                title: f.title.clone(),
                finding_type: f.finding_type.as_str().to_string(),
                component: f.component.clone(),
                severity: f.severity.as_str().to_string(),
                confidence: f.confidence,
                status: f.initial_status().as_str().to_string(),
                detector: f.detector.clone(),
                anchor_fqn: f.anchor_fqn.clone(),
                commit: commit.clone(),
                // An agent-recorded finding may carry its own shape too — the write-back
                // path is not a lesser citizen than a compiled capability.
                capability_data: f.capability_data.as_ref().map(|v| v.to_string()),
            },
        )?;
        Store::insert_occurrence(
            &tx,
            up.id,
            baseline.scan_id,
            &nexus_store::NewOccurrence {
                file_path: f.evidence.first().map(|e| e.file.clone()),
                start_line: f.evidence.first().map(|e| e.line as i64),
                status: up.status.clone(),
                confidence: f.confidence,
                evidence_json: serde_json::to_string(&f.evidence)?,
                commit,
            },
        )?;
        tx.commit().map_err(nexus_store::StoreError::from)?;

        Ok(RecordedFinding {
            uid: up.uid,
            is_new: up.is_new,
            status: up.status,
        })
    }
}
