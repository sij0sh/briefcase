//! Explicit, read-only inspection of operator-selected installed files.
//! Observations are advisory reports, scoped to a retained source view and time.
use super::{contracts::{rows,text},freshness,identity,ledger::Ledger,require,Result};
use serde_json::{json,Value};
use std::{io::Read,path::PathBuf};
const MAX_BYTES:u64=64*1024*1024;
pub fn inspect(l:&Ledger,view:&str,paths:&[PathBuf])->Result<Value>{
    require(!paths.is_empty()&&paths.len()<=32,"ARGUMENTS","Select 1..32 external paths explicitly")?;
    let source=l.retained(view)?;
    require(source.scope=="source","SCOPE","Inspection requires a source view")?;
    let mut items=Vec::new();
    for path in paths {
        require(path.is_absolute(),"ARGUMENTS","External paths must be absolute")?;
        let observation=(||->Result<Value>{
            let resolved=path.canonicalize()?;
            let mut file=std::fs::File::open(&resolved)?;let before=file.metadata()?;
            require(before.is_file()&&before.len()<=MAX_BYTES,"EXTERNAL","A regular file at most 64 MiB is required")?;
            let mut bytes=Vec::new();file.by_ref().take(MAX_BYTES+1).read_to_end(&mut bytes)?;
            let after=file.metadata()?;
            require(bytes.len() as u64==before.len()&&before.len()==after.len()&&before.modified().ok()==after.modified().ok(),
                "EXTERNAL","File changed during inspection; retry")?;
            let digest=identity::bytes_digest(&bytes);
            let matches:Vec<_>=rows(&source.bundle,"snapshots").iter().flat_map(|snapshot|rows(snapshot,"files"))
                .filter(|f|f["digest"]==digest).map(|f|text(f,"path")).collect();
            Ok(json!({"path":path,"resolved_path":resolved,"state":"observed","digest":digest,"bytes":bytes.len(),
                "matching_source_paths":matches,"identity":if matches.len()==1 {"exact_retained_bytes"}else if matches.is_empty(){"no_retained_match"}else{"ambiguous_retained_matches"}}))
        })();
        items.push(observation.unwrap_or_else(|error|json!({"path":path,"state":"unavailable","reason":error.to_string()})));
    }
    let mut report=json!({"kind":"external_inspection","view_id":source.id,"pinned_views":[source.id],
        "observed_at_unix_ms":freshness::now_millis(),"items":items,
        "interpretation":"Equality establishes observed bytes, not install causality, process identity, execution or active configuration. Paths may change after inspection."});
    let id=identity::id("external-inspection",&report)?;report["report_id"]=json!(id);
    l.save_report(&id,&report,&[])?;Ok(report)
}
