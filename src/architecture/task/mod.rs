//! Duty-oriented composition over retained evidence. These are read artifacts,
//! not a new authority: every relationship keeps its assertion or byte basis.
mod brief;
mod duty;
pub use duty::{Duty, DutyTemplate};
mod index;
mod joins;
mod queries;
mod producers;

pub use brief::{brief, page};
pub(crate) use brief::table_paths as report_sections;
pub use queries::{explain, neighbors, occurrences};
pub(crate) use index::precompute;

use crate::architecture::{
    contracts::{rows, text, Payloads}, identity, ledger::{Ledger, View}, require, source, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const METHOD: &str = "task-context-v1";

pub(super) fn inventory_summary(inventory:&Value) -> Value {
    if !inventory.is_object() {return json!({"state":"unknown"});}
    let mut summary=inventory.clone();
    for field in ["unretained_tracked_paths","exclusions","diagnostics"] {
        let count=rows(inventory,field).len();
        summary.as_object_mut().unwrap().remove(field);
        summary[format!("{field}_count")]=json!(count);
    }
    summary
}


/// Independent availability layers. A missing semantic or runtime layer never
/// suppresses retained text; availability is not a freshness guarantee.
pub fn readiness(l: &Ledger) -> Result<Value> {
    let source=l.resolve("latest").ok().map(|id|l.retained(&id)).transpose()?;
    let source_state=source.as_ref().map(|v|json!({"state":"retained","view_id":v.id,
        "capture":v.diagnostics["capture"],"file_count":rows(&v.bundle,"snapshots").iter().map(|s|rows(s,"files").len()).sum::<usize>()}))
        .unwrap_or(json!({"state":"unavailable"}));
    let semantic=|predicate:&str| {
        let runs:Vec<_>=source.as_ref().into_iter().flat_map(|v|rows(&v.bundle,"runs")).filter(|r|
            rows(&r["coverage"],"guarantees").iter().any(|g|g["predicate"] == predicate)).collect();
        json!({"state":if runs.is_empty() {"not_run"} else if runs.iter().all(|r|r["coverage"]["resolution_complete"] == true) {"complete_for_producer_scope"} else {"partial"},
            "runs":runs.iter().map(|r|json!({"id":r["id"],"coverage":r["coverage"]})).collect::<Vec<_>>()})
    };
    let history=l.history_tip()?;
    let sessions=l.session_publications()?;
    Ok(json!({"source_text":source_state,"parsed_source":semantic("DECLARES"),"static_analysis":semantic("CALLS_STATIC"),
        "history":{"state":if history.is_some() {"retained"} else {"not_run"},"tip":history.map(|h|h.0)},
        "sessions":{"state":if sessions.is_empty() {"not_imported"} else {"retained"},"publications":sessions.len()},
        "deployment":{"publications":l.scoped_views("deployment")?.len()},"runtime":{"publications":l.scoped_views("runtime")?.len()},
        "degraded_operations":["brief","search","occurrences","neighbors","page"],
        "interpretation":"Each layer has independent coverage. Retained data can lag live files, HEAD, sessions or runtime."}))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub artifact_id: String,
    pub digest: String,
    pub role: String,
    pub language: String,
    pub bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Witness {
    pub path: String,
    pub artifact_id: String,
    pub digest: String,
    pub start_byte: usize,
    pub end_byte: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub id: String,
    pub predicate: String,
    pub from: String,
    pub to: String,
    pub class: String,
    pub basis: String,
    pub assertion_ids: Vec<String>,
    pub witnesses: Vec<Witness>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub value: Option<Value>,
    #[serde(default)]
    pub report_ids: Vec<String>,
    pub from_files: BTreeSet<String>,
    pub to_files: BTreeSet<String>,
}
impl Link {
    #[allow(clippy::too_many_arguments)] // One explicit, explainable relationship record.
    fn new(predicate: &str, from: &str, to: &str, class: &str, basis: &str,
        assertion_ids: Vec<String>, witnesses: Vec<Witness>,
        from_files: BTreeSet<String>, to_files: BTreeSet<String>) -> Result<Self> {
        let id = identity::id("task-link", &json!([METHOD,predicate,from,to,class,basis,assertion_ids,witnesses]))?;
        Ok(Self { id, predicate: predicate.into(), from: from.into(), to: to.into(), class: class.into(),
            basis: basis.into(), assertion_ids, witnesses, value:None, report_ids:vec![], from_files, to_files })
    }
}

/// Preserve the packet contract while reducing presentation. Full tables stay
/// in the retained report; every dropped row updates its section cursor.
pub(crate) fn trim_packet(value: &mut Value) -> bool {
    if value["kind"] != "brief" {return false;}
    let largest=brief::table_paths(value).into_iter().filter(|p|p != "/not_covered")
        .filter_map(|p|value.pointer(&p).and_then(Value::as_array).map(|a|(p,a.len())))
        .filter(|(_,n)|*n>0).max_by_key(|(_,n)|*n);
    let Some((pointer,_))=largest else {return false};
    let items=value.pointer_mut(&pointer).unwrap().as_array_mut().unwrap();items.pop();let returned=items.len();
    let delivery=&mut value["coverage"]["delivery"][&pointer];
    let total=delivery["total"].as_u64().unwrap_or(returned as u64+1);
    *delivery=json!({"total":total,"returned":returned,"omitted":total.saturating_sub(returned as u64),"next_offset":returned});
    value["continue_with"]=json!({"operation":"page","arguments":{"report_id":value["id"],"section":pointer,"offset":returned}});
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entity {
    key: String,
    kind: String,
    names: BTreeSet<String>,
    files: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Index {
    id: String,
    source_view: String,
    retraction_epoch: u64,
    files: BTreeMap<String, Artifact>,
    entities: BTreeMap<String, Entity>,
    links: Vec<Link>,
    coverage: Value,
}
struct Snapshot {
    view: View,
    profiles: Vec<View>,
    profile_boundaries: Vec<Value>,
    evidence_views: Vec<View>,
    producer_links: Vec<Link>,
    files: BTreeMap<String, Artifact>,
    contents: BTreeMap<String, String>,
}
impl Snapshot {
    fn load(l:&Ledger,selector:&str)->Result<Self>{Self::load_with(l,selector,&json!({}))}
    fn load_with(l:&Ledger,selector:&str,args:&Value)->Result<Self>{
        let mut snapshot=Self::source(l,selector)?;
        producers::attach(l,&mut snapshot,args)?;
        Ok(snapshot)
    }
    fn source(l: &Ledger, selector: &str) -> Result<Self> {
        let mut view = l.retained(selector)?;
        require(view.scope == "source", "SCOPE", "Task context requires a source view")?;
        let mut profiles=vec![view.clone()];
        let mut profile_boundaries=vec![];
        let snapshot_id=crate::architecture::knowledge::context(&view.bundle)["snapshot_id"].clone();
        if let Some((_,_,state))=l.sync_state()? {
            for pin in rows(&state,"profile_views") {
                let Some(id)=pin["view_id"].as_str().filter(|id|*id!=view.id) else {continue};
                let profile=l.retained(id)?;
                if crate::architecture::knowledge::context(&profile.bundle)["snapshot_id"] == snapshot_id {
                    crate::architecture::knowledge::merge(&mut view.bundle,&profile.bundle)?;
                    view.eligible.extend(profile.eligible.clone());
                    view.invalidated |= profile.invalidated;
                    profiles.push(profile);
                } else {
                    profile_boundaries.push(json!({"area":"profile_snapshot","view_id":id,
                        "reason":"Profile snapshot differs from the selected source snapshot; its semantic facts were not joined."}));
                }
            }
        }
        let snapshot = rows(&view.bundle,"snapshots").iter()
            .find(|s| s["id"] == crate::architecture::knowledge::context(&view.bundle)["snapshot_id"])
            .ok_or("TASK: Missing pinned snapshot")?;
        // Match manifest paths AND digests; analysis transports are not source files.
        let mut artifacts = BTreeMap::new();
        for file in rows(snapshot,"files") {
            let artifact = rows(&view.bundle,"artifacts").iter().find(|a|
                a["path"] == file["path"] && a["digest"] == file["digest"] && a["source_kind"] != "transport")
                .ok_or("DIGEST: Snapshot source artifact is missing")?;
            artifacts.insert(text(file,"path"), artifact);
        }
        let ids: Vec<_> = artifacts.values().map(|a| text(a,"id")).collect();
        let payloads: Payloads = l.payloads_for(&ids)?;
        let mut files = BTreeMap::new();
        let mut contents = BTreeMap::new();
        for (path,a) in artifacts {
            let id = text(a,"id");
            let bytes = &payloads[&id];
            require(identity::bytes_digest(bytes) == a["digest"], "DIGEST", "Retained task source is corrupted")?;
            let content = std::str::from_utf8(bytes)?.to_owned();
            files.insert(path.clone(), Artifact { path: path.clone(), artifact_id:id, digest:text(a,"digest"),
                role:source::artifact_role(&path).into(), language:source::language(&path).into(), bytes:bytes.len() });
            contents.insert(path, content);
        }
        Ok(Self {view,profiles,profile_boundaries,evidence_views:vec![],producer_links:vec![],files,contents})
    }
    fn witness(&self, path: &str, start: usize, end: usize) -> Witness {
        let a = &self.files[path];
        Witness {path:path.into(),artifact_id:a.artifact_id.clone(),digest:a.digest.clone(),start_byte:start,end_byte:end}
    }
}

fn words_match(haystack: &str, needle: &str) -> bool {
    !mentions(haystack,needle).is_empty()
}
fn mentions(haystack:&str,needle:&str) -> Vec<(usize,usize)> {
    if needle.is_empty() { return vec![]; }
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    haystack.match_indices(needle.as_str()).filter_map(|(start,_)| {
        let end = start + needle.len();
        let word = |c:char| c.is_alphanumeric() || c == '_';
        let matched=(start == 0 || !haystack[..start].chars().next_back().is_some_and(word))
            && (end == haystack.len() || !haystack[end..].chars().next().is_some_and(word))
        ;matched.then_some((start,end))
    }).collect()
}

/// Exact identity first. Mentions and terminal identifier matches are candidates;
/// several identities remain ambiguous, even when one seems more relevant.
fn resolve(ix: &Index, target: &str, explicit: bool) -> (String, BTreeSet<String>, BTreeSet<String>) {
    let mut keys = BTreeSet::new();
    let mut files = BTreeSet::new();
    if [".","./","/"].contains(&target) {
        return ("resolved".into(),ix.files.keys().cloned().collect(),ix.files.keys().cloned().collect());
    } else if ix.files.contains_key(target) {
        keys.insert(target.into()); files.insert(target.into());
    } else if ix.entities.contains_key(target) {
        keys.insert(target.into()); files.extend(ix.entities[target].files.clone());
    } else if explicit && target.ends_with('/') {
        files.extend(ix.files.keys().filter(|p| p.starts_with(target)).cloned());
        keys.extend(files.clone());
        return (if files.is_empty() {"unresolved"} else {"resolved"}.into(),keys,files);
    } else {
        let mut named=vec![];
        for (path,_) in &ix.files {
            if !explicit && words_match(target,path) { keys.insert(path.clone()); }
        }
        for (key,e) in &ix.entities {
            if e.names.iter().any(|n| if explicit {n.eq_ignore_ascii_case(target)} else {words_match(target,n)})
                || (!explicit && words_match(target,key)) {
                keys.insert(key.clone());
            }
            if !explicit {
                for name in &e.names {for (start,end) in mentions(target,name) {named.push((key.clone(),start,end));}}
            }
        }
        if !explicit {
            // A short alias inside a longer exact identity mention is not an
            // independent anchor. Disjoint mentions and equal-span identities
            // remain ambiguous, as in the semantic feature router.
            let all=named.clone();
            named.retain(|(key,s,e)|!all.iter().any(|(other,s2,e2)|other!=key && s2<=s && e<=e2 && e2-s2>e-s));
            let survivors:BTreeSet<_>=named.into_iter().map(|(key,_,_)|key).collect();
            keys.retain(|key|ix.files.contains_key(key) || survivors.contains(key));
        }
        // A file mention is stronger than the modules it happens to contain.
        let paths: BTreeSet<_> = keys.iter().filter(|k| ix.files.contains_key(*k)).cloned().collect();
        if !paths.is_empty() { keys = paths; }
        if keys.len() == 1 {
            for key in &keys {
                if ix.files.contains_key(key) { files.insert(key.clone()); }
                if let Some(e) = ix.entities.get(key) { files.extend(e.files.clone()); }
            }
        }
    }
    (match keys.len() {0=>"unresolved",1=>"resolved",_=>"ambiguous"}.into(),keys,files)
}

fn page_items(kind: &str, view: &str, items: &[Value], offset: usize, limit: usize) -> Value {
    let end = offset.saturating_add(limit).min(items.len());
    let returned = items.get(offset..end).unwrap_or(&[]);
    json!({"kind":kind,"view_id":view,"items":returned,"page":{"offset":offset,"returned":returned.len(),
        "total":items.len(),"remaining":items.len().saturating_sub(end),"next_offset":(end < items.len()).then_some(end)},
        "ordering":"deterministic, exhaustive within the stated retained scope; not ranked"})
}
