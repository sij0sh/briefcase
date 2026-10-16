//! Task-level regression cases, independent of optional evaluation fixtures
//! and external language toolchains.
use serde_json::{json,Value};
use std::{collections::BTreeSet,path::PathBuf,process::Command};
use briefcase::architecture::{capture::Capture,contracts::{rows,text},identity,ledger::Ledger,service,source,task};

struct Fixture {
    _dir:tempfile::TempDir,
    root:PathBuf,
    db:PathBuf,
    ledger:Ledger,
    view:String,
}
impl Fixture {
    fn new() -> Self {
        let dir=tempfile::tempdir().unwrap();let root=dir.path().join("repo");let db=dir.path().join("context.db");
        std::fs::create_dir_all(&root).unwrap();
        for (path,body) in [
            ("src/store.rs","// briefcase: implements-plan docs/plans/import.md#validation\npub fn save() { let _ = std::env::var(\"DEMO_TOKEN\"); }\n"),
            ("tests/store.rs","#[path = \"../src/store.rs\"] mod store;\n#[test] fn stores(){store::save();}\n"),
            ("docs/plans/import.md","# Validation\nUpdate `src/store.rs` and `tests/store.rs`.\n"),
            ("docs/usage.md","The implementation is in src/store.rs.\n"),
            ("scripts/verify","#!/bin/sh\ncat src/store.rs\n"),
            ("install.sh","cp src/store.rs \"$HOME/.config/demo/store.rs\"\n"),
            ("config/demo.env","DEMO_TOKEN=example\n"),
            ("tokens.txt","aaaéaaa\n"),
        ] {
            let destination=root.join(path);std::fs::create_dir_all(destination.parent().unwrap()).unwrap();std::fs::write(destination,body).unwrap();
        }
        let mut ledger=Ledger::open(&db).unwrap();
        let repository=identity::id("repository",&json!({"root":root.to_string_lossy()})).unwrap();
        let generation=ledger.begin(&repository,None).unwrap();
        let p=Capture::read(&root,None).unwrap().text_publication(&repository,generation).unwrap();
        let view=ledger.commit(generation,&p).unwrap();ledger.publish(&view,true).unwrap();
        Self {_dir:dir,root,db,ledger,view}
    }
    fn call(&self,name:&str,args:Value) -> Value {service::call(&self.ledger,name,&args).unwrap()}
}

#[test]
fn refactor_packet_surfaces_tests_plans_scripts_configuration_and_install_touchpoints() {
    let f=Fixture::new();
    let packet=f.call("brief",json!({"task":"rename the storage entry safely","target":"src/store.rs","duty":"refactor","max_bytes":1048576}));
    assert_eq!(packet["anchors"]["status"],"resolved");
    for (role,path) in [("impl","src/store.rs"),("test","tests/store.rs"),("plan","docs/plans/import.md"),("doc","docs/usage.md"),("script","scripts/verify"),("config","config/demo.env"),("install","install.sh")] {
        assert!(rows(&packet["slice"],role).iter().any(|row|row["path"] == path),"missing {role}: {path}");
    }
    for role in source::ARTIFACT_ROLES {assert!(packet["slice"][*role].is_array());}
    assert!(rows(&packet,"external_touchpoints").iter().any(|e|e["predicate"] == "READS_CONFIG" && e["to"] == "env:DEMO_TOKEN"));
    assert!(rows(&packet,"external_touchpoints").iter().any(|e|e["predicate"] == "INSTALLS_TO"));
    assert!(rows(&packet,"not_covered").iter().any(|e|e["area"] == "executed_test_coverage"));
    assert_eq!(packet["outcome"],"partial");
}

#[test]
fn reverse_test_relationships_retain_exact_byte_witnesses() {
    let f=Fixture::new();
    let result=f.call("neighbors",json!({"target":"src/store.rs","direction":"in","predicate":"TESTS"}));
    assert_eq!(result["resolution"]["status"],"resolved");
    let test=rows(&result,"items").iter().find(|e|e["from"] == "tests/store.rs").unwrap();
    assert_eq!(test["class"],"derived");
    let witness=&rows(test,"witnesses")[0];
    let bytes=std::fs::read(f.root.join(witness["path"].as_str().unwrap())).unwrap();
    let start=witness["start_byte"].as_u64().unwrap() as usize;let end=witness["end_byte"].as_u64().unwrap() as usize;
    assert_eq!(&bytes[start..end],b"../src/store.rs");
    assert_eq!(witness["digest"],identity::bytes_digest(&bytes));
    let proof=f.call("task_explain",json!({"index_id":result["index_id"],"link_id":test["id"]}));
    assert_eq!(proof["witnesses"][0]["text"],"../src/store.rs");
}

