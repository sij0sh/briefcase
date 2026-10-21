//! Architectural impact composition through the public CLI/MCP service.
//! Self-contained fixtures; no omitted evaluation archive or runtime server.
#[path = "context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use briefcase::architecture::{capture::Capture, contracts::{self,rows}, history, identity, mcp, python, service};
use serde_json::{json,Value};
use std::{collections::BTreeSet,path::Path};

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path,content).unwrap();
}
fn declaration() -> Value {
    json!({"schema_version":"1.0","authority":"reviewed-fixture","revision":"r1",
        "assignments":{"auth/token.py":"Authentication","api/handler.py":"API"},
        "rules":[{"id":"no-api-token-internals","mode":"report-only","from":"api/handler.py","to":"auth/token.py"}],
        "features":[{"id":"authentication","name":"Authentication","domain":"Security"}],
        "workflows":[{"id":"login","name":"Login","features":["authentication"],
            "entrypoints":[{"kind":"symbol","key":"api/handler.py::handle"}],
            "participants":[{"kind":"module","key":"auth/token.py"}]}],
        "feature_implementations":[{"feature":"authentication","subject":{"kind":"module","key":"auth/token.py"}}]})
}
fn publish(f: &mut Fixture, policy: &Value) {
    let policy_path = f.dir.path().join("architecture.json");
    std::fs::write(&policy_path,serde_json::to_vec(policy).unwrap()).unwrap();
    let repository = f.ledger.repository().unwrap();
    let generation = f.ledger.begin(&repository,None).unwrap();
    let prepared = python::produce(Capture::read(&f.root,Some(&policy_path)).unwrap(),
        &repository,generation,&python::ScanOptions::default()).unwrap();
    f.view = f.ledger.commit(generation,&prepared).unwrap();
    f.ledger.publish(&f.view,true).unwrap();
}
fn fixture() -> Fixture {
    let mut f = Fixture::new();
    write(&f.root,"auth/token.py","def parse(value):\n    return value\n");
    write(&f.root,"api/handler.py","from auth import token\ndef handle(value):\n    return token.parse(value)\n");
    write(&f.root,"worker/queue.py","from api import handler\ndef run(value):\n    return handler.handle(value)\n");
    write(&f.root,"tests/test_auth.py","from auth import token\ndef test_parse():\n    assert token.parse('ok') == 'ok'\n");
    publish(&mut f,&declaration());f
}
fn pre(f: &Fixture, extra: Value) -> Value {
    let mut args = json!({"mode":"pre","task":"change token encoding safely","view":f.view,"targets":["auth/token.py"],"max_bytes":1048576});
    for (key,value) in extra.as_object().unwrap() { args[key]=value.clone(); }
    f.call("impact",args)
}
fn retained(f: &Fixture, report: &Value) -> Value {
    let doc = f.ledger.report(report["report_id"].as_str().unwrap()).unwrap();
    contracts::schema(&doc,"architectural-impact-contract",None).unwrap();doc
}
fn components<'a>(doc: &'a Value, side: &str, relationship: &str) -> Vec<&'a Value> {
    rows(doc,"affected_components").iter().filter(|r|r["side"]==side && r["relationship"]==relationship).collect()
}
fn assert_refs(f: &Fixture, value: &Value) {
    if let Some(object)=value.as_object() {
        if let Some(refs)=object.get("evidence_refs").and_then(Value::as_array) {
            for reference in refs {
                let mut args=reference.clone();args["max_bytes"]=json!(1048576);
                let proof=f.call("explain",args);
                assert!(proof.is_object(),"{proof:#}");
            }
        }
        for child in object.values() { assert_refs(f,child); }
    } else if let Some(items)=value.as_array() { for child in items { assert_refs(f,child); } }
}

#[test]
fn prospective_impact_finds_direct_and_indirect_consumers_with_explainable_trails() {
    let f=fixture();let doc=retained(&f,&pre(&f,json!({"depth":3})));
    assert!(components(&doc,"candidate","direct_change").iter().any(|r|r["id"]=="auth/token.py"));
    let consumers=components(&doc,"candidate","possible_consumer");
    assert!(consumers.iter().any(|r|r["id"]=="api/handler.py"),"{consumers:#?}");
    let worker=consumers.iter().find(|r|r["id"]=="worker/queue.py").unwrap();
    assert!(rows(worker,"evidence_refs").len()>=3,"An indirect consumer needs the full dependency trail");
    assert_refs(&f,&doc);
}

