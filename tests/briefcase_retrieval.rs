//! Retrieval membership, budgets and positive historical/session discovery.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use briefcase::architecture::{capture::Capture,contracts::{rows,text},git_history,rust,session};
use serde_json::{json,Value};
use std::{collections::BTreeSet,process::Command,time::{Duration,Instant}};

fn paths(doc:&Value)->BTreeSet<String> {
    briefcase::architecture::source::ARTIFACT_ROLES.iter().flat_map(|r|rows(&doc["slice"],r)).map(|f|text(f,"path")).collect()
}
fn full_args(task:&str)->Value {json!({"task":task,"target":"src/lib.rs","duty":"debug","limit":1000,"max_bytes":1048576})}
fn write(f:&Fixture,path:&str,body:&str) {
    let destination=f.root.join(path);std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(destination,body).unwrap();
}

#[test]
fn lexical_retrieval_adds_files_even_when_an_explicit_anchor_resolves() {
    let mut f=Fixture::new();write(&f,"src/resume.rs","pub fn quasar_marker() {}\n");f.recapture();
    let args=full_args("repair quasar_marker");let packet=f.call("brief",args.clone());
    assert!(paths(&packet).contains("src/resume.rs"));
    let candidate=rows(&packet,"candidates").iter().find(|c|c["path"]=="src/resume.rs").unwrap();
    assert!(rows(candidate,"signals").iter().any(|s|s["facet"]=="lexical"));
    let mut ablated=args;ablated["exclude_facets"]=json!(["lexical"]);
    assert!(!paths(&f.call("brief",ablated)).contains("src/resume.rs"));
}

#[test]
fn bounded_selection_is_ranked_pinned_and_keeps_omitted_candidates_pageable() {
    let mut f=Fixture::new();
    for i in 0..40 {write(&f,&format!("src/part_{i}.rs"),"pub fn quasar_marker() {}\n");}
    f.recapture();let mut args=full_args("repair quasar_marker");args["max_files"]=json!(6);
    let packet=f.call("brief",args.clone());let selected=paths(&packet);
    assert_eq!(selected.len(),6);assert!(selected.contains("src/lib.rs"));
    assert_eq!(packet["slice"]["impl"][0]["path"],"src/lib.rs");
    assert_eq!(packet["slice"]["impl"][0]["rank"],1);
    assert!(rows(&packet,"not_covered").iter().any(|g|g["area"]=="candidate_selection"));
    assert_ne!(packet["outcome"],"complete");
    let alternatives=f.call("page",json!({"report_id":packet["case_id"],"section":"/candidates","offset":6,"limit":1000,"max_bytes":1048576}));
    assert!(!rows(&alternatives,"items").is_empty());assert!(rows(&alternatives,"items").iter().all(|c|c["selected"]==false));
    args["limit"]=json!(12);args["max_bytes"]=json!(8000);
    let manifest=f.call("agent_context",args);
    assert_eq!(manifest["case_id"],packet["case_id"]);assert_eq!(manifest["kind"],"agent_context");
    assert!(serde_json::to_vec(&manifest).unwrap().len()+1<=8000);assert!(!rows(&manifest,"files").is_empty());
    assert_eq!(manifest["files"][0]["path"],"src/lib.rs");
    let proof=f.call("explain",manifest["files"][0]["fetch"]["arguments"].clone());
    assert!(proof["text"].as_str().unwrap().contains("pub fn start"));
    assert_eq!(manifest["coverage"]["completeness"],packet["coverage"]["completeness"]);
}

#[test]
fn a_reached_hub_does_not_expand_to_every_unrelated_file() {
    let mut f=Fixture::new();let mut hub=String::new();
    write(&f,"src/lib.rs","// See src/hub.rs\npub fn root_marker() {}\n");
    for i in 0..60 {
        let path=format!("src/other_{i}.rs");write(&f,&path,"pub fn unrelated() {}\n");hub.push_str(&format!("// {path}\n"));
    }
    write(&f,"src/hub.rs",&hub);f.recapture();
    let mut args=full_args("repair root_marker");args["exclude_facets"]=json!(["lexical"]);
    let packet=f.call("brief",args);
    assert!(paths(&packet).contains("src/hub.rs"));
    assert!(!paths(&packet).iter().any(|p|p.starts_with("src/other_")));
}

#[test]
fn exact_test_includes_localize_the_test_and_its_runner() {
    let mut f=Fixture::new();
    write(&f,"tests/storage.rs","#[path=\"../src/lib.rs\"] mod implementation;\n");
    write(&f,"tests/runner.rs","#[path=\"storage.rs\"] mod storage;\n");f.recapture();
    let packet=f.call("brief",json!({"task":"which tests exercise storage","target":"src/lib.rs","duty":"test","limit":1000,"max_bytes":1048576}));
    assert!(paths(&packet).contains("tests/storage.rs"));
    // The runner's literal include is retrieved as text or a bounded relation,
    // without attributing executed coverage to either file.
    assert_eq!(packet["coverage"]["completeness"]["verification"],"unknown");
    let adjacency=f.call("neighbors",json!({"target":"tests/storage.rs","direction":"in","predicate":"REFERENCES_ARTIFACT"}));
    assert!(rows(&adjacency,"items").iter().any(|e|e["from"]=="tests/runner.rs"));
}