#[test]
fn literals_are_exhaustive_overlapping_and_pinned_to_retained_bytes() {
    let f=Fixture::new();let mut offset=0;let mut starts=vec![];
    loop {
        let result=f.call("occurrences",json!({"text":"aa","path":"tokens.txt","view":f.view,"offset":offset,"limit":1}));
        assert_eq!(result["page"]["total"],4);
        starts.extend(rows(&result,"items").iter().map(|v|v["start_byte"].as_u64().unwrap()));
        match result["page"]["next_offset"].as_u64(){Some(next)=>offset=next,None=>break}
    }
    assert_eq!(starts,vec![0,1,5,6]);
    std::fs::write(f.root.join("tokens.txt"),"changed live text").unwrap();
    let result=f.call("occurrences",json!({"text":"éa","path":"tokens.txt","view":f.view}));
    assert_eq!(result["page"]["total"],1);
    assert_eq!(result["items"][0]["start_byte"],3);assert_eq!(result["items"][0]["column"],4);
    assert_eq!(result["items"][0]["end_byte"],6);
}

#[test]
fn response_budget_pagination_does_not_skip_undelivered_matches() {
    let mut f=Fixture::new();std::fs::write(f.root.join("tokens.txt"),"needle ".repeat(1500)).unwrap();
    let repository=f.ledger.repository().unwrap();let g=f.ledger.begin(&repository,None).unwrap();
    let p=Capture::read(&f.root,None).unwrap().text_publication(&repository,g).unwrap();
    let view=f.ledger.commit(g,&p).unwrap();f.ledger.publish(&view,true).unwrap();
    let mut offset=0;let mut found=BTreeSet::new();let mut iterations=0;
    loop {
        let page=f.call("occurrences",json!({"text":"needle","path":"tokens.txt","view":view,"offset":offset,"limit":1000,"max_bytes":8192}));
        assert!(serde_json::to_vec(&page).unwrap().len()+1<=8192);
        assert!(!rows(&page,"items").is_empty(),"A useful page should fit: {page}");
        for item in rows(&page,"items") {assert!(found.insert(item["start_byte"].as_u64().unwrap()));}
        iterations+=1;assert!(iterations<200);
        match page["page"]["next_offset"].as_u64(){Some(next)=>{assert!(next>offset);offset=next;},None=>break}
    }
    assert_eq!(found.len(),1500);assert_eq!(found.last(),Some(&(1499*7)));
}

#[test]
fn text_and_task_queries_survive_graph_projection_loss() {
    let f=Fixture::new();assert!(!f.ledger.load(&f.view).unwrap().stale);
    let connection=rusqlite::Connection::open(&f.db).unwrap();connection.execute_batch("DELETE FROM graph; DELETE FROM search;").unwrap();
    assert!(f.ledger.load(&f.view).unwrap().stale,"external SQLite changes must invalidate verified-view cache");
    let result=f.call("occurrences",json!({"text":"save","path":"src/store.rs"}));assert!(result["page"]["total"].as_u64().unwrap()>0);
    let packet=f.call("brief",json!({"task":"debug storage","target":"src/store.rs","max_bytes":1048576}));
    assert_eq!(packet["anchors"]["status"],"resolved");
    assert!(task::readiness(&f.ledger).unwrap()["source_text"]["state"] == "retained");
    assert!(service::call(&f.ledger,"dependencies",&json!({})).is_ok());
}

#[test]
fn retractions_invalidate_task_indexes_and_annotate_old_packets() {
    let mut f=Fixture::new();
    let before=f.call("brief",json!({"task":"review storage","target":"src/store.rs","max_bytes":1048576}));
    let v=f.ledger.load(&f.view).unwrap();let assertion=text(&rows(&v.bundle,"assertions")[0],"id");
    f.ledger.retract(&assertion,"regression retraction").unwrap();
    let after=f.call("brief",json!({"task":"review storage","target":"src/store.rs","max_bytes":1048576}));
    assert_ne!(before["index_id"],after["index_id"]);
    assert!(!rows(&after,"neighborhood").iter().any(|e|rows(e,"assertion_ids").iter().any(|id|id == &json!(assertion))));
    let old=f.call("task_page",json!({"report_id":before["id"],"section":"/not_covered"}));assert_eq!(old["historical_packet"],true);
}

#[test]
fn unresolved_task_anchors_return_candidates_without_promoting_lexical_membership() {
    let f=Fixture::new();let packet=f.call("brief",json!({"task":"investigate nonexistent_capability","max_bytes":1048576}));
    assert_eq!(packet["anchors"]["status"],"unresolved");
    assert!(source::ARTIFACT_ROLES.iter().all(|r|rows(&packet["slice"],r).is_empty()));
    assert!(rows(&packet,"not_covered").iter().any(|v|v["area"] == "anchors"));
}

