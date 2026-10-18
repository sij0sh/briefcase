//! P2 acceptance tests: resumable discovery bounds and lazy verified source access.
use briefcase::architecture::{capture::Capture, contracts::rows, identity, ledger::Ledger, service};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Fixture { temp:tempfile::TempDir, root:PathBuf, ledger:Ledger }
impl Fixture {
    fn new(files:&[(String,String)])->Self {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");
        for (path,content) in files {
            let path=root.join(path);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,content).unwrap();
        }
        assert!(std::process::Command::new("git").args(["init","-q"]).current_dir(&root).status().unwrap().success());
        let mut ledger=Ledger::open(&temp.path().join("evidence.db")).unwrap();
        publish(&mut ledger,&root);
        Self{temp,root,ledger}
    }
    fn call(&self,name:&str,args:Value)->Value {
        service::call_with_context(&self.ledger,name,&args,Some(&self.root),false).unwrap()
    }
    fn lexical(&self,case:&Value)->Value {
        self.call("explain",json!({"report_id":case,"ref":"/coverage/retrieval/lexical_discovery","max_bytes":1048576}))["entry"].clone()
    }
    fn candidates(&self,case:&Value)->Vec<String> {
        let page=self.call("page",json!({"report_id":case,"section":"/candidates","limit":1000,"max_bytes":1048576}));
        rows(&page,"items").iter().map(|c|c["path"].as_str().unwrap().to_owned()).collect()
    }
}
fn publish(ledger:&mut Ledger,root:&Path) {
    let repo=identity::id("repository",&json!({"root":root.to_string_lossy()})).unwrap();
    let generation=ledger.begin(&repo,None).unwrap();
    let prepared=Capture::read(root,None).unwrap().text_publication(&repo,generation).unwrap();
    let view=ledger.commit(generation,&prepared).unwrap();ledger.publish(&view,true).unwrap();
}
fn file(path:String,content:String)->(String,String) {(path,content)}

#[test]
fn duplicate_chunks_of_distractors_cannot_exhaust_lexical_discovery() {
    // 90 distractors × ~14 chunks each exceed a 1,000-chunk pool, and each
    // chunk outranks the single relevant mention.
    let noise="alpha ".repeat(16)+"\n";
    let mut files:Vec<_>=(0..90).map(|i|file(format!("src/noise{i:02}.rs"),noise.repeat(220))).collect();
    files.push(file("src/zz_target.rs".into(),"// the alpha handler lives here\npub fn target() {}\n".into()));
    let f=Fixture::new(&files);
    let case=f.call("agent_context",json!({"task":"alpha","max_files":4}))["case_id"].clone();
    let lexical=f.lexical(&case);
    assert_eq!(lexical["candidate_unit"],"file");assert_eq!(lexical["matched_files"],91);
    assert_eq!(lexical["candidate_limit_reached"],false);
    assert!(f.candidates(&case).contains(&"src/zz_target.rs".to_owned()),"the relevant file stays a candidate");
}

#[test]
fn files_beyond_the_candidate_bound_remain_pageable_in_rank_order() {
    let files:Vec<_>=(0..1030).map(|i|file(format!("docs/note{i:04}.md"),format!("beta note {i}\n"))).collect();
    let f=Fixture::new(&files);
    let case=f.call("agent_context",json!({"task":"beta","max_files":4}))["case_id"].clone();
    let lexical=f.lexical(&case);
    assert_eq!(lexical["matched_files"],1030);assert_eq!(lexical["candidate_limit_reached"],true);
    assert_eq!(lexical["omitted_candidates"],30);
    let rest=f.call("page",json!({"report_id":lexical["report_id"],
        "cursor":lexical["continue_with"]["arguments"]["cursor"],"limit":1000,"max_bytes":1048576}));
    assert_eq!(rows(&rest,"items").len(),30);assert_eq!(rest["page"]["offset"],1000);
    let mut seen:std::collections::BTreeSet<String>=f.candidates(&case).into_iter().collect();
    seen.extend(rows(&rest,"items").iter().map(|r|r["path"].as_str().unwrap().to_owned()));
    assert_eq!(seen.len(),1030,"every matched file is a candidate or an explicit continuation");
}

#[test]
fn explicit_identifiers_survive_long_tasks_and_omitted_terms_are_reported() {
    let filler:Vec<_>=(0..40).map(|i|format!("word{i:02}x")).collect();
    let task=format!("{} then repair `zz_late_identifier` and parseConfigValue",filler.join(" "));
    let f=Fixture::new(&[file("src/late.rs".into(),"pub fn zz_late_identifier() {}\n".into()),
        file("src/config.rs".into(),"pub fn parse_config_value() {}\n".into())]);
    let case=f.call("agent_context",json!({"task":task,"max_files":4}))["case_id"].clone();
    let lexical=f.lexical(&case);
    let terms:Vec<_>=rows(&lexical,"query_terms").iter().map(|t|t.as_str().unwrap().to_owned()).collect();
    assert_eq!(terms[..2],["zz_late_identifier".to_owned(),"parseconfigvalue".to_owned()]);
    assert_eq!(terms.len(),32);assert!(!rows(&lexical,"omitted_terms").is_empty());
    let candidates=f.candidates(&case);
    assert!(candidates.contains(&"src/late.rs".to_owned()));assert!(candidates.contains(&"src/config.rs".to_owned()));
    let gaps=f.call("page",json!({"report_id":case,"section":"/not_covered","limit":100,"max_bytes":1048576}));
    assert!(rows(&gaps,"items").iter().any(|g|g["area"]=="candidate_selection"));
}