#[test]
fn small_retained_cochanges_can_introduce_a_file_without_a_source_edge() {
    let mut f=Fixture::new();write(&f,"src/partner.rs","pub fn partner() {}\n");f.recapture();
    let git=|args:&[&str]| {let result=Command::new("git").current_dir(&f.root).args(args).output().unwrap();assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));};
    git(&["init","-q"]);git(&["add","src/lib.rs","src/partner.rs"]);
    git(&["-c","user.name=Test","-c","user.email=test@example.test","commit","-qm","paired maintenance"]);
    git_history::update(&f.ledger,&f.root,Instant::now()+Duration::from_secs(20)).unwrap();
    let args=full_args("repair entrypoint");let packet=f.call("brief",args.clone());
    assert!(paths(&packet).contains("src/partner.rs"));
    let candidate=rows(&packet,"candidates").iter().find(|c|c["path"]=="src/partner.rs").unwrap();
    assert!(rows(candidate,"signals").iter().any(|s|s["facet"]=="history"));
    let mut ablated=args;ablated["exclude_facets"]=json!(["history"]);
    assert!(!paths(&f.call("brief",ablated)).contains("src/partner.rs"));
}

fn import_read(f:&mut Fixture,root:&str,path:&str,success:bool,id:&str) {
    let records=vec![json!({"type":"session","version":3,"id":id,"cwd":root,"timestamp":"2026-01-01T00:00:00Z"}),
        json!({"type":"message","message":{"role":"user","content":"payments rollover repair"}}),
        json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"r","name":"read","arguments":{"path":path}}]}}),
        json!({"type":"message","message":{"role":"toolResult","toolCallId":"r","isError":!success,"content":[{"type":"text","text":"recorded result"}]}})];
    let raw=records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")+"\n";
    session::import(&mut f.ledger,raw.as_bytes(),&session::ImportOptions {profile:"pi-v1".into(),repository_root:root.into(),
        label:None,repository_id:None,source_name:"inline.jsonl".into(),retention:session::Retention::Full}).unwrap();
}

#[test]
fn matching_prior_requests_seed_only_successful_local_recorded_reads() {
    let mut f=Fixture::new();
    for name in ["partner","failed","foreign"] {write(&f,&format!("src/{name}.rs"),"pub fn unrelated() {}\n");}
    f.recapture();let root=f.root.to_string_lossy().into_owned();
    import_read(&mut f,&root,"src/partner.rs",true,"local");
    import_read(&mut f,&root,"src/failed.rs",false,"failed");
    let foreign=f.dir.path().join("foreign");std::fs::create_dir_all(&foreign).unwrap();
    import_read(&mut f,&foreign.to_string_lossy(),"src/foreign.rs",true,"foreign");
    let args=full_args("repair payments rollover");let packet=f.call("brief",args.clone());let selected=paths(&packet);
    assert!(selected.contains("src/partner.rs"));assert!(!selected.contains("src/failed.rs"));assert!(!selected.contains("src/foreign.rs"));
    let candidate=rows(&packet,"candidates").iter().find(|c|c["path"]=="src/partner.rs").unwrap();
    assert!(rows(candidate,"signals").iter().any(|s|s["facet"]=="sessions"&&s["kind"]=="file_read"));
    let mut ablated=args;ablated["exclude_facets"]=json!(["sessions"]);
    assert!(!paths(&f.call("brief",ablated)).contains("src/partner.rs"));
}

#[test]
fn thousands_of_producer_occurrences_do_not_evict_neighbors_or_duplicate_case_gaps() {
    let mut f=Fixture::new();
    write(&f,"Cargo.toml","[package]\nname='bounded-health'\nversion='0.1.0'\nedition='2021'\n");
    let code=(0..1200).map(|i|format!("use missing::Item{i};\n")).collect::<String>();write(&f,"src/lib.rs",&code);
    let repo=f.ledger.repository().unwrap();let generation=f.ledger.begin(&repo,None).unwrap();
    let prepared=rust::produce(Capture::read(&f.root,None).unwrap(),&repo,generation,&rust::ScanOptions {analyzer:None,..Default::default()}).unwrap();
    let view=f.ledger.commit(generation,&prepared).unwrap();f.ledger.publish(&view,true).unwrap();
    let neighbors=f.call("neighbors",json!({"target":"src/lib.rs","limit":1,"max_bytes":8000}));
    assert!(!rows(&neighbors,"items").is_empty());assert!(serde_json::to_vec(&neighbors).unwrap().len()+1<=8000);
    assert!(neighbors["coverage"]["unresolved_occurrences"].as_u64().unwrap()>=1200);
    let summary=f.call("page",neighbors["coverage"]["details"]["arguments"].clone());
    let producer=&rows(&summary,"items")[0];
    let detail=f.call("page",json!({"report_id":producer["detail_report_id"],"section":"/items","limit":5,"max_bytes":8000}));
    assert_eq!(rows(&detail,"items").len(),5);
    let packet=f.call("brief",full_args("inspect imports"));
    let gaps:Vec<_>=rows(&packet,"not_covered").iter().filter(|g|g["area"]=="producer_boundary").collect();
    assert_eq!(gaps.len(),1);assert!(gaps[0]["count"].as_u64().unwrap()>=1200);assert!(rows(gaps[0],"examples").len()<=3);
}