#[test]
fn retained_sections_are_pageable_and_repeated_reads_reuse_indexes() {
    let f=Fixture::new();let first=f.call("brief",json!({"task":"review storage","target":"src/store.rs","limit":1,"max_bytes":1048576}));
    let second=f.call("brief",json!({"task":"review storage","target":"src/store.rs","limit":1,"max_bytes":1048576}));
    assert_eq!(first["id"],second["id"]);assert_eq!(first["index_id"],second["index_id"]);
    let all=f.call("task_page",json!({"report_id":first["id"],"section":"/neighborhood","limit":1000,"max_bytes":1048576}));
    assert_eq!(all["page"]["total"],first["coverage"]["delivery"]["/neighborhood"]["total"]);
    assert!(all["page"]["total"].as_u64().unwrap()>=rows(&first,"neighborhood").len() as u64);
}

#[test]
fn neighbor_continuations_pin_the_index_and_label_later_retractions() {
    let mut f=Fixture::new();
    let first=f.call("neighbors",json!({"target":"src/store.rs","limit":1}));
    assert!(first["page"]["next_offset"].is_number());
    let view=f.ledger.load(&f.view).unwrap();let aid=text(&rows(&view.bundle,"assertions")[0],"id");
    f.ledger.retract(&aid,"after first page").unwrap();
    let next=f.call("page",first["continue_with"]["arguments"].clone());
    assert_eq!(next["index_id"],first["index_id"]);assert_eq!(next["historical_index"],true);
    assert_eq!(next["page"]["total"],first["page"]["total"]);
    assert_eq!(next["page"]["offset"],1);
}

#[test]
fn artifact_roles_cover_common_harness_files() {
    for (path,role) in [("AGENTS.md","doc"),("scripts/verify","script"),("install","install"),("docs/adr/0001.md","adr"),
        ("plans/refactor.md","plan"),("specs/store_test.rs","test"),("fixtures/data.json","fixture"),("Cargo.toml","build"),(".env","config"),("src/generated/a.rs","generated")] {
        assert_eq!(source::artifact_role(path),role,"{path}");
    }
}

#[test]
fn ask_routes_coding_duties_to_a_packet_instead_of_an_architecture_intent() {
    let f=Fixture::new();
    let packet=f.call("ask",json!({"question":"which tests cover src/store.rs","max_bytes":1048576}));
    assert_eq!(packet["kind"],"brief");assert_eq!(packet["duty"],"test");
    assert!(rows(&packet["slice"],"test").iter().any(|v|v["path"] == "tests/store.rs"));
}

#[test]
fn profiles_with_identical_source_snapshots_are_composed_and_stale_ones_are_disclosed() {
    let mut f=Fixture::new();
    std::fs::write(f.root.join("Cargo.toml"),"[package]\nname='context_demo'\nversion='0.1.0'\nedition='2021'\n").unwrap();
    std::fs::write(f.root.join("src/lib.rs"),"pub mod store;\n").unwrap();
    let repo=f.ledger.repository().unwrap();
    let g=f.ledger.begin(&repo,None).unwrap();
    let p=Capture::read(&f.root,None).unwrap().text_publication(&repo,g).unwrap();
    let text_view=f.ledger.commit(g,&p).unwrap();f.ledger.publish(&text_view,true).unwrap();
    let g=f.ledger.begin(&repo,None).unwrap();
    let p=briefcase::architecture::rust::produce(Capture::read(&f.root,None).unwrap(),&repo,g,
        &briefcase::architecture::rust::ScanOptions {analyzer:None,timeout:std::time::Duration::from_secs(30),features:vec![]}).unwrap();
    let rust_view=f.ledger.commit(g,&p).unwrap();f.ledger.publish(&rust_view,true).unwrap();
    let state=json!({"profile_views":[{"view_id":text_view},{"view_id":rust_view},{"view_id":f.view}]});
    let connection=rusqlite::Connection::open(&f.db).unwrap();
    connection.execute("INSERT INTO sync_state(singleton,key,view_id,body) VALUES(1,?,?,?)",rusqlite::params!["profiles-test",rust_view,state.to_string()]).unwrap();
    let packet=f.call("brief",json!({"task":"review src/store.rs","max_bytes":1048576}));
    assert_eq!(rows(&packet["coverage"],"analysis_profiles").len(),2);
    assert!(rows(&packet,"not_covered").iter().any(|v|v["area"] == "profile_snapshot"));
    assert_eq!(packet["coverage"]["completeness"]["neighborhood"],"partial");
}