#[test]
fn declared_features_workflows_domains_and_report_only_constraints_keep_their_authority() {
    let f=fixture();let doc=retained(&f,&pre(&f,json!({})));
    let declared=components(&doc,"candidate","declared_binding");
    for key in ["feature:authentication","workflow:login","Security"] {
        assert!(declared.iter().any(|r|r["id"]==key),"{key}: {declared:#?}");
    }
    let constraint=rows(&doc,"constraints").iter().find(|r|r["rule_id"]=="no-api-token-internals").unwrap();
    assert_eq!(constraint["mode"],"report-only");
    assert_eq!(constraint["status"],"violation");
    assert_eq!(doc["assessment"],"partial");
    assert_eq!(doc["governing_decisions"],json!([]));
    assert_eq!(doc["governing_decisions_status"],"not_connected");
}

#[test]
fn associated_tests_are_suggested_and_never_claimed_to_have_run() {
    let f=fixture();let doc=retained(&f,&pre(&f,json!({})));
    assert!(rows(&doc,"verification").iter().any(|r|r["path"]=="tests/test_auth.py"));
    assert!(rows(&doc,"verification").iter().any(|r|r["basis"]=="interface_relationship"));
    for check in rows(&doc,"verification") {
        assert_eq!(check["requirement"],"suggested");assert_eq!(check["execution_status"],"not_executed");
    }
    let mut forged=doc.clone();forged["verification"][0]["execution_status"]=json!("passed");
    assert!(contracts::schema(&forged,"architectural-impact-contract",None).is_err());
    let mut forged=doc.clone();forged["verification"][0]["requirement"]=json!("mandatory");
    assert!(contracts::schema(&forged,"architectural-impact-contract",None).is_err());
}

#[test]
fn report_identity_is_independent_of_delivery_limits_and_target_order() {
    let f=fixture();
    let a=pre(&f,json!({"targets":["auth/token.py","api/handler.py"],"limit":1}));
    let b=pre(&f,json!({"targets":["api/handler.py","auth/token.py"],"limit":100,"max_bytes":50000}));
    assert_eq!(a["report_id"],b["report_id"]);
    let doc=retained(&f,&a);let mut input=doc.clone();input.as_object_mut().unwrap().remove("report_id");
    assert_eq!(doc["report_id"],identity::id("impact-contract",&input).unwrap());
    assert!(doc.get("delivery").is_none());assert!(doc.get("budget").is_none());
    let first=f.call("page",json!({"report_id":a["report_id"],"section":"/affected_components","limit":1,"max_bytes":1048576}));
    let second=f.call("page",first["continue_with"]["arguments"].clone());
    assert_eq!(second["page"]["offset"],1);
    assert_ne!(first["items"][0],second["items"][0]);
}

#[test]
fn a_case_supplies_the_pin_and_stays_immutable_when_live_source_changes() {
    let f=fixture();let packet=f.call("brief",json!({"task":"change token encoding safely","target":"auth/token.py","view":f.view}));
    let args=json!({"mode":"pre","case_id":packet["case_id"],"max_bytes":1048576});
    let a=f.call("impact",args.clone());
    write(&f.root,"auth/token.py","raise Exception('live bytes must not execute or be read')\n");
    let b=f.call("impact",args);
    assert_eq!(a["report_id"],b["report_id"]);
    assert_eq!(a["source"]["candidate_view"],f.view);
    let doc=retained(&f,&b);assert_eq!(doc["case_id"],packet["case_id"]);
}

#[test]
fn post_mode_includes_changes_outside_the_initial_slice_and_both_sides() {
    let mut f=fixture();let base=f.view.clone();
    let case=f.call("brief",json!({"task":"change token encoding safely","target":"auth/token.py","view":base,"max_files":1}));
    write(&f.root,"auth/token.py","def parse(value):\n    return value.strip()\n");
    write(&f.root,"unrelated/new.py","def added(): return True\n");
    std::fs::remove_file(f.root.join("worker/queue.py")).unwrap();
    publish(&mut f,&declaration());
    let packet=f.call("impact",json!({"mode":"post","base":base,"view":f.view,"case_id":case["case_id"],"max_bytes":1048576}));
    let doc=retained(&f,&packet);
    for (path,state) in [("auth/token.py","modified"),("unrelated/new.py","added"),("worker/queue.py","deleted")] {
        let change=rows(&doc,"changes").iter().find(|r|r["path"]==path).unwrap();assert_eq!(change["state"],state);
    }
    assert_eq!(rows(&doc,"changes").iter().find(|r|r["path"]=="unrelated/new.py").unwrap()["outside_initial_slice"],true);
    assert!(components(&doc,"base","possible_consumer").iter().any(|r|r["id"]=="worker/queue.py"));
    assert!(rows(&doc,"relationship_changes").iter().any(|r|r["state"]=="observed_only_in_base"));
    assert_refs(&f,&doc);
}