#[test]
fn occurrence_pages_read_only_their_window_after_the_count_inventory() {
    let mut files:Vec<_>=(0..40).map(|i|file(format!("src/m{i:02}.rs"),format!("// gamma {i}\nfn gamma{i}() {{}}\n"))).collect();
    files.push(file("docs/other.md".into(),"gamma outside\n".into()));
    let f=Fixture::new(&files);
    let first=f.call("occurrences",json!({"text":"gamma","path":"src","limit":10,"max_bytes":1048576}));
    assert_eq!(first["page"]["total"],80);assert_eq!(first["work"]["count_inventory"],"built");
    // A path filter never reads out-of-scope bytes: 40 in-scope files are
    // verified for counts, then the 5 files overlapping the window are read.
    assert_eq!(first["work"]["source"]["files_in_snapshot"],41);assert_eq!(first["work"]["source"]["files_loaded"],45);
    let mut paged=rows(&first,"items").to_vec();let mut next=first["page"]["next_offset"].as_u64();
    while let Some(offset)=next {
        let page=f.call("occurrences",json!({"text":"gamma","path":"src","offset":offset,"limit":10,"max_bytes":1048576}));
        assert_eq!(page["work"]["count_inventory"],"hit");
        assert!(page["work"]["source"]["files_loaded"].as_u64().unwrap()<=6,"{}",page["work"]);
        paged.extend(rows(&page,"items").iter().cloned());next=page["page"]["next_offset"].as_u64();
    }
    let whole=f.call("occurrences",json!({"text":"gamma","path":"src","limit":1000,"max_bytes":1048576}));
    assert_eq!(paged,rows(&whole,"items").to_vec(),"paged enumeration equals one exhaustive page");
}

#[test]
fn lazy_reads_still_reject_corrupted_retained_bytes() {
    let f=Fixture::new(&[file("src/a.rs".into(),"delta one\n".into()),file("src/b.rs".into(),"delta two\n".into())]);
    let connection=rusqlite::Connection::open(f.temp.path().join("evidence.db")).unwrap();
    let artifact=rows(&f.call("occurrences",json!({"text":"delta","path":"src/b.rs"})),"items")[0]["artifact_id"].as_str().unwrap().to_owned();
    connection.execute("UPDATE payloads SET bytes=? WHERE artifact_id=?",rusqlite::params![b"delta tw0\n".to_vec(),artifact]).unwrap();
    let error=service::call_with_context(&f.ledger,"occurrences",&json!({"text":"delta","path":"src/b.rs","offset":0,"limit":5}),Some(&f.root),false);
    // The count inventory for src/b.rs is cached; the page still re-reads and verifies.
    assert!(error.is_err_and(|e|e.to_string().contains("DIGEST")));
    let unaffected=f.call("occurrences",json!({"text":"delta","path":"src/a.rs"}));
    assert_eq!(unaffected["page"]["total"],1);
}

#[test]
fn monorepo_projects_left_unselected_get_an_anchored_follow_up() {
    let mut files:Vec<_>=(0..12).map(|i|file(format!("pkg/big/src/f{i:02}.rs"),format!("// epsilon epsilon epsilon {i}\npub fn epsilon{i}() {{}}\n"))).collect();
    files.push(file("pkg/big/Cargo.toml".into(),"[package]\nname=\"big\"\n".into()));
    files.push(file("pkg/small/package.json".into(),"{\"name\":\"small\"}\n".into()));
    files.push(file("pkg/small/index.js".into(),"// epsilon consumer\n".into()));
    let f=Fixture::new(&files);
    let case=f.call("agent_context",json!({"task":"epsilon","max_files":2}))["case_id"].clone();
    let scope=f.call("explain",json!({"report_id":case,"ref":"/coverage/retrieval/project_scope","max_bytes":1048576}))["entry"].clone();
    assert_eq!(scope["projects"],2);assert_eq!(scope["state"],"unselected_candidate_projects");
    let small=rows(&scope,"candidate_projects").iter().find(|p|p["root"]=="pkg/small").unwrap().clone();
    assert_eq!(small["selected"],0);assert_eq!(small["manifests"],json!(["package.json"]));
    let follow=&rows(&scope,"continue_with")[0];assert_eq!(follow["arguments"]["target"],"pkg/small/");
    let scoped=f.call(follow["operation"].as_str().unwrap(),follow["arguments"].clone());
    assert!(f.candidates(&scoped["case_id"]).contains(&"pkg/small/index.js".to_owned()));
}
