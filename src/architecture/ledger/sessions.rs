//! Narrow reads of retained session metadata and the session fact index.
//! Listing does not hydrate transcripts. Fact search returns only persisted
//! assertion literals; architecture Evidence queries still use the validated
//! View.
use super::*;
impl Ledger {
    pub(crate) fn session_publications(&self) -> Result<Vec<Value>> {
        let mut statement = self.db.prepare("SELECT v.id,s.label,json_extract(v.bundle,'$.session_contexts[0]') FROM views v JOIN scans s ON s.generation=v.generation WHERE s.scope='session' AND v.projection_generation>0 ORDER BY v.generation DESC")?;
        let raw = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter()
            .map(|(view, label, raw)| {
                Ok(json!({"view_id":view,"label":label,"context":identity::parse(raw.as_bytes())?}))
            })
            .collect()
    }

    /// Rank persisted session facts over current publications. Old span
    /// representations and views waiting for a fact rebuild are excluded.
    pub(crate) fn session_fact_search(
        &self,
        expression: &str,
        configuration: &str,
        limit: usize,
    ) -> Result<Vec<(String, Value)>> {
        let mut statement = self.db.prepare(
            "SELECT v.id,p.id,u.id,u.body,f.evidence_text,f.routing_text
             FROM retrieval_fts f
             JOIN retrieval_units u ON u.id=f.unit_id AND u.projection_id=f.projection_id
             JOIN projections p ON p.id=f.projection_id
             JOIN views v ON v.id=p.view_id AND v.projection_generation=p.generation
             JOIN scans s ON s.generation=v.generation
             WHERE retrieval_fts MATCH ?
               AND s.scope='session'
               AND p.kind='retrieval'
               AND json_extract(p.body,'$.configuration_digest')=?
               AND p.watermark >=
                   (SELECT COALESCE(MAX(watermark),0) FROM retractions)
               AND json_array_length(p.body,'$.unit_ids')=
                   (SELECT COUNT(*) FROM retrieval_units u2 WHERE u2.projection_id=p.id)
               AND json_array_length(p.body,'$.unit_ids')=
                   (SELECT COUNT(*) FROM retrieval_fts f2 WHERE f2.projection_id=p.id)
             ORDER BY bm25(retrieval_fts), v.generation DESC, f.unit_id
             LIMIT ?",
        )?;
        let rows = statement
            .query_map(params![expression, configuration, limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(view, projection, stored_id, raw, evidence, routing)| {
                let unit = identity::parse(raw.as_bytes())?;
                let uid = text(&unit, "id");
                let mut body = unit.clone();
                body.as_object_mut().unwrap().remove("id");
                require(
                    uid == identity::id("retrieval-unit", &body)?,
                    "FRESHNESS",
                    "Session fact projection needs rebuild",
                )?;
                require(
                    stored_id == format!("{projection}/{uid}")
                        && unit["text"] == evidence
                        && unit["routing_text"] == routing,
                    "FRESHNESS",
                    "Session fact projection needs rebuild",
                )?;
                Ok((view, unit))
            })
            .collect()
    }

    /// `(indexed, total)` session publications for one fact configuration.
    pub(crate) fn session_fact_coverage(&self, configuration: &str) -> Result<(usize, usize)> {
        let total = self.session_publications()?.len();
        let indexed: i64 = self.db.query_row(
            "SELECT COUNT(DISTINCT v.id)
             FROM projections p
             JOIN views v ON v.id=p.view_id AND v.projection_generation=p.generation
             JOIN scans s ON s.generation=v.generation
             WHERE s.scope='session'
               AND p.kind='retrieval'
               AND json_extract(p.body,'$.configuration_digest')=?
               AND p.watermark >=
                   (SELECT COALESCE(MAX(watermark),0) FROM retractions)
               AND json_array_length(p.body,'$.unit_ids')=
                   (SELECT COUNT(*) FROM retrieval_units u2 WHERE u2.projection_id=p.id)
               AND json_array_length(p.body,'$.unit_ids')=
                   (SELECT COUNT(*) FROM retrieval_fts f2 WHERE f2.projection_id=p.id)",
            [configuration],
            |r| r.get(0),
        )?;
        Ok((indexed as usize, total))
    }
}