#[test]
fn text_only_snapshots_preserve_partial_analysis_and_pinned_source_fetching() {
    let mut f=Fixture::new();let base=f.view.clone();
    write(&f.root,"src/another.rs","pub fn another() {}\n");f.recapture();
    let doc=retained(&f,&f.call("impact",json!({"mode":"post","base":base,"view":f.view,"max_bytes":1048576})));
    assert!(rows(&doc,"changes").iter().any(|r|r["path"]=="src/another.rs"));
    assert!(rows(&doc,"unknowns").iter().any(|g|g["area"]=="dependencies"));
    assert_eq!(doc["assessment"],"partial");assert_refs(&f,&doc);
}

#[test]
fn unresolved_and_ambiguous_targets_are_never_direct_changes() {
    let mut f=fixture();
    write(&f.root,"other/token.py","def parse(value): return str(value)\n");publish(&mut f,&declaration());
    for (target,status) in [("missing.py","unresolved"),("parse","ambiguous")] {
        let packet=pre(&f,json!({"targets":[target]}));let doc=retained(&f,&packet);
        assert_eq!(doc["resolutions"][0]["status"],status);
        assert!(components(&doc,"candidate","direct_change").is_empty());
        assert!(rows(&doc,"unknowns").iter().any(|g|g["area"]=="targets"));
    }
}

#[test]
fn bounded_traversal_is_visible_and_does_not_truncate_the_source_delta() {
    let f=fixture();let doc=retained(&f,&pre(&f,json!({"depth":1,"max_nodes":1})));
    assert!(rows(&doc,"unknowns").iter().any(|g|g["area"]=="traversal"));
    assert_eq!(doc["changes"][0]["path"],"auth/token.py");
    assert_eq!(doc["assessment"],"partial");
}

#[test]
fn retracted_relationships_are_not_reused_from_an_old_case() {
    let mut f=fixture();let packet=pre(&f,json!({}));let old=retained(&f,&packet);
    let report=f.call("neighbors",json!({"target":"api/handler.py","predicate":"IMPORTS","direction":"out","view":f.view,"max_bytes":1048576}));
    let ids: BTreeSet<_>=rows(&report,"items").iter().flat_map(|r|contracts::strings(&r["assertion_ids"])).collect();
    assert!(!ids.is_empty());for id in ids {f.ledger.retract(&id,"fixture withdrawal").unwrap();}
    let new=f.call("impact",json!({"mode":"pre","case_id":old["case_id"],"max_bytes":1048576}));
    let current=retained(&f,&new);
    assert_ne!(old["report_id"],current["report_id"]);
    assert!(current["retraction_epoch"].as_u64().unwrap()>old["retraction_epoch"].as_u64().unwrap());
    assert!(rows(&current,"unknowns").iter().any(|g|g["area"]=="retractions"));
    let page=f.call("page",json!({"report_id":old["report_id"],"section":"/affected_components"}));
    assert_eq!(page["historical_packet"],true);
}

#[test]
fn same_snapshot_scopes_and_mismatched_case_pins_are_rejected() {
    let mut f=fixture();
    assert!(service::call(&f.ledger,"impact",&json!({"mode":"post","base":f.view,"view":f.view})).is_err());
    let case=pre(&f,json!({}))["case_id"].clone();
    write(&f.root,"auth/token.py","def parse(value): return str(value)\n");publish(&mut f,&declaration());
    assert!(service::call(&f.ledger,"impact",&json!({"mode":"pre","case_id":case,"view":f.view})).is_err());
    let (deployment,_) = f.deployment("impact-scope");
    assert!(service::call(&f.ledger,"impact",&json!({"mode":"pre","task":"inspect","view":deployment})).is_err());
}

#[test]
fn invalid_mode_combinations_types_and_untrusted_execution_arguments_are_rejected() {
    let f=Fixture::new();
    for args in [json!({}),json!({"mode":"future"}),json!({"mode":"pre","task":"change"}),
        json!({"mode":"post","base":f.view}),json!({"mode":"post","base":f.view,"view":f.view,"targets":["src/lib.rs"]}),
        json!({"mode":"pre","task":"change","view":f.view,"targets":[1]}),
        json!({"mode":"pre","task":"change","view":f.view,"depth":4}),
        json!({"mode":"pre","task":"change","view":f.view,"max_nodes":0}),
        json!({"mode":"pre","task":"change","view":f.view,"run":"sh payload.sh"}),
        json!({"mode":"pre","task":"change","view":f.view,"targets":["  "]})] {
        assert!(service::call(&f.ledger,"impact",&args).is_err(),"{args}");
    }
}

