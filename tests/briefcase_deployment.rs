//! Desired-state producer invariants through the shared context index.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use serde_json::json;
use briefcase::architecture::contracts::{rows,text};
#[test]
fn exact_build_chain_is_generic_adjacency_with_retained_proof(){
    let mut f=Fixture::new();let (desired,_)=f.deployment("desired");
    let result=f.call("neighbors",json!({"target":"src/lib.rs","direction":"both","predicate":"INCLUDES_SOURCE,BUILDS,DEPLOYS_AS","depth":3,"limit":1000,"max_bytes":1048576}));
    assert!(rows(&result,"items").iter().any(|e|e["predicate"]=="INCLUDES_SOURCE"));
    let edge=rows(&result,"items").iter().find(|e|e["predicate"]=="DEPLOYS_AS").unwrap();
    assert!(["evidence","declaration"].contains(&edge["class"].as_str().unwrap()));
    let proof=f.call("explain",json!({"report_id":result["report_id"],"ref":edge["id"],"max_bytes":1048576}));
    assert!(rows(&proof,"assertions").iter().any(|a|a["source_view"]==desired));
}
#[test]
fn source_revision_mismatch_is_a_gap_not_a_deployment_join(){
    let mut f=Fixture::new();f.deployment("old");
    std::fs::write(f.root.join("src/lib.rs"),"pub fn changed(){}\n").unwrap();f.recapture();
    let result=f.call("neighbors",json!({"target":"src/lib.rs","predicate":"INCLUDES_SOURCE","max_bytes":1048576}));assert!(rows(&result,"items").is_empty());
    let packet=f.call("brief",json!({"task":"review src/lib.rs","max_bytes":1048576}));
    assert!(rows(&packet,"not_covered").iter().any(|gap|gap["area"]=="deployment_source"));
}
#[test]
fn retracted_build_evidence_withholds_current_source_joins(){
    let mut f=Fixture::new();let (desired,_)=f.deployment("desired");let view=f.ledger.load(&desired).unwrap();
    let assertion=rows(&view.bundle,"assertions").iter().find(|a|a["proposition"]["predicate"]=="BUILDS").unwrap();
    f.ledger.retract(&text(assertion,"id"),"test withdrawal").unwrap();
    let result=f.call("neighbors",json!({"target":"src/lib.rs","predicate":"INCLUDES_SOURCE"}));assert!(rows(&result,"items").is_empty());
}
