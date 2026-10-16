//! Verified retained evidence upstream of task tracing.
use super::*;

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
    /// Files that support an assertion but are not traversal endpoints.
    #[serde(default, skip_serializing_if="BTreeSet::is_empty")]
    pub support_files: BTreeSet<String>,
}
impl Link {
    #[allow(clippy::too_many_arguments)] // One explicit, explainable relationship record.
    pub(super) fn new(predicate: &str, from: &str, to: &str, class: &str, basis: &str,
        assertion_ids: Vec<String>, witnesses: Vec<Witness>,
        from_files: BTreeSet<String>, to_files: BTreeSet<String>) -> Result<Self> {
        let id = identity::id("task-link", &json!([METHOD,predicate,from,to,class,basis,assertion_ids,witnesses]))?;
        Ok(Self { id, predicate: predicate.into(), from: from.into(), to: to.into(), class: class.into(),
            basis: basis.into(), assertion_ids, witnesses, value:None, report_ids:vec![], from_files, to_files,
            support_files:BTreeSet::new() })
    }
}

pub(super) struct Snapshot {
    pub(super) view: View,
    pub(super) profiles: Vec<View>,
    pub(super) profile_boundaries: Vec<Value>,
    pub(super) evidence_views: Vec<View>,
    pub(super) producer_links: Vec<Link>,
    pub(super) files: BTreeMap<String, Artifact>,
    pub(super) contents: BTreeMap<String, String>,
}
impl Snapshot {
    pub(super) fn load(l:&Ledger,selector:&str)->Result<Self>{Self::load_with(l,selector,&json!({}))}
    pub(super) fn load_with(l:&Ledger,selector:&str,args:&Value)->Result<Self>{
        let mut snapshot=Self::source(l,selector)?;
        producers::attach(l,&mut snapshot,args)?;
        Ok(snapshot)
    }
    pub(super) fn source(l: &Ledger, selector: &str) -> Result<Self> {
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
    pub(super) fn witness(&self, path: &str, start: usize, end: usize) -> Witness {
        let a = &self.files[path];
        Witness {path:path.into(),artifact_id:a.artifact_id.clone(),digest:a.digest.clone(),start_byte:start,end_byte:end}
    }
}
