use super::*;
impl Ledger {
    /// Select one already published profile under the caller's writer lock.
    pub(crate) fn select_source_profile(&self, id: &str) -> Result<()> {
        require(
            self.scope_of(id)? == "source" && !self.pending_source()?,
            "FRESHNESS",
            "Cannot select an unpublished or pending source profile",
        )?;
        let published:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM views v JOIN scans s ON s.generation=v.generation WHERE v.id=? AND s.status='published')",[id],|r|r.get(0))?;
        require(published, "FRESHNESS", "Profile must be published")?;
        self.db
            .execute("UPDATE namespace SET current_view=?", [id])?;
        Ok(())
    }
    pub fn publish(&mut self, view_id: &str, select_current: bool) -> Result<Vec<Value>> {
        let mut view = self.load(view_id)?;
        let payloads = self.payloads(&view.bundle)?;
        contracts::validate(&view.bundle, &payloads)?;
        let removed = self.retracted()?;
        let retraction_epoch = self.retraction_epoch()?;
        let inputs: Vec<_> = rows(&view.bundle, "assertions")
            .iter()
            .filter(|a| !removed.contains(&text(a, "id")))
            .map(|r| text(r, "id"))
            .collect();
        let input_set: BTreeSet<String> = inputs.iter().cloned().collect();
        let units = super::super::evidence_index::build_units(
            &view.bundle,
            &payloads,
            &input_set,
            &view.scope,
        )?;
        let retrieval_configuration = super::super::evidence_index::configuration();
        let context = rows(&view.bundle, "runs")[0]["context"].clone();
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_retraction: u64 = tx.query_row(
            "SELECT COALESCE(MAX(watermark),0) FROM retractions",
            [],
            |r| r.get(0),
        )?;
        require(
            current_retraction == retraction_epoch,
            "FRESHNESS",
            "Retraction occurred during projection preparation; retry",
        )?;
        let intended: u64 = tx.query_row("SELECT intended FROM namespace", [], |r| r.get(0))?;
        require(
            !select_current || view.scope == "source",
            "SCOPE",
            "Only a source capture can become the current view",
        )?;
        require(
            !select_current || intended == view.generation,
            "STALE_PUBLICATION",
            "A newer scan owns current publication",
        )?;
        let watermark: u64 =
            tx.query_row("SELECT COALESCE(MAX(watermark),0) FROM events", [], |r| {
                r.get(0)
            })?;
        let old: u64 = tx.query_row(
            "SELECT projection_generation FROM views WHERE id=?",
            [view_id],
            |r| r.get(0),
        )?;
        let generation = old + 1;
        // Manifests remain resolvable for pinned reports. Superseded derived
        // units do not: keeping every generation would preserve the exact span
        // cost this projection change is meant to remove.
        tx.execute(
            "DELETE FROM retrieval_fts WHERE projection_id IN
             (SELECT id FROM projections WHERE view_id=?)",
            [view_id],
        )?;
        tx.execute(
            "DELETE FROM retrieval_units WHERE projection_id IN
             (SELECT id FROM projections WHERE view_id=?)",
            [view_id],
        )?;
        let mut manifests = Vec::new();
        for kind in ["graph", "retrieval"] {
            let mut m = json!({"kind":kind,"context":context,"ledger_watermark":watermark,"generation":generation,"input_assertion_ids":inputs,"eligible_classes":["evidence","derived","declaration","claim"]});
            if kind == "retrieval" {
                for key in ["chunker", "tokenizer", "embedding_mode", "dimensions"] {
                    m[key] = retrieval_configuration[key].clone();
                }
                m["configuration_digest"] = json!(identity::digest(&retrieval_configuration)?);
                m["source_artifact_ids"] = json!(rows(&view.bundle, "artifacts")
                    .iter()
                    .filter(|a| a["source_kind"] != "transport")
                    .map(|a| text(a, "id"))
                    .collect::<BTreeSet<_>>());
                m["unit_ids"] = json!(units.iter().map(|u| text(u, "id")).collect::<Vec<_>>());
            }
            m["id"] = json!(identity::id(
                "projection",
                &json!({"view_id":view_id,"manifest":m})
            )?);
            let id = text(&m, "id");
            tx.execute("INSERT INTO projections(id,view_id,generation,kind,watermark,body) VALUES(?,?,?,?,?,?)",params![id,view_id,generation,kind,watermark,m.to_string()])?;
            if kind == "retrieval" {
                for unit in &units {
                    let uid = text(unit, "id");
                    // Unit IDs identify representation content. Publication
                    // membership is separate so the same content can recur.
                    let stored_id = format!("{id}/{uid}");
                    tx.execute(
                        "INSERT INTO retrieval_units(id,projection_id,body) VALUES(?,?,?)",
                        params![stored_id, id, unit.to_string()],
                    )?;
                    tx.execute("INSERT INTO retrieval_fts(unit_id,projection_id,evidence_text,routing_text) VALUES(?,?,?,?)",
                        params![stored_id,id,text(unit,"text"),text(unit,"routing_text")])?;
                }
            }
            manifests.push(m);
        }
        tx.execute(
            "UPDATE views SET projection_generation=? WHERE id=?",
            params![generation, view_id],
        )?;
        tx.execute(
            "UPDATE scans SET status='published' WHERE generation=?",
            [view.generation],
        )?;
        if select_current {
            tx.execute("UPDATE namespace SET current_view=?", [view_id])?;
        }
        tx.commit()?;
        view.projections = manifests.clone();
        Ok(manifests)
    }
    pub fn rebuild(&mut self, selector: &str) -> Result<Vec<Value>> {
        let id = self.resolve(selector)?;
        let current: Option<String> = self
            .db
            .query_row("SELECT current_view FROM namespace", [], |r| r.get(0))
            .optional()?
            .flatten();
        let intended: u64 = self
            .db
            .query_row("SELECT intended FROM namespace", [], |r| r.get(0))?;
        let generation = self.load(&id)?.generation;
        self.publish(
            &id,
            current.as_deref() == Some(&id) && generation == intended,
        )
    }
    pub fn repository(&self) -> Result<String> {
        self.db
            .query_row("SELECT repository FROM namespace", [], |r| r.get(0))
            .optional()?
            .ok_or_else(|| {
                "NAMESPACE: Capture a source revision before other evidence producers".into()
            })
    }
    pub fn scope_of(&self, view_id: &str) -> Result<String> {
        Ok(self.db.query_row(
            "SELECT s.scope FROM views v JOIN scans s ON s.generation=v.generation WHERE v.id=?",
            [view_id],
            |r| r.get(0),
        )?)
    }
    /// `latest` selects the newest published view of one publication scope.
    /// Explicit IDs and labels must belong to the requested scope.
    pub fn resolve_scoped(&self, scope: &str, selector: &str) -> Result<String> {
        let id = if selector == "latest" && scope != "source" {
            self.db
                .query_row(
                    "SELECT v.id FROM views v JOIN scans s ON s.generation=v.generation WHERE s.scope=? AND v.projection_generation>0 ORDER BY v.generation DESC LIMIT 1",
                    [scope],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| format!("VIEW: No published {scope} view"))?
        } else {
            self.resolve(selector)?
        };
        require(
            self.scope_of(&id)? == scope,
            "SCOPE",
            &format!("View {id} is not a {scope} publication"),
        )?;
        Ok(id)
    }
    /// Published views of one scope, newest first.
    pub fn scoped_views(&self, scope: &str) -> Result<Vec<String>> {
        let mut s = self.db.prepare("SELECT v.id FROM views v JOIN scans s ON s.generation=v.generation WHERE s.scope=? AND v.projection_generation>0 ORDER BY v.generation DESC")?;
        let ids = s
            .query_map([scope], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }
    pub fn resolve(&self, selector: &str) -> Result<String> {
        if selector == "latest" {
            return self
                .db
                .query_row(
                    "SELECT current_view FROM namespace WHERE current_view IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| "VIEW: No published view; scan or recover first".into());
        }
        let row:Option<String>=self.db.query_row("SELECT id FROM views WHERE id=? UNION SELECT view_id FROM scans WHERE label=? AND view_id IS NOT NULL",params![selector,selector],|r|r.get(0)).optional()?;
        row.ok_or_else(|| format!("VIEW: Unknown view or label {selector}").into())
    }
}
