//! Compare selected live bytes to an immutable source pin without publishing.
//! This inventory covers every capturable path, not just the first task slice.
use super::*;
use std::path::Path;

pub fn worktree(l:&Ledger,a:&Value,root:&Path)->Result<Value> {
    let case=if let Some(id)=a["case_id"].as_str() {Some(serde_json::to_value(case::load(l,id)?)?)} else {None};
    let selector=case.as_ref().and_then(|c|c["view_id"].as_str()).unwrap_or_else(||a["view"].as_str().unwrap_or("latest"));
    require(a["view"].as_str().is_none_or(|v|v=="latest"||v==selector),"WORKTREE","view differs from the case source pin")?;
    let view=l.retained(selector)?;
    require(view.scope=="source","SCOPE","Worktree comparison needs a source view")?;
    let root=root.canonicalize()?;
    let recorded=view.diagnostics["capture"]["repository_root"].as_str().ok_or("WORKTREE: Capture has no repository root")?;
    require(Path::new(recorded).canonicalize()?==root,"WORKTREE","Invocation root differs from the retained capture")?;
    let capture=crate::architecture::capture::Capture::read_excluding(&root,None,&l.source_exclusions()?)?;
    let old:BTreeMap<String,String>=rows(&view.bundle,"snapshots").iter().flat_map(|s|rows(s,"files"))
        .map(|f|(text(f,"path"),text(f,"digest"))).collect();
    let current:BTreeMap<String,String>=capture.files.iter().map(|(p,b)|(p.clone(),identity::bytes_digest(b))).collect();
    let selected:BTreeSet<String>=case.as_ref().into_iter().flat_map(|c|source::ARTIFACT_ROLES.iter()
        .flat_map(move |role|rows(&c["slice"],role))).map(|row|text(row,"path")).collect();
    let paths:BTreeSet<_>=old.keys().chain(current.keys()).cloned().collect();
    let mut items=vec![];
    for path in paths {
        let (before,after)=(old.get(&path),current.get(&path));
        if before==after {continue;}
        let state=match (before,after) {
            (None,Some(_))=>"added",(Some(_),Some(_))=>"modified",
            // An excluded/unreadable path is not a demonstrated deletion.
            (Some(_),None)=>if root.join(&path).try_exists()? {"unavailable"} else {"deleted"},
            _=>unreachable!(),
        };
        items.push(json!({"path":path,"state":state,"before_digest":before,"after_digest":after,
            "outside_initial_slice":case.as_ref().map(|_|!selected.contains(&path))}));
    }
    let tree_digest=identity::digest(&json!({"root":root,"head":capture.inventory["capture_head"],"files":current}))?;
    let complete=capture.complete && capture.exclusions.is_empty()
        && capture.inventory["tracked_inventory_available"]==true
        && capture.inventory["unretained_tracked_paths"].as_array().is_some_and(Vec::is_empty);
    let outside=items.iter().filter(|i|i["outside_initial_slice"]==true).count();
    let changed=items.len();
    let old_head=&view.diagnostics["capture"]["capture_head"];
    let live_head=&capture.inventory["capture_head"];
    let head_changed=old_head!=live_head;
    let doc=json!({"kind":"worktree","view_id":view.id,"pinned_views":[view.id],"case_id":a["case_id"],
        "tree_digest":tree_digest,"captured_at_unix_ms":capture.captured_at_unix_ms,
        "alignment":if changed>0||head_changed {"changed"}else if complete {"matches"}else{"unknown"},
        "head_alignment":{"capture_head":old_head,"live_head":live_head,"changed":head_changed},
        "changed_paths":changed,"outside_initial_slice":outside,"items":items,
        "coverage":{"state":if complete {"complete_for_capturable_text"}else{"partial"},
            "inventory":capture.inventory,"capture_complete":capture.complete,
            "not_covered":["ignored, excluded or unavailable files","external repositories and installed files","edits after this read"]},
        "instruction":"Changes include paths outside the initial slice. Refresh source capture before resuming an old task investigation; old graph assertions do not describe new bytes."});
    crate::architecture::reports::retain_items(l,doc,a["limit"].as_u64().unwrap_or(100) as usize)
}
