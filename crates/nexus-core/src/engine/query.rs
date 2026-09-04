//! Read-only queries: what is here, what it reaches, and what is known about it.
//!
//! Nothing here writes to the index. `record_fact` and `ignore_finding` each write a single
//! row and are here because they are what a caller asking a question does next.

use super::*;
use crate::context::seeds::SEED_QUERY_CAP;
use crate::context::{
    self, estimate_tokens, expand, seeds, Candidate, ContextPackage, InclusionLedger, Intent,
    ItemKind, PackageBasis, ProjectSummary, Purpose, Seed, SeedResult, SignalIndex, TaskRequest,
};

impl Engine {
    pub fn status(&self) -> Result<StatusReport> {
        let (commit, dirty) = self.head();
        let baseline = self.store.baseline(self.project_id)?;
        let (files, symbols) = self.store.index_counts(self.project_id)?;
        let commits_behind = match (&self.repo, &baseline) {
            (Some(r), Some(b)) => b.commit_sha.as_ref().and_then(|s| r.commits_since(s)),
            _ => None,
        };
        let drifted = match &baseline {
            Some(b) => dirty || b.commit_sha != commit || commits_behind.unwrap_or(0) > 0,
            None => true,
        };
        Ok(StatusReport {
            project: self.name(),
            profile: self.load_profile()?,
            baseline: baseline.as_ref().map(|b| Revision {
                scan_uid: Some(b.scan_uid.clone()),
                commit: b.commit_sha.clone(),
                dirty: b.dirty,
            }),
            current: Revision {
                scan_uid: None,
                commit,
                dirty,
            },
            commits_behind,
            scans: self.store.scan_count(self.project_id)?,
            files,
            symbols,
            drifted,
        })
    }
    pub fn changes(&self, entity: Option<&str>) -> Result<Vec<nexus_store::ChangeRow>> {
        let Some(b) = self.store.baseline(self.project_id)? else {
            return Err(EngineError::NoBaseline);
        };
        Ok(self.store.changes_for_scan(b.scan_id, entity)?)
    }
    /// Findings attached to a file, a symbol or a component — "what do we already know
    /// about this code?"
    pub fn findings_for(&self, target: &str) -> Result<Vec<FindingSummary>> {
        Ok(self
            .store
            .findings_for(self.project_id, target)?
            .into_iter()
            .map(to_summary)
            .collect())
    }
    /// Remember something about this project that is not a symbol, an edge or a finding.
    pub fn record_fact(&mut self, f: FactInput) -> Result<()> {
        // §2's namespace list, enforced. A key outside it is refused rather than stored under
        // a prefix nothing will ever look for.
        crate::memory::check_key(&f.key).map_err(EngineError::Unsupported)?;
        let baseline = self
            .store
            .baseline(self.project_id)?
            .ok_or(EngineError::NoBaseline)?;
        self.store.record_fact(
            self.project_id,
            baseline.scan_id,
            &nexus_store::NewFact {
                key: f.key,
                scope: f.scope,
                subject: f.subject,
                claim: f.claim,
                source: f.source,
                evidence_json: Some(serde_json::to_string(&f.evidence)?),
                confidence: f.confidence,
            },
        )?;
        Ok(())
    }
    /// Facts, most relevant first by §4's formula.
    ///
    /// `subject` narrows; it does not rank. Ranking is `memory::rank`, the one function every
    /// consumer calls — the Context Engine passes its seeds, this path passes none, and both
    /// get the same order for the same inputs.
    pub fn facts(&self, subject: Option<&str>) -> Result<Vec<Fact>> {
        let seeds: Vec<String> = subject.map(str::to_string).into_iter().collect();
        let current = self.current_scan_id()?;
        Ok(
            crate::memory::rank(self.store.facts(self.project_id, subject)?, &seeds, current)
                .into_iter()
                .map(|r| Fact {
                    key: r.key,
                    scope: r.scope,
                    subject: r.subject,
                    claim: r.claim,
                    source: r.source,
                    confidence: r.confidence,
                    durable: r.durable,
                    validated_count: r.validated_count,
                })
                .collect(),
        )
    }

    /// The baseline scan's id, or 0 before there is one. The clock §4's decay runs on.
    fn current_scan_id(&self) -> Result<i64> {
        Ok(self
            .store
            .baseline(self.project_id)?
            .map_or(0, |b| b.scan_id))
    }
    /// The context package for a request.
    ///
    /// Phase 1 serves `Purpose::Session` with a **fixed query** — profile, open findings,
    /// durable facts, greedy fill to the budget. There is no ranking here on purpose: a
    /// scoring function invented before the ledger has any data to justify its weights is
    /// folklore, and Phase 2.6 replaces this body with the real one behind this signature.
    ///
    /// Reads only. A hook that writes to the database when a session opens is a side effect
    /// nobody asked for, so a project with no baseline gets `NoBaseline` and the advice to
    /// scan, not an implicit scan.
    pub fn context(&self, req: &TaskRequest) -> Result<ContextPackage> {
        match req.purpose {
            Purpose::Session => self.session_package(req),
            _ => self.task_package(req),
        }
    }

