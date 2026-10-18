use super::*;
impl Ledger {
    pub(crate) fn source_exclusions(&self) -> Result<Vec<std::path::PathBuf>> {
        let file:String=self.db.query_row("SELECT file FROM pragma_database_list WHERE name='main'",[],|r|r.get(0))?;
        Ok(if file.is_empty() {vec![]} else {workspace::database_files(Path::new(&file))})
    }
    /// Cheap readiness signal; do not hydrate every scan's diagnostics for a read.
    pub(crate) fn pending_source(&self) -> Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM namespace n JOIN scans s ON s.generation=n.intended WHERE s.scope='source' AND s.status!='published')", [], |r|r.get(0))?)
    }
    /// Capture metadata only. Projection validity is explicitly not checked here.
    pub(crate) fn view_freshness(&self, id: &str) -> Result<Value> {
        let (scope, capture, state): (String, Option<String>, Option<String>) = self.db.query_row(
            "SELECT s.scope,json_extract(v.diagnostics,'$.capture'),COALESCE(json_extract(v.bundle,'$.snapshots[0].capture_state'),json_extract(v.diagnostics,'$.capture_state')) FROM views v JOIN scans s ON s.generation=v.generation WHERE v.id=?",
            [id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let capture = capture
            .map(|raw| identity::parse(raw.as_bytes()))
            .transpose()?
            .unwrap_or(Value::Null);
        Ok(
            json!({"view_id":id,"scope":scope,"capture_state":state,"captured_at":capture["captured_at"],
            "captured_at_unix_ms":capture["captured_at_unix_ms"],"capture_head":capture["capture_head"],"repository_root":capture["repository_root"],"stale":null,"projection_check":"not_requested","projection_state":"unknown"}),
        )
    }
    pub fn status(&self) -> Result<Value> {
        let namespace: Option<(String, u64, Option<String>)> = self
            .db
            .query_row(
                "SELECT repository,intended,current_view FROM namespace",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let mut s = self.db.prepare(
            "SELECT generation,status,label,view_id,diagnostics,scope FROM scans ORDER BY generation",
        )?;
        let scans=s.query_map([],|r|Ok(json!({"generation":r.get::<_,u64>(0)?,"status":r.get::<_,String>(1)?,"label":r.get::<_,Option<String>>(2)?,"view_id":r.get::<_,Option<String>>(3)?,"diagnostics":serde_json::from_str::<Value>(&r.get::<_,String>(4)?).unwrap_or(Value::Null),"scope":r.get::<_,String>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        // Pending source intent; deployment/runtime publications never own `latest`.
        let pending = namespace.as_ref().is_some_and(|(_, g, _)| {
            scans
                .iter()
                .any(|s| s["generation"] == *g && s["status"] != "published")
        });
        Ok(
            json!({"repository":namespace.as_ref().map(|x|&x.0),"intended_generation":namespace.as_ref().map(|x|x.1),"current_view":namespace.as_ref().and_then(|x|x.2.as_ref()),"pending_or_interrupted":pending,"ledger_watermark":self.watermark()?,"scans":scans,"retracted_count":self.retracted()?.len()}),
        )
    }
    /// Claim coverage is historical, pinned to a source view; never a readiness prerequisite.
    pub fn claims_coverage(&self) -> Result<Value> {
        let current = self.resolve("latest").ok();
        let mut result = json!({"state":"manual"});
        for (domain, operation) in [("domains", "classify"), ("lineage", "classify-lineage")] {
            let latest: Option<String> = self.db.query_row(
                "SELECT view_id FROM executions WHERE json_extract(body,'$.operation')=? ORDER BY id DESC LIMIT 1",
                [operation], |r| r.get(0)).optional()?;
            let covered = current.as_ref().is_some_and(|view| self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM executions WHERE view_id=? AND json_extract(body,'$.operation')=?)",
                params![view, operation], |r| r.get::<_, bool>(0)).unwrap_or(false));
            result[domain] = json!({"latest_source_view":current,"latest_claim_view":latest,
                "state":if covered {"run_for_current_view"} else {"not_run_for_current_view"},
                "action":format!("briefcase claims sync {domain} --provider ...")});
        }
        Ok(result)
    }
    pub fn save_report(
        &self,
        id: &str,
        report: &Value,
        explanations: &[(String, Value)],
    ) -> Result<()> {
        let before=self.db.total_changes();
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO reports(id,body) VALUES(?,?)",
            params![id, report.to_string()],
        )?;
        for (handle, body) in explanations {
            tx.execute(
                "INSERT OR IGNORE INTO explanations(handle,body) VALUES(?,?)",
                params![handle, body.to_string()],
            )?;
        }
        tx.commit()?;
        self.report_changes.set(self.report_changes.get()+self.db.total_changes().saturating_sub(before));
        Ok(())
    }
    /// Content-addressed full-validation verdicts. A hit means this exact
    /// validator accepted these exact bytes before; any byte change misses
    /// and revalidates. Rows from older validator versions never match.
    pub(crate) fn validation_cached(&self, digest: &str, validator: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM validation_cache WHERE digest=? AND validator=?)",
            params![digest, validator],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn record_validation(&self, digest: &str, validator: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO validation_cache(digest,validator) VALUES(?,?)",
            params![digest, validator],
        )?;
        self.db.execute(
            "DELETE FROM validation_cache WHERE validator!=?",
            [validator],
        )?;
        Ok(())
    }
    pub(crate) fn adoption_prompt(&self,key:&str)->Result<Option<Value>> {
        let body:Option<String>=self.db.query_row(
            "SELECT body FROM reports WHERE json_extract(body,'$.kind')='adoption_prompt' AND json_extract(body,'$.session_key')=? ORDER BY rowid DESC LIMIT 1",
            [key],|row|row.get(0)).optional()?;
        body.map(|raw|identity::parse(raw.as_bytes())).transpose()
    }
    /// Newest explicit external-file observation for one source pin.
    pub(crate) fn external_inspection(&self,view:&str)->Result<Option<Value>> {
        let body:Option<String>=self.db.query_row(
            "SELECT body FROM reports WHERE json_extract(body,'$.kind')='external_inspection' AND json_extract(body,'$.view_id')=? ORDER BY rowid DESC LIMIT 1",
            [view],|row|row.get(0)).optional()?;
        body.map(|raw|identity::parse(raw.as_bytes())).transpose()
    }
    /// Operational outcome of the newest Rust call attempt for unchanged inputs.
    /// These receipts never alter semantic runs, coverage, or source identity.
    pub(crate) fn static_call_attempt(&self,view:&str,input_key:&str)->Result<Option<Value>> {
        let body:Option<String>=self.db.query_row(
            "SELECT body FROM executions WHERE view_id=? AND json_extract(body,'$.operation')='sync-static-calls' AND json_extract(body,'$.input_key')=? ORDER BY id DESC LIMIT 1",
            params![view,input_key],|row|row.get(0)).optional()?;
        body.map(|raw|identity::parse(raw.as_bytes()).map(|value|value["result"].clone())).transpose()
    }
    pub fn report(&self, id: &str) -> Result<Value> {
        let body: String = self
            .db
            .query_row("SELECT body FROM reports WHERE id=?", [id], |r| r.get(0))?;
        identity::parse(body.as_bytes())
    }
    pub fn explanation(&self, handle: &str) -> Result<Value> {
        let body: Option<String> = self
            .db
            .query_row(
                "SELECT body FROM explanations WHERE handle=?",
                [handle],
                |r| r.get(0),
            )
            .optional()?;
        identity::parse(
            body.ok_or("EXPLAIN: Unknown explanation handle")?
                .as_bytes(),
        )
    }
}