#[test]
fn cli_and_mcp_route_to_the_same_retained_operation() {
    let f=fixture();let args=json!({"mode":"pre","task":"inspect token format","view":f.view,"targets":["auth/token.py"],"max_bytes":1048576});
    let expected=f.call("impact",args.clone());
    let response=mcp::dispatch(&f.ledger,&json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"briefcase_query","arguments":{"operation":"impact","arguments":args}}}));
    assert!(response.get("error").is_none(),"{response:#}");
    assert_eq!(response["result"]["isError"],false,"{response:#}");
    let body:Value=serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["report_id"],expected["report_id"]);
    let output=std::process::Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .env_remove("BRIEFCASE_DB").env_remove("BRIEFCASE_SYNC")
        .args(["--db",f.db.to_str().unwrap(),"--project-root",f.root.to_str().unwrap(),"--json","call","impact","--args",&args.to_string()])
        .output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let body:Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(body["report_id"],expected["report_id"]);
}

#[test]
fn prepared_exact_correspondence_is_exposed_without_inventing_file_lineage() {
    let mut f=fixture();let base=f.view.clone();let before=f.ledger.load(&base).unwrap();
    std::fs::rename(f.root.join("worker/queue.py"),f.root.join("worker/jobs.py")).unwrap();
    publish(&mut f,&declaration());let after=f.ledger.load(&f.view).unwrap();
    history::prepare(&mut f.ledger,&before,&after).unwrap();
    let doc=retained(&f,&f.call("impact",json!({"mode":"post","base":base,"view":f.view,"max_bytes":1048576})));
    assert!(rows(&doc,"correspondence").iter().any(|r|r["detail"]["certainty_class"]=="exact_content_move"),"{:#}",doc["correspondence"]);
    assert!(rows(&doc,"changes").iter().any(|r|r["path"]=="worker/queue.py" && r["state"]=="deleted"));
    assert!(rows(&doc,"changes").iter().any(|r|r["path"]=="worker/jobs.py" && r["state"]=="added"));
    assert_refs(&f,&doc);
}

#[test]
fn budgeted_summaries_leave_a_recoverable_retained_report() {
    let f=fixture();let packet=pre(&f,json!({"max_bytes":4000}));
    assert!(serde_json::to_vec(&packet).unwrap().len()<4000);
    let doc=retained(&f,&packet);assert!(!rows(&doc,"affected_components").is_empty());
    assert_eq!(packet["continue_with"]["operation"],"page");
    let page=f.call("page",packet["continue_with"]["arguments"].clone());
    assert_eq!(page["report_id"],packet["report_id"]);
}

#[test]
fn incomplete_candidate_inventory_does_not_prove_a_deletion() {
    let mut f=Fixture::new();let base=f.view.clone();
    std::fs::remove_file(f.root.join("src/lib.rs")).unwrap();
    std::fs::write(f.root.join("binary.dat"),[0_u8,1,2]).unwrap();f.recapture();
    let doc=retained(&f,&f.call("impact",json!({"mode":"post","base":base,"view":f.view,"max_bytes":1048576})));
    let missing=rows(&doc,"changes").iter().find(|r|r["path"]=="src/lib.rs").unwrap();
    assert_eq!(missing["state"],"unavailable");
    assert!(rows(&doc,"unknowns").iter().any(|g|g["area"]=="capture"));
}

#[test]
fn declaration_only_changes_keep_previous_constraints_visible_for_review() {
    let mut f=fixture();let base=f.view.clone();let mut policy=declaration();
    policy["revision"]=json!("r2");policy["rules"]=json!([]);publish(&mut f,&policy);
    let doc=retained(&f,&f.call("impact",json!({"mode":"post","base":base,"view":f.view,"max_bytes":1048576})));
    assert!(rows(&doc,"constraints").iter().any(|r|r["side"]=="base" && r["rule_id"]=="no-api-token-internals"));
    assert!(rows(&doc,"unknowns").iter().any(|g|g["area"]=="declarations"));
    assert!(rows(&doc,"relationship_changes").iter().any(|r|r["predicate"]=="FORBIDS_DEPENDENCY" && r["state"]=="observed_only_in_base"));
}
