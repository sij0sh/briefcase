use super::discovery::examples;
use super::*;
#[test]
fn catalog_is_typed_unique_and_examples_validate(){
    let ops=catalog();
    let names:std::collections::BTreeSet<_>=ops.iter().map(|o|o.name).collect();
    let tools:std::collections::BTreeSet<_>=ops.iter().map(|o|o.tool.as_str()).collect();
    assert_eq!(names.len(),ops.len());assert_eq!(tools.len(),ops.len());
    for op in &ops {for example in examples(op.name){validate(&op.schema,&example["arguments"]).unwrap();}}
    for deleted in ["map","map_explain","product","feature_map","feature_map_load","feature_explain","session_map","simplification_review","simplification_compare",
        "data_reads","data_writes","lineage","deployment","desired_connections","observed_calls","runtime_connections","static_observed","endpoint_context"] {
        assert!(find(deleted).is_none(),"{deleted}");
        assert!(find(&format!("architecture_{deleted}")).is_none());
    }
}
#[test]
fn aliases_share_the_authoritative_implementation(){
    for (alias,canonical) in [("ask","brief"),("recall","search"),("retrieve","search"),("task_page","page"),("task_explain","explain"),("dependencies","neighbors"),("callgraph","neighbors")] {
        assert_eq!(find(alias).unwrap().kind,find(canonical).unwrap().kind);
        assert!(!catalog().iter().any(|o|o.name==alias));
    }
    let brief=find("brief").unwrap();
    let a=normalize("ask",&brief,&json!({"question":"how does src/store.rs work"}));
    assert_eq!(a["task"],"how does src/store.rs work");validate(&brief.schema,&a).unwrap();
    let neighbors=find("neighbors").unwrap();
    let a=normalize("dependencies",&neighbors,&json!({"component":"src/store.rs"}));
    assert_eq!(a["target"],"src/store.rs");assert_eq!(a["predicate"],"IMPORTS,DEPENDS_ON");
    validate(&neighbors.schema,&a).unwrap();
}
#[test]
fn one_duty_vocabulary_drives_validation(){
    let brief=find("brief").unwrap();
    assert_eq!(brief.schema["properties"]["duty"]["enum"],json!(crate::architecture::task::Duty::names()));
    assert!(validate(&brief.schema,&json!({"task":"x","duty":"invented"})).is_err());
    assert!(validate(&brief.schema,&json!({"task":"x","depth":4})).is_err());
    assert!(validate(&brief.schema,&json!({"task":"x","sql":"DROP"})).is_err());
    let reach=find("reachability").unwrap();
    assert!(validate(&reach.schema,&json!({"from":"x"})).is_err());
    for write in ["scan","sync","retract","rebuild","recover","classify"] {assert!(find(write).is_none());}
}
#[test]
fn discovery_prefers_generic_adjacency_for_callers(){
    let query=json!({"query":"what calls this function","limit":2});
    let result=discover(&query).unwrap();
    assert_eq!(result,discover(&query).unwrap());assert_eq!(result[0]["name"],"neighbors");
    assert_eq!(discover(&json!({"names":true})).unwrap().as_array().unwrap().len(),catalog().len());
    assert!(discover(&json!({"operation":"map"})).is_err());
    let search=find("search").unwrap();
    assert_eq!(normalize_aliases(&search.schema,&json!({"query":"store"})),json!({"text":"store"}));
    assert!(validate(&search.schema,&json!(["bad"])).is_err());
}
