//! P0/P1 acceptance tests with no historical fixture archive or language toolchain.
use briefcase::architecture::{capture::Capture, contracts::rows, identity, ledger::Ledger, service};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Fixture { _temp:tempfile::TempDir, root:PathBuf, ledger:Ledger }
impl Fixture {
    fn new()->Self {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        for index in 0..6 {
            std::fs::write(root.join(format!("src/part{index}.rs")),format!("// p0p1token {}\n", "雪é".repeat(2000))).unwrap();
        }
        assert!(std::process::Command::new("git").args(["init","-q"]).current_dir(&root).status().unwrap().success());
        let mut ledger=Ledger::open(&temp.path().join("evidence.db")).unwrap();
        publish(&mut ledger,&root);
        Self{_temp:temp,root,ledger}
    }
    fn call(&self,name:&str,args:Value)->Value {
        service::call_with_context(&self.ledger,name,&args,Some(&self.root),false).unwrap()
    }
}
fn publish(ledger:&mut Ledger,root:&Path) {
    let repo=identity::id("repository",&json!({"root":root.to_string_lossy()})).unwrap();
    let generation=ledger.begin(&repo,None).unwrap();
    let prepared=Capture::read(root,None).unwrap().text_publication(&repo,generation).unwrap();
    let view=ledger.commit(generation,&prepared).unwrap();ledger.publish(&view,true).unwrap();
}

#[test]
fn unselected_candidates_have_pinned_source_and_budgeted_bytes_never_skip() {
    let f=Fixture::new();
    let packet=f.call("agent_context",json!({"task":"p0p1token","target":"src/part0.rs","max_files":1}));
    let candidates=f.call("page",json!({"report_id":packet["case_id"],"section":"/candidates","max_bytes":1048576}));
    let candidate=rows(&candidates,"items").iter().find(|c|c["selected"]==false).unwrap();
    assert!(candidate["source_ref"].as_str().unwrap().starts_with("/candidates/"));
    let mut offset=0;let mut reconstructed:Vec<u8>=Vec::new();
    loop {
        let source=f.call("explain",json!({"report_id":packet["case_id"],"ref":candidate["source_ref"],
            "source_offset":offset,"max_bytes":2600}));
        assert!(serde_json::to_vec(&source).unwrap().len()+1<=2600);
        assert_eq!(source["entry"]["path"],candidate["path"]);
        let text=source["text"].as_str().unwrap();assert!(!text.is_empty());
        reconstructed.extend(text.as_bytes());
        match source["next_source_offset"].as_u64() {
            Some(next)=>{assert_eq!(next,offset+text.len() as u64);offset=next;},None=>break,
        }
    }
    assert_eq!(reconstructed,std::fs::read(f.root.join(candidate["path"].as_str().unwrap())).unwrap());
}

#[test]
fn changed_new_and_deleted_paths_outside_the_slice_are_retained_and_pageable() {
    let f=Fixture::new();
    let packet=f.call("agent_context",json!({"task":"p0p1token","target":"src/part0.rs","max_files":1}));
    let before=f.call("worktree",json!({"case_id":packet["case_id"]}));
    assert_eq!(before["alignment"],"matches");
    std::fs::write(f.root.join("src/part1.rs"),"// changed\n").unwrap();
    std::fs::remove_file(f.root.join("src/part2.rs")).unwrap();
    std::fs::write(f.root.join("src/new.rs"),"// new consumer\n").unwrap();
    let after=f.call("worktree",json!({"case_id":packet["case_id"],"limit":1}));
    assert_eq!(after["alignment"],"changed");assert_eq!(after["changed_paths"],3);
    assert_eq!(after["outside_initial_slice"],3);assert_ne!(before["tree_digest"],after["tree_digest"]);
    let all=f.call("page",json!({"report_id":after["report_id"],"section":"/items"}));
    assert_eq!(rows(&all,"items").len(),3);
    for state in ["added","modified","deleted"] {assert!(rows(&all,"items").iter().any(|r|r["state"]==state));}
    assert!(service::call(&f.ledger,"worktree",&json!({})).is_err(),"No implicit live root");
}