    /// The Phase 1 fixed query: profile, open findings, durable facts, in store order.
    fn session_package(&self, req: &TaskRequest) -> Result<ContextPackage> {
        let status = self.status()?;
        let Some(baseline) = status.baseline.clone() else {
            return Err(EngineError::NoBaseline);
        };

        // A scan that covers one module of something larger answers impact questions with a
        // confidently small blast radius. Saying so costs one query and is the single most
        // useful correction an agent can be handed at session start.
        let graph = self.graph()?;
        let scope_warning = (graph.edges_sibling >= SIBLING_WARN_FLOOR as i64).then(|| {
            format!(
                "{} edges point at code this project owns that was not scanned — impact \
                 answers here are understated; scan from the repository root",
                graph.edges_sibling
            )
        });

        let project = ProjectSummary {
            name: status.project.clone(),
            profile: status.profile.clone(),
            files: status.files,
            symbols: status.symbols,
            scope_warning,
        };

        let mut candidates = Vec::new();

        // Open findings: what is broken now. FIXED and IGNORED are history, not news.
        for f in self.findings(None, None, None)? {
            if matches!(f.status.as_str(), "FIXED" | "IGNORED") {
                continue;
            }
            let anchor = match (&f.file, f.line) {
                (Some(file), Some(line)) => Some(CodeRef {
                    file: file.clone(),
                    line: line.max(0) as u32,
                    note: String::new(),
                }),
                _ => None,
            };
            candidates.push(Candidate {
                kind: ItemKind::Finding,
                label: f.uid.clone(),
                anchor,
                why: format!("open finding, {}", f.status.to_lowercase()),
                score: 0.0,
                terms: Default::default(),
                component: String::new(),
                text: format!(
                    "{}  {}  {}  {}",
                    f.uid,
                    f.status,
                    f.component.as_deref().unwrap_or("-"),
                    f.title
                ),
            });
        }

        // Durable facts: what previous sessions worked out and the project kept.
        //
        // Durability is now asked for rather than approximated. It was approximated by store
        // order while the lifecycle was Phase 3.1; the lifecycle landed and the approximation
        // outlived it, which is how 671 imported claims came to buy the session budget nine
        // at a time in alphabetical order.
        for row in self.store.durable_facts(self.project_id)? {
            let anchor = row
                .evidence_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<Vec<CodeRef>>(j).ok())
                .and_then(|refs| refs.into_iter().next());
            candidates.push(Candidate {
                kind: ItemKind::Fact,
                label: row.key.clone(),
                anchor,
                why: format!(
                    "{} fact about {}",
                    row.source,
                    row.subject.as_deref().unwrap_or("the project")
                ),
                text: format!("{}  {}  [{}]", row.key, row.claim, row.source),
                score: 0.0,
                terms: Default::default(),
                component: String::new(),
            });
        }

