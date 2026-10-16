//! Narrow reads of retained session metadata and recorded user requests.
//! Listing/lexical recall does not hydrate transcripts or rebuild architecture
//! projections. Architecture Evidence queries still use the validated View.
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

    /// JSON1 selects only normalized user-message records. Large tool results
    /// and the raw transcript never cross this read boundary. Metadata retention
    /// has no text and therefore cannot become a lexical request hit.
    pub(crate) fn session_request_events(&self, limit: usize) -> Result<Vec<Value>> {
        for publication in self.session_publications()? {
            let id = text(&publication["context"], "normalized_artifact_id");
            let (body,raw):(String,Vec<u8>)=self.db.query_row("SELECT r.body,p.bytes FROM records r JOIN payloads p ON p.artifact_id=r.id WHERE r.id=?",[&id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let artifact = identity::parse(body.as_bytes())?;
            require(
                identity::bytes_digest(&raw) == artifact["digest"],
                "DIGEST",
                "Retained normalized session payload is corrupted",
            )?;
        }
        let mut statement = self.db.prepare("SELECT v.id,json_extract(v.bundle,'$.session_contexts[0].id'),json_extract(v.bundle,'$.session_contexts[0].harness'),e.value FROM views v JOIN scans s ON s.generation=v.generation JOIN payloads p ON p.artifact_id=json_extract(v.bundle,'$.session_contexts[0].normalized_artifact_id') JOIN json_each(CAST(p.bytes AS TEXT),'$.events') e WHERE s.scope='session' AND v.projection_generation>0 AND json_extract(e.value,'$.kind')='user_message' AND json_type(e.value,'$.text')='text' ORDER BY v.generation DESC,CAST(json_extract(e.value,'$.ordinal') AS INTEGER) LIMIT ?")?;
        let raw = statement
            .query_map([limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter().map(|(view,context,harness,event)|Ok(json!({"view_id":view,"session_context_id":context,
            "harness":identity::parse(harness.as_bytes())?,"event":identity::parse(event.as_bytes())?}))).collect()
    }
}
