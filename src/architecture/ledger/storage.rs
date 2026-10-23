use super::*;
impl Ledger {
    /// Open a Briefcase database, creating a private one when the path is absent.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            match workspace::private_file(path) {
                Ok(_) => {}
                Err(_) if path.exists() => {}
                Err(e) => return Err(e),
            }
        }
        Self::connect(path)
    }
    /// Open an existing Briefcase database without creating one. Readers use this
    /// so a mistyped or unselected path never appears as an empty ledger.
    pub fn open_existing(path: &Path) -> Result<Self> {
        require(
            path.is_file(),
            "NOT_INITIALIZED",
            &format!(
                "No Briefcase database at {}; run `briefcase sync`, `scan` or `capture-text` first",
                path.display()
            ),
        )?;
        Self::connect(path)
    }
    /// Alignment checks use a SQLite reader without schema/bootstrap writes.
    pub fn read_only(path: &Path) -> Result<Self> {
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(Duration::from_secs(10))?;
        require(
            ownership(&db)? == Ownership::Marked,
            "NOT_INITIALIZED",
            "Database has no Briefcase schema",
        )?;
        Ok(Self { db, view_cache: RefCell::new(BTreeMap::new()), retained_cache: RefCell::new(BTreeMap::new()), report_changes: Cell::new(0) })
    }
    /// Check ownership before any schema write. A foreign database is
    /// rejected with its bytes unchanged.
    pub(crate) fn connect(path: &Path) -> Result<Self> {
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(Duration::from_secs(10))?;
        let owner = ownership(&db)?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS namespace (singleton INTEGER PRIMARY KEY CHECK(singleton=1), repository TEXT NOT NULL, intended INTEGER NOT NULL DEFAULT 0, current_view TEXT);
          CREATE TABLE IF NOT EXISTS events (watermark INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, body TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS scans (generation INTEGER PRIMARY KEY, status TEXT NOT NULL, label TEXT UNIQUE, view_id TEXT, diagnostics TEXT NOT NULL DEFAULT '{}', scope TEXT NOT NULL DEFAULT 'source');
          CREATE TABLE IF NOT EXISTS records (id TEXT PRIMARY KEY, family TEXT NOT NULL, body TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS payloads (artifact_id TEXT PRIMARY KEY REFERENCES records(id), bytes BLOB NOT NULL);
          CREATE TABLE IF NOT EXISTS batches (id TEXT PRIMARY KEY, digest TEXT NOT NULL, bundle_digest TEXT NOT NULL, watermark INTEGER NOT NULL REFERENCES events(watermark));
          CREATE TABLE IF NOT EXISTS occurrences (watermark INTEGER NOT NULL REFERENCES events(watermark), assertion_id TEXT NOT NULL REFERENCES records(id), PRIMARY KEY(watermark,assertion_id));
          CREATE TABLE IF NOT EXISTS views (id TEXT PRIMARY KEY, generation INTEGER NOT NULL REFERENCES scans(generation), watermark INTEGER NOT NULL, bundle TEXT NOT NULL, diagnostics TEXT NOT NULL, projection_generation INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS view_capture_states (view_id TEXT PRIMARY KEY REFERENCES views(id), capture_state TEXT);
          CREATE TABLE IF NOT EXISTS view_summaries (view_id TEXT NOT NULL REFERENCES views(id), kind TEXT NOT NULL, key TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(view_id,kind));
          CREATE TABLE IF NOT EXISTS projections (id TEXT PRIMARY KEY, view_id TEXT NOT NULL REFERENCES views(id), generation INTEGER NOT NULL, kind TEXT NOT NULL, watermark INTEGER NOT NULL, body TEXT NOT NULL, UNIQUE(view_id,generation,kind));
          CREATE TABLE IF NOT EXISTS retractions (assertion_id TEXT PRIMARY KEY REFERENCES records(id), watermark INTEGER NOT NULL REFERENCES events(watermark), reason TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS overlays (id TEXT PRIMARY KEY, scope TEXT NOT NULL, watermark INTEGER NOT NULL REFERENCES events(watermark), bundle TEXT NOT NULL, assertion_ids TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS view_overlays (view_id TEXT NOT NULL REFERENCES views(id), overlay_id TEXT NOT NULL REFERENCES overlays(id), PRIMARY KEY(view_id,overlay_id));
          CREATE TABLE IF NOT EXISTS executions (id INTEGER PRIMARY KEY AUTOINCREMENT, view_id TEXT NOT NULL REFERENCES views(id), body TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS retrieval_units (id TEXT PRIMARY KEY, projection_id TEXT NOT NULL REFERENCES projections(id), body TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS retrieval_units_projection ON retrieval_units(projection_id);
          CREATE VIRTUAL TABLE IF NOT EXISTS retrieval_fts USING fts5(unit_id UNINDEXED, projection_id UNINDEXED, evidence_text, routing_text);
          CREATE TABLE IF NOT EXISTS reports (id TEXT PRIMARY KEY, body TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS explanations (handle TEXT PRIMARY KEY, body TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS validation_cache (digest TEXT PRIMARY KEY, validator TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS recall_indexes (view_id TEXT NOT NULL REFERENCES views(id), configuration TEXT NOT NULL, unit_count INTEGER NOT NULL, PRIMARY KEY(view_id,configuration));
          CREATE TABLE IF NOT EXISTS recall_units (id INTEGER PRIMARY KEY, view_id TEXT NOT NULL, configuration TEXT NOT NULL, artifact_id TEXT NOT NULL REFERENCES records(id), path TEXT NOT NULL, source_kind TEXT NOT NULL, start_byte INTEGER NOT NULL, end_byte INTEGER NOT NULL);
          CREATE INDEX IF NOT EXISTS recall_units_view ON recall_units(view_id,configuration);
          CREATE VIRTUAL TABLE IF NOT EXISTS recall_fts USING fts5(unit_id UNINDEXED, view_id UNINDEXED, configuration UNINDEXED, path, body);
          CREATE VIRTUAL TABLE IF NOT EXISTS recall_parts_fts USING fts5(unit_id UNINDEXED, view_id UNINDEXED, configuration UNINDEXED, path, body, path_parts, body_parts);
          CREATE TABLE IF NOT EXISTS history_commits (oid TEXT PRIMARY KEY, configuration TEXT NOT NULL, parents TEXT NOT NULL, author TEXT NOT NULL, timestamp INTEGER NOT NULL, message TEXT NOT NULL, files_total INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS history_changes (oid TEXT NOT NULL REFERENCES history_commits(oid) ON DELETE CASCADE, ordinal INTEGER NOT NULL, path TEXT NOT NULL, old_path TEXT, status TEXT NOT NULL, patch TEXT NOT NULL, truncated INTEGER NOT NULL, PRIMARY KEY(oid,ordinal));
          CREATE TABLE IF NOT EXISTS history_units (id INTEGER PRIMARY KEY, oid TEXT NOT NULL REFERENCES history_commits(oid) ON DELETE CASCADE, path TEXT NOT NULL, text TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS history_units_oid ON history_units(oid);
          CREATE TABLE IF NOT EXISTS history_reach (oid TEXT PRIMARY KEY);
          CREATE TABLE IF NOT EXISTS history_state (singleton INTEGER PRIMARY KEY CHECK(singleton=1), tip TEXT NOT NULL, configuration TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS sync_state (singleton INTEGER PRIMARY KEY CHECK(singleton=1), key TEXT NOT NULL, view_id TEXT NOT NULL REFERENCES views(id), body TEXT NOT NULL);")?;
        if owner != Ownership::Marked {
            db.execute_batch(&format!(
                "PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version=1;"
            ))?;
        }
        Ok(Self { db, view_cache: RefCell::new(BTreeMap::new()), retained_cache: RefCell::new(BTreeMap::new()), report_changes: Cell::new(0) })
    }
    pub fn begin(&mut self, repository: &str, label: Option<&str>) -> Result<u64> {
        self.begin_scope(repository, label, "source")
    }
    /// Deployment and runtime publications are pinned views with their own
    /// context and projections. Only source captures own `latest`/intent.
    pub fn begin_scope(
        &mut self,
        repository: &str,
        label: Option<&str>,
        scope: &str,
    ) -> Result<u64> {
        require(
            ["source", "deployment", "runtime", "session"].contains(&scope),
            "SCOPE",
            "Unknown publication scope",
        )?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT repository FROM namespace WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            require(
                existing == repository,
                "NAMESPACE",
                "Database belongs to another repository",
            )?;
        } else {
            tx.execute(
                "INSERT INTO namespace(singleton,repository) VALUES(1,?)",
                [repository],
            )?;
        }
        let generation: u64 = tx.query_row(
            "SELECT MAX(COALESCE((SELECT MAX(generation) FROM scans),0),intended)+1 FROM namespace",
            [],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO scans(generation,status,label,scope) VALUES(?,'preparing',?,?)",
            params![generation, label, scope],
        )?;
        if scope == "source" {
            tx.execute("UPDATE namespace SET intended=?", [generation])?;
        }
        tx.commit()?;
        Ok(generation)
    }
    pub fn fail(&self, generation: u64, message: &str) -> Result<()> {
        self.db.execute("UPDATE scans SET status='interrupted',diagnostics=? WHERE generation=? AND status='preparing'",params![json!({"error":message}).to_string(),generation])?;
        Ok(())
    }
    pub fn commit(&mut self, generation: u64, p: &Prepared) -> Result<String> {
        for (batch, cap) in &p.batches {
            contracts::validate_batch(batch, &p.bundle, &p.payloads, *cap)?;
            require(
                batch["expected_generation"].as_u64() == generation.checked_sub(1),
                "GENERATION",
                "Batch token differs from prepared attempt",
            )?;
        }
        let selected: BTreeSet<_> = p
            .batches
            .iter()
            .flat_map(|(b, _)| strings(&b["record_ids"]))
            .collect();
        require(
            selected
                == rows(&p.bundle, "assertions")
                    .iter()
                    .map(|a| text(a, "id"))
                    .collect(),
            "CAPABILITY",
            "Every assertion must belong to an authorized typed batch",
        )?;
        let bytes = serde_json::to_vec(&p.bundle)?;
        let bundle_digest = identity::bytes_digest(&bytes);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scan: (String, Option<String>) = tx.query_row(
            "SELECT status,view_id FROM scans WHERE generation=?",
            [generation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut replay = 0;
        for (batch, _) in &p.batches {
            let prior: Option<(String, String)> = tx
                .query_row(
                    "SELECT digest,bundle_digest FROM batches WHERE id=?",
                    [text(batch, "batch_id")],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((digest, bundle)) = prior {
                require(
                    digest == text(batch, "payload_digest") && bundle == bundle_digest,
                    "CONFLICTING_REPLAY",
                    "Batch ID was committed with different bytes",
                )?;
                replay += 1;
            }
        }
        if p.batches.is_empty() {
            if let Some(view) = &scan.1 {
                let retained: String =
                    tx.query_row("SELECT bundle FROM views WHERE id=?", [view], |r| r.get(0))?;
                require(
                    identity::bytes_digest(retained.as_bytes()) == bundle_digest,
                    "CONFLICTING_REPLAY",
                    "Session metadata replay differs from the committed publication",
                )?;
                return Ok(view.clone());
            }
        } else if replay == p.batches.len() {
            return scan
                .1
                .ok_or_else(|| "REPLAY: Attempt does not own replayed view".into());
        }
        require(
            replay == 0,
            "CONFLICTING_REPLAY",
            "Cannot partially replay an atomic publication",
        )?;
        require(
            scan.0 == "preparing",
            "LIFECYCLE",
            "Attempt is no longer preparing",
        )?;
        Self::retain(&tx, p, &selected)?;
        tx.execute(
            "INSERT INTO events(kind,body) VALUES('commit',?)",
            [json!({"generation":generation,"bundle_digest":bundle_digest}).to_string()],
        )?;
        let watermark = tx.last_insert_rowid();
        for (batch, _) in &p.batches {
            tx.execute(
                "INSERT INTO batches(id,digest,bundle_digest,watermark) VALUES(?,?,?,?)",
                params![
                    text(batch, "batch_id"),
                    text(batch, "payload_digest"),
                    bundle_digest,
                    watermark
                ],
            )?;
            for aid in strings(&batch["record_ids"]) {
                tx.execute(
                    "INSERT INTO occurrences(watermark,assertion_id) VALUES(?,?)",
                    params![watermark, aid],
                )?;
            }
        }
        let view_id = format!("view:{generation}");
        tx.execute(
            "INSERT INTO views(id,generation,watermark,bundle,diagnostics) VALUES(?,?,?,?,?)",
            params![
                view_id,
                generation,
                watermark,
                String::from_utf8(bytes)?,
                p.diagnostics.to_string()
            ],
        )?;
        // Freshness reads this per call; extracting it from a large bundle costs a full JSON parse.
        let capture_state = rows(&p.bundle, "snapshots").first().and_then(|s| s["capture_state"].as_str())
            .or_else(|| p.diagnostics["capture_state"].as_str());
        tx.execute(
            "INSERT INTO view_capture_states(view_id,capture_state) VALUES(?,?)",
            params![view_id, capture_state],
        )?;
        tx.execute(
            "UPDATE scans SET status='committed',view_id=?,diagnostics=? WHERE generation=?",
            params![view_id, p.diagnostics.to_string(), generation],
        )?;
        tx.commit()?;
        Ok(view_id)
    }
    /// One retention boundary owns record collisions and immutable payloads for
    /// both capture and additive overlays. Projections never call this service.
    pub(crate) fn retain(
        tx: &rusqlite::Transaction<'_>,
        p: &Prepared,
        authorized: &BTreeSet<String>,
    ) -> Result<()> {
        for table in contracts::TABLES {
            for record in rows(&p.bundle, table) {
                let id = text(record, "id");
                let body = record.to_string();
                let prior: Option<(String, String)> = tx
                    .query_row("SELECT family,body FROM records WHERE id=?", [&id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                if let Some((family, prior)) = prior {
                    require(
                        family == *table && prior == body,
                        "IDENTITY_COLLISION",
                        "ID was reused for a different canonical record",
                    )?;
                } else {
                    require(
                        *table != "assertions" || authorized.contains(&id),
                        "CAPABILITY",
                        "New assertion lacks an authorized typed batch",
                    )?;
                    tx.execute(
                        "INSERT INTO records(id,family,body) VALUES(?,?,?)",
                        params![id, table, body],
                    )?;
                }
            }
        }
        for (id, bytes) in &p.payloads {
            let artifact = knowledge::row(&p.bundle, "artifacts", id)?;
            require(
                artifact["availability"] == "available"
                    && artifact["digest"] == identity::bytes_digest(bytes),
                "DIGEST",
                "Submitted payload lacks its matching canonical artifact",
            )?;
            let prior: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT bytes FROM payloads WHERE artifact_id=?",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(prior) = prior {
                require(
                    &prior == bytes,
                    "IDENTITY_COLLISION",
                    "Retained payload differs from immutable artifact",
                )?;
            } else {
                tx.execute(
                    "INSERT INTO payloads(artifact_id,bytes) VALUES(?,?)",
                    params![id, bytes],
                )?;
            }
        }
        Ok(())
    }
    pub(crate) fn source_overlays(&self, view_id: &str) -> Result<Vec<Value>> {
        let mut s = self.db.prepare("SELECT o.bundle FROM overlays o JOIN view_overlays v ON o.id=v.overlay_id WHERE v.view_id=? AND o.scope='source' ORDER BY o.watermark")?;
        let raw = s
            .query_map([view_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.iter()
            .map(|body| identity::parse(body.as_bytes()))
            .collect()
    }
    pub(crate) fn explanation_overlays(&self, views: &[String]) -> Result<Vec<Value>> {
        let mut keys = BTreeSet::new();
        let mut out = Vec::new();
        for view in views {
            let mut s=self.db.prepare("SELECT o.id,o.bundle FROM overlays o JOIN view_overlays v ON o.id=v.overlay_id WHERE v.view_id=? ORDER BY o.watermark")?;
            let raw = s
                .query_map([view], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, body) in raw {
                if keys.insert(id) {
                    out.push(identity::parse(body.as_bytes())?);
                }
            }
        }
        Ok(out)
    }
    pub(crate) fn has_overlay(&self, key: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM overlays WHERE id=?)",
            [key],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn overlay_watermark(&self, key: &str) -> Result<Option<u64>> {
        Ok(self
            .db
            .query_row("SELECT watermark FROM overlays WHERE id=?", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub(crate) fn comparison_overlay(&self, key: &str) -> Result<Option<Value>> {
        let raw: Option<String> = self
            .db
            .query_row(
                "SELECT bundle FROM overlays WHERE id=? AND scope='comparison'",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|body| identity::parse(body.as_bytes())).transpose()
    }
    /// Inference-backed lineage overlays for one snapshot pair. Exact
    /// correspondence keeps its deterministic key; each lineage inference
    /// commits its own overlay, and queries union them without merging
    /// certainty classes.
    pub(crate) fn lineage_overlays(
        &self,
        before_snapshot: &str,
        after_snapshot: &str,
    ) -> Result<Vec<Value>> {
        let mut s = self.db.prepare(
            "SELECT id,watermark,bundle FROM overlays WHERE scope='comparison' ORDER BY watermark",
        )?;
        let raw = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::new();
        for (id, watermark, body) in raw {
            let bundle = identity::parse(body.as_bytes())?;
            let claims: Vec<Value> = rows(&bundle, "assertions")
                .iter()
                .filter(|a| {
                    a["class"] == "claim"
                        && a["context"]["kind"] == "comparison"
                        && a["context"]["snapshot_ids"] == json!([before_snapshot, after_snapshot])
                })
                .cloned()
                .collect();
            if !claims.is_empty() {
                out.push(json!({"overlay_id":id,"watermark":watermark,"bundle":bundle,"claim_ids":claims.iter().map(|a|text(a,"id")).collect::<Vec<_>>()}));
            }
        }
        Ok(out)
    }
    /// Extensions go through the same typed batch validator and collision checks
    /// as capture. Scope membership is append-only, never an edit to old records.
    pub(crate) fn commit_extension(
        &mut self,
        key: &str,
        scope: &str,
        views: &[String],
        p: &Prepared,
        selected: &BTreeSet<String>,
        expected_watermark: u64,
    ) -> Result<()> {
        require(
            ["source", "comparison"].contains(&scope),
            "SCOPE",
            "Invalid overlay scope",
        )?;
        contracts::validate(&p.bundle, &p.payloads)?;
        for (batch, cap) in &p.batches {
            contracts::validate_batch(batch, &p.bundle, &p.payloads, *cap)?;
        }
        let submitted: BTreeSet<_> = p
            .batches
            .iter()
            .flat_map(|(b, _)| strings(&b["record_ids"]))
            .collect();
        require(
            &submitted == selected,
            "CAPABILITY",
            "Extension assertions must have exactly one authorized batch membership",
        )?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<String> = tx
            .query_row("SELECT bundle FROM overlays WHERE id=?", [key], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(body) = prior {
            require(
                identity::parse(body.as_bytes())? == p.bundle,
                "CONFLICTING_REPLAY",
                "Overlay key reused with different knowledge",
            )?;
            return Ok(());
        }
        let watermark: u64 =
            tx.query_row("SELECT COALESCE(MAX(watermark),0) FROM events", [], |r| {
                r.get(0)
            })?;
        require(
            watermark == expected_watermark,
            "FRESHNESS",
            "Knowledge changed during overlay preparation; retry",
        )?;
        for view in views {
            let _: String =
                tx.query_row("SELECT id FROM views WHERE id=?", [view], |r| r.get(0))?;
        }
        Self::retain(&tx, p, selected)?;
        tx.execute(
            "INSERT INTO events(kind,body) VALUES('overlay',?)",
            [json!({"id":key,"scope":scope,"assertion_ids":selected}).to_string()],
        )?;
        let mark = tx.last_insert_rowid();
        for (batch, _) in &p.batches {
            tx.execute(
                "INSERT INTO batches(id,digest,bundle_digest,watermark) VALUES(?,?,?,?)",
                params![
                    text(batch, "batch_id"),
                    text(batch, "payload_digest"),
                    identity::bytes_digest(p.bundle.to_string().as_bytes()),
                    mark
                ],
            )?;
            for aid in strings(&batch["record_ids"]) {
                tx.execute(
                    "INSERT INTO occurrences(watermark,assertion_id) VALUES(?,?)",
                    params![mark, aid],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO overlays(id,scope,watermark,bundle,assertion_ids) VALUES(?,?,?,?,?)",
            params![
                key,
                scope,
                mark,
                p.bundle.to_string(),
                json!(selected).to_string()
            ],
        )?;
        for view in views {
            tx.execute(
                "INSERT INTO view_overlays(view_id,overlay_id) VALUES(?,?)",
                params![view, key],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn record_execution(&self, view: &str, body: &Value) -> Result<String> {
        self.db.execute(
            "INSERT INTO executions(view_id,body) VALUES(?,?)",
            params![view, body.to_string()],
        )?;
        Ok(format!("execution:{}", self.db.last_insert_rowid()))
    }
    pub(crate) fn pass_changes(
        &self,
        view: &str,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let v = self.load(view)?;
        let previous: Option<String> = self
            .db
            .query_row(
                "SELECT v.id FROM views v JOIN scans s ON s.generation=v.generation WHERE v.generation<? AND s.scope=? ORDER BY v.generation DESC LIMIT 1",
                params![v.generation, v.scope],
                |r| r.get(0),
            )
            .optional()?;
        let artifacts = |b: &Value| {
            rows(b, "pass_manifests")
                .iter()
                .flat_map(|m| strings(&m["output_artifact_ids"]))
                .collect::<BTreeSet<_>>()
        };
        let current = artifacts(&v.bundle);
        let old = if let Some(previous) = previous {
            artifacts(&self.load(&previous)?.bundle)
        } else {
            BTreeSet::new()
        };
        Ok((
            current.intersection(&old).cloned().collect(),
            current.difference(&old).cloned().collect(),
            old.difference(&current).cloned().collect(),
        ))
    }
    pub fn watermark(&self) -> Result<u64> {
        Ok(self
            .db
            .query_row("SELECT COALESCE(MAX(watermark),0) FROM events", [], |r| {
                r.get(0)
            })?)
    }
    pub fn retraction_epoch(&self) -> Result<u64> {
        Ok(self.db.query_row(
            "SELECT COALESCE(MAX(watermark),0) FROM retractions",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn retracted(&self) -> Result<BTreeSet<String>> {
        let mut s = self
            .db
            .prepare("SELECT assertion_id FROM retractions ORDER BY assertion_id")?;
        let values = s
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(values)
    }
}
