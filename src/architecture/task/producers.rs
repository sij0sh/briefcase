//! Optional producers supply ordinary entities and links to the context index.
//! Publication scopes and exact source/runtime joins remain explicit.
use super::*;
use crate::architecture::{deployment,knowledge,runtime_observations};
const MAX_PUBLICATIONS:usize=32;

pub(super) fn attach(l:&Ledger,s:&mut Snapshot,a:&Value)->Result<()> {
    let snapshot=knowledge::context(&s.view.bundle)["snapshot_id"].clone();
    let selected_deployment=a["deployment_view"].as_str();
    let deployment_ids=if let Some(id)=selected_deployment {vec![l.resolve_scoped("deployment",id)?]} else {l.scoped_views("deployment")?};
    let mut environments=BTreeSet::new();
    let mut desired=vec![];
    for id in deployment_ids.iter().take(MAX_PUBLICATIONS) {
        let view=l.retained(id)?;
        let env=text(&view.diagnostics,"environment");
        if a["runtime_environment"].as_str().is_some_and(|e|e!=env) {continue;}
        if !environments.insert(env.clone()) {continue;}
        if view.diagnostics["source_view_snapshot"]!=snapshot {
            s.profile_boundaries.push(json!({"area":"deployment_source","view_id":id,"environment":env,
                "reason":"Newest desired deployment does not pin the selected source snapshot; no source-to-deployment join established."}));
            continue;
        }
        source_links(l,s,&view)?;
        desired.push(view.clone());
        s.evidence_views.push(view);
    }
    if deployment_ids.len()>MAX_PUBLICATIONS {s.profile_boundaries.push(json!({"area":"deployment_publications","reason":"Publication selection bound reached","omitted":deployment_ids.len()-MAX_PUBLICATIONS}));}
    let from=a["runtime_from"].as_str().map(runtime_observations::seconds).transpose()?;
    let to=a["runtime_to"].as_str().map(runtime_observations::seconds).transpose()?;
    require(from.zip(to).is_none_or(|(f,t)|f<t),"ARGUMENTS","runtime_from must precede runtime_to")?;
    let runtime_ids=l.scoped_views("runtime")?;
    let mut runtime_envs=BTreeSet::new();
    let mut runtime_selected=false;
    for id in runtime_ids.iter().take(MAX_PUBLICATIONS) {
        let view=l.retained(id)?;
        let env=text(&view.diagnostics,"environment");
        if a["runtime_environment"].as_str().is_some_and(|e|e!=env) {continue;}
        let start=view.diagnostics["interval"]["start"].as_str().map(runtime_observations::seconds).transpose()?;
        let end=view.diagnostics["interval"]["end"].as_str().map(runtime_observations::seconds).transpose()?;
        if from.is_some_and(|f|end.is_none_or(|e|e<=f))||to.is_some_and(|t|start.is_none_or(|v|v>=t)) {continue;}
        // Without an interval request, select the newest recording per environment.
        if from.is_none()&&to.is_none()&&!runtime_envs.insert(env.clone()) {continue;}
        runtime_selected=true;
        let d=desired.iter().find(|d|d.diagnostics["environment"]==env);
        if !view.invalidated && d.is_none_or(|d|!d.invalidated) {
            for (_,mapping) in runtime_observations::mappings(l,&view,d)? {
                if mapping["status"]=="exact" {
                    let from=text(&mapping,"runtime_service");let to=text(&mapping,"service");
                    let ids=crate::architecture::contracts::strings(&mapping["desired_assertion_ids"]);
                    s.producer_links.push(Link::new("MAPS_TO_SERVICE",&from,&to,"derived",
                        "explicit retained runtime resource identity and deployment assertions; interval remains separate from source time",
                        ids,vec![],BTreeSet::new(),BTreeSet::new())?);
                } else {
                    s.profile_boundaries.push(json!({"area":"runtime_identity","view_id":id,"mapping":mapping,
                        "reason":"Runtime identity did not join one desired service; name similarity is not a join."}));
                }
            }
        } else {
            s.profile_boundaries.push(json!({"area":"runtime_identity","view_id":id,"reason":"Retraction affects the selected runtime/deployment identity; cross-scope joins withheld."}));
        }
        s.evidence_views.push(view);
    }
    if runtime_ids.len()>MAX_PUBLICATIONS {s.profile_boundaries.push(json!({"area":"runtime_publications","omitted":runtime_ids.len()-MAX_PUBLICATIONS,"reason":"Publication selection bound reached"}));}
    if !runtime_selected && (from.is_some()||to.is_some()||a["runtime_environment"].is_string()) {
        s.profile_boundaries.push(json!({"area":"runtime_selection","environment":a["runtime_environment"],
            "from":a["runtime_from"],"to":a["runtime_to"],"reason":"No retained runtime recording within the bounded publication scan matches the requested environment/interval; this does not establish absence of activity."}));
    }
    if let Some(report)=l.external_inspection(&s.view.id)? {
        let report_id=text(&report,"report_id");
        for observation in rows(&report,"items").iter().filter(|o|o["state"]=="observed") {
            let path=format!("path:{}",text(observation,"path"));
            let matches=crate::architecture::contracts::strings(&observation["matching_source_paths"]);
            if matches.len()!=1 {continue;}
            let mut edge=Link::new("INSTALLED_FROM",&path,&matches[0],"derived",
                &format!("observed byte equality only; no installation causality or active process claim; {report_id}"),
                vec![],vec![],BTreeSet::new(),BTreeSet::from([matches[0].clone()]))?;
            edge.report_ids.push(report_id.clone());s.producer_links.push(edge);
        }
    }
    for view in &s.evidence_views {
        knowledge::merge(&mut s.view.bundle,&view.bundle)?;
        s.view.eligible.extend(view.eligible.clone());
    }
    Ok(())
}
fn source_links(l:&Ledger,s:&mut Snapshot,d:&View)->Result<()> {
    let snapshot=knowledge::context(&s.view.bundle)["snapshot_id"].clone();
    let chains=deployment::chains(d,&l.retracted()?,false);
    let payloads=l.payloads(&d.bundle)?;
    for artifact in rows(&d.bundle,"artifacts") {
        let id=text(artifact,"id");
        let Some(bytes)=payloads.get(&id) else {continue};
        require(identity::bytes_digest(bytes)==artifact["digest"],"DIGEST","Deployment payload is corrupted")?;
        let Ok(manifest)=identity::parse(bytes) else {continue};
        if manifest["kind"]!="briefcase-build-manifest"||manifest["source_snapshot_id"]!=snapshot {continue;}
        let artifact_key=deployment::artifact_key(&text(&manifest,"repository"),&text(&manifest,"image_digest"));
        let eligible:Vec<_>=chains.iter().filter(|c|c["artifact"]==artifact_key&&c["source_revision"]==snapshot).collect();
        let ids:BTreeSet<_>=eligible.iter().flat_map(|c|crate::architecture::contracts::strings(&c["assertion_ids"])).collect();
        if ids.is_empty() {continue;}
        for file in rows(&manifest,"context_files").iter().chain(std::iter::once(&manifest["dockerfile"])) {
            let path=text(file,"path");
            if !s.files.get(&path).is_some_and(|f|f.digest==file["digest"]) {continue;}
            s.producer_links.push(Link::new("INCLUDES_SOURCE",&path,&artifact_key,"derived",
                "retained build manifest and eligible build chain pin the exact source snapshot, path and digest",
                ids.iter().cloned().collect(),vec![Witness{path:text(artifact,"path"),artifact_id:id.clone(),digest:text(artifact,"digest"),start_byte:0,end_byte:bytes.len()}],
                BTreeSet::from([path.clone()]),BTreeSet::new())?);
        }
    }
    Ok(())
}
