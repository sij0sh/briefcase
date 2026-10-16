//! Observations retain scope, sampling and exact mapping boundaries in context.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use serde_json::json;
use briefcase::architecture::contracts::rows;
#[test]
fn runtime_calls_are_separate_from_static_calls_and_keep_interval_pins(){
    let mut f=Fixture::new();let (_,digest)=f.deployment("desired");let runtime=f.runtime(&digest,false,"runtime");
    let result=f.call("neighbors",json!({"target":"src/lib.rs","predicate":"CALLS_OBSERVED,MAPS_TO_SERVICE","depth":3,"max_bytes":1048576}));
    assert!(rows(&result,"items").iter().any(|e|e["predicate"]=="MAPS_TO_SERVICE"));
    assert!(rows(&result,"items").iter().any(|e|e["predicate"]=="CALLS_OBSERVED"));
    let packet=f.call("brief",json!({"task":"debug src/lib.rs","max_bytes":1048576}));
    assert!(packet["pinned_views"].as_array().unwrap().contains(&json!(runtime)));
    assert!(rows(&packet,"not_covered").iter().any(|g|g["area"]=="runtime_observation_scope"));
}
#[test]
fn service_name_alone_never_joins_desired_identity(){
    let mut f=Fixture::new();let (_,digest)=f.deployment("desired");f.runtime(&digest,true,"name-only");
    let result=f.call("neighbors",json!({"target":"src/lib.rs","predicate":"MAPS_TO_SERVICE","max_bytes":1048576}));assert!(rows(&result,"items").is_empty());
    let packet=f.call("brief",json!({"task":"debug src/lib.rs","max_bytes":1048576}));
    assert!(rows(&packet,"not_covered").iter().any(|g|g["area"]=="runtime_identity"));
}
#[test]
fn explicit_interval_selection_does_not_imply_negative_runtime_coverage(){
    let mut f=Fixture::new();let (_,digest)=f.deployment("desired");f.runtime(&digest,false,"runtime");
    let result=f.call("neighbors",json!({"target":"src/lib.rs","predicate":"CALLS_OBSERVED","runtime_from":"1970-01-02T00:00:00Z","max_bytes":1048576}));
    assert!(rows(&result,"items").is_empty());assert!(!result.to_string().contains("known_absent"));
}
