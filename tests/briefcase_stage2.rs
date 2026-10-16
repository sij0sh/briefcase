//! Stage 2 regressions over retained text; no native analyzer or external fixtures.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use briefcase::architecture::{contracts::{rows,text},source,task};
use serde_json::{json,Value};
use std::collections::BTreeSet;

fn write(f:&Fixture,path:&str,body:&str) {
    let destination=f.root.join(path);std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(destination,body).unwrap();
}
fn paths(packet:&Value)->BTreeSet<String> {
    source::ARTIFACT_ROLES.iter().flat_map(|role|rows(&packet["slice"],role)).map(|file|text(file,"path")).collect()
}

#[test]
fn each_facet_ablation_can_execute_and_is_recorded_in_case_identity() {
    let f=Fixture::new();let args=json!({"task":"repair entrypoint","target":"src/lib.rs","limit":1000,"max_bytes":1048576});
    let baseline=f.call("brief",args.clone());
    for facet in task::retrieval_facets() {
        let mut args=args.clone();args["exclude_facets"]=json!([facet]);
        let packet=f.call("brief",args);
        assert_ne!(packet["case_id"],baseline["case_id"]);
        assert_eq!(packet["coverage"]["retrieval"]["excluded_facets"],json!([facet]));
    }
}

#[test]
fn evaluation_answers_are_excluded_before_lexical_limits_and_remain_explicitly_inspectable() {
    let mut f=Fixture::new();
    write(&f,"src/relevant.rs","pub fn routing_marker() {}\n");
    write(&f,"src/misleading.rs","pub fn unrelated() {}\n");
    let report=format!("src/misleading.rs\n{}",format!("routing_marker evaluation_only_needle {}\n","noise ".repeat(220)).repeat(1500));
    write(&f,"EVALUATION_RERUN.md",&report);
    write(&f,"evaluation/STAGE2.md","routing_marker src/misleading.rs\n");f.recapture();
    let packet=f.call("brief",json!({"task":"repair routing_marker","target":"src/lib.rs","duty":"debug","limit":1000,"max_bytes":1048576}));
    assert!(paths(&packet).contains("src/relevant.rs"));
    assert!(!paths(&packet).contains("src/misleading.rs"));
    assert!(rows(&packet,"candidates").iter().all(|candidate|!text(candidate,"path").contains("EVALUATION_RERUN")
        && !text(candidate,"path").starts_with("evaluation/")));
    let normal=f.call("search",json!({"text":"routing_marker","history":false,"max_bytes":1048576}));
    assert!(rows(&normal,"items").iter().any(|hit|hit["path"]=="src/relevant.rs"));
    assert!(rows(&normal,"items").iter().all(|hit|hit["path"]!="EVALUATION_RERUN.md"));
    assert!(normal["coverage"]["evaluation_artifacts"]["excluded_artifacts"].as_u64().unwrap()>=2);
    let explicit=f.call("search",json!({"text":"evaluation_only_needle","history":false,"include_evaluation":true,"max_bytes":1048576}));
    assert!(rows(&explicit,"items").iter().any(|hit|hit["path"]=="EVALUATION_RERUN.md"));
    assert_eq!(explicit["coverage"]["evaluation_artifacts"]["included"],true);
    let targeted=f.call("agent_context",json!({"task":"inspect this report","target":"EVALUATION_RERUN.md","max_files":1,"max_bytes":8000}));
    assert_eq!(targeted["anchor"]["path"],"EVALUATION_RERUN.md");
    let mut fetch=targeted["fetch_source"]["arguments"].clone();fetch["ref"]=targeted["anchor"]["ref"].clone();
    assert!(f.call("explain",fetch)["text"].as_str().unwrap().contains("src/misleading.rs"));
}

#[test]
fn included_test_modules_and_their_runner_reach_the_implementation_at_test_depth_one() {
    let mut f=Fixture::new();
    write(&f,"Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\nautotests=false\n[[test]]\nname='runner'\npath='tests/runner.rs'\n");
    write(&f,"tests/runner.rs","mod suite;\n");
    write(&f,"tests/suite.rs","#[path=\"parts/leaf.rs\"] mod cases;\n");
    write(&f,"tests/parts/leaf.rs","#[test] fn retains() { let _source=\"../../src/lib.rs\"; }\n");
    write(&f,"tests/comment.rs","// #[path=\"parts/leaf.rs\"] mod imaginary;\n");f.recapture();
    let args=json!({"task":"which tests exercise retention","target":"src/lib.rs","duty":"test","max_files":4,
        "exclude_facets":["lexical"],"limit":1000,"max_bytes":1048576});
    let packet=f.call("brief",args.clone());let selected=paths(&packet);
    for path in ["src/lib.rs","tests/runner.rs","tests/suite.rs","tests/parts/leaf.rs"] {assert!(selected.contains(path),"{packet}");}
    assert!(!selected.contains("tests/comment.rs"));
    assert_eq!(packet["coverage"]["completeness"]["verification"],"unknown");
    let edges=f.call("neighbors",json!({"target":"src/lib.rs","direction":"in","predicate":"TESTS","limit":1000,"max_bytes":1048576}));
    for path in ["tests/runner.rs","tests/suite.rs","tests/parts/leaf.rs"] {
        assert!(rows(&edges,"items").iter().any(|edge|edge["from"]==path),"{edges}");
    }
    let includes=f.call("neighbors",json!({"target":"tests/parts/leaf.rs","direction":"in","predicate":"CONTAINS","max_bytes":1048576}));
    assert!(rows(&includes,"items").iter().any(|edge|edge["from"]=="tests/suite.rs"));
    assert!(!rows(&includes,"items").iter().any(|edge|edge["from"]=="tests/comment.rs"));
    let mut args=args;args["max_bytes"]=json!(8000);
    let manifest=f.call("agent_context",args);assert_eq!(manifest["case_id"],packet["case_id"]);
    for file in rows(&manifest,"files").iter().chain(std::iter::once(&manifest["anchor"])) {
        let mut fetch=manifest["fetch_source"]["arguments"].clone();fetch["ref"]=file["ref"].clone();
        fetch["source_offset"]=json!(file["source_offset"].as_u64().unwrap_or(0));
        let proof=f.call("explain",fetch);assert_eq!(proof["entry"]["path"],file["path"]);
        assert!(!proof["text"].as_str().unwrap().is_empty());
    }
}
