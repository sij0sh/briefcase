//! Rebuildable, digest-verified evidence spans used by ledger projection checks.
//! Query ranking belongs to the shared search engine. Projection IDs never
//! become canonical entity or assertion identities.
use super::source as units;
use super::{contracts::{rows, strings, text, Payloads},identity,knowledge,require,Result};
use serde_json::{json,Value};
use std::collections::{BTreeMap,BTreeSet};

pub(crate) fn configuration() -> Value {
    json!({"chunker":"briefcase-retained-span-v1","tokenizer":"sqlite-fts5-unicode61-v1","embedding_mode":"disabled",
        "dimensions":0,"sqlite_version":rusqlite::version(),"max_chunk_chars":1024,
        "implementation_digest":identity::bytes_digest(include_bytes!("evidence_index.rs")),
        "source_implementation_digest":identity::bytes_digest(include_bytes!("source.rs"))})
}
pub(crate) fn build_units(
    b: &Value,
    payloads: &Payloads,
    eligible: &BTreeSet<String>,
) -> Result<Vec<Value>> {
    let ctx = knowledge::context(b);
    let entities: BTreeMap<_, _> = rows(b, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), e))
        .collect();
    // Evidence has direct source references. Index those immutable records once
    // instead of tracing/re-hashing the same file for every source declaration
    // during projection construction and query freshness verification.
    let references: BTreeMap<_, _> = rows(b, "evidence_refs")
        .iter()
        .map(|r| (text(r, "id"), r))
        .collect();
    let artifacts: BTreeMap<_, _> = rows(b, "artifacts")
        .iter()
        .map(|r| (text(r, "id"), r))
        .collect();
    let mut verified_artifacts = BTreeSet::new();
    let mut labels: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for a in rows(b, "assertions").iter().filter(|a| {
        eligible.contains(&text(a, "id"))
            && a["class"] == "declaration"
            && a["proposition"]["predicate"] == "BELONGS_TO_DOMAIN"
    }) {
        let subject = text(&a["proposition"], "subject_id");
        let domain =
            &entities[&text(&a["proposition"]["object"], "entity_version_id")]["canonical_key"];
        if let Some(s) = knowledge::module_source(b, payloads, &subject)? {
            labels
                .entry(s.path)
                .or_default()
                .push(json!({"assertion_id":a["id"],"class":"declaration",
                "domain":domain,"entity_version_id":subject,"origin":a["origin"]}));
        }
    }
    let mut out = BTreeMap::new();
    for a in rows(b, "assertions")
        .iter()
        .filter(|a| eligible.contains(&text(a, "id")) && a["class"] == "evidence")
    {
        let aid = text(a, "id");
        let p = &a["proposition"];
        let entity_ids: Vec<_> = [
            text(p, "subject_id"),
            text(&p["object"], "entity_version_id"),
        ]
        .into_iter()
        .filter(|id| !id.is_empty())
        .collect();
        for eid in strings(&a["evidence_ref_ids"]) {
            let er = references
                .get(&eid)
                .ok_or("UNKNOWN_REFERENCE: Missing source evidence")?;
            if er["locator"]["kind"] != "span" {
                continue;
            }
            let artifact_id = text(er, "artifact_id");
            let ar = artifacts
                .get(&artifact_id)
                .ok_or("UNKNOWN_REFERENCE: Missing source artifact")?;
            let Some(bytes) = payloads.get(&artifact_id) else {
                continue;
            };
            if verified_artifacts.insert(artifact_id.clone()) {
                require(
                    identity::bytes_digest(bytes) == ar["digest"],
                    "DIGEST",
                    "Retained source payload is corrupted",
                )?;
            }
            let start = er["locator"]["start"]
                .as_u64()
                .ok_or("ANCHOR: Missing start")? as usize;
            let end = er["locator"]["end"].as_u64().ok_or("ANCHOR: Missing end")? as usize;
            require(
                start < end && end <= bytes.len(),
                "ANCHOR",
                "Retained span is invalid",
            )?;
            require(
                super::contracts::char_boundary(bytes, start),
                "ANCHOR",
                "Span splits UTF-8 codepoint",
            )?;
            let s = knowledge::Source {
                assertion_id: aid.clone(),
                evidence_id: eid,
                artifact_id,
                path: text(ar, "path"),
                start,
                end,
                text: std::str::from_utf8(&bytes[start..end])?.to_owned(),
            };
            let routing_support = labels.get(&s.path).cloned().unwrap_or_default();
            let keys = entity_ids
                .iter()
                .filter_map(|id| entities.get(id))
                .map(|e| text(e, "canonical_key"))
                .collect::<Vec<_>>()
                .join(" ");
            let routing = format!(
                "{} {} {} {}",
                s.path,
                keys,
                text(p, "predicate"),
                routing_support
                    .iter()
                    .map(|r| text(r, "domain"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            for (evidence, start, end) in units::split_oversized(&s.text, 1024) {
                let mut unit = json!({"schema_version":"1.0","context":ctx,"assertion_ids":[aid],"entity_version_ids":entity_ids,
                    "evidence_ref_ids":[s.evidence_id],"artifact_id":s.artifact_id,"source_locator":s.path,
                    "source_byte_range":{"start":s.start+start,"end":s.start+end},"evidence_text":evidence,
                    "routing_text":routing,"routing_support":routing_support,"evidence_class":"evidence",
                    "token_count":units::estimate_tokens(&evidence)});
                let id = identity::id("retrieval-unit", &unit)?;
                unit["id"] = json!(id);
                out.insert(id, unit);
            }
        }
    }
    Ok(out.into_values().collect())
}