#[test]
fn session_commit_joins_require_a_paired_git_receipt_and_reject_echoed_hashes() {
    let mut f=Fixture::new();
    let git=|args:&[&str]| {
        let output=Command::new("git").current_dir(&f.root).args(args).output().unwrap();
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));output
    };
    git(&["init","-q"]);git(&["add","."]);
    let committed=git(&["-c","user.name=Test","-c","user.email=test@example.test","commit","-m","fix"]);
    let receipt=String::from_utf8(committed.stdout).unwrap();
    briefcase::architecture::git_history::update(&f.ledger,&f.root,
        std::time::Instant::now()+std::time::Duration::from_secs(30)).unwrap();
    let mut imported=vec![];
    for (id,command) in [("actual","git commit -m fix"),("echoed","echo 'git commit -m fake'")] {
        let records=vec![
            json!({"type":"session","version":3,"id":id,"cwd":f.root,"timestamp":"2026-01-01T00:00:00Z"}),
            json!({"type":"message","message":{"role":"user","content":"fix storage"}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"r","name":"read","arguments":{"path":"src/store.rs"}}]}}),
            json!({"type":"message","message":{"role":"toolResult","toolCallId":"r","isError":false,"content":[{"type":"text","text":"source"}]}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"c","name":"bash","arguments":{"command":command}}]}}),
            json!({"type":"message","message":{"role":"toolResult","toolCallId":"c","isError":false,"content":[{"type":"text","text":receipt}]}}),
        ];
        let raw=records.iter().map(|v|serde_json::to_string(v).unwrap()).collect::<Vec<_>>().join("\n")+"\n";
        imported.push(briefcase::architecture::session::import(&mut f.ledger,raw.as_bytes(),
            &briefcase::architecture::session::ImportOptions {profile:"pi-v1".into(),repository_root:f.root.to_string_lossy().into_owned(),
                label:None,repository_id:None,source_name:"inline.jsonl".into(),retention:briefcase::architecture::session::Retention::Full}).unwrap());
    }
    let packet=f.call("brief",json!({"task":"review storage","target":"src/store.rs","max_bytes":1048576,"limit":1000}));
    let links:Vec<_>=rows(&packet,"joins").iter().filter(|l|l["predicate"] == "SESSION_PRODUCED_COMMIT").collect();
    assert_eq!(links.len(),1);assert_eq!(links[0]["from"],imported[0]["view_id"]);
    assert!(rows(&packet,"not_covered").iter().any(|g|g["area"] == "session_commit_join"));
    assert_eq!(packet["coverage"]["completeness"]["sessions"],"partial");
}

#[test]
fn tracked_files_override_gitignore_and_binary_omissions_are_explicit() {
    let dir=tempfile::tempdir().unwrap();let root=dir.path();
    assert!(Command::new("git").args(["init","-q"]).arg(root).status().unwrap().success());
    std::fs::create_dir_all(root.join(".agents")).unwrap();
    std::fs::write(root.join(".gitignore"),".agents/\n*.odd\n").unwrap();
    std::fs::write(root.join(".agents/instructions"),"read all tracked files").unwrap();
    std::fs::write(root.join("design.odd"),"a tracked new file format").unwrap();
    std::fs::write(root.join("binary.odd"),[0,255,1]).unwrap();
    assert!(Command::new("git").current_dir(root).args(["add","-f",".agents/instructions","design.odd","binary.odd",".gitignore"]).status().unwrap().success());
    let capture=Capture::read(root,None).unwrap();
    assert!(capture.files.contains_key(".agents/instructions"));assert!(capture.files.contains_key("design.odd"));
    assert!(!capture.files.contains_key("binary.odd"));assert!(capture.complete);
    assert_eq!(capture.inventory["tracked_inventory_available"],true);
    assert_eq!(capture.inventory["unretained_tracked_paths"],json!(["binary.odd"]));
    assert_eq!(capture.inventory["tracked_text_complete"],false);
}

#[cfg(unix)]
#[test]
fn tracked_parent_symlinks_never_escape_the_capture() {
    let dir=tempfile::tempdir().unwrap();let root=dir.path().join("repo");std::fs::create_dir(&root).unwrap();
    assert!(Command::new("git").args(["init","-q"]).arg(&root).status().unwrap().success());
    std::fs::create_dir(root.join("docs")).unwrap();std::fs::write(root.join("docs/plan"),"original").unwrap();
    std::fs::write(root.join("README"),"retained").unwrap();
    assert!(Command::new("git").current_dir(&root).args(["add","docs/plan","README"]).status().unwrap().success());
    std::fs::remove_dir_all(root.join("docs")).unwrap();let external=dir.path().join("external");std::fs::create_dir(&external).unwrap();std::fs::write(external.join("plan"),"outside").unwrap();
    std::os::unix::fs::symlink(&external,root.join("docs")).unwrap();
    let capture=Capture::read(&root,None).unwrap();assert!(!capture.files.contains_key("docs/plan"));
    assert!(capture.exclusions.iter().any(|e|e.contains("docs/plan")));
}
