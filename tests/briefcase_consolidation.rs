//! Regression coverage for the consolidated product, independent of gold data.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use serde_json::{json,Value};
use briefcase::architecture::{adoption,contracts::{rows,text},external,ledger::Ledger,mcp,service,task};

#[test]
fn first_read_retains_text_and_every_question_uses_a_brief(){
    let f=Fixture::new();
    let explained=f.call("ask",json!({"question":"how does src/lib.rs work","max_bytes":1048576}));
    assert_eq!(explained["kind"],"brief");assert_eq!(explained["duty"],"explain");
    assert_eq!(explained["anchors"]["status"],"resolved");
    assert!(!rows(&explained,"not_covered").iter().any(|g|["executed_test_coverage","plan_outcomes","external_installations"].contains(&g["area"].as_str().unwrap_or(""))));
    let same=service::open_context(&f.db,&f.root).unwrap();assert_eq!(same.resolve("latest").unwrap(),f.view);
    assert_eq!(same.status().unwrap()["scans"].as_array().unwrap().len(),1);
    for duty in task::Duty::names(){let packet=f.call("brief",json!({"task":"src/lib.rs","duty":duty,"max_bytes":1048576}));assert_eq!(packet["duty"],duty);}
}
#[test]
fn ranked_aliases_share_items_and_reports_are_pinned(){
    let mut f=Fixture::new();let args=json!({"text":"demo-tool","history":false,"limit":1,"max_bytes":1048576});
    let first=f.call("search",args.clone());
    for alias in ["recall","retrieve"]{let other=f.call(alias,args.clone());assert_eq!(first["items"],other["items"]);}
    let all=f.call("page",json!({"report_id":first["report_id"],"limit":1000,"max_bytes":1048576}));
    assert!(!rows(&all,"items").is_empty());
    std::fs::write(f.root.join("src/lib.rs"),"pub fn replaced(){}\n").unwrap();f.recapture();
    let pinned=f.call("page",json!({"report_id":first["report_id"],"limit":1000,"max_bytes":1048576}));
    assert_eq!(all["items"],pinned["items"]);
    let proof=f.call("explain",json!({"report_id":first["report_id"],"ref":"/items/0","max_bytes":1048576}));
    assert!(proof["text"].as_str().unwrap().contains("demo-tool"));
    let evidence=f.call("explain",json!({"report_id":first["report_id"],"ref":"/items/0/evidence_matches/0","max_bytes":1048576}));
    assert_eq!(evidence["support"][0]["assertion"]["id"],first["items"][0]["evidence_matches"][0]["assertion_id"]);
    assert!(service::call(&f.ledger,"page",&json!({"report_id":first["report_id"],"cursor":json!({"report_id":"wrong","section":"/items","offset":0}).to_string()})).is_err());
}
#[test]
fn external_inspection_establishes_only_exact_byte_matches(){
    let f=Fixture::new();let installed=f.dir.path().join("installed-tool");std::fs::copy(f.root.join("src/lib.rs"),&installed).unwrap();
    let observation=external::inspect(&f.ledger,&f.view,&[installed.clone()]).unwrap();
    assert_eq!(observation["items"][0]["identity"],"exact_retained_bytes");
    let neighbors=f.call("neighbors",json!({"target":"src/lib.rs","predicate":"INSTALLED_FROM"}));
    let edge=rows(&neighbors,"items").first().unwrap();assert_eq!(edge["class"],"derived");
    let proof=f.call("explain",json!({"report_id":neighbors["report_id"],"ref":edge["id"],"max_bytes":1048576}));
    assert_eq!(proof["observation_reports"][0]["report_id"],observation["report_id"]);
    std::fs::write(&installed,"different installed bytes").unwrap();external::inspect(&f.ledger,&f.view,&[installed]).unwrap();
    let after=f.call("neighbors",json!({"target":"src/lib.rs","predicate":"INSTALLED_FROM"}));assert!(rows(&after,"items").is_empty());
}
#[test]
fn mcp_first_read_uses_the_same_degraded_path(){
    let f=Fixture::new();let db=f.dir.path().join("mcp.db");
    let request=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"briefcase_query","arguments":{"operation":"brief","arguments":{"task":"explain src/lib.rs","max_bytes":1048576}}}}).to_string()+"\n";
    let mut output=Vec::new();mcp::serve_path_for_root(mcp::Surface::Gateway,&db,Some(&f.root),std::io::Cursor::new(request),&mut output).unwrap();
    let response:Value=serde_json::from_slice(&output).unwrap();assert_eq!(response["result"]["structuredContent"]["kind"],"brief");
    assert!(Ledger::open_existing(&db).unwrap().resolve("latest").is_ok());
    let invalid_db=f.dir.path().join("invalid.db");
    assert!(service::call_path(&invalid_db,Some(&f.root),"brief",&json!({"task":"x","duty":"invented"}),false).is_err());assert!(!invalid_db.exists());
}
#[test]
fn lifecycle_outputs_never_block_or_continue_the_agent(){
    let f=Fixture::new();let prompt=json!({"session_id":"demo-session","cwd":f.root,"prompt":"review src/lib.rs"});
    let out=adoption::event(&f.db,&f.root,"prompt",&prompt).unwrap();
    assert_eq!(out["hookSpecificOutput"]["hookEventName"],"UserPromptSubmit");assert!(out.get("decision").is_none());
    let finished=adoption::event(&f.db,&f.root,"complete",&prompt).unwrap();
    assert!(finished["systemMessage"].as_str().unwrap().contains("worktree check"));
    assert!(finished.get("decision").is_none());assert!(finished.get("continue").is_none());assert!(finished.get("hookSpecificOutput").is_none());
    let sql=rusqlite::Connection::open(&f.db).unwrap();
    let brief_count:i64=sql.query_row("SELECT count(*) FROM reports WHERE json_extract(body,'$.kind')='brief'",[],|r|r.get(0)).unwrap();
    assert_eq!(brief_count,0,"Hooks retain cases and comparison reports, never brief projections");
    let raw:String=sql.query_row("SELECT body FROM reports WHERE json_extract(body,'$.kind')='case_worktree_check'",[],|r|r.get(0)).unwrap();
    let check:Value=serde_json::from_str(&raw).unwrap();
    assert!(check["prompt_case_id"].as_str().unwrap().starts_with("task-case:"));
    assert!(check["case_id"].as_str().unwrap().starts_with("task-case:"));
    assert_eq!(check["task_cycle"]["dimension_comparison"],"not_requested");
    assert_eq!(check["prompt_case_id"],check["case_id"]);
}
#[test]
fn foreign_sessions_join_only_recorded_external_paths_or_literal_invocations(){
    let mut f=Fixture::new();let foreign=f.dir.path().join("other");std::fs::create_dir_all(&foreign).unwrap();
    let events=vec![json!({"type":"session","version":3,"id":"foreign","cwd":foreign,"timestamp":"2026-01-01T00:00:00Z"}),
        json!({"type":"message","message":{"role":"user","content":"check installed tool"}}),
        json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"cmd","name":"bash","arguments":{"command":"/opt/demo-tool --help"}}]}}),
        json!({"type":"message","message":{"role":"toolResult","toolCallId":"cmd","isError":false,"content":[{"type":"text","text":"help"}]}})];
    let raw=events.iter().map(|v|serde_json::to_string(v).unwrap()).collect::<Vec<_>>().join("\n")+"\n";
    let imported=briefcase::architecture::session::import(&mut f.ledger,raw.as_bytes(),&briefcase::architecture::session::ImportOptions{
        profile:"pi-v1".into(),repository_root:foreign.to_string_lossy().into_owned(),repository_id:None,label:None,source_name:"foreign.jsonl".into(),retention:briefcase::architecture::session::Retention::Full}).unwrap();
    let packet=f.call("brief",json!({"task":"debug src/lib.rs","target":"src/lib.rs","max_bytes":1048576}));
    assert!(rows(&packet,"sessions").iter().any(|s|s["view_id"]==imported["view_id"]&&s["same_repository"]==false));
    assert!(rows(&packet,"joins").iter().any(|e|e["predicate"]=="SESSION_INVOKED_TOUCHPOINT"));
    assert!(!rows(&packet,"joins").iter().any(|e|e["predicate"]=="SESSION_PRODUCED_COMMIT"));
}

