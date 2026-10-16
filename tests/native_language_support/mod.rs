#![allow(dead_code)]
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path, process::Command};
use briefcase::architecture::{
    capture::Capture,
    contracts::{self, rows, text},
    ledger::{Ledger, Prepared},
    query::{self, Options},
};

pub fn source(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}
pub fn options(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
pub fn publish(l: &mut Ledger, label: &str, produce: impl FnOnce(u64) -> Prepared) -> Value {
    let generation = l.begin("native-tests", Some(label)).unwrap();
    let prepared = produce(generation);
    contracts::validate(&prepared.bundle, &prepared.payloads).unwrap();
    let view = l.commit(generation, &prepared).unwrap();
    l.publish(&view, true).unwrap();
    prepared.diagnostics
}
pub fn pairs(bundle: &Value, predicate: &str) -> BTreeSet<(String, String)> {
    let key = |id: &str| {
        text(
            rows(bundle, "entity_versions")
                .iter()
                .find(|e| e["id"] == id)
                .unwrap(),
            "canonical_key",
        )
        .split('#')
        .next()
        .unwrap()
        .to_owned()
    };
    rows(bundle, "assertions")
        .iter()
        .filter(|a| a["proposition"]["predicate"] == predicate)
        .map(|a| {
            (
                key(&text(&a["proposition"], "subject_id")),
                key(&text(&a["proposition"]["object"], "entity_version_id")),
            )
        })
        .collect()
}
pub fn cli(root: &Path, args: &[&str]) -> (bool, Value, String) {
    let result = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(root)
        .env_remove("BRIEFCASE_DB")
        .args(args)
        .output()
        .unwrap();
    (
        result.status.success(),
        serde_json::from_slice(&result.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&result.stderr).into_owned(),
    )
}
/// A rebuild may produce new report handles and trim different metadata.
/// The selected retained evidence must stay identical and explainable.
pub fn assert_retained_retrieval(l: &Ledger, before: &Value, after: &Value) {
    let evidence = |report: &Value| rows(report,"items").iter().map(|item|json!({
        "artifact_id":item["artifact_id"],"digest":item["digest"],"path":item["path"],
        "byte_range":item["byte_range"],"text":item["text"],"rank":item["rank"],
        "evidence_matches":item["evidence_matches"]})).collect::<Vec<_>>();
    assert_eq!(evidence(before),evidence(after));
    for report in [before,after] {
        let item=rows(report,"items").first().unwrap();
        let proof=briefcase::architecture::service::call(l,"explain",&json!({
            "report_id":report["report_id"],"ref":"/items/0","max_bytes":16384})).unwrap();
        assert_eq!(proof["entry"]["digest"],item["digest"]);
        let payload=l.payloads_for(&[text(item,"artifact_id")]).unwrap();
        let bytes=&payload[&text(item,"artifact_id")];
        assert_eq!(briefcase::architecture::identity::bytes_digest(bytes),text(item,"digest"));
        let start=item["byte_range"]["start"].as_u64().unwrap() as usize;
        let end=item["byte_range"]["end"].as_u64().unwrap() as usize;
        assert_eq!(std::str::from_utf8(&bytes[start..end]).unwrap(),text(item,"text"));
    }
}

pub fn proof_and_retraction(l: &mut Ledger, from: &str, to: &str) {
    let report = query::reachability(l, &options("base"), from, to).unwrap();
    assert_eq!(
        report["response"]["results"][0]["conclusion"], "present",
        "{report}"
    );
    let handle = text(&report["response"]["results"][0], "explanation_handle");
    let proof = query::explain(l, &handle, 1000, 0).unwrap();
    let call = rows(&proof, "support")
        .iter()
        .find(|s| s["assertion"]["proposition"]["predicate"] == "CALLS_STATIC")
        .unwrap();
    assert!(!rows(call, "sources").is_empty());
    let id = text(&call["assertion"], "id");
    l.retract(&id, "corrected native call").unwrap();
    l.rebuild("base").unwrap();
    assert_eq!(
        query::reachability(l, &options("base"), from, to).unwrap()["response"]["results"][0]
            ["conclusion"],
        "unknown"
    );
    assert!(
        rows(&query::explain(l, &handle, 1000, 0).unwrap(), "support")
            .iter()
            .any(|s| s["assertion"]["id"] == id && s["currently_retracted"] == true)
    );
}
pub fn policy(root: &Path) -> std::path::PathBuf {
    let file = root.join("policy.json");
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({"authority":"owner","revision":"1",
        "assignments":{"app.go":"Checkout","App.java":"Checkout"},
        "rules":[{"id":"reverse","from":"app.go","to":"store.go","mode":"report-only"}]}))
        .unwrap(),
    )
    .unwrap();
    file
}
pub fn capture(root: &Path) -> Capture {
    Capture::read(root, None).unwrap()
}
