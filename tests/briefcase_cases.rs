//! Retained case authority, delivery independence and legacy report compatibility.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use briefcase::architecture::{contracts::{self,rows},identity,service,task};
use serde_json::json;

#[test]
fn every_duty_and_target_uses_a_presentation_independent_case() {
    let f=Fixture::new();
    for duty in task::Duty::names() {for target in ["src/lib.rs","src/","missing-name","."] {
        let full=f.call("brief",json!({"task":"inspect retained evidence","duty":duty,"target":target,"limit":1000,"max_bytes":1048576}));
        let small=f.call("brief",json!({"task":"inspect retained evidence","duty":duty,"target":target,"limit":2,"max_bytes":8000}));
        assert_eq!(full["id"],full["case_id"]);assert_eq!(full["report_id"],full["case_id"]);
        assert_eq!(small["case_id"],full["case_id"],"{duty} {target}");
        let id=full["case_id"].as_str().unwrap();assert!(id.starts_with("task-case:"));
        let retained=f.ledger.report(id).unwrap();contracts::schema(&retained,"task-case",None).unwrap();
        assert_eq!(retained["kind"],"case");assert_eq!(retained["outcome_scope"],"retained_evidence");
        for field in ["instruction","continue_with","freshness","budget","report_id","case_id"] {assert!(retained.get(field).is_none(),"{field}");}
        assert!(retained["request"].get("limit").is_none());assert!(retained["request"].get("max_bytes").is_none());
        assert!(retained["coverage"].get("delivery").is_none());assert!(retained["coverage"]["completeness_contract"].get("C7_delivery").is_none());
        if target=="missing-name" {assert_eq!(full["outcome"],"unknown");}
        else if ["debug","refactor","extend","review","test"].contains(&duty.as_str()) {
            assert_eq!(full["outcome"],"partial");assert_eq!(full["coverage"]["completeness"]["verification"],"unknown");
        }
    }}
    let sql=rusqlite::Connection::open(&f.db).unwrap();
    let briefs:i64=sql.query_row("SELECT count(*) FROM reports WHERE json_extract(body,'$.kind')='brief'",[],|r|r.get(0)).unwrap();
    assert_eq!(briefs,0,"Only cases are retained by brief calls");
}

#[test]
fn retained_case_paging_and_explanation_keep_all_rows_and_reach_links() {
    let mut f=Fixture::new();
    for n in 0..5 {std::fs::write(f.root.join(format!("src/use-{n}.rs")),"// See src/lib.rs.\n").unwrap();}
    f.recapture();
    let packet=f.call("brief",json!({"task":"explain src/lib.rs","target":"src/lib.rs","limit":2,"max_bytes":1048576}));
    let id=packet["case_id"].as_str().unwrap();let full=f.ledger.report(id).unwrap();
    let delivered=packet["coverage"]["delivery"]["/slice/impl"]["returned"].as_u64().unwrap() as usize;
    let omitted=f.call("page",json!({"report_id":id,"section":"/slice/impl","offset":delivered,"limit":1000,"max_bytes":1048576}));
    assert_eq!(rows(&omitted,"items"),&rows(&full["slice"],"impl")[delivered..]);
    assert_eq!(packet["coverage"]["completeness"],full["coverage"]["completeness"]);
    for (i,row) in rows(&full["slice"],"impl").iter().enumerate() {
        let proof=f.call("explain",json!({"report_id":id,"ref":format!("/slice/impl/{i}"),"max_bytes":1048576}));
        assert!(proof["text"].is_string());
        if let Some(link)=row["reached_by"]["via_link"].as_str() {
            let why=f.call("explain",json!({"report_id":id,"ref":link,"max_bytes":1048576}));
            assert_eq!(why["link_id"],link);
        } else {
            assert!(row["selection"]=="resolved_anchor" || rows(row,"selection_reasons").iter().any(|s|s["facet"]=="lexical"),"{row}");
            assert_eq!(row["reached_by"]["depth"],0);
        }
    }
    let metadata=f.call("explain",json!({"report_id":id,"ref":"/coverage/completeness","max_bytes":8000}));
    assert_eq!(metadata["entry"],full["coverage"]["completeness"]);
}

#[test]
fn old_brief_ids_remain_pageable_and_explainable() {
    let f=Fixture::new();
    let packet=f.call("brief",json!({"task":"src/lib.rs","limit":1000,"max_bytes":1048576}));
    let mut legacy=f.ledger.report(packet["case_id"].as_str().unwrap()).unwrap();
    legacy["kind"]=json!("brief");legacy["instruction"]=json!("Legacy retained brief");
    for key in ["id","request","outcome_scope"] {legacy.as_object_mut().unwrap().remove(key);}
    legacy["coverage"].as_object_mut().unwrap().remove("completeness");
    for role in briefcase::architecture::source::ARTIFACT_ROLES {
        for row in legacy["slice"][*role].as_array_mut().unwrap() {row.as_object_mut().unwrap().remove("reached_by");}
    }
    for gap in legacy["not_covered"].as_array_mut().unwrap() {gap.as_object_mut().unwrap().remove("dimension");}
    let id=identity::id("task-brief",&legacy).unwrap();legacy["id"]=json!(id);legacy["report_id"]=json!(id);
    contracts::schema(&legacy,"task-brief",None).unwrap();
    f.ledger.save_report(&id,&legacy,&[]).unwrap();
    let page=f.call("page",json!({"report_id":id,"section":"/slice/impl","max_bytes":1048576}));
    assert_eq!(page["items"],legacy["slice"]["impl"]);
    let proof=f.call("explain",json!({"report_id":id,"ref":"/slice/impl/0","max_bytes":1048576}));
    assert!(proof["text"].as_str().unwrap().contains("demo-tool"));
}

#[test]
fn invalid_presentation_and_scope_fail_before_creating_a_database_or_report() {
    let f=Fixture::new();let db=f.dir.path().join("invalid-case.db");
    for extra in [json!({"limit":0}),json!({"depth":4}),json!({"max_bytes":255}),
        json!({"runtime_from":"bad"}),json!({"runtime_from":"2026-01-02T00:00:00Z","runtime_to":"2026-01-01T00:00:00Z"})] {
        let mut args=json!({"task":"src/lib.rs"});args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert!(service::call_path(&db,Some(&f.root),"brief",&args,false).is_err());assert!(!db.exists());
        let before=rusqlite::Connection::open(&f.db).unwrap().query_row("SELECT count(*) FROM reports",[],|r|r.get::<_,i64>(0)).unwrap();
        assert!(task::brief(&f.ledger,&args).is_err());
        let after=rusqlite::Connection::open(&f.db).unwrap().query_row("SELECT count(*) FROM reports",[],|r|r.get::<_,i64>(0)).unwrap();
        assert_eq!(before,after);
    }
}

#[test]
fn corrupted_retained_cases_fail_closed_on_page_reads() {
    let f=Fixture::new();let packet=f.call("brief",json!({"task":"src/lib.rs","max_bytes":1048576}));
    let id=packet["case_id"].as_str().unwrap();let sql=rusqlite::Connection::open(&f.db).unwrap();
    sql.execute("UPDATE reports SET body=json_set(body,'$.task','corrupted') WHERE id=?",[id]).unwrap();
    assert!(service::call(&f.ledger,"page",&json!({"report_id":id,"section":"/slice/impl"})).is_err());
}
