//! Rebuildable session fact units used by the retrieval projection.
//! Query ranking belongs to the shared search engine. Projection IDs never
//! become canonical entity or assertion identities.
use super::{
    contracts::{rows, text, Payloads},
    identity, knowledge, source, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// One fact is one assertion. Long literals are capped so a pasted transcript
/// cannot turn a bounded session index into a second copy of the transcript.
pub(crate) const MAX_FACT_BYTES: usize = 16 * 1024;

pub(crate) fn configuration() -> Value {
    json!({"chunker":"briefcase-session-fact-v1","tokenizer":"sqlite-fts5-unicode61-v1",
        "embedding_mode":"disabled","dimensions":0,"sqlite_version":rusqlite::version(),
        "max_fact_bytes":MAX_FACT_BYTES,
        "implementation_digest":identity::bytes_digest(include_bytes!("evidence_index.rs"))})
}
pub(crate) fn configuration_digest() -> Result<String> {
    identity::digest(&configuration())
}

/// The fact represented by one normalized event, using the same conditions as
/// session import. A failed read or edit attempt is not a fact.
fn event_fact(e: &Value) -> Option<(&'static str, &str)> {
    match e["kind"].as_str()? {
        "user_message" => Some(("RECORDED_USER_MESSAGE", e["text"].as_str()?)),
        "file_read"
            if e["outcome"] == "succeeded" && e["path"]["location"] != "unresolved" =>
        {
            Some(("READ_PATH", e["path"]["path"].as_str()?))
        }
        "file_edit_confirmed" if e["path"]["location"] != "unresolved" => {
            Some(("EDITED_PATH", e["path"]["path"].as_str()?))
        }
        "command_finished" => Some(("RAN_COMMAND", e["command"].as_str()?)),
        _ => None,
    }
}

fn key(predicate: &str, literal: &str) -> (String, String) {
    (
        predicate.to_owned(),
        identity::bytes_digest(literal.as_bytes()),
    )
}

fn capped(literal: &str) -> (String, bool) {
    let mut end = 0;
    for (index, character) in literal.char_indices() {
        let next = index + character.len_utf8();
        if next > MAX_FACT_BYTES {
            break;
        }
        end = next;
    }
    (literal[..end].to_owned(), end < literal.len())
}

fn predicate_words(predicate: &str) -> &'static str {
    match predicate {
        "RECORDED_USER_MESSAGE" => "user message request",
        "READ_PATH" => "read path file",
        "EDITED_PATH" => "edited path file",
        "RAN_COMMAND" => "ran command shell",
        _ => "session fact",
    }
}

pub(crate) fn build_units(
    b: &Value,
    payloads: &Payloads,
    eligible: &BTreeSet<String>,
    scope: &str,
) -> Result<Vec<Value>> {
    if scope != "session" {
        return Ok(Vec::new());
    }
    let ctx = knowledge::context(b);
    let session = rows(b, "session_contexts")
        .first()
        .ok_or("SESSION: Missing session context")?;
    let session_id = text(session, "id");
    let harness = session["harness"].clone();
    let normalized_id = text(session, "normalized_artifact_id");

    // Index assertions by the event fact they represent. One assertion can be
    // supported by several recorded lines; the first line is the display handle.
    let mut facts: BTreeMap<(String, String), (&Value, &str)> = BTreeMap::new();
    for a in rows(b, "assertions")
        .iter()
        .filter(|a| eligible.contains(&text(a, "id")) && a["class"] == "evidence")
    {
        let predicate = text(&a["proposition"], "predicate");
        if !super::session::PREDICATES.contains(&predicate.as_str()) {
            continue;
        }
        let Some(literal) = a["proposition"]["object"]["value"].as_str() else {
            continue;
        };
        facts
            .entry(key(&predicate, literal))
            .or_insert((a, literal));
    }

    let mut handles: BTreeMap<(String, String), Value> = BTreeMap::new();
    if let Some(bytes) = payloads.get(&normalized_id) {
        let normalized = identity::parse(bytes)?;
        for e in rows(&normalized, "events") {
            let Some((predicate, literal)) = event_fact(e) else {
                continue;
            };
            let fact_key = key(predicate, literal);
            if facts.contains_key(&fact_key) {
                let mut handle = json!({
                    "event_id": e["id"],
                    "ordinal": e["ordinal"],
                    "timestamp": e["timestamp"],
                });
                if handle["event_id"].is_null() {
                    handle.as_object_mut().unwrap().remove("event_id");
                }
                if handle["timestamp"].is_null() {
                    handle.as_object_mut().unwrap().remove("timestamp");
                }
                handles.entry(fact_key).or_insert(handle);
            }
        }
    }

    let mut out = BTreeMap::new();
    for (fact_key, (a, literal)) in facts {
        let (body_text, truncated) = capped(literal);
        let omitted = literal.len().saturating_sub(body_text.len());
        let predicate = text(&a["proposition"], "predicate");
        let harness_name = text(&harness, "name");
        let external_id = session["external_session_id"].as_str().unwrap_or("");
        let routing = format!(
            "{} {} {} {}",
            predicate_words(&predicate),
            predicate,
            harness_name,
            external_id
        );
        let mut unit = json!({
            "schema_version":"1.0",
            "context":ctx,
            "assertion_ids":[text(a,"id")],
            "entity_version_ids":[text(&a["proposition"],"subject_id")],
            "evidence_ref_ids":a["evidence_ref_ids"].clone(),
            "evidence_class":"evidence",
            "predicate":predicate,
            "session_context_id":session_id,
            "harness":harness,
            "text":body_text,
            "routing_text":routing,
            "omitted_bytes":omitted,
            "truncated":truncated,
            "token_count":source::estimate_tokens(&body_text)
        });
        if !truncated {
            unit["literal"] = json!(literal);
        }
        if let Some(handle) = handles.get(&fact_key) {
            for (k, v) in handle.as_object().unwrap() {
                unit[k] = v.clone();
            }
        }
        let id = identity::id("retrieval-unit", &unit)?;
        unit["id"] = json!(id);
        out.insert(id, unit);
    }
    Ok(out.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_on_a_character_boundary_and_keeps_the_omitted_byte_count() {
        let literal = "a".repeat(MAX_FACT_BYTES) + "é";
        let (body, truncated) = capped(&literal);
        assert!(truncated);
        assert_eq!(body.len(), MAX_FACT_BYTES);
        assert_eq!(literal.len() - body.len(), "é".len());

        let (body, truncated) = capped("short");
        assert!(!truncated);
        assert_eq!(body, "short");
    }

    #[test]
    fn only_completed_and_resolved_events_are_facts() {
        let read = json!({"kind":"file_read","outcome":"succeeded",
            "path":{"location":"repository","path":"src/lib.rs"}});
        assert_eq!(event_fact(&read), Some(("READ_PATH", "src/lib.rs")));
        let failed = json!({"kind":"file_read","outcome":"failed",
            "path":{"location":"repository","path":"src/lib.rs"}});
        assert_eq!(event_fact(&failed), None);
        let unresolved = json!({"kind":"file_read","outcome":"succeeded",
            "path":{"location":"unresolved","path":"src/lib.rs"}});
        assert_eq!(event_fact(&unresolved), None);
        let command = json!({"kind":"command_finished","command":"cargo test"});
        assert_eq!(event_fact(&command), Some(("RAN_COMMAND", "cargo test")));
    }
}