        let mut ledger = InclusionLedger::default();
        let considered = candidates.len();
        // The summary is not a candidate — it is what the package is *about*, and a package
        // that dropped it under budget pressure would describe findings in an unnamed project.
        let mut package = ContextPackage {
            purpose: req.purpose,
            project,
            items_included: 0,
            items: Vec::new(),
            ledger: InclusionLedger::default(),
            basis: PackageBasis {
                scan_uid: baseline.scan_uid,
                commit: status.current.commit.clone(),
                dirty: status.current.dirty,
                selection: "phase-1 fixed query: open findings then durable facts, in store order"
                    .into(),
            },
            budget_tokens: req.budget_tokens,
            tokens_estimated: 0,
            items_considered: considered,
            intent: None,
            notes: Vec::new(),
        };
        let (items, tokens_estimated) = context::fill(
            candidates,
            req.budget_tokens,
            envelope_cost(&package),
            context::Selection::Ordered,
            &mut ledger,
        );
        package.items_included = items.len();
        package.items = items;
        package.ledger = ledger;
        package.tokens_estimated = tokens_estimated;
        Ok(finish(package, req))
    }
    /// Stage 2 of the context pipeline: what in the code this request is about.
    ///
    /// Public because the pipeline is assembled stage by stage across Phase 2 and each stage
    /// is testable on its own. `Engine::context` will call it once stages 4 to 7 exist.
    pub fn seeds(&self, req: &TaskRequest, intent: Intent) -> Result<SeedResult> {
        Ok(seeds::resolve(&self.store, self.project_id, req, intent)?)
    }
    /// Stage 3 of the context pipeline: what else the seeds reach.
    pub fn expand(&self, seeds: &[Seed], intent: Intent) -> Result<ImpactReport> {
        Ok(expand::run(&self.store, self.project_id, seeds, intent)?)
    }
    /// Stages 1–6, assembled. The seven-stage pipeline of `05-context-engine.md`, minus the
    /// caching that 2.9 adds around it.
    ///
    /// No stage calls a model, and none can: the whole path is a table lookup, a handful of
    /// indexed queries, one graph traversal and a sort.
    fn task_package(&self, req: &TaskRequest) -> Result<ContextPackage> {
        let status = self.status()?;
        let Some(baseline) = status.baseline.clone() else {
            return Err(EngineError::NoBaseline);
        };
        let loaded = crate::policy::load(&self.root.join(crate::NEXUS_DIR).join("policy.toml"));
        let w = loaded.weights;

        // 1 — intent. `--recent` is read here and nowhere else: §14.1 makes the previous
        // message an input to classification, never something stored.
        let for_intent = match &req.recent {
            Some(prev) => format!("{prev}\n{}", req.text),
            None => req.text.clone(),
        };
        // 2 — seeds. Resolved before the intent is final, because whether a turn is
        // referential depends on whether anything in it anchored — which only the index can
        // say, and which a verb table must not guess at.
        // A caller that declares its purpose has better evidence than the verb table, which
        // can only read words. Declaring beats deriving; declaring nothing changes nothing.
        let declared = req.purpose.declared_intent();
        let provisional = match declared {
            Some(intent) => crate::context::IntentMatch {
                intent,
                signal: Some("declared by the caller".into()),
                confident: true,
            },
            None => crate::context::classify(&for_intent),
        };
        let seeded = seeds::resolve(&self.store, self.project_id, req, provisional.intent)?;
        let anchored = seeded
            .seeds
            .iter()
            .any(|s| s.source != crate::context::SeedSource::Carried);
        let intent = match declared {
            Some(_) => provisional,
            None => crate::context::intent::classify_turn(
                &for_intent,
                anchored,
                !req.carry_seeds.is_empty(),
            ),
        };
        // 3 — expand.
        let reached = expand::run(&self.store, self.project_id, &seeded.seeds, intent.intent)?;
        // 4 — signals, once. `candidate_fqns` is every seed plus everything expansion
        // reached, deduplicated and capped at SEED_QUERY_CAP right here — the one list stage
        // 5 below ranks, `index.for_candidate` looks up against, and the facts-by-subject
        // query further down runs against. Built once so there is exactly one cap: this used
        // to be built a second time down at the facts query, deduplicated and capped there
        // but not here, so the copy that reached `SignalIndex::build` (and its own fact read
        // via `facts_for_seeds`) was unbounded — a Review intent's changed-symbol seed set
        // blew that copy past SQLITE_MAX_COMPOUND_SELECT before the capped copy downstream
        // ever ran.
        let findings = self.findings(None, None, None)?;
        let mut candidate_fqns: Vec<String> = seeded
            .seeds
            .iter()
            .map(|s| s.symbol.fqn.clone())
            .chain(reached.items.iter().map(|i| i.fqn.clone()))
            .collect();
        // Seeds and what they reach can name the same symbol twice; dedup with a seen-set
        // (not a `BTreeSet`, which would reorder) so seeds stay ahead of expansion items —
        // the cap below truncates the tail, so order decides what survives it.
        let mut seen = std::collections::HashSet::with_capacity(candidate_fqns.len());
        candidate_fqns.retain(|fqn| seen.insert(fqn.clone()));
        let mut cap_note = None;
        if candidate_fqns.len() > SEED_QUERY_CAP {
            cap_note = Some(format!(
                "{} symbols are relevant here; memory was queried for the first \
                 {SEED_QUERY_CAP}, so a fact about the outer edge of the expansion may be \
                 missing",
                candidate_fqns.len()
            ));
            candidate_fqns.truncate(SEED_QUERY_CAP);
        }
        let index = SignalIndex::build(
            &self.store,
            self.project_id,
            &findings,
            &candidate_fqns,
            self.churn(),
            profile_anchors(&status),
        )?;

        let mut notes = seeded.notes.clone();
        notes.extend(index.notes().iter().cloned());
        if let Some(n) = cap_note {
            notes.push(n);
        }
        if let Some(n) = loaded.note {
            notes.push(n);
        }
        if !intent.confident {
            notes.push(
                "intent was not determined from the text, so balanced weights were used".into(),
            );
        }

        // §11 — the package cache. Checked after seeds because the seed set is part of the
        // key: two different prompts that anchor to the same symbols really are the same
        // question, and answering the second from the first is the point.
        let cache_dir = self.root.join(crate::NEXUS_DIR).join("cache");
        let dirty_hash = self.working_tree_fingerprint();
        let key = crate::context::cache::Key {
            intent: intent.intent.as_str(),
            seeds: seeded.seeds.iter().map(|s| s.symbol.fqn.clone()).collect(),
            commit: status.current.commit.as_deref(),
            dirty_hash: &dirty_hash,
            budget_tokens: req.budget_tokens,
            weights_hash: &w.hash(),
            explain: req.explain,
            memory: &self.memory_fingerprint()?,
        };
        if let Some(hit) = crate::context::cache::get(&cache_dir, &key) {
            return Ok(hit);
        }

        // 5 — rank. Seeds score 1.0 on proximity by definition: they are what was asked
        // about. Everything else inherits the graph score that reached it, which is the
        // product of edge weights and confidences along a chain the item still carries.
        let budget = req.budget_tokens.max(1) as f64;
        let mut candidates = Vec::new();
        for seed in &seeded.seeds {
            let text = format!("{}  {}", seed.symbol.fqn, seed.source.as_str());
            let signals = index.for_candidate(
                &seed.symbol.fqn,
                &seed.symbol.file_path,
                crate::impact::is_test(&seed.symbol.file_path, &seed.symbol.fqn),
            );
            let (score, terms) = crate::context::rank::score(
                &crate::context::rank::Inputs {
                    seed_proximity: 1.0,
                    graph_score: 0.0,
                    signals: &signals,
                    token_cost_norm: estimate_tokens(&text) as f64 / budget,
                },
                &w,
            );
            candidates.push(Candidate {
                kind: ItemKind::Symbol,
                label: seed.symbol.fqn.clone(),
                anchor: Some(CodeRef {
                    file: seed.symbol.file_path.clone(),
                    line: seed.symbol.start_line.max(0) as u32,
                    note: String::new(),
                }),
                why: format!("seed: {}", seed.why),
                text,
                score,
                terms,
                component: seed.symbol.file_path.clone(),
            });
        }
        for item in &reached.items {
            let text = format!("{}  depth {}", item.fqn, item.depth);
            let signals = index.for_candidate(
                &item.fqn,
                &item.file,
                crate::impact::is_test(&item.file, &item.fqn),
            );
            let (score, terms) = crate::context::rank::score(
                &crate::context::rank::Inputs {
                    seed_proximity: 0.0,
                    graph_score: item.score,
                    signals: &signals,
                    token_cost_norm: estimate_tokens(&text) as f64 / budget,
                },
                &w,
            );
            let via = item
                .path
                .first()
                .map(|h| format!("{} {}", h.edge, h.from))
                .unwrap_or_else(|| "graph".into());
            candidates.push(Candidate {
                kind: ItemKind::Symbol,
                label: item.fqn.clone(),
                anchor: Some(CodeRef {
                    file: item.file.clone(),
                    line: item.line.max(0) as u32,
                    note: String::new(),
                }),
                why: format!("{}: via {via}", reached.direction),
                text,
                score,
                terms,
                component: item.file.clone(),
            });
        }
        // Facts about anything the request reached. Knowledge competes with code on the same
        // scale, which is the point of one formula: a fact that answers the question outranks
        // a symbol that merely mentions it. Seeds *and* what they reach: a fact about a
        // method the change calls is exactly as relevant as one about the method being
        // changed — that is what expansion is for, and matching on seeds alone dropped the
        // idempotency fact from a package about the controller that enforces it. Runs against
        // `candidate_fqns` from stage 4 above, already deduplicated and capped — not a second
        // seed list with its own cap to keep in sync with the first.
        let current_scan = self.current_scan_id()?;
        for row in crate::memory::rank(
            self.store
                .facts_for_seeds(self.project_id, &candidate_fqns)?,
            &candidate_fqns,
            current_scan,
        ) {
            let Some(subject) = row.subject.clone() else {
                continue;
            };
            let anchor = row
                .evidence_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<Vec<CodeRef>>(j).ok())
                .and_then(|refs| refs.into_iter().next());
            // Relevance to the *seeds*, not to itself. The signal index answers "is there a
            // fact about this symbol", which is trivially true of a fact asked about its own
            // subject — and that let a fact about an unrelated module into every package
            // until a test asked whether it belonged there.
            let to_seeds = crate::memory::subject_match(Some(&subject), &candidate_fqns);
            if !candidate_fqns.is_empty() && to_seeds <= 0.3 {
                continue;
            }
            let text = format!("{}  {}  [{}]", row.key, row.claim, row.source);
            let signals = crate::context::Signals {
                fact: crate::memory::relevance(&row, &candidate_fqns, current_scan),
                ..Default::default()
            };
            let (score, terms) = crate::context::rank::score(
                &crate::context::rank::Inputs {
                    // A fact about the very symbol the request names is as close to the seed
                    // as anything gets, and `to_seeds` already measured it. Passing zero here
                    // threw away the strongest signal a fact has: six facts about
                    // `SafeWriter` scored 0.10 against a 0.15 floor while `SafeWriter` itself
                    // scored 1.36, so nothing the project had learned ever surfaced.
                    seed_proximity: to_seeds,
                    graph_score: 0.0,
                    signals: &signals,
                    token_cost_norm: estimate_tokens(&text) as f64 / budget,
                },
                &w,
            );
            candidates.push(Candidate {
                kind: ItemKind::Fact,
                label: row.key.clone(),
                anchor,
                why: format!("fact about {subject}"),
                text,
                score,
                terms,
                // The subject is the component, so §7's diversity guard applies to knowledge
                // too. Without it a well-documented symbol pushed its own symbols out of the
                // package: importing 678 claims put six about `SafeWriter` in and left two of
                // `SafeWriter`'s own methods out.
                component: subject.clone(),
            });
        }

        // 6 — budget.
        let mut ledger = InclusionLedger::default();
        let considered = candidates.len();

        // 7 — package. Built empty first, so the budget is charged for the envelope it is
        // about to be spent inside. The profile, the notes and the basis are not free, and a
        // budget that only counts items is a budget that misses most of the payload.
        let mut package = ContextPackage {
            purpose: req.purpose,
            project: ProjectSummary {
                name: status.project.clone(),
                profile: status.profile.clone(),
                files: status.files,
                symbols: status.symbols,
                scope_warning: None,
            },
            items_included: 0,
            items: Vec::new(),
            ledger: InclusionLedger::default(),
            basis: PackageBasis {
                scan_uid: baseline.scan_uid,
                commit: status.current.commit.clone(),
                dirty: status.current.dirty,
                selection: "ranked: intent, seeds, expand, signals, weighted sum, density budget"
                    .into(),
            },
            budget_tokens: req.budget_tokens,
            tokens_estimated: 0,
            items_considered: considered,
            intent: Some(intent),
            notes,
        };
        let (items, tokens_estimated) = context::fill(
            candidates,
            req.budget_tokens,
            envelope_cost(&package),
            context::Selection::Ranked {
                min_score_x1000: (w.min_score * 1000.0) as i64,
                max_per_component: w.max_per_component,
            },
            &mut ledger,
        );
        package.items_included = items.len();
        package.items = items;
        package.ledger = ledger;
        package.tokens_estimated = tokens_estimated;
        let package = finish(package, req);
        crate::context::cache::put(&cache_dir, &key, &package);
        Ok(package)
    }

    /// A fingerprint of what this project remembers, for the cache key.
    ///
    /// A single counter, bumped by every fact and finding write — not a `COUNT(*)` over
    /// them. Cheap enough to run on every request, which is the point — the alternative is a
    /// cache that serves an answer from before the thing it should have known.
    fn memory_fingerprint(&self) -> Result<String> {
        Ok(self.store.memory_version(self.project_id)?.to_string())
    }

    /// A fingerprint of the uncommitted state, for the cache key.
    ///
    /// An agent editing files without committing is the normal case, so a key over HEAD alone
    /// would serve a package describing code that no longer exists (R9). Cheap by design: the
    /// set of dirty paths, not their contents — a rescan is what notices a content change,
    /// and it moves the scan uid the package already carries.
    fn working_tree_fingerprint(&self) -> String {
        let Some(repo) = self.repo.as_ref() else {
            return "no-vcs".into();
        };
        match repo.dirty_paths() {
            Ok(paths) if paths.is_empty() => "clean".into(),
            Ok(paths) => blake3::hash(paths.join("\u{1f}").as_bytes()).to_hex()[..16].to_string(),
            Err(_) => "unknown".into(),
        }
    }
    /// Every live fact as `(key, validated_count, durable)`.
    ///
    /// The lifecycle behind the retrieval view, for a caller that needs the state rather than
    /// the ranking — the Markdown exporter, and the tests that pin §3's transitions.
    pub fn fact_states(&self) -> Result<Vec<(String, i64, bool)>> {
        Ok(self
            .store
            .facts(self.project_id, None)?
            .into_iter()
            .map(|f| (f.key, f.validated_count, f.durable))
            .collect())
    }
    /// Every live fact, grouped by namespace, for the Markdown view (§6).
    ///
    /// Returns data, not files: the engine does not write to the project, and a view that the
    /// core wrote directly would be one more thing that could touch a developer's tree.
    pub fn memory_export(&self) -> Result<Vec<(String, Vec<crate::memory::ExportedFact>)>> {
        let mut by_namespace: std::collections::BTreeMap<String, Vec<_>> = Default::default();
        for row in self.store.facts(self.project_id, None)? {
            let f = crate::memory::ExportedFact::from_row(&row);
            by_namespace.entry(f.namespace.clone()).or_default().push(f);
        }
        for facts in by_namespace.values_mut() {
            facts.sort_by(|a, b| a.key.cmp(&b.key));
        }
        Ok(by_namespace.into_iter().collect())
    }
    /// Everything portable about this project's memory (§7).
    ///
    /// Read-only, and evidence travels as `path:line` references. Never source text: a
    /// knowledge file carrying code would be a second copy of the repository with none of its
    /// access control, and the whole point is that this is safe to commit.
    pub fn export_portable(&self) -> Result<crate::portable::Portable> {
        let facts = self
            .store
            .facts(self.project_id, None)?
            .iter()
            .map(|r| {
                let e = crate::memory::ExportedFact::from_row(r);
                crate::portable::PortableFact {
                    key: r.key.clone(),
                    scope: r.scope.clone(),
                    subject: r.subject.clone(),
                    claim: r.claim.clone(),
                    source: r.source.clone(),
                    confidence: r.confidence,
                    evidence: e.evidence,
                }
            })
            .collect();
        let findings = self
            .findings(None, None, None)?
            .into_iter()
            .map(|f| crate::portable::PortableFinding {
                fingerprint: f.slug.clone(),
                capability: f.capability,
                uid: f.uid,
                title: f.title,
                finding_type: f.finding_type,
                severity: f.severity,
                status: f.status,
                component: f.component,
                file: f.file,
                line: f.line,
            })
            .collect();
        Ok(crate::portable::Portable {
            format: crate::portable::FORMAT,
            project: self.name(),
            exported_at: nexus_store::now(),
            facts,
            findings,
        })
    }

    /// Merge a portable document. Conflicts are reported and skipped.
    ///
    /// Two people who believe different things under one fact key have a disagreement, and
    /// picking one silently produces a database that says something neither of them said. So
    /// a differing claim is a line in the report and nothing else, and the local row stands.
    pub fn import_portable(
        &mut self,
        doc: &crate::portable::Portable,
    ) -> Result<crate::portable::ImportReport> {
        let mut report = crate::portable::ImportReport::default();
        if doc.format > crate::portable::FORMAT {
            return Err(EngineError::Unsupported(format!(
                "this file is format {} and this build reads {} — upgrade nexus",
                doc.format,
                crate::portable::FORMAT
            )));
        }

        let existing: std::collections::BTreeMap<String, String> = self
            .store
            .facts(self.project_id, None)?
            .into_iter()
            .map(|f| (f.key, f.claim))
            .collect();

        for f in &doc.facts {
            match existing.get(&f.key) {
                Some(claim) if claim == &f.claim => {
                    report.facts_unchanged += 1;
                    continue;
                }
                Some(claim) => {
                    report.conflicts.push(format!(
                        "fact {}: here \"{claim}\", incoming \"{}\" — kept the local one",
                        f.key, f.claim
                    ));
                    continue;
                }
                None => {}
            }
            let evidence = f
                .evidence
                .iter()
                .filter_map(|e| {
                    let (file, line) = e.rsplit_once(':')?;
                    Some(CodeRef {
                        file: file.to_string(),
                        line: line.parse().ok()?,
                        note: String::new(),
                    })
                })
                .collect();
            self.record_fact(FactInput {
                key: f.key.clone(),
                scope: f.scope.clone(),
                subject: f.subject.clone(),
                claim: f.claim.clone(),
                source: f.source.clone(),
                evidence,
                confidence: f.confidence,
            })?;
            report.facts_added += 1;
        }

        // Findings are identified by fingerprint, which is what makes the same defect on two
        // machines one finding rather than two. The local uid is not carried across: a
        // display id is local, and importing one would collide with a number already in use.
        let here: std::collections::BTreeMap<String, String> = self
            .findings(None, None, None)?
            .into_iter()
            .map(|f| (f.slug, f.status))
            .collect();
        for f in &doc.findings {
            match here.get(&f.fingerprint) {
                Some(status) if status == &f.status => report.findings_unchanged += 1,
                Some(status) => report.conflicts.push(format!(
                    "finding {}: here {status}, incoming {} — kept the local status",
                    f.fingerprint, f.status
                )),
                None => report.conflicts.push(format!(
                    "finding {} ({}) is not present here — a finding is produced by running a \
                     capability, not by importing one",
                    f.fingerprint, f.title
                )),
            }
        }
        Ok(report)
    }

    /// How many verification attempts have been recorded against findings.
    pub fn verification_attempts(&self) -> Result<i64> {
        Ok(self.store.verification_attempt_count(self.project_id)?)
    }
    /// How many symbols a snapshot for this scope would hold. Exists so the saving in
    /// roadmap 5.4 is measurable rather than asserted.
    pub fn context_symbol_count(&self, paths: Option<Vec<String>>) -> Result<usize> {
        Ok(self
            .store
            .symbol_facts_for(self.project_id, paths.as_deref())?
            .len())
    }
    /// How many screen strings are indexed.
    pub fn ui_string_count(&self) -> Result<i64> {
        Ok(self.store.ui_string_count(self.project_id)?)
    }
    /// How many verification runs this project has recorded.
    pub fn test_run_count(&self) -> Result<i64> {
        Ok(self.store.test_run_count(self.project_id)?)
    }
    /// How many commits this project's ledger holds.
    pub fn commit_count(&self) -> Result<i64> {
        Ok(self.store.commit_count(self.project_id)?)
    }
    /// Per-path churn over the history window, normalised to 0.0..=1.0.
    pub fn churn(&self) -> std::collections::HashMap<String, f64> {
        crate::history::churn(self.repo.as_ref())
    }
    /// The scan before the current baseline — what `--changed` is measured against.
    pub fn previous_scan_id(&self) -> Result<Option<i64>> {
        Ok(self.store.previous_scan_id(self.project_id)?)
    }
    pub fn capability_list(&self) -> Vec<CapabilityInfo> {
        self.capabilities
            .all()
            .map(|c| CapabilityInfo {
                id: c.id().to_string(),
                finding_prefix: c.finding_prefix().to_string(),
                describes: c.describe().to_string(),
            })
            .collect()
    }
    /// Findings, optionally narrowed to one capability.
    pub fn findings(
        &self,
        capability: Option<&str>,
        status: Option<&str>,
        severity: Option<&str>,
    ) -> Result<Vec<FindingSummary>> {
        Ok(self
            .store
            .findings(
                self.project_id,
                &nexus_store::FindingQuery {
                    status,
                    severity,
                    capability,
                },
            )?
            .into_iter()
            .map(to_summary)
            .collect())
    }
    pub fn finding(&self, uid: &str) -> Result<Option<FindingDetail>> {
        let Some(row) = self
            .store
            .findings(
                self.project_id,
                &nexus_store::FindingQuery {
                    capability: Some("bughunter"),
                    ..Default::default()
                },
            )?
            .into_iter()
            .find(|b| b.uid.eq_ignore_ascii_case(uid))
        else {
            return Ok(None);
        };
        let fingerprint = row.fingerprint.clone();
        let evidence: Vec<CodeRef> = self
            .store
            .finding_evidence(self.project_id, &row.uid)?
            .as_deref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        let history = self
            .store
            .finding_history(self.project_id, &row.uid)?
            .into_iter()
            .map(|e| FindingEvent {
                scan_uid: e.scan_uid,
                commit: e.commit,
                status: e.status,
                confidence: e.confidence,
            })
            .collect();
        Ok(Some(FindingDetail {
            summary: to_summary(row),
            fingerprint,
            evidence,
            history,
        }))
    }
    /// Dismiss a finding. A human decision is sticky: a later scan will not re-open it.
    pub fn ignore_finding(&self, uid: &str) -> Result<bool> {
        Ok(self
            .store
            .set_finding_status(self.project_id, uid, "IGNORED")?)
    }
    /// Blast radius of a symbol, a file, or a bare name.
    ///
    /// Returns `Ambiguous` rather than picking one of several matches: in the face of
    /// ambiguity, refuse the temptation to guess. That is the same contract the
    /// clarification protocol formalizes for the investigation entry point.
    pub fn impact(&self, q: &ImpactQuery) -> Result<Resolved<ImpactReport>> {
        let matches = self.store.find_symbols(self.project_id, &q.target, 25)?;
        if matches.is_empty() {
            return Ok(Resolved::NotFound {
                target: q.target.clone(),
            });
        }
        // An exact FQN match, or every symbol in one file, is unambiguous.
        let exact: Vec<SymbolRef> = matches
            .iter()
            .filter(|m| m.fqn == q.target)
            .cloned()
            .collect();
        let same_file = matches.iter().all(|m| m.file_path == matches[0].file_path)
            && (q.target.contains('/') || q.target.ends_with(".java"));

        let seeds = if !exact.is_empty() {
            exact
        } else if matches.len() == 1 || same_file {
            matches.clone()
        } else {
            return Ok(Resolved::Ambiguous(
                matches
                    .into_iter()
                    .map(|m| SeedRef {
                        fqn: m.fqn,
                        kind: m.kind,
                        file: m.file_path,
                        line: m.start_line,
                    })
                    .collect(),
            ));
        };

        Ok(Resolved::One(impact::run(
            &self.store,
            self.project_id,
            &seeds,
            q,
        )?))
    }
    /// One symbol in detail, or the candidates when the target is ambiguous.
    pub fn symbol(&self, target: &str) -> Result<Resolved<SymbolDetail>> {
        let mut matches = self.store.find_symbols(self.project_id, target, 25)?;
        if matches.is_empty() {
            // A name that no longer exists may have moved. Consulting aliases before
            // declaring it unknown is what makes a rename survivable for a caller that
            // still knows the old name.
            if let Some(aliased) = self.store.resolve_alias(self.project_id, target)? {
                matches = vec![aliased];
            } else {
                return Ok(Resolved::NotFound {
                    target: target.to_string(),
                });
            }
        }
        let exact: Vec<_> = matches
            .iter()
            .filter(|m| m.fqn == target)
            .cloned()
            .collect();
        let chosen = if let Some(one) = exact.first() {
            one.clone()
        } else if matches.len() == 1 {
            matches[0].clone()
        } else {
            return Ok(Resolved::Ambiguous(
                matches
                    .into_iter()
                    .map(|m| SeedRef {
                        fqn: m.fqn,
                        kind: m.kind,
                        file: m.file_path,
                        line: m.start_line,
                    })
                    .collect(),
            ));
        };

        let source = self.read_lines(&chosen.file_path, chosen.start_line as usize, 80);
        Ok(Resolved::One(SymbolDetail {
            fqn: chosen.fqn.clone(),
            kind: chosen.kind,
            file: chosen.file_path,
            line: chosen.start_line,
            depends_on: self
                .store
                .edges_out(chosen.id)?
                .into_iter()
                .map(|n| Neighbourhood {
                    fqn: n.fqn,
                    edge: n.edge_type.as_str(),
                    resolution: n.resolution.as_str(),
                    confidence: n.confidence,
                })
                .collect(),
            depended_on_by: self
                .store
                .edges_into(chosen.id)?
                .into_iter()
                .map(|n| Neighbourhood {
                    fqn: n.fqn,
                    edge: n.edge_type.as_str(),
                    resolution: n.resolution.as_str(),
                    confidence: n.confidence,
                })
                .collect(),
            source,
        }))
    }
    /// A capped excerpt. Source is expensive context, so it is opt-in and bounded rather
    /// than attached to every result.
    fn read_lines(&self, rel: &str, start: usize, max: usize) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join(rel)).ok()?;
        let lines: Vec<&str> = text
            .lines()
            .skip(start.saturating_sub(1))
            .take(max)
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
    /// The questions a person or an agent actually has, each answered from the index.
    ///
    /// This lives here rather than in an adapter because every answer needs more than one
    /// query, and the rule is that a caller needing two `Engine` calls has found a missing
    /// `Engine` method.
    pub fn ask(&self, q: &Question) -> Result<Answer> {
        match q {
            Question::Changed => Ok(Answer::Changed {
                since: self.status()?.baseline.and_then(|b| b.scan_uid),
                symbols: self
                    .changes(Some("symbol"))?
                    .into_iter()
                    .filter_map(|(_, _, t, _)| t)
                    .collect(),
                files: self.changes(Some("file"))?.len(),
            }),

            // "What is affected by this change?" and "Where is this symbol used?" are the
            // same traversal asked from two directions, so they share an answer.
            Question::Affected(target) => {
                let query = ImpactQuery {
                    target: target.clone(),
                    direction: crate::impact::Direction::Reverse,
                    ..Default::default()
                };
                match self.impact(&query)? {
                    Resolved::One(r) => Ok(Answer::Affected {
                        target: target.clone(),
                        crossed_seam: r.crossed_seam,
                        symbols: r
                            .items
                            .into_iter()
                            .map(|i| Affected {
                                fqn: i.fqn,
                                score: i.score,
                                min_confidence: i.min_confidence,
                            })
                            .collect(),
                    }),
                    _ => Ok(Answer::Affected {
                        target: target.clone(),
                        symbols: Vec::new(),
                        crossed_seam: 0,
                    }),
                }
            }

            // "Have we seen this problem before?" — the question worth asking before changing
            // anything, and the one persistent knowledge exists to answer.
            Question::Known(target) => Ok(Answer::Known {
                findings: self.findings_for(target)?,
                facts: self.facts(Some(target))?,
                target: target.clone(),
            }),

            Question::Facts => Ok(Answer::Facts {
                facts: self.facts(None)?,
            }),

            Question::Next => Ok(Answer::Next {
                suggestions: self.suggest()?,
            }),
        }
    }
    /// What to look at next: changed symbols, ranked by how much they affect and by whether
    /// anything has gone wrong there before.
    ///
    /// Both halves are already indexed, so this is a ranking rather than an analysis — which
    /// is the point. Nexus does not need to think about what to examine; it already knows.
    ///
    /// **Two queries per candidate, up to forty candidates.** That is worth naming rather
    /// than leaving to be rediscovered. `impact` is inherently one traversal per seed and has
    /// no batched form. `findings_for` looks batchable and is not: it matches on five
    /// conditions — the occurrence's file path, an exact fqn, two `LIKE` forms over the fqn,
    /// and the component — so collapsing it into one query grouped by component would change
    /// which findings count, and reimplementing all five in Rust would leave two definitions
    /// of "a finding about this symbol" free to disagree. Batching it properly needs a store
    /// method that keeps the matching in SQL, where it already is.
    fn suggest(&self) -> Result<Vec<Suggestion>> {
        let changed: Vec<String> = self
            .changes(Some("symbol"))?
            .into_iter()
            .filter_map(|(_, _, target, _)| target)
            .take(40)
            .collect();

        let mut out = Vec::new();
        for fqn in changed {
            let reach = match self.impact(&ImpactQuery {
                target: fqn.clone(),
                ..Default::default()
            })? {
                Resolved::One(r) => r.items.len(),
                _ => 0,
            };
            let prior = self.findings_for(&fqn)?.len();
            // Reach is the cost of being wrong; prior findings are evidence that this code
            // has been wrong before. Neither alone is a good reason to look.
            let score = reach as f64 + prior as f64 * 3.0;
            if score <= 0.0 {
                continue;
            }
            out.push(Suggestion {
                why: match (reach, prior) {
                    (r, 0) => format!("changed, and {r} symbols depend on it"),
                    (0, p) => format!("changed, and {p} findings already exist here"),
                    (r, p) => {
                        format!("changed, {r} symbols depend on it, {p} findings already here")
                    }
                },
                target: fqn,
                score,
            });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.truncate(10);
        Ok(out)
    }
    pub fn graph(&self) -> Result<GraphReport> {
        let counts = self.store.edge_counts(self.project_id)?;
        Ok(GraphReport {
            edges_total: counts.total,
            edges_resolved: counts.resolved,
            edges_external: counts.external,
            edges_sibling: counts.sibling,
            by_resolution: self.store.edges_by_resolution(self.project_id)?,
        })
    }

    /// Every indexed file path. The accuracy harness's coverage denominator: a file with no
    /// edges is still a file the oracle was supposed to index.
    pub fn indexed_files(&self) -> Result<Vec<String>> {
        Ok(self.store.file_paths(self.project_id)?)
    }

    /// The uncollapsed edge list, for out-of-band accuracy measurement.
    ///
    /// [`Engine::graph`] reports how many call sites resolved; this reports what each
    /// candidate actually bound to, which is the only unit in which "is it the *right*
    /// symbol" can be asked.
    pub fn edge_records(&self) -> Result<Vec<EdgeRecord>> {
        Ok(self
            .store
            .all_edges(self.project_id)?
            .into_iter()
            .map(|e| EdgeRecord {
                src_fqn: e.src_fqn,
                src_file: e.src_file,
                site_line: e.site_line,
                edge_type: e.edge_type,
                dst_fqn: e.dst_fqn,
                dst_file: e.dst_file,
                dst_start_line: e.dst_start_line,
                dst_end_line: e.dst_end_line,
                resolution: e.resolution,
                confidence: e.confidence,
            })
            .collect())
    }
    pub fn doctor(&self) -> Result<Vec<Check>> {
        let mut checks = Vec::new();

        checks.push(match &self.repo {
            Some(r) => Check {
                name: "git",
                level: "ok",
                detail: match r.head_sha()? {
                    Some(sha) => format!("repository at HEAD {}", Repo::short_sha(&sha)),
                    None => "repository with no commits yet".into(),
                },
                remedy: None,
            },
            None => Check {
                name: "git",
                level: "warn",
                detail: "not a git repository — every rescan falls back to a full walk".into(),
                remedy: Some("git init".into()),
            },
        });

        let v = self.store.schema_version()?;
        checks.push(Check {
            name: "database",
            level: if v == nexus_store::SCHEMA_VERSION {
                "ok"
            } else {
                "error"
            },
            detail: format!(
                "{NEXUS_DIR}/{DB_FILE}, schema {v} (binary supports {})",
                nexus_store::SCHEMA_VERSION
            ),
            remedy: (v != nexus_store::SCHEMA_VERSION).then(|| "upgrade {bin}".into()),
        });

        checks.push(Check {
            name: "languages",
            level: if self.registry.is_empty() {
                "error"
            } else {
                "ok"
            },
            // Read off the analyzers, not off `tool_versions()`: that map is keyed for cache
            // invalidation, and a key shaped to be unique is not a name to show anyone.
            // Grouped by language because two analyzers may claim one — the GraphQL schema
            // reader reports TypeScript — and listing it twice reads as a bug in the build.
            detail: {
                let mut by_language: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
                for a in self.registry.analyzers() {
                    by_language
                        .entry(a.language().as_str())
                        .or_default()
                        .push(a.grammar_version());
                }
                by_language
                    .iter()
                    .map(|(lang, versions)| format!("{lang} ({})", versions.join(", ")))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            remedy: None,
        });

        if let Some(p) = self.load_profile()? {
            checks.push(Check {
                name: "build system",
                level: if p.build_system.is_some() {
                    "ok"
                } else {
                    "warn"
                },
                detail: p
                    .build_system
                    .clone()
                    .unwrap_or_else(|| "not detected".into()),
                remedy: p
                    .build_system
                    .is_none()
                    .then(|| "set build_system in .nexus/config.toml".into()),
            });
            let unanalyzed: Vec<&str> = p
                .languages
                .iter()
                .filter(|l| !l.analyzed && l.files >= 5)
                .map(|l| l.lang.as_str())
                .collect();
            if !unanalyzed.is_empty() {
                checks.push(Check {
                    name: "coverage",
                    level: "warn",
                    detail: format!(
                        "{} present but not analyzed in this build",
                        unanalyzed.join(", ")
                    ),
                    remedy: Some("those analyzers land in V1 — see docs/roadmap.md".into()),
                });
            }
        }

        match self.store.baseline(self.project_id)? {
            None => checks.push(Check {
                name: "baseline",
                level: "warn",
                detail: "no baseline yet".into(),
                remedy: Some("{bin} scan".into()),
            }),
            Some(b) => {
                let behind = self
                    .repo
                    .as_ref()
                    .zip(b.commit_sha.as_ref())
                    .and_then(|(r, s)| r.commits_since(s))
                    .unwrap_or(0);
                checks.push(Check {
                    name: "baseline",
                    level: if behind > 0 { "warn" } else { "ok" },
                    detail: if behind > 0 {
                        format!("{} is {behind} commits behind HEAD", b.scan_uid)
                    } else {
                        format!("{} is current", b.scan_uid)
                    },
                    remedy: (behind > 0).then(|| "{bin} rescan".into()),
                });
            }
        }

        let bytes = dir_size(&self.root.join(NEXUS_DIR));
        checks.push(Check {
            name: "disk",
            level: "ok",
            detail: format!("{NEXUS_DIR}/ {}", human_bytes(bytes)),
            remedy: None,
        });
        Ok(checks)
    }
}

pub(super) fn to_summary(r: nexus_store::FindingRow) -> FindingSummary {
    FindingSummary {
        capability: r.capability,
        uid: r.uid,
        slug: r.slug,
        title: r.title,
        finding_type: r.finding_type,
        component: r.component,
        severity: r.severity,
        confidence: r.confidence,
        status: r.status,
        detector: r.detector,
        file: r.file,
        line: r.line,
        introduced_commit: r.introduced_commit,
        fixed_commit: r.fixed_commit,
    }
}

/// Files the project profile anchors on: the build file CI would have invoked, the compose
/// file that proved the datastore. A change near one of these is architectural by definition,
/// which is what §6's `w_arch` term is for.
fn profile_anchors(status: &StatusReport) -> Vec<String> {
    let mut out = Vec::new();
    for p in [
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "Cargo.toml",
        "package.json",
        "docker-compose.yml",
        "docker-compose.yaml",
    ] {
        out.push(p.to_string());
    }
    if status.profile.is_none() {
        out.clear();
    }
    out
}

/// Drop the explanation unless it was asked for, then price what will actually be sent.
///
/// `tokens_estimated` used to count the text of the included items and nothing else, while
/// the package on the wire carried the ledger, the score terms, the profile and the JSON
/// itself. It reported 253 tokens and shipped 11,113 — a budget that measures a twentieth of
/// the payload is not a budget. This measures the serialized form, which is what the agent
/// pays for.
/// What the package costs before a single item goes in it: the profile, the basis, the notes,
/// the braces. Measured by serialising the empty shell, because guessing at it is how the
/// number drifts away from what the agent is actually billed.
fn envelope_cost(package: &ContextPackage) -> usize {
    serde_json::to_string(package)
        .map(|s| estimate_tokens(&s))
        .unwrap_or(0)
}

fn finish(mut package: ContextPackage, req: &TaskRequest) -> ContextPackage {
    if !req.explain {
        package.ledger.rows.clear();
        for item in &mut package.items {
            item.terms = Default::default();
        }
    }
    // Serialize once with the field zeroed, so the number does not have to predict its own
    // width. An estimate, and the package says so by carrying the same estimator everywhere.
    package.tokens_estimated = 0;
    package.tokens_estimated = serde_json::to_string(&package)
        .map(|s| estimate_tokens(&s))
        .unwrap_or(0);
    package
}
