//! `scan` — the full index: walk, hash, parse, resolve, and set the baseline.
//!
//! Split out of `engine.rs` by responsibility. `Engine`'s public API is unchanged: an `impl`
//! block in another file of the same module is the same type, and no caller can tell.

use super::*;

impl Engine {
    /// Full scan. The only run that reads the whole repository.
    pub fn scan(&mut self) -> Result<ScanReport> {
        let started = Instant::now();
        let walked = walk::walk(&self.root, &[]);
        let hashed = walk::hash_all(&self.root, &walked);

        let tree: BTreeMap<String, String> = hashed
            .iter()
            .map(|h| (h.path.clone(), h.content_hash.clone()))
            .collect();
        let (commit, dirty) = self.head();
        let (scan_id, scan_uid) = self.store.begin_scan(
            self.project_id,
            ScanKind::Full,
            None,
            commit.as_deref(),
            &walk::working_tree_hash(&tree),
            dirty,
            &self.tool_versions(),
        )?;

        let parsed = parse_all(&self.registry, &self.root, &hashed);
        let existing: BTreeSet<String> = self
            .store
            .live_files(self.project_id)?
            .into_iter()
            .map(|f| f.path)
            .collect();
        let seen: BTreeSet<String> = hashed.iter().map(|h| h.path.clone()).collect();

        let mut warnings = Vec::new();
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut symbols_indexed = 0usize;

        // Where every fact's evidence points, read before the index is rewritten.
        let anchors = self.fact_anchors(&mut warnings)?;

        let tx = self.store.transaction()?;
        let mut pending_edges: Vec<(FileId, Vec<NewEdge>)> = Vec::new();
        for (file, outcome) in &parsed {
            let (status, error, symbols, edges) = classify(outcome);
            match status {
                ParseStatus::Failed => failed += 1,
                ParseStatus::Skipped => skipped += 1,
                _ => {}
            }
            if let Some(e) = &error {
                warnings.push(format!("{}: {e}", file.path));
            }
            let lang = self
                .registry
                .language_for_path(&file.path)
                .map(|l| l.as_str());
            let file_id = Store::upsert_file(
                &tx,
                self.project_id,
                scan_id,
                &file.path,
                lang,
                &file.content_hash,
                file.size_bytes as i64,
                Some(file.loc as i64),
                Some(file.mtime_ns),
                status,
                error.as_deref(),
            )?;
            if let Some(syms) = symbols {
                symbols_indexed +=
                    Store::replace_symbols(&tx, self.project_id, file_id, scan_id, &syms)?;
            }
            if !edges.is_empty() {
                pending_edges.push((file_id, edges));
            }
        }
        for gone in existing.difference(&seen) {
            Store::mark_file_deleted(&tx, self.project_id, gone, scan_id)?;
        }
        // Edges are written only after every symbol exists: an edge's source must be
        // resolvable, and resolution needs the complete symbol table — which is precisely
        // why an analyzer cannot do this itself.
        for (file_id, edges) in &pending_edges {
            Store::replace_edges_for_file(&tx, self.project_id, *file_id, scan_id, edges)?;
        }
        // An external graph, if the project asked for one. After every parsed file has a
        // row, so an unanalysed file can be given a module node without racing the walk.
        if let crate::graphify::Mode::On(path) = crate::graphify::mode(&self.root) {
            let (edges, note) = crate::graphify::read(&path);
            if let Some(note) = note {
                warnings.push(note);
            }
            let mut imported = 0usize;
            let mut nodes: BTreeMap<String, i64> = BTreeMap::new();
            for edge in &edges {
                let mut resolve = |p: &str| -> Result<Option<i64>> {
                    if let Some(id) = nodes.get(p) {
                        return Ok(Some(*id));
                    }
                    // Only a file this scan actually saw. An edge naming a path outside the
                    // index is the external graph's business, not ours to invent a node for.
                    let Some(file_id) = Store::file_id_by_path(&tx, self.project_id, p)? else {
                        return Ok(None);
                    };
                    let id =
                        Store::upsert_module_symbol(&tx, self.project_id, file_id, scan_id, p)?;
                    nodes.insert(p.to_string(), id);
                    Ok(Some(id))
                };
                let (Some(src), Some(dst)) = (resolve(&edge.from)?, resolve(&edge.to)?) else {
                    continue;
                };
                Store::insert_external_edge(
                    &tx,
                    self.project_id,
                    scan_id,
                    src,
                    dst,
                    edge.kind.as_deref().unwrap_or("imports"),
                    crate::graphify::confidence(edge.confidence),
                )?;
                imported += 1;
            }
            if imported > 0 {
                warnings.push(format!(
                    "{imported} edges imported from {} at confidence ≤ {} — they are outside \
                     the resolution rate because nobody parsed them",
                    path.display(),
                    crate::graphify::MAX_CONFIDENCE
                ));
            }
        }

        // The commit ledger. Append-only and idempotent, so a rescan that sees the same
        // history re-inserts nothing. Recorded here rather than in a separate pass because
        // it belongs to the same transaction as the index it describes.
        for c in self
            .repo
            .as_ref()
            .and_then(|r| r.recent_commits(nexus_vcs::HISTORY_WINDOW_COMMITS).ok())
            .unwrap_or_default()
        {
            Store::insert_commit(&tx, self.project_id, &crate::history::to_record(c))?;
        }
        let resolve = Store::resolve_edges(&tx, self.project_id)?;
        let facts_invalidated =
            Store::invalidate_moved_facts(&tx, self.project_id, &anchors, &nexus_store::now())?
                .len();
        tx.commit().map_err(nexus_store::StoreError::from)?;
        if resolve.unresolved > 0 {
            warnings.push(format!(
                "{} edges point inside the project but matched no symbol (overloads, inherited methods)",
                resolve.unresolved
            ));
        }
        // The most consequential thing a scan can discover about itself: it is looking at
        // one module of something larger. Silence here is what lets an impact query report
        // a small blast radius with total confidence.
        if resolve.sibling >= SIBLING_WARN_FLOOR {
            let owner = resolve.owner.as_deref().unwrap_or("this project");
            warnings.push(format!(
                "{} edges point at {owner}.* code that was not scanned — this looks like one \
                 module of a larger project; scan from the repository root to see it",
                resolve.sibling
            ));
        }

        let health = if failed > 0 {
            Health::Degraded
        } else {
            Health::Ok
        };
        self.store.finish_scan(
            scan_id,
            ScanStatus::Ok,
            hashed.len() as i64,
            failed as i64,
            symbols_indexed as i64,
            None,
        )?;
        self.store.set_baseline(self.project_id, scan_id)?;

        Ok(ScanReport {
            scan_uid,
            kind: "full",
            commit,
            dirty,
            files_scanned: hashed.len(),
            files_failed: failed,
            files_skipped: skipped,
            symbols_indexed,
            facts_invalidated,
            edges_resolved: resolve.resolved(),
            edges_total: resolve.total,
            edges_external: resolve.external,
            edges_sibling: resolve.sibling,
            health,
            warnings,
            duration_ms: started.elapsed().as_millis(),
        })
    }
}
