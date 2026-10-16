use super::*;
impl Ledger {
    /// Whether a rebuildable recall projection exists for a view and configuration.
    pub(crate) fn recall_ready(&self, view: &str, configuration: &str) -> Result<Option<u64>> {
        Ok(self
            .db
            .query_row(
                "SELECT unit_count FROM recall_indexes WHERE view_id=? AND configuration=?",
                [view, configuration],
                |r| r.get(0),
            )
            .optional()?)
    }
    /// Store one complete recall projection atomically; a concurrent builder's
    /// identical projection wins without duplication.
    pub(crate) fn store_recall(
        &self,
        view: &str,
        configuration: &str,
        units: &[crate::architecture::recall::Unit],
    ) -> Result<u64> {
        let tx = rusqlite::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        if let Some(count) = tx
            .query_row(
                "SELECT unit_count FROM recall_indexes WHERE view_id=? AND configuration=?",
                [view, configuration],
                |r| r.get(0),
            )
            .optional()?
        {
            return Ok(count);
        }
        tx.execute(
            "DELETE FROM recall_fts WHERE view_id=? AND configuration=?",
            [view, configuration],
        )?;
        tx.execute(
            "DELETE FROM recall_units WHERE view_id=? AND configuration=?",
            [view, configuration],
        )?;
        {
            let mut unit = tx.prepare("INSERT INTO recall_units(view_id,configuration,artifact_id,path,source_kind,start_byte,end_byte) VALUES(?,?,?,?,?,?,?)")?;
            let mut fts = tx.prepare(
                "INSERT INTO recall_fts(unit_id,view_id,configuration,path,body) VALUES(?,?,?,?,?)",
            )?;
            for u in units {
                unit.execute(params![
                    view,
                    configuration,
                    u.artifact_id,
                    u.path,
                    u.source_kind,
                    u.start as i64,
                    u.end as i64
                ])?;
                let id = tx.last_insert_rowid();
                fts.execute(params![id, view, configuration, u.path, u.text])?;
            }
        }
        tx.execute(
            "INSERT INTO recall_indexes(view_id,configuration,unit_count) VALUES(?,?,?)",
            params![view, configuration, units.len() as i64],
        )?;
        tx.commit()?;
        Ok(units.len() as u64)
    }
    /// Ranked recall candidates over one source view and, when `history`
    /// is set, the Git units reachable from the current tip.
    pub(crate) fn recall_search(
        &self,
        view: &str,
        configuration: &str,
        history: Option<&str>,
        expression: &str,
        limit: usize,
    ) -> Result<Vec<RecallHit>> {
        let mut s = self.db.prepare(
            "SELECT f.view_id='git',f.unit_id FROM recall_fts f
             WHERE recall_fts MATCH ?1 AND ((f.view_id=?2 AND f.configuration=?3)
               OR (?4 IS NOT NULL AND f.view_id='git' AND f.configuration=?4
                   AND EXISTS(SELECT 1 FROM history_units u JOIN history_reach r ON r.oid=u.oid WHERE u.id=f.unit_id)))
             ORDER BY bm25(recall_fts,0,0,0,2.0,1.0), f.rowid LIMIT ?5",
        )?;
        let rows = s
            .query_map(
                params![expression, view, configuration, history, limit as i64],
                |r| {
                    Ok(if r.get::<_, bool>(0)? {
                        RecallHit::Commit(r.get(1)?)
                    } else {
                        RecallHit::Source(r.get(1)?)
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    /// A source recall unit: artifact, path, source kind, start and end byte.
    pub(crate) fn recall_source_unit(
        &self,
        unit: i64,
    ) -> Result<(String, String, String, usize, usize)> {
        Ok(self.db.query_row(
            "SELECT artifact_id,path,source_kind,start_byte,end_byte FROM recall_units WHERE id=?",
            [unit],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)? as usize,
                    r.get::<_, i64>(4)? as usize,
                ))
            },
        )?)
    }
    pub(crate) fn history_tip(&self) -> Result<Option<(String, String)>> {
        Ok(self
            .db
            .query_row(
                "SELECT tip,configuration FROM history_state WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
    pub(crate) fn set_history_tip(&self, tip: &str, configuration: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO history_state(singleton,tip,configuration) VALUES(1,?1,?2)
             ON CONFLICT(singleton) DO UPDATE SET tip=?1,configuration=?2",
            [tip, configuration],
        )?;
        Ok(())
    }
    pub(crate) fn history_reachable_count(&self) -> Result<u64> {
        Ok(self.db.query_row(
            "SELECT COUNT(*) FROM history_reach r JOIN history_commits c ON c.oid=r.oid",
            [],
            |r| r.get(0),
        )?)
    }
    /// Reachable retained commits with their retained changed paths, newest
    /// first. Patches and messages are not read: evolution metrics use
    /// metadata only.
    pub(crate) fn history_commits(&self) -> Result<Vec<HistoricalCommit>> {
        let mut commits: Vec<HistoricalCommit> = Vec::new();
        let mut s = self.db.prepare(
            "SELECT c.oid,c.parents,c.timestamp,c.files_total,h.path,h.old_path,h.status,c.message
             FROM history_reach r JOIN history_commits c ON c.oid=r.oid
             LEFT JOIN history_changes h ON h.oid=c.oid
             ORDER BY c.timestamp DESC, c.oid, h.ordinal",
        )?;
        let mut rows = s.query([])?;
        while let Some(r) = rows.next()? {
            let oid: String = r.get(0)?;
            if commits.last().is_none_or(|c| c.oid != oid) {
                let parents: String = r.get(1)?;
                commits.push(HistoricalCommit {
                    oid,
                    parents: parents.split_whitespace().map(str::to_owned).collect(),
                    timestamp: r.get(2)?,
                    files_total: r.get::<_, i64>(3)? as usize,
                    message: r.get(7)?,
                    changes: vec![],
                });
            }
            if let Some(path) = r.get::<_, Option<String>>(4)? {
                commits.last_mut().unwrap().changes.push(HistoricalChange {
                    path,
                    old_path: r.get(5)?,
                    status: r.get(6)?,
                });
            }
        }
        Ok(commits)
    }
    /// Commits retained under one history configuration.
    pub(crate) fn history_stored(&self, configuration: &str) -> Result<HashSet<String>> {
        let mut s = self
            .db
            .prepare("SELECT oid FROM history_commits WHERE configuration=?")?;
        let oids = s
            .query_map([configuration], |r| r.get(0))?
            .collect::<rusqlite::Result<HashSet<String>>>()?;
        Ok(oids)
    }
    pub(crate) fn history_extend_reach(&self, oids: &[String]) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        {
            let mut insert = tx.prepare("INSERT OR IGNORE INTO history_reach(oid) VALUES(?)")?;
            for oid in oids {
                insert.execute([oid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Replace current reachability. Retained commits stay; only commits
    /// retained under a different representation are dropped for re-ingest.
    pub(crate) fn history_replace_reach(&self, oids: &[String], configuration: &str) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM recall_fts WHERE view_id='git' AND configuration<>?",
            [configuration],
        )?;
        tx.execute(
            "DELETE FROM history_commits WHERE configuration<>?",
            [configuration],
        )?;
        tx.execute("DELETE FROM history_reach", [])?;
        {
            let mut insert = tx.prepare("INSERT OR IGNORE INTO history_reach(oid) VALUES(?)")?;
            for oid in oids {
                insert.execute([oid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Retain one commit and its searchable units atomically.
    pub(crate) fn store_commit(
        &self,
        commit: &crate::architecture::git_history::Commit,
        changes: &[crate::architecture::git_history::Change],
        files_total: usize,
        units: &[(String, String)],
        configuration: &str,
    ) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR REPLACE INTO history_commits(oid,configuration,parents,author,timestamp,message,files_total) VALUES(?,?,?,?,?,?,?)",
            params![commit.oid, configuration, commit.parents.join(" "), commit.author, commit.timestamp, commit.message, files_total as i64],
        )?;
        for (ordinal, c) in changes.iter().enumerate() {
            tx.execute(
                "INSERT INTO history_changes(oid,ordinal,path,old_path,status,patch,truncated) VALUES(?,?,?,?,?,?,?)",
                params![commit.oid, ordinal as i64, c.path, c.old_path, c.status, c.patch, c.truncated],
            )?;
        }
        for (path, text) in units {
            tx.execute(
                "INSERT INTO history_units(oid,path,text) VALUES(?,?,?)",
                params![commit.oid, path, text],
            )?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO recall_fts(unit_id,view_id,configuration,path,body) VALUES(?,'git',?,?,?)",
                params![id, configuration, path, text],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Ranked history units reachable from the current tip: (oid, path, text, timestamp).
    pub(crate) fn history_unit(&self, unit: i64) -> Result<(String, String, String, i64)> {
        Ok(self.db.query_row(
            "SELECT u.oid,u.path,u.text,c.timestamp FROM history_units u JOIN history_commits c ON c.oid=u.oid WHERE u.id=?",
            [unit],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?)
    }
    /// Drop recall projections of views other than `keep`; they rebuild on demand.
    pub(crate) fn prune_recall(&self, keep: &str) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(&self.db, TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM recall_fts WHERE view_id<>'git' AND view_id<>?",
            [keep],
        )?;
        tx.execute("DELETE FROM recall_units WHERE view_id<>?", [keep])?;
        tx.execute("DELETE FROM recall_indexes WHERE view_id<>?", [keep])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn clear_history_tip(&self) -> Result<()> {
        self.db.execute("DELETE FROM history_state", [])?;
        Ok(())
    }
    /// The last successful sync's no-op key and view.
    pub(crate) fn sync_state(&self) -> Result<Option<(String, String, Value)>> {
        Ok(self
            .db
            .query_row(
                "SELECT key,view_id,body FROM sync_state WHERE singleton=1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
                    ))
                },
            )
            .optional()?)
    }
    pub(crate) fn record_sync(&self, key: &str, view: &str, body: &Value) -> Result<()> {
        self.db.execute(
            "INSERT INTO sync_state(singleton,key,view_id,body) VALUES(1,?1,?2,?3)
             ON CONFLICT(singleton) DO UPDATE SET key=?1,view_id=?2,body=?3",
            params![key, view, body.to_string()],
        )?;
        Ok(())
    }
}
