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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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

/// Verified source bytes, loaded by artifact identity on first use. Each load
/// checks the retained digest, so laziness never weakens corruption detection.
pub(super) struct Contents<'l> {
    ledger: Option<&'l Ledger>,
    cells: BTreeMap<String, std::cell::OnceCell<std::result::Result<String,String>>>,
    loaded: std::cell::Cell<(usize,usize)>,
}
impl<'l> Contents<'l> {
    /// Verified text of one captured file. Unknown paths are absent.
    pub(super) fn get(&self, files: &BTreeMap<String,Artifact>, path: &str) -> Result<Option<&str>> {
        let Some(cell)=self.cells.get(path) else {return Ok(None)};
        if cell.get().is_none() {self.load(files,&[path])?;}
        match cell.get() {Some(Ok(content))=>Ok(Some(content.as_str())),Some(Err(error))=>Err(error.clone().into()),None=>Err("TASK: Source was not loaded".into())}
    }
    /// One batched, verified read for files a pass will scan.
    pub(super) fn load(&self, files: &BTreeMap<String,Artifact>, paths: &[&str]) -> Result<()> {
        let pending:Vec<_>=paths.iter().filter(|p|self.cells.get(**p).is_some_and(|c|c.get().is_none())).collect();
        if pending.is_empty() {return Ok(());}
        let ledger=self.ledger.ok_or("TASK: Source contents are not attached to a ledger")?;
        let ids:Vec<_>=pending.iter().map(|p|files[**p].artifact_id.clone()).collect();
        let payloads:Payloads=ledger.payloads_for(&ids)?;
        let (mut count,mut bytes)=self.loaded.get();
        for path in pending {
            let a=&files[*path];let payload=&payloads[&a.artifact_id];
            count+=1;bytes+=payload.len();
            let content=if identity::bytes_digest(payload)!=a.digest {Err("DIGEST: Retained task source is corrupted".into())}
                else {std::str::from_utf8(payload).map(str::to_owned).map_err(|e|format!("TASK: Retained source is not UTF-8: {e}"))};
            let _=self.cells[*path].set(content);
        }
        self.loaded.set((count,bytes));
        Ok(())
    }
    /// Visit verified contents in bounded batches without retaining them, so a
    /// whole-corpus pass holds one batch of bytes rather than the snapshot.
    pub(super) fn scan(&self, files: &BTreeMap<String,Artifact>, paths: &[&str], mut visit: impl FnMut(&str,&str)) -> Result<()> {
        for chunk in paths.chunks(256) {
            let fresh:Vec<_>=chunk.iter().filter(|p|self.cells.get(**p).is_some_and(|c|c.get().is_none())).map(|p|files[*p].artifact_id.clone()).collect();
            let payloads:Payloads=if fresh.is_empty() {Payloads::new()} else {
                self.ledger.ok_or("TASK: Source contents are not attached to a ledger")?.payloads_for(&fresh)?};
            for path in chunk {
                if let Some(Some(result))=self.cells.get(*path).map(|c|c.get()) {visit(path,result.as_deref().map_err(|e|e.clone())?);continue;}
                let a=&files[*path];let payload=&payloads[&a.artifact_id];
                let (count,bytes)=self.loaded.get();self.loaded.set((count+1,bytes+payload.len()));
                require(identity::bytes_digest(payload)==a.digest,"DIGEST","Retained task source is corrupted")?;
                visit(path,std::str::from_utf8(payload)?);
            }
        }
        Ok(())
    }
    /// All verified contents, for whole-snapshot passes such as index builds.
    pub(super) fn all(&self, files: &BTreeMap<String,Artifact>) -> Result<BTreeMap<String,&str>> {
        let paths:Vec<_>=self.cells.keys().map(String::as_str).collect();
        self.load(files,&paths)?;
        paths.into_iter().map(|p|Ok((p.to_owned(),self.get(files,p)?.unwrap()))).collect()
    }
    /// Measured source work in this process: files read and bytes digested.
    pub(super) fn work(&self) -> Value {
        let (files,bytes)=self.loaded.get();
        json!({"files_in_snapshot":self.cells.len(),"files_loaded":files,"bytes_verified":bytes})
    }
}

pub(super) struct Snapshot<'l> {
    pub(super) view: View,
    pub(super) profiles: Vec<View>,
    pub(super) profile_boundaries: Vec<Value>,
    pub(super) evidence_views: Vec<View>,
    pub(super) producer_links: Vec<Link>,
    pub(super) files: BTreeMap<String, Artifact>,
    pub(super) contents: Contents<'l>,
}
impl<'l> Snapshot<'l> {
    pub(super) fn load(l:&'l Ledger,selector:&str)->Result<Self>{Self::load_with(l,selector,&json!({}))}
    pub(super) fn load_with(l:&'l Ledger,selector:&str,args:&Value)->Result<Self>{
        let mut snapshot=Self::source(l,selector)?;
        producers::attach(l,&mut snapshot,args)?;
        Ok(snapshot)
    }
    pub(super) fn source(l: &'l Ledger, selector: &str) -> Result<Self> {
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
        let by_identity:std::collections::HashMap<_,_>=rows(&view.bundle,"artifacts").iter().filter(|a|a["source_kind"]!="transport")
            .map(|a|((text(a,"path"),text(a,"digest")),a)).collect();
        let mut artifacts = BTreeMap::new();
        for file in rows(snapshot,"files") {
            let artifact = by_identity.get(&(text(file,"path"),text(file,"digest"))).ok_or("DIGEST: Snapshot source artifact is missing")?;
            artifacts.insert(text(file,"path"), *artifact);
        }
        let ids: Vec<_> = artifacts.values().map(|a| text(a,"id")).collect();
        // Sizes come from retained payload metadata; bytes are read and
        // verified only when a pass needs them.
        let sizes = l.payload_sizes(&ids)?;
        let mut files = BTreeMap::new();
        for (path,a) in artifacts {
            let id = text(a,"id");
            files.insert(path.clone(), Artifact { path: path.clone(), bytes:sizes[&id], artifact_id:id, digest:text(a,"digest"),
                role:source::artifact_role(&path).into(), language:source::language(&path).into() });
        }
        let cells=files.keys().map(|p|(p.clone(),std::cell::OnceCell::new())).collect();
        let contents=Contents {ledger:Some(l),cells,loaded:Default::default()};
        Ok(Self {view,profiles,profile_boundaries,evidence_views:vec![],producer_links:vec![],files,contents})
    }
    /// Verified text of one captured file.
    pub(super) fn content(&self, path: &str) -> Result<Option<&str>> {self.contents.get(&self.files,path)}
    pub(super) fn all_contents(&self) -> Result<BTreeMap<String,&str>> {self.contents.all(&self.files)}
    pub(super) fn witness(&self, path: &str, start: usize, end: usize) -> Witness {
        let a = &self.files[path];
        Witness {path:path.into(),artifact_id:a.artifact_id.clone(),digest:a.digest.clone(),start_byte:start,end_byte:end}
    }
}
