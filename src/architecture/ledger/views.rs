use super::*;
impl Ledger {
    /// Read durable capture records without requiring rebuildable graph/search
    /// projections. Used for literal queries and degraded task packets only.
    /// This does not grant a negative architecture guarantee.
    pub(crate) fn retained(&self, selector: &str) -> Result<View> {
        let id = self.resolve(selector)?;
        let (generation, body, diagnostics): (u64, String, String) = self.db.query_row(
            "SELECT generation,bundle,diagnostics FROM views WHERE id=?", [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut bundle = identity::parse(body.as_bytes())?;
        for overlay in self.source_overlays(&id)? { knowledge::merge(&mut bundle, &overlay)?; }
        let removed = self.retracted()?;
        let eligible = rows(&bundle, "assertions").iter().map(|a| text(a,"id"))
            .filter(|id| !removed.contains(id)).collect();
        let invalidated = rows(&bundle,"assertions").iter().any(|a| removed.contains(&text(a,"id")));
        Ok(View { id: id.clone(), generation, bundle, diagnostics: identity::parse(diagnostics.as_bytes())?,
            projections: vec![], eligible, stale: true, invalidated,
            retraction_watermark: self.retraction_epoch()?, overlay_watermark: 0, scope: self.scope_of(&id)? })
    }

    pub fn load(&self, selector: &str) -> Result<View> {
        let id = self.resolve(selector)?;
        let data_version: u64 = self.db.query_row("PRAGMA data_version", [], |r|r.get(0))?;
        let stamp=(data_version,self.db.total_changes().saturating_sub(self.report_changes.get()));
        if let Some((prior,view))=self.view_cache.borrow().get(&id) {
            if *prior==stamp {return Ok(view.clone());}
        }
        let (generation, body, diagnostics, pg): (u64, String, String, u64) = self.db.query_row(
            "SELECT generation,bundle,diagnostics,projection_generation FROM views WHERE id=?",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let mut s = self.db.prepare(
            "SELECT body FROM projections WHERE view_id=? AND generation=? ORDER BY kind",
        )?;
        let ps = s
            .query_map(params![id, pg], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let projections = ps
            .iter()
            .map(|p| identity::parse(p.as_bytes()))
            .collect::<Result<Vec<_>>>()?;
        let eligible = projections
            .first()
            .map(|p| strings(&p["input_assertion_ids"]).into_iter().collect())
            .unwrap_or_default();
        // Retractions affect all views. Later unrelated commits do not invalidate an already pinned view.
        let last_retraction = self.retraction_epoch()?;
        let mut stale = projections.len() != 2
            || projections
                .iter()
                .any(|p| p["ledger_watermark"].as_u64().unwrap_or(0) < last_retraction)
            || projections[0]["ledger_watermark"] != projections[1]["ledger_watermark"]
            || projections[0]["input_assertion_ids"] != projections[1]["input_assertion_ids"];
        for p in &projections {
            let expected: BTreeSet<String> =
                strings(&p["input_assertion_ids"]).into_iter().collect();
            let sql = if p["kind"] == "graph" {
                "SELECT assertion_id FROM graph WHERE projection_id=?"
            } else {
                "SELECT assertion_id FROM search WHERE projection_id=?"
            };
            let mut index = self.db.prepare(sql)?;
            let actual = index
                .query_map([text(p, "id")], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<BTreeSet<_>>>()?;
            stale |= actual != expected;
            if p["kind"] == "retrieval" && !p["unit_ids"].is_null() {
                let mut statement=self.db.prepare("SELECT u.body FROM retrieval_units u JOIN retrieval_fts f ON u.id=f.unit_id AND u.projection_id=f.projection_id WHERE u.projection_id=?")?;
                let raw = statement
                    .query_map([text(p, "id")], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let actual = raw
                    .iter()
                    .map(|r| identity::parse(r.as_bytes()).map(|r| text(&r, "id")))
                    .collect::<Result<BTreeSet<_>>>()?;
                stale |= actual != strings(&p["unit_ids"]).into_iter().collect();
            }
        }
        let mut bundle = identity::parse(body.as_bytes())?;
        for overlay in self.source_overlays(&id)? {
            knowledge::merge(&mut bundle, &overlay)?;
        }
        let projection_context = knowledge::context(&bundle);
        for p in &projections {
            let mut manifest = p.clone();
            manifest.as_object_mut().unwrap().remove("id");
            stale |=
                p["id"] != identity::id("projection", &json!({"view_id":id,"manifest":manifest}))?;
            stale |= p["context"] != projection_context || p["generation"].as_u64() != Some(pg);
            let stored: (String, u64, u64, String) = self.db.query_row(
                "SELECT kind,generation,watermark,view_id FROM projections WHERE id=?",
                [text(p, "id")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            stale |= p["kind"] != stored.0
                || p["generation"].as_u64() != Some(stored.1)
                || p["ledger_watermark"].as_u64() != Some(stored.2)
                || id != stored.3;
        }
        if let Some(p) = projections
            .iter()
            .find(|p| p["kind"] == "retrieval" && !p["unit_ids"].is_null())
        {
            let mut statement=self.db.prepare("SELECT u.body,f.evidence_text,f.routing_text FROM retrieval_units u JOIN retrieval_fts f ON u.id=f.unit_id AND u.projection_id=f.projection_id WHERE u.projection_id=? ORDER BY u.id")?;
            let stored = statement
                .query_map([text(p, "id")], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let expected_units =
                super::super::evidence_index::build_units(&bundle, &self.payloads(&bundle)?, &eligible)?;
            let expected_units: BTreeMap<_, _> = expected_units
                .into_iter()
                .map(|u| (text(&u, "id"), u))
                .collect();
            for (raw, evidence, routing) in stored {
                let unit = identity::parse(raw.as_bytes())?;
                stale |= unit["evidence_text"] != evidence
                    || unit["routing_text"] != routing
                    || expected_units.get(&text(&unit, "id")) != Some(&unit);
            }
            stale |= strings(&p["unit_ids"]).into_iter().collect::<BTreeSet<_>>()
                != expected_units.keys().cloned().collect();
        }
        let removed = self.retracted()?;
        let current_inputs: BTreeSet<_> = rows(&bundle, "assertions")
            .iter()
            .filter(|a| !removed.contains(&text(a, "id")))
            .map(|a| text(a, "id"))
            .collect();
        stale |= eligible != current_inputs;
        let invalidated = rows(&bundle, "assertions")
            .iter()
            .any(|a| removed.contains(&text(a, "id")));
        let scope = self.scope_of(&id)?;
        let view = View {
            id: id.clone(),
            generation,
            bundle,
            diagnostics: identity::parse(diagnostics.as_bytes())?,
            projections,
            eligible,
            stale,
            invalidated,
            retraction_watermark: last_retraction,
            overlay_watermark: 0,
            scope,
        };
        let mut cache=self.view_cache.borrow_mut();
        if cache.len()>=4 {cache.clear();}
        cache.insert(id,(stamp,view.clone()));
        Ok(view)
    }
    pub fn load_historical(&self, selector: &str) -> Result<View> {
        let mut view = self.load(selector)?;
        let mut statement = self.db.prepare(
            "SELECT body FROM projections WHERE view_id=? AND generation=1 ORDER BY kind",
        )?;
        let raw = statement
            .query_map([&view.id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let projections = raw
            .iter()
            .map(|s| identity::parse(s.as_bytes()))
            .collect::<Result<Vec<_>>>()?;
        require(
            projections.len() == 2,
            "FRESHNESS",
            "Original publication manifests are unavailable",
        )?;
        require(
            projections[0]["context"] == projections[1]["context"]
                && projections[0]["ledger_watermark"] == projections[1]["ledger_watermark"]
                && projections[0]["input_assertion_ids"] == projections[1]["input_assertion_ids"],
            "FRESHNESS",
            "Original publication manifests are incompatible",
        )?;
        view.eligible = strings(&projections[0]["input_assertion_ids"])
            .into_iter()
            .collect();
        view.projections = projections;
        Ok(view)
    }
    pub fn payloads(&self, bundle: &Value) -> Result<Payloads> {
        let ids: Vec<String> = rows(bundle, "artifacts")
            .iter()
            .filter(|a| a["availability"] == "available")
            .map(|a| text(a, "id"))
            .collect();
        self.payloads_for(&ids)
    }
    /// Load retained bytes with chunked membership queries instead of one
    /// statement per artifact. Missing bytes remain a hard integrity failure.
    pub fn payloads_for(&self, ids: &[String]) -> Result<Payloads> {
        let mut out = Payloads::new();
        for chunk in ids.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT artifact_id,bytes FROM payloads WHERE artifact_id IN ({placeholders})"
            );
            let mut statement = self.db.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let rows = statement
                .query_map(params.as_slice(), |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            require(
                rows.len() == chunk.len(),
                "DIGEST",
                "Retained source payload is missing",
            )?;
            out.extend(rows);
        }
        Ok(out)
    }
    pub fn retract(&mut self, assertion: &str, reason: &str) -> Result<Value> {
        let mut removed = self.retracted()?;
        let mut newly = BTreeSet::from([assertion.to_owned()]);
        let mut s=self.db.prepare("SELECT family,body FROM records WHERE family IN ('assertions','derivations','inferences','input_packets')")?;
        let all = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(s);
        let mut records = BTreeMap::new();
        for (family, body) in all {
            let r = identity::parse(body.as_bytes())?;
            records.insert(text(&r, "id"), (family, r));
        }
        require(
            records
                .get(assertion)
                .is_some_and(|(f, _)| f == "assertions"),
            "UNKNOWN_REFERENCE",
            "Retraction target is not an assertion",
        )?;
        removed.extend(newly.clone());
        loop {
            let before = removed.len();
            for (id, (family, r)) in &records {
                if family == "assertions" {
                    let mut deps = strings(&r["supports"]);
                    if r["class"] == "derived" {
                        if let Some((_, d)) = records.get(&text(&r["origin"], "derivation_id")) {
                            deps.extend(strings(&d["input_assertion_ids"]));
                        }
                    }
                    if r["class"] == "claim" {
                        if let Some((_, inf)) = records.get(&text(&r["origin"], "inference_id")) {
                            if let Some((_, p)) = records.get(&text(inf, "packet_id")) {
                                deps.extend(strings(&p["assertion_ids"]));
                            }
                        }
                    }
                    if deps.iter().any(|d| removed.contains(d)) && removed.insert(id.clone()) {
                        newly.insert(id.clone());
                    }
                }
            }
            if removed.len() == before {
                break;
            }
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO events(kind,body) VALUES('retraction',?)",
            [json!({"assertion_id":assertion,"reason":reason,"closure":newly}).to_string()],
        )?;
        let watermark = tx.last_insert_rowid();
        for id in &newly {
            tx.execute(
                "INSERT OR IGNORE INTO retractions(assertion_id,watermark,reason) VALUES(?,?,?)",
                params![id, watermark, reason],
            )?;
        }
        tx.commit()?;
        Ok(
            json!({"ledger_watermark":watermark,"retracted":newly,"projections_require_rebuild":true}),
        )
    }
}
