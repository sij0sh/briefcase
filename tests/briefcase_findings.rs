//! Phase 1 vertical slice: one capability/guard fixture, end to end.
//! Capability (declared), policy/filesystem guards (static) and runtime
//! satisfaction (unknown) must never be conflated; a test association must
//! never be reported as executed coverage.
#[path = "context_support/mod.rs"]
mod context_support;
use briefcase::architecture::{
    contracts::{self, rows},
    identity,
};
use context_support::Fixture;
use serde_json::json;

fn fixture() -> Fixture {
    let mut f = Fixture::new();
    std::fs::create_dir_all(f.root.join("config")).unwrap();
    std::fs::create_dir_all(f.root.join("tests")).unwrap();
    std::fs::write(f.root.join("src/adapter.rs"),
        "pub fn rollback() -> Result<(), String> {\n    if !restore_is_safe() { return Err(\"restore unsafe\".into()); }\n    let _token = std::env::var(\"ROLLBACK_TOKEN\");\n    Ok(())\n}\nfn restore_is_safe() -> bool { false }\n").unwrap();
    std::fs::write(f.root.join("config/app.env"), "ROLLBACK_TOKEN=fixture\n").unwrap();
    std::fs::write(
        f.root.join("tests/adapter_test.rs"),
        "// Exercises src/adapter.rs statically.\n",
    )
    .unwrap();
    f.recapture();
    f
}

#[test]
fn explanation_separates_capability_conditions_and_runtime() {
    let f = fixture();
    let packet = f.call(
        "brief",
        json!({"task":"explain the rollback adapter and its conditions",
        "target":"src/adapter.rs","limit":100,"max_bytes":1048576}),
    );
    let id = packet["case_id"].as_str().unwrap();
    let retained = f.ledger.report(id).unwrap();
    contracts::schema(&retained, "task-case", None).unwrap();
    assert_eq!(retained["schema_version"], "1.1");
    assert_eq!(retained["findings_status"], "evaluated");
    assert_eq!(retained["coverage"]["findings"]["status"], "evaluated");
    let findings = rows(&retained, "findings");
    let finding = findings
        .iter()
        .find(|f| f["subjects"] == json!(["src/adapter.rs"]))
        .unwrap_or_else(|| panic!("adapter finding missing: {findings:?}"));
    // Comparison key is logical and stable; content_id is the version hash.
    assert_eq!(
        finding["comparison_key"],
        "behavior_condition:src/adapter.rs"
    );
    assert!(finding["content_id"]
        .as_str()
        .unwrap()
        .starts_with("finding:"));
    let conditions = rows(&finding["payload"], "conditions");
    let config = conditions
        .iter()
        .find(|c| c["subject"] == "env:ROLLBACK_TOKEN")
        .unwrap_or_else(|| panic!("config condition missing: {conditions:?}"));
    assert_eq!(config["relationship_assessment"], "established");
    assert_eq!(
        config["runtime_satisfaction"], "unknown",
        "a static config read must not claim the guard passes at runtime"
    );
    assert!(
        conditions
            .iter()
            .any(|c| c["relationship_assessment"] == "unknown"
                && c["description"]
                    .as_str()
                    .unwrap_or("")
                    .contains("restore_is_safe")),
        "guard must stay an unresolved candidate: {conditions:?}"
    );
    // Every cited link resolves in the pinned index; no claim is witness-free.
    for evidence in rows(&finding["basis"], "evidence") {
        if let Some(link) = evidence["link_id"].as_str() {
            let proof = f.call(
                "explain",
                json!({"report_id":id,"ref":link,"max_bytes":8000}),
            );
            assert!(
                proof.get("link_id").is_some()
                    || proof.get("entry").is_some()
                    || proof.get("text").is_some(),
                "finding evidence link must be explainable: {proof}"
            );
        }
    }
    // Brief + agent manifest deliver compact findings under the same budget.
    assert!(packet["coverage"]["delivery"].get("/findings").is_some());
    let manifest = f.call(
        "agent_context",
        json!({"task":"explain the rollback adapter and its conditions",
        "target":"src/adapter.rs","max_bytes":8000}),
    );
    let compact = rows(&manifest, "findings");
    assert!(
        compact
            .iter()
            .any(|f| f["ref"] == "/findings/0" || f["subjects"] == json!(["src/adapter.rs"])),
        "{compact:?}"
    );
    assert_eq!(manifest["findings_status"], "evaluated");
    let page = f.call(
        "page",
        json!({"report_id":id,"section":"/findings","offset":0,"limit":20,"max_bytes":1048576}),
    );
    assert_eq!(rows(&page, "items"), findings);
}

#[test]
fn import_chain_alone_does_not_create_test_verification() {
    let f = fixture();
    let packet = f.call(
        "brief",
        json!({"task":"explain src/lib.rs","target":"src/lib.rs",
        "limit":100,"max_bytes":1048576}),
    );
    let retained = f
        .ledger
        .report(packet["case_id"].as_str().unwrap())
        .unwrap();
    for finding in rows(&retained, "findings") {
        for check in rows(&finding["payload"], "verification") {
            assert_ne!(check["assessment"], "established");
            assert!(
                !check["statement"].as_str().unwrap_or("").contains("passed"),
                "static association must not be phrased as a passing test: {check}"
            );
        }
    }
}

#[test]
fn legacy_1_0_cases_remain_readable_and_mean_not_evaluated() {
    let f = fixture();
    let packet = f.call(
        "brief",
        json!({"task":"explain src/adapter.rs","target":"src/adapter.rs","max_bytes":1048576}),
    );
    let mut legacy = f
        .ledger
        .report(packet["case_id"].as_str().unwrap())
        .unwrap();
    legacy.as_object_mut().unwrap().remove("findings");
    legacy.as_object_mut().unwrap().remove("findings_status");
    legacy["coverage"]
        .as_object_mut()
        .unwrap()
        .remove("findings");
    legacy["schema_version"] = json!("1.0");
    legacy.as_object_mut().unwrap().remove("id");
    let id = identity::id("task-case", &legacy).unwrap();
    legacy["id"] = json!(id);
    contracts::schema(&legacy, "task-case", None).unwrap();
    f.ledger.save_report(&id, &legacy, &[]).unwrap();
    let page = f.call(
        "page",
        json!({"report_id":id,"section":"/slice/impl","max_bytes":1048576}),
    );
    assert!(!rows(&page, "items").is_empty());
    assert!(
        legacy.get("findings").is_none(),
        "absent findings in a 1.0 case mean not evaluated, not none exist"
    );
}

#[test]
fn behavior_facet_opt_out_keeps_the_slice_and_records_the_request() {
    let f = fixture();
    let args = json!({"task":"explain the rollback adapter and its conditions",
        "target":"src/adapter.rs","limit":100,"max_bytes":1048576});
    let normal = f.call("brief", args.clone());
    let mut ablated_args = args;
    ablated_args["exclude_facets"] = json!(["behavior"]);
    let ablated = f.call("brief", ablated_args);
    let retained = f
        .ledger
        .report(ablated["case_id"].as_str().unwrap())
        .unwrap();
    contracts::schema(&retained, "task-case", None).unwrap();
    assert_eq!(
        retained["coverage"]["findings"]["status"],
        "disabled_by_request"
    );
    assert!(rows(&retained, "findings")
        .iter()
        .all(|f| f["kind"] != "behavior_condition"));
    // Same slice either way: outcome trials compare findings, not selection.
    let normal = f
        .ledger
        .report(normal["case_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(normal["slice"], retained["slice"]);
}
