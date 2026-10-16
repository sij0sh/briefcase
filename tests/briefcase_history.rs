use serde_json::{json, Value};
use std::path::Path;
use briefcase::architecture::{
    capture::Capture,
    contracts::{rows, strings, text},
    ledger::Ledger,
    query::{self, Options},
    rust::{self, ScanOptions},
};

fn source(root: &Path, modules: &[(&str, &str)]) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"history-tests\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    let mut lib = String::new();
    for (name, code) in modules {
        lib.push_str(&format!("pub mod {name};\n"));
        std::fs::write(root.join(format!("src/{name}.rs")), code).unwrap();
    }
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
}
fn options(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
fn scan(l: &mut Ledger, root: &Path, label: &str) {
    let generation = l.begin("history-tests", Some(label)).unwrap();
    let prepared = rust::produce(
        Capture::read(root, None).unwrap(),
        "history-tests",
        generation,
        &ScanOptions {
            analyzer: None,
            ..Default::default()
        },
    )
    .unwrap();
    let id = l.commit(generation, &prepared).unwrap();
    l.publish(&id, true).unwrap();
}
fn uncertainty(before: &[(&str, &str)], after: &[(&str, &str)], expected: &str) {
    let temp = tempfile::tempdir().unwrap();
    let br = temp.path().join("before");
    let ar = temp.path().join("after");
    source(&br, before);
    source(&ar, after);
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    scan(&mut l, &br, "before");
    scan(&mut l, &ar, "after");
    query::prepare_compare(&mut l, "before", &options("after")).unwrap();
    let report = query::compare(&l, "before", &options("after")).unwrap();
    let (row, detail) = report["details"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, d)| d["status"] == expected)
        .expect("source-supported uncertainty finding");
    assert_eq!(detail["exact_match"], false);
    assert_eq!(
        strings(&detail["from_entity_version_ids"]).len(),
        before.len()
    );
    assert_eq!(
        strings(&detail["candidate_entity_version_ids"]).len(),
        after.len()
    );
    assert!(!report["details"]
        .as_object()
        .unwrap()
        .values()
        .any(|d| d["predicate"] == "RENAMED_FROM"));
    let result = rows(&report["response"], "results")
        .iter()
        .find(|r| r["id"] == *row)
        .unwrap();
    assert_eq!(result["conclusion"], "unknown");
    let handle = text(result, "explanation_handle");
    let proof = query::explain(&l, &handle, 1000, 0).unwrap();
    assert!(proof["correspondence_derivation"]["id"].is_string());
    assert!(!rows(&proof, "correspondence_artifacts").is_empty());
    let paths: std::collections::BTreeSet<_> = rows(&proof, "support")
        .iter()
        .flat_map(|s| rows(s, "sources"))
        .map(|s| text(&s["artifact"], "path"))
        .collect();
    for (name, _) in before.iter().chain(after) {
        assert!(paths.contains(&format!("src/{name}.rs")));
    }
    std::fs::remove_dir_all(br).unwrap();
    std::fs::remove_dir_all(ar).unwrap();
    l.rebuild("before").unwrap();
    l.rebuild("after").unwrap();
    let again = query::compare(&l, "before", &options("after")).unwrap();
    let contents = |r: &Value| {
        r["details"]
            .as_object()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(contents(&again), contents(&report));
    let support = strings(&detail["input_assertion_ids"])[0].clone();
    l.retract(&support, "uncertainty source correction")
        .unwrap();
    l.rebuild("before").unwrap();
    l.rebuild("after").unwrap();
    let corrected = query::compare(&l, "before", &options("after")).unwrap();
    assert!(rows(&corrected["response"], "results")
        .iter()
        .all(|r| !strings(&r["assertion_ids"]).contains(&support)));
    let old = query::explain(&l, &handle, 1000, 0).unwrap();
    assert!(rows(&old, "support")
        .iter()
        .any(|s| s["assertion"]["id"] == support && s["currently_retracted"] == true));
    let mut historical = options("after");
    historical.historical = true;
    let retained = query::compare(&l, "before", &historical).unwrap();
    assert!(retained["details"]
        .as_object()
        .unwrap()
        .values()
        .any(|d| d["status"] == expected));
}
const CHARGE: &str = "pub fn charge(amount: u32) -> u32 { amount + 2 }\n";
const REFUND: &str = "pub fn refund(amount: u32) -> u32 { amount }\n";
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn edited_move_remains_uncertain_not_exact_identity() {
    uncertainty(
        &[("payment", &format!("{CHARGE}{REFUND}"))],
        &[(
            "settlement",
            &format!("{}{REFUND}", CHARGE.replace("+ 2", "+ 3")),
        )],
        "possible_edited_move",
    );
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn split_keeps_all_distinct_candidate_versions() {
    uncertainty(
        &[("payment", &format!("{CHARGE}{REFUND}"))],
        &[("charge_service", CHARGE), ("refund_service", REFUND)],
        "possible_split",
    );
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn merge_keeps_all_distinct_candidate_versions() {
    uncertainty(
        &[("charge_service", CHARGE), ("refund_service", REFUND)],
        &[(
            "settlement",
            &format!("{}{REFUND}", CHARGE.replace("+ 2", "+ 3")),
        )],
        "possible_merge",
    );
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn unsupported_replacement_retains_sources_without_invented_lineage() {
    uncertainty(
        &[(
            "legacy",
            "pub fn encode_date(year: u32) -> String { year.to_string() }\n",
        )],
        &[(
            "queue",
            "pub fn next_job(ids: &[u32]) -> Option<u32> { ids.first().copied() }\n",
        )],
        "unsupported_mapping",
    );
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn unchanged_keys_do_not_gain_fuzzy_lineage() {
    let temp = tempfile::tempdir().unwrap();
    let br = temp.path().join("before");
    let ar = temp.path().join("after");
    source(&br, &[("payment", CHARGE)]);
    source(&ar, &[("payment", REFUND)]);
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    scan(&mut l, &br, "before");
    scan(&mut l, &ar, "after");
    query::prepare_compare(&mut l, "before", &options("after")).unwrap();
    let report: Value = query::compare(&l, "before", &options("after")).unwrap();
    assert_eq!(report["details"], json!({}));
}