#[test]
fn foreign_relative_paths_and_command_prose_do_not_join_local_files(){
    let mut f=Fixture::new();let foreign=f.dir.path().join("foreign-root");std::fs::create_dir_all(&foreign).unwrap();
    let mut excluded=vec![];
    for (id,tool,args) in [("relative","read",json!({"path":"src/lib.rs"})),
        ("prose","bash",json!({"command":"echo /opt/demo-tool"})),
        ("indirect","bash",json!({"command":"/opt/demo-tool --help && echo done"}))] {
        let events=vec![json!({"type":"session","version":3,"id":id,"cwd":foreign,"timestamp":"2026-01-01T00:00:00Z"}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"call","name":tool,"arguments":args}]}}),
            json!({"type":"message","message":{"role":"toolResult","toolCallId":"call","isError":false,"content":[{"type":"text","text":"recorded"}]}})];
        let raw=events.iter().map(|v|v.to_string()).collect::<Vec<_>>().join("\n")+"\n";
        let imported=briefcase::architecture::session::import(&mut f.ledger,raw.as_bytes(),&briefcase::architecture::session::ImportOptions{
            profile:"pi-v1".into(),repository_root:foreign.to_string_lossy().into_owned(),repository_id:None,label:None,source_name:format!("{id}.jsonl"),
            retention:briefcase::architecture::session::Retention::Full}).unwrap();
        excluded.push(imported["view_id"].clone());
    }
    let packet=f.call("brief",json!({"task":"debug src/lib.rs","target":"src/lib.rs","limit":1000,"max_bytes":1048576}));
    assert!(!rows(&packet,"sessions").iter().any(|s|excluded.contains(&s["view_id"])));
    assert!(!rows(&packet,"joins").iter().any(|e|excluded.contains(&e["from"])));
}