#[test]
fn byte_reduction_rewinds_source_continuation_to_the_first_undelivered_byte() {
    let value=json!({"kind":"explain","report_id":"case:a","ref":"/candidates/99","source_offset":100,
        "next_source_offset":4100,"omitted_bytes":900,"text":"é".repeat(2000),
        "entry":{"bytes":5000},"continue_with":{"operation":"explain","arguments":{"source_offset":4100}}});
    let result=briefcase::architecture::response::bounded(value,1200).unwrap();
    let delivered=result["text"].as_str().unwrap().len() as u64;
    assert!(delivered>0 && delivered<4000);
    assert_eq!(result["next_source_offset"],100+delivered);
    assert_eq!(result["continue_with"]["arguments"]["source_offset"],100+delivered);
    assert_eq!(result["omitted_bytes"],5000-100-delivered);
}

#[test]
fn revision_changes_are_visible_even_when_source_bytes_are_identical() {
    let f=Fixture::new();
    let packet=f.call("agent_context",json!({"task":"p0p1token","target":"src/part0.rs","max_files":1}));
    assert!(std::process::Command::new("git").args(["add","."]).current_dir(&f.root).status().unwrap().success());
    assert!(std::process::Command::new("git").args(["-c","user.name=Fixture","-c","user.email=fixture@local.invalid",
        "-c","commit.gpgsign=false","-c","core.hooksPath=/dev/null","commit","-qm","Capture revision"])
        .current_dir(&f.root).status().unwrap().success());
    let check=f.call("worktree",json!({"case_id":packet["case_id"]}));
    assert_eq!(check["alignment"],"changed");assert_eq!(check["changed_paths"],0);
    assert_eq!(check["head_alignment"]["changed"],true);
}

#[test]
fn oversized_status_keeps_the_pins_that_freshness_checks_compare() {
    let value=json!({"operation":"status","current_view":"view:9","pending_or_interrupted":false,"retracted_count":0,
        "ledger_watermark":4,"pinned_views":["view:9"],"inventory":{"rows":"x".repeat(4000)}});
    let result=briefcase::architecture::response::bounded(value,1200).unwrap();
    assert_eq!(result["truncated"],true);
    assert_eq!(result["current_view"],"view:9");assert_eq!(result["pending_or_interrupted"],false);
    assert_eq!(result["retracted_count"],0);assert_eq!(result["ledger_watermark"],4);
}

#[test]
fn candidate_pages_progress_when_one_full_row_exceeds_the_budget() {
    let row=|n:usize|json!({"path":format!("src/f{n}.rs"),"rank":n,"role":"impl","score":1,"selected":false,
        "source_ref":format!("/candidates/{n}"),"signals":[{"detail":"s".repeat(3000)}],"spans":[{"detail":"p".repeat(1000)}]});
    let value=json!({"kind":"page","report_id":"task-case:a","section":"/candidates",
        "page":{"offset":0,"returned":3,"total":3,"remaining":0,"next_offset":null},"items":[row(0),row(1),row(2)]});
    let result=briefcase::architecture::response::bounded(value,2000).unwrap();
    assert!(serde_json::to_vec(&result).unwrap().len()+1<=2000);
    let items=rows(&result,"items");assert!(!items.is_empty(),"{result}");
    assert_eq!(items[0]["source_ref"],"/candidates/0");assert!(items[0].get("signals").is_none());
    assert_eq!(result["page"]["next_offset"],json!((items.len()<3).then_some(items.len())));
}

#[test]
fn small_pages_and_source_reads_compact_freshness_before_rows_or_bytes() {
    let freshness=json!({"views":[{"view_id":"view:1","scope":"x".repeat(4000),"projection_state":"consistent"}],
        "pending_or_interrupted":false,"capture_state":"complete"});
    let page=json!({"kind":"page","report_id":"task-case:a","view_id":"view:1","section":"/not_covered","freshness":freshness,
        "page":{"offset":0,"returned":1,"total":1,"remaining":0,"next_offset":null},"items":[{"area":"a","detail":"d".repeat(1500)}]});
    let result=briefcase::architecture::response::bounded(page,3000).unwrap();
    assert_eq!(rows(&result,"items").len(),1,"{result}");
    assert_eq!(result["freshness"]["presentation"],"summary_by_budget");
    assert_eq!(result["freshness"]["pending_or_interrupted"],false);assert_eq!(result["view_id"],"view:1");
    let source=json!({"report_id":"task-case:a","ref":"/candidates/0","view_id":"view:1","freshness":freshness,
        "entry":{"path":"src/a.rs","bytes":2000},"source_offset":0,"next_source_offset":null,"omitted_bytes":0,"text":"t".repeat(2000)});
    let result=briefcase::architecture::response::bounded(source,3000).unwrap();
    assert_eq!(result["text"].as_str().unwrap().len(),2000,"{result}");
}
