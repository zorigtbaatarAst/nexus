//! `rescan` — the incremental cascade, and the rename resolution that makes it honest.
//!
//! Renames are resolved after every changed file has been seen, never per file: the two
//! halves of a package move live in different files. That is why the buffering lives here
//! rather than inside the per-file loop.

use super::*;

impl Engine {
    /// The everyday command. Cost is proportional to what changed, not to how much exists.
    pub fn rescan(&mut self) -> Result<RescanReport> {
        let started = Instant::now();
        let Some(baseline) = self.store.baseline(self.project_id)? else {
            return Err(EngineError::NoBaseline);
        };
        let (commit, dirty) = self.head();
        let base_rev = Revision {
            scan_uid: Some(baseline.scan_uid.clone()),
            commit: baseline.commit_sha.clone(),
            dirty: baseline.dirty,
        };
        let cur_rev = Revision {
            scan_uid: None,
            commit: commit.clone(),
            dirty,
        };

        // A grammar or analyzer upgrade must force a re-parse even though every content hash
        // still matches — otherwise the index silently keeps the symbols the old grammar
        // produced, indefinitely, with no error anywhere.
        let forced_full = (baseline.tool_versions_json != self.tool_versions())
            .then(|| "analyzer or grammar version changed since the baseline".to_string());

        // ── Tier 0: repo gate ──
        if forced_full.is_none()
            && !dirty
            && !baseline.dirty
            && commit.is_some()
            && commit == baseline.commit_sha
        {
            return Ok(RescanReport {
                scan_uid: None,
                baseline: base_rev,
                current: cur_rev,
                unchanged: true,
                forced_full: None,
                files_changed: 0,
                files_deleted: 0,
                symbols_changed: 0,
                facts_invalidated: 0,
                items: Vec::new(),
                files_failed: 0,
                health: Health::Ok,
                warnings: Vec::new(),
                duration_ms: started.elapsed().as_millis(),
            });
        }

        // ── Tier 1: candidate file set ──
        let stored: BTreeMap<String, nexus_store::FileRow> = self
            .store
            .live_files(self.project_id)?
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();

        let mut warnings = Vec::new();
        let (candidates, deleted, exhaustive) =
            self.candidates(&baseline, forced_full.is_some(), &mut warnings);

        let mut changed: Vec<HashedFile> = Vec::new();
        let mut deleted_paths: BTreeSet<String> = deleted;

        for path in &candidates {
            let abs = self.root.join(path);
            let Ok(meta) = std::fs::metadata(&abs) else {
                if stored.contains_key(path) {
                    deleted_paths.insert(path.clone());
                }
                continue;
            };
            let size = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            if let Some(row) = stored.get(path) {
                // stat fast path: ~1µs, and it eliminates hashing for almost everything.
                if !forced_full.is_some()
                    && row.size_bytes == size as i64
                    && row.mtime_ns == Some(mtime)
                {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&abs) else {
                    continue;
                };
                let hash = walk::hash_bytes(&bytes);
                if hash == row.content_hash && forced_full.is_none() {
                    continue; // touched, not changed
                }
                changed.push(HashedFile {
                    path: path.clone(),
                    size_bytes: size,
                    mtime_ns: mtime,
                    content_hash: hash,
                    loc: bytes.iter().filter(|b| **b == b'\n').count() as u32 + 1,
                });
            } else {
                let Ok(bytes) = std::fs::read(&abs) else {
                    continue;
                };
                changed.push(HashedFile {
                    path: path.clone(),
                    size_bytes: size,
                    mtime_ns: mtime,
                    content_hash: walk::hash_bytes(&bytes),
                    loc: bytes.iter().filter(|b| **b == b'\n').count() as u32 + 1,
                });
            }
        }

        if exhaustive {
            let seen: BTreeSet<&String> = candidates.iter().collect();
            for path in stored.keys() {
                if !seen.contains(path) {
                    deleted_paths.insert(path.clone());
                }
            }
        }

        // Rename detection: a path that vanished and a path that appeared sharing a content
        // hash is a rename, not a delete plus an add. Without this, every package refactor
        // invents a repository full of "new" symbols.
        let renames = detect_renames(&stored, &changed, &mut deleted_paths);

        if changed.is_empty() && deleted_paths.is_empty() {
            return Ok(RescanReport {
                scan_uid: None,
                baseline: base_rev,
                current: cur_rev,
                unchanged: true,
                forced_full,
                files_changed: 0,
                files_deleted: 0,
                symbols_changed: 0,
                facts_invalidated: 0,
                items: Vec::new(),
                files_failed: 0,
                health: Health::Ok,
                warnings,
                duration_ms: started.elapsed().as_millis(),
            });
        }

        // ── record the scan, then Tier 2 ──
        let mut tree: BTreeMap<String, String> = stored
            .iter()
            .map(|(p, f)| (p.clone(), f.content_hash.clone()))
            .collect();
        for c in &changed {
            tree.insert(c.path.clone(), c.content_hash.clone());
        }
        for d in &deleted_paths {
            tree.remove(d);
        }

        let (scan_id, scan_uid) = self.store.begin_scan(
            self.project_id,
            if forced_full.is_some() {
                ScanKind::Full
            } else {
                ScanKind::Incremental
            },
            Some(baseline.scan_id),
            commit.as_deref(),
            &walk::working_tree_hash(&tree),
            dirty,
            &self.tool_versions(),
        )?;

        let parsed = parse_all(&self.registry, &self.root, &changed);
        let mut items = Vec::new();
        let mut failed = 0usize;
        let mut symbols_changed = 0usize;
        // Appearances and disappearances are held until every changed file has been seen:
        // a rename is only visible from both halves at once, and they can be in different
        // files — which is exactly what a package move is.
        let mut appeared: Vec<SymbolDelta> = Vec::new();
        let mut vanished: Vec<SymbolDelta> = Vec::new();

        // Read every symbol set we will need *before* opening the transaction: rusqlite's
        // Transaction holds a mutable borrow of the connection for its entire lifetime.
        let mut old_by_path: BTreeMap<String, BTreeMap<String, nexus_store::SymbolRow>> =
            BTreeMap::new();
        for path in deleted_paths.iter().chain(changed.iter().map(|c| &c.path)) {
            if let Some(row) = stored.get(path) {
                let map = self
                    .store
                    .symbols_for_file(row.id)?
                    .into_iter()
                    .map(|s| (s.fqn.clone(), s))
                    .collect();
                old_by_path.insert(path.clone(), map);
            }
        }

        // Where every fact's evidence points, read against the index this scan is about to
        // rewrite. Resolved here for the same reason `old_by_path` is: the transaction holds
        // the connection.
        let anchors = self.fact_anchors(&mut warnings)?;

        let tx = self.store.transaction()?;

        for path in &deleted_paths {
            if stored.contains_key(path) {
                for s in old_by_path
                    .get(path)
                    .cloned()
                    .unwrap_or_default()
                    .into_values()
                {
                    // A file that disappeared is the commonest source of a rename, so its
                    // symbols are held back with the rest rather than reported as deleted.
                    vanished.push(SymbolDelta {
                        fqn: s.fqn.clone(),
                        path: path.clone(),
                        name: s.name.clone(),
                        sig_hash: s.sig_hash.clone(),
                        body_hash: s.body_hash.clone(),
                        old_id: Some(s.id),
                    });
                }
            }
            let ct = if renames.contains_key(path) {
                ChangeType::Renamed
            } else {
                ChangeType::Deleted
            };
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "file",
                    entity_id: stored.get(path).map(|r| r.id),
                    path: Some(path.clone()),
                    fqn: None,
                    change_type: ct,
                    detail: None,
                    before_hash: stored.get(path).map(|r| r.content_hash.clone()),
                    after_hash: None,
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "file",
                change_type: ct.as_str(),
                kind: None,
                path: Some(path.clone()),
                fqn: None,
                from_fqn: None,
            });
            Store::mark_file_deleted(&tx, self.project_id, path, scan_id)?;
        }

        let mut pending_edges: Vec<(FileId, Vec<NewEdge>)> = Vec::new();
        for (file, outcome) in &parsed {
            let (status, error, symbols, edges) = classify(outcome);
            if !edges.is_empty() {
                pending_edges.push((0, edges));
            }
            if status == ParseStatus::Failed {
                failed += 1;
            }
            if let Some(e) = &error {
                warnings.push(format!("{}: {e}", file.path));
            }

            let was = stored.get(&file.path);
            let old_symbols = old_by_path.get(&file.path).cloned().unwrap_or_default();

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

            let file_ct = if was.is_some() {
                ChangeType::Modified
            } else {
                ChangeType::Added
            };
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "file",
                    entity_id: Some(file_id),
                    path: Some(file.path.clone()),
                    fqn: None,
                    change_type: file_ct,
                    detail: Some("content"),
                    before_hash: was.map(|r| r.content_hash.clone()),
                    after_hash: Some(file.content_hash.clone()),
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "file",
                change_type: file_ct.as_str(),
                kind: None,
                path: Some(file.path.clone()),
                fqn: None,
                from_fqn: None,
            });

            // ── Tier 2: symbol-level diff ──
            if let Some(new_symbols) = symbols {
                for s in &new_symbols {
                    let kind = match old_symbols.get(&s.fqn) {
                        None => {
                            // Held back: this may be half of a rename, and that can only be
                            // decided once every changed file has been seen — the other half
                            // is usually in a different file.
                            appeared.push(SymbolDelta {
                                fqn: s.fqn.clone(),
                                path: file.path.clone(),
                                name: s.name.clone(),
                                sig_hash: s.sig_hash.clone(),
                                body_hash: s.body_hash.clone(),
                                old_id: None,
                            });
                            continue;
                        }
                        Some(old) => symbol_change(old, s),
                    };
                    let Some(kind) = kind else { continue };
                    Store::insert_change(
                        &tx,
                        scan_id,
                        &ChangeRecord {
                            entity: "symbol",
                            entity_id: None,
                            path: Some(file.path.clone()),
                            fqn: Some(s.fqn.clone()),
                            change_type: kind.change_type(),
                            detail: kind.detail(),
                            before_hash: old_symbols.get(&s.fqn).map(|o| o.body_hash.clone()),
                            after_hash: Some(s.body_hash.clone()),
                            commit_sha: commit.clone(),
                        },
                    )?;
                    items.push(ChangeItem {
                        entity: "symbol",
                        change_type: kind.change_type().as_str(),
                        kind: Some(kind.as_str()),
                        path: Some(file.path.clone()),
                        fqn: Some(s.fqn.clone()),
                        from_fqn: None,
                    });
                    symbols_changed += 1;
                }
                let new_fqns: BTreeSet<&str> = new_symbols.iter().map(|s| s.fqn.as_str()).collect();
                for (fqn, old) in &old_symbols {
                    if new_fqns.contains(fqn.as_str()) {
                        continue;
                    }
                    vanished.push(SymbolDelta {
                        fqn: fqn.clone(),
                        path: file.path.clone(),
                        name: old.name.clone(),
                        sig_hash: old.sig_hash.clone(),
                        body_hash: old.body_hash.clone(),
                        old_id: Some(old.id),
                    });
                }
                Store::replace_symbols(&tx, self.project_id, file_id, scan_id, &new_symbols)?;
            }
            if let Some(last) = pending_edges.last_mut() {
                if last.0 == 0 {
                    last.0 = file_id;
                }
            }
        }
        // ── rename resolution ──
        //
        // Every symbol is now written, so an appearance can be matched to a disappearance
        // and the pair collapsed into one rename. Matching on (name, sig_hash, body_hash)
        // is what survives a package move: the FQN changes and nothing else does.
        let renamed = resolve_symbol_renames(&appeared, &vanished);

        for (old_idx, new_idx) in &renamed {
            let old = &vanished[*old_idx];
            let new = &appeared[*new_idx];
            if let Some(id) = Store::symbol_id_by_fqn(&tx, self.project_id, &new.fqn)? {
                Store::record_alias(&tx, self.project_id, &old.fqn, id, scan_id)?;
            }
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "symbol",
                    entity_id: None,
                    path: Some(new.path.clone()),
                    fqn: Some(new.fqn.clone()),
                    change_type: ChangeType::Renamed,
                    detail: None,
                    before_hash: Some(old.body_hash.clone()),
                    after_hash: Some(new.body_hash.clone()),
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "symbol",
                change_type: "renamed",
                kind: Some(ChangeKind::Renamed.as_str()),
                path: Some(new.path.clone()),
                fqn: Some(new.fqn.clone()),
                from_fqn: Some(old.fqn.clone()),
            });
            symbols_changed += 1;
        }

        let matched_old: BTreeSet<usize> = renamed.iter().map(|(o, _)| *o).collect();
        let matched_new: BTreeSet<usize> = renamed.iter().map(|(_, n)| *n).collect();

        for (i, d) in vanished.iter().enumerate() {
            if matched_old.contains(&i) {
                continue;
            }
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "symbol",
                    entity_id: d.old_id,
                    path: Some(d.path.clone()),
                    fqn: Some(d.fqn.clone()),
                    change_type: ChangeType::Deleted,
                    detail: None,
                    before_hash: Some(d.body_hash.clone()),
                    after_hash: None,
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "symbol",
                change_type: "deleted",
                kind: Some(ChangeKind::Deleted.as_str()),
                path: Some(d.path.clone()),
                fqn: Some(d.fqn.clone()),
                from_fqn: None,
            });
            symbols_changed += 1;
        }

        for (i, d) in appeared.iter().enumerate() {
            if matched_new.contains(&i) {
                continue;
            }
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "symbol",
                    entity_id: None,
                    path: Some(d.path.clone()),
                    fqn: Some(d.fqn.clone()),
                    change_type: ChangeType::Added,
                    detail: None,
                    before_hash: None,
                    after_hash: Some(d.body_hash.clone()),
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "symbol",
                change_type: "added",
                kind: Some(ChangeKind::Added.as_str()),
                path: Some(d.path.clone()),
                fqn: Some(d.fqn.clone()),
                from_fqn: None,
            });
            symbols_changed += 1;
        }

        for (file_id, edges) in &pending_edges {
            if *file_id != 0 {
                Store::replace_edges_for_file(&tx, self.project_id, *file_id, scan_id, edges)?;
            }
        }
        // Tier 3: an added or renamed symbol can resolve edges elsewhere without those
        // files changing, so resolution re-runs over the unresolved set every scan.
        Store::resolve_edges(&tx, self.project_id)?;
        // A fact about code this scan changed or removed is a trap for the next reader.
        // Inside the transaction, so a crash cannot leave the index new and the memory old.
        let facts_invalidated =
            Store::invalidate_moved_facts(&tx, self.project_id, &anchors, &nexus_store::now())?
                .len();
        tx.commit().map_err(nexus_store::StoreError::from)?;

        let (files, symbols) = self.store.index_counts(self.project_id)?;
        self.store
            .finish_scan(scan_id, ScanStatus::Ok, files, failed as i64, symbols, None)?;
        self.store.set_baseline(self.project_id, scan_id)?;

        Ok(RescanReport {
            scan_uid: Some(scan_uid),
            baseline: base_rev,
            current: cur_rev,
            unchanged: false,
            forced_full,
            files_changed: changed.len(),
            files_deleted: deleted_paths.len(),
            symbols_changed,
            facts_invalidated,
            items,
            files_failed: failed,
            health: if failed > 0 {
                Health::Degraded
            } else {
                Health::Ok
            },
            warnings,
            duration_ms: started.elapsed().as_millis(),
        })
    }
    /// Candidate paths for Tier 1, and whether the set is exhaustive (a full walk, from
    /// which deletions can be inferred) or a git delta (which reports deletions itself).
    fn candidates(
        &self,
        baseline: &nexus_store::Baseline,
        force_full: bool,
        warnings: &mut Vec<String>,
    ) -> (Vec<String>, BTreeSet<String>, bool) {
        // A baseline taken on a dirty tree indexed content that no commit describes, so
        // `git diff <baseline commit>` is not a sufficient candidate set: reverting a file
        // to its committed state leaves git reporting nothing while the index still holds
        // the uncommitted version. The only safe candidate set is then everything — and the
        // stat fast path makes that cheap.
        let dirty_baseline = baseline.dirty;
        if !force_full && !dirty_baseline {
            if let (Some(repo), Some(from)) = (&self.repo, &baseline.commit_sha) {
                match repo.changed_paths_since(from) {
                    Ok(d) => {
                        let changed = d
                            .changed
                            .into_iter()
                            .filter(|p| !walk::is_excluded(p))
                            .collect();
                        let deleted = d
                            .deleted
                            .into_iter()
                            .filter(|p| !walk::is_excluded(p))
                            .collect();
                        return (changed, deleted, false);
                    }
                    Err(VcsError::Unreachable(sha)) => {
                        // Falling back is correct; doing it silently is not. A wrong diff is
                        // far worse than a slow scan.
                        warnings.push(format!(
                            "baseline commit {} is unreachable (force-push, rebase or shallow clone) — fell back to a full walk",
                            Repo::short_sha(&sha)
                        ));
                    }
                    Err(e) => {
                        warnings.push(format!("git diff failed ({e}) — fell back to a full walk"))
                    }
                }
            }
        }
        let walked = walk::walk(&self.root, &[]);
        (
            walked.into_iter().map(|f| f.path).collect(),
            BTreeSet::new(),
            true,
        )
    }
}