#[test]
fn search_filters_retained_paths_and_keeps_a_contiguous_budgeted_prefix(){
    let mut f=Fixture::new();
    for i in 0..12 {std::fs::write(f.root.join(format!("src/needle-{i:02}.rs")),format!("// needle {}\npub fn retained_{i}(){{}}\n","é".repeat(600))).unwrap();}
    f.recapture();
    let all=f.call("search",json!({"text":"needle","component":"src","history":false,"limit":1000,"max_bytes":1048576}));
    let retained=rows(&all,"items");assert_eq!(retained.len(),12);
    assert!(retained.iter().all(|i|text(i,"path").starts_with("src/")));
    let small=f.call("search",json!({"text":"needle","component":"src","history":false,"limit":1000,"max_bytes":5500}));
    let delivered=rows(&small,"items");
    assert_eq!(delivered.iter().map(|i|text(i,"artifact_id")).collect::<Vec<_>>(),retained[..delivered.len()].iter().map(|i|text(i,"artifact_id")).collect::<Vec<_>>());
    let next=f.call("page",json!({"report_id":small["report_id"],"offset":delivered.len(),"limit":1000,"max_bytes":1048576}));
    assert_eq!(rows(&next,"items"),&retained[delivered.len()..]);
    let single=f.call("search",json!({"text":"needle","component":"src/needle-00.rs","history":false,"max_bytes":1048576}));
    assert_eq!(rows(&single,"items").len(),1);
    assert_eq!(single["reading_list"][0]["path"],"src/needle-00.rs");
    assert!(service::call(&f.ledger,"search",&json!({"text":"needle","include_structure":true})).is_err());
}
#[test]
fn invalid_first_reads_do_not_create_a_database(){
    let f=Fixture::new();let db=f.dir.path().join("rejected.db");
    for task in ["   ".to_owned(),"é".repeat(9000)] {
        assert!(service::call_path(&db,Some(&f.root),"brief",&json!({"task":task}),false).is_err());
        assert!(!db.exists());
    }
    assert!(service::call_path(&db,Some(&f.root),"search",&json!({"text":" "}),false).is_err());
    assert!(!db.exists());
    let whitespace=f.call("occurrences",json!({"text":"\n","max_bytes":1048576}));
    assert!(whitespace["page"]["total"].as_u64().unwrap()>0,"literal whitespace remains searchable");
}
#[test]
fn live_freshness_distinguishes_commits_from_dirty_files(){
    let mut f=Fixture::new();
    let git_root=f.root.clone();
    let git=|args:&[&str]| {
        let result=std::process::Command::new("git").current_dir(&git_root)
            .args(["-c","user.name=Context Test","-c","user.email=context@example.invalid"])
            .args(args).output().unwrap();
        assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    };
    git(&["init","-q"]);git(&["add","."]);git(&["commit","-qm","captured"]);
    f.recapture();
    let initial=service::call_with_context(&f.ledger,"brief",&json!({"task":"src/lib.rs","max_bytes":1048576}),Some(&f.root),false).unwrap();
    assert_eq!(initial["freshness"]["git"]["state"],"same_head");
    std::fs::write(f.root.join("README.md"),"Changed after capture.\n").unwrap();
    git(&["add","README.md"]);git(&["commit","-qm","new head"]);
    std::fs::write(f.root.join("src/lib.rs"),"pub fn dirty(){}\n").unwrap();
    let live=service::call_with_context(&f.ledger,"brief",&json!({"task":"src/lib.rs","max_bytes":1048576}),Some(&f.root),false).unwrap();
    assert_eq!(live["freshness"]["git"]["commits_behind"],1);
    assert_eq!(live["freshness"]["git"]["changed_paths"],json!(["README.md"]));
    assert_eq!(live["freshness"]["dirty_hit_paths"]["paths"],json!(["src/lib.rs"]));
}
