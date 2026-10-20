//! Shared operations on retained canonical knowledge, not on a working tree.
use super::{
    contracts::{self, add, rows, strings, text, Payloads},
    identity, require, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub fn context(b: &Value) -> Value {
    rows(b, "runs")[0]["context"].clone()
}
pub fn row<'a>(b: &'a Value, family: &str, id: &str) -> Result<&'a Value> {
    rows(b, family)
        .iter()
        .find(|r| r["id"] == id)
        .ok_or_else(|| format!("UNKNOWN_REFERENCE: Missing {family} {id}").into())
}
/// Row positions by id for repeated lookups within one pass. Bundle tables
/// only grow while a pass runs, so a stale or missing position falls back to
/// the scan in `row`; the empty index is exactly `row`.
#[derive(Default)]
pub struct Positions(BTreeMap<(&'static str, String), usize>);
impl Positions {
    pub fn of(b: &Value, families: &[&'static str]) -> Self {
        let mut index = BTreeMap::new();
        for &family in families {
            for (i, r) in rows(b, family).iter().enumerate() {
                index.entry((family, text(r, "id"))).or_insert(i);
            }
        }
        Self(index)
    }
    pub fn row<'a>(&self, b: &'a Value, family: &'static str, id: &str) -> Result<&'a Value> {
        if let Some(r) = self
            .0
            .get(&(family, id.to_owned()))
            .and_then(|&i| rows(b, family).get(i))
            .filter(|r| r["id"] == id)
        {
            return Ok(r);
        }
        row(b, family, id)
    }
}
pub fn merge(b: &mut Value, other: &Value) -> Result<()> {
    for family in contracts::TABLES {
        // Index once per table instead of scanning per row. Collision verdicts
        // are unchanged: equal records merge, conflicting ids still fail.
        let mut known: BTreeMap<String, usize> = BTreeMap::new();
        for (i, r) in rows(b, family).iter().enumerate() {
            known.entry(text(r, "id")).or_insert(i);
        }
        for r in rows(other, family) {
            if let Some(&i) = known.get(&text(r, "id")) {
                require(
                    &rows(b, family)[i] == r,
                    "IDENTITY_COLLISION",
                    "Conflicting retained records",
                )?;
            } else {
                // Absence is already established; push directly instead of
                // re-scanning through add().
                known.insert(text(r, "id"), rows(b, family).len());
                b[family]
                    .as_array_mut()
                    .expect("owned bundle table")
                    .push(r.clone());
            }
        }
    }
    Ok(())
}
pub fn artifact(
    b: &mut Value,
    payloads: &mut Payloads,
    category: &str,
    bytes: Vec<u8>,
) -> Result<String> {
    let digest = identity::bytes_digest(&bytes);
    let path = format!(
        ".briefcase/{category}/{}.json",
        digest.trim_start_matches("sha256:")
    );
    let id = identity::id(
        "artifact",
        &json!({"path":path,"digest":digest,"source_kind":"transport"}),
    )?;
    add(
        b,
        "artifacts",
        json!({"id":id,"path":path,"digest":digest,"source_kind":"transport","availability":"available"}),
    );
    payloads.insert(id.clone(), bytes);
    Ok(id)
}
pub fn target(id: &str) -> Value {
    json!({"kind":"entity","entity_version_id":id})
}
pub fn derived(
    b: &mut Value,
    ctx: &Value,
    subject: &str,
    predicate: &str,
    object: Value,
    origin: &str,
    supports: Vec<String>,
) -> Result<String> {
    let mut a = json!({"context":ctx,"class":"derived","proposition":{"subject_id":subject,"predicate":predicate,"object":object},
        "origin":{"derivation_id":origin},"supports":supports,"support_class":"evidence_only"});
    let id = identity::id("assertion", &a)?;
    a["id"] = json!(id);
    add(b, "assertions", a);
    Ok(id)
}

/// Scores are non-identity metadata. The inference ID binds the exact raw output,
/// including the score, without introducing floating-point identity primitives.
pub fn claim(
    b: &mut Value,
    ctx: &Value,
    subject: &str,
    domain: &str,
    inference: &str,
    supports: Vec<String>,
    score: Option<Value>,
) -> Result<String> {
    claim_predicate(
        b,
        ctx,
        subject,
        "BELONGS_TO_DOMAIN",
        domain,
        inference,
        supports,
        score,
    )
}

/// Typed Claim constructor for validated provider proposals beyond domain
/// classification, such as inference-backed historical lineage. Predicate,
/// class, context, and kind admissibility are enforced by contract
/// validation; this constructor only preserves score/metadata separation.
#[allow(clippy::too_many_arguments)]
pub fn claim_predicate(
    b: &mut Value,
    ctx: &Value,
    subject: &str,
    predicate: &str,
    object: &str,
    inference: &str,
    supports: Vec<String>,
    score: Option<Value>,
) -> Result<String> {
    let mut a = json!({"context":ctx,"class":"claim","proposition":{"subject_id":subject,"predicate":predicate,"object":target(object)},
        "origin":{"inference_id":inference},"supports":supports});
    let id = identity::id("assertion", &a)?;
    a["id"] = json!(id);
    if let Some(score) = score {
        a["model_score"] = score;
    }
    add(b, "assertions", a);
    Ok(id)
}

fn support_closure(b: &Value, at: &Positions, roots: &[String]) -> Result<Vec<String>> {
    let mut todo = roots.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(id) = todo.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let a = at.row(b, "assertions", &id)?;
        todo.extend(strings(&a["supports"]));
        if a["class"] == "derived" {
            todo.extend(strings(
                &at.row(b, "derivations", &text(&a["origin"], "derivation_id"))?
                    ["input_assertion_ids"],
            ));
        }
        if a["class"] == "claim" {
            let inf = at.row(b, "inferences", &text(&a["origin"], "inference_id"))?;
            todo.extend(strings(
                &at.row(b, "input_packets", &text(inf, "packet_id"))?["assertion_ids"],
            ));
        }
    }
    Ok(seen.into_iter().collect())
}
#[derive(Clone)]
pub struct Source {
    pub assertion_id: String,
    pub evidence_id: String,
    pub artifact_id: String,
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
}
pub fn sources(b: &Value, payloads: &Payloads, roots: &[String]) -> Result<Vec<Source>> {
    sources_in(b, &Positions::default(), payloads, roots)
}
pub fn sources_in(b: &Value, at: &Positions, payloads: &Payloads, roots: &[String]) -> Result<Vec<Source>> {
    let mut out = BTreeMap::new();
    for id in support_closure(b, at, roots)? {
        let a = at.row(b, "assertions", &id)?;
        for eid in strings(&a["evidence_ref_ids"]) {
            let er = at.row(b, "evidence_refs", &eid)?;
            if er["locator"]["kind"] != "span" {
                continue;
            }
            let ar = at.row(b, "artifacts", &text(er, "artifact_id"))?;
            let Some(bytes) = payloads.get(&text(ar, "id")) else {
                continue;
            };
            require(
                identity::bytes_digest(bytes) == ar["digest"],
                "DIGEST",
                "Retained source payload is corrupted",
            )?;
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
            let source = Source {
                assertion_id: id.clone(),
                evidence_id: eid.clone(),
                artifact_id: text(ar, "id"),
                path: text(ar, "path"),
                start,
                end,
                text: std::str::from_utf8(&bytes[start..end])?.to_owned(),
            };
            out.entry(eid).or_insert(source);
        }
    }
    Ok(out.into_values().collect())
}
/// Only an entire retained module body qualifies for exact content movement.
/// A declaration in its parent file is not the module's implementation.
pub fn module_source(b: &Value, payloads: &Payloads, module: &str) -> Result<Option<Source>> {
    module_source_in(b, &Positions::default(), payloads, module, &containment(b))
}
/// Evidence CONTAINS assertion ids by contained entity, for repeated
/// `module_source_in` lookups without rescanning every assertion.
pub fn containment(b: &Value) -> BTreeMap<String, Vec<(String, Vec<String>)>> {
    let mut index: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for a in rows(b, "assertions") {
        if a["class"] == "evidence" && a["proposition"]["predicate"] == "CONTAINS" {
            if let Some(object) = a["proposition"]["object"]["entity_version_id"].as_str() {
                index
                    .entry(object.to_owned())
                    .or_default()
                    .push((text(a, "id"), strings(&a["evidence_ref_ids"])));
            }
        }
    }
    index
}
pub fn module_source_in(
    b: &Value,
    at: &Positions,
    payloads: &Payloads,
    module: &str,
    containment: &BTreeMap<String, Vec<(String, Vec<String>)>>,
) -> Result<Option<Source>> {
    let entity = at.row(b, "entity_versions", module)?;
    let Some(er) = entity["source_evidence_ref_id"].as_str() else {
        return Ok(None);
    };
    let ids: Vec<_> = containment
        .get(module)
        .into_iter()
        .flatten()
        .filter(|(_, refs)| refs.iter().any(|id| id == er))
        .map(|(id, _)| id.clone())
        .collect();
    // Whole-file binding also supplies retrieval routing and inference packets.
    // Inline modules cannot establish exact file movement.
    Ok(sources_in(b, at, payloads, &ids)?
        .into_iter()
        .filter(|s| {
            super::source::code_extension(&s.path).is_some()
                && s.start == 0
                && payloads
                    .get(&s.artifact_id)
                    .is_some_and(|bytes| bytes.len() == s.end)
                && s.evidence_id == er
        })
        .max_by_key(|s| s.end))
}