/// Which hash moved, and therefore how far the change ripples. ADR-010.
pub(super) fn symbol_change(old: &nexus_store::SymbolRow, new: &NewSymbol) -> Option<ChangeKind> {
    let sig_text_changed = old.signature.as_deref().unwrap_or_default()
        != new.signature.as_deref().unwrap_or_default();
    let old_anns: Vec<String> = old
        .annotations_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    let ann_changed = old_anns != new.annotations;
    let body_changed = old.body_hash != new.body_hash;

    match (sig_text_changed, ann_changed, body_changed) {
        (false, false, false) => None,
        (true, _, true) => Some(ChangeKind::ApiAndBodyChanged),
        (true, _, false) => Some(ChangeKind::ApiChanged),
        // Annotations carry no signature diff and frequently matter more: removing
        // @Transactional changes nothing a compiler notices and everything about correctness.
        (false, true, _) => Some(ChangeKind::ContractChanged),
        (false, false, true) => Some(ChangeKind::BodyChanged),
    }
}
/// One symbol that appeared or disappeared during a rescan.
#[derive(Debug, Clone)]
pub(super) struct SymbolDelta {
    fqn: String,
    path: String,
    name: String,
    sig_hash: String,
    body_hash: String,
    old_id: Option<SymbolId>,
}
/// Pair disappearances with appearances that are the same symbol under a new name.
///
/// The key is `(name, sig_hash, body_hash)`: a package move, a class move or a directory
/// rename changes the FQN and nothing else, so all three survive. A method whose body also
/// changed in the same commit will not match, which is the right call — that is a delete
/// and an add as far as identity goes, and guessing otherwise would attach an old bug
/// history to code that is no longer the same code.
///
/// Only unambiguous 1:1 matches count. Boilerplate accessors and generated equals/hashCode
/// collide on this key constantly, and carrying identity to an arbitrary one of five
/// candidates is worse than reporting a delete and an add.
pub(super) fn resolve_symbol_renames(
    appeared: &[SymbolDelta],
    vanished: &[SymbolDelta],
) -> Vec<(usize, usize)> {
    let key = |d: &SymbolDelta| (d.name.clone(), d.sig_hash.clone(), d.body_hash.clone());

    let mut new_by_key: BTreeMap<(String, String, String), Vec<usize>> = BTreeMap::new();
    for (i, d) in appeared.iter().enumerate() {
        new_by_key.entry(key(d)).or_default().push(i);
    }
    let mut old_by_key: BTreeMap<(String, String, String), Vec<usize>> = BTreeMap::new();
    for (i, d) in vanished.iter().enumerate() {
        old_by_key.entry(key(d)).or_default().push(i);
    }

    let mut out = Vec::new();
    for (k, olds) in &old_by_key {
        if olds.len() != 1 {
            continue;
        }
        if let Some(news) = new_by_key.get(k) {
            if news.len() == 1 && appeared[news[0]].fqn != vanished[olds[0]].fqn {
                out.push((olds[0], news[0]));
            }
        }
    }
    out
}
pub(super) fn detect_renames(
    stored: &BTreeMap<String, nexus_store::FileRow>,
    added: &[HashedFile],
    deleted: &mut BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut by_hash: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for path in deleted.iter() {
        if let Some(row) = stored.get(path) {
            by_hash
                .entry(row.content_hash.as_str())
                .or_default()
                .push(path);
        }
    }
    let mut renames = BTreeMap::new();
    for a in added {
        if stored.contains_key(&a.path) {
            continue;
        }
        if let Some(olds) = by_hash.get(a.content_hash.as_str()) {
            // Only an unambiguous 1:1 match counts. Two identical files moving at once is
            // not a rename anyone can attribute, and guessing would carry symbol identity
            // to the wrong place.
            if olds.len() == 1 {
                renames.insert(olds[0].clone(), a.path.clone());
            }
        }
    }
    renames
}
