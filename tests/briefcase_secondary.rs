use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};
use briefcase::architecture::{
    capture::Capture,
    contracts::{self, rows, strings, text},
    identity,
    ledger::Ledger,
    lineage,
    query::{self, Options},
    retrieval,
    rust::{self, ScanOptions},
};

fn opts(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
fn source(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"secondary-tests\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    for (name, text) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}
fn scan(l: &mut Ledger, root: &Path, label: &str) {
    let generation = l.begin("secondary-tests", Some(label)).unwrap();
    let p = rust::produce(
        Capture::read(root, None).unwrap(),
        "secondary-tests",
        generation,
        &ScanOptions {
            analyzer: None,
            timeout: Duration::from_secs(45),
            features: vec![],
        },
    )
    .unwrap();
    let view = l.commit(generation, &p).unwrap();
    l.publish(&view, true).unwrap();
}
fn records_snapshot(db: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(db).unwrap();
    let mut statement = conn
        .prepare("SELECT id,body FROM records ORDER BY id")
        .unwrap();
    statement
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let body: String = r.get(1)?;
            Ok((id, body))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn lineage_provider(temp: &Path, name: &str, script: &str) -> PathBuf {
    let program = temp.join(format!("{name}.py"));
    std::fs::write(&program, script).unwrap();
    let config = temp.join(format!("{name}.json"));
    std::fs::write(&config, json!({"schema_version":"1.0","name":name,"model_name":"lineage-test","model_version":"1","settings":{},"command":{"program":"/usr/bin/python3","args":[program]},"timeout_seconds":10,"opaque":false,"deterministic":false}).to_string()).unwrap();
    config
}
const PROPOSE_FIRST_PAIR: &str = r#"import json, sys
request=json.load(sys.stdin)
packet=request['packet']
before=[p for p in packet['participants'] if p['side']=='before'][0]
after=[p for p in packet['participants'] if p['side']=='after'][0]
print(json.dumps({'proposals':[{'subject_id':after['entity_version_id'],'object_id':before['entity_version_id'],'predicate':'LIKELY_SAME_ENTITY','supports':packet['assertion_ids'][:2],'score':{'value':0.7,'semantics':'uncalibrated'},'limitations':['test only']}],'abstained':False,'limitations':[]}))
"#;
const PROPOSE_WRONG_DIRECTION: &str = r#"import json, sys
request=json.load(sys.stdin)
packet=request['packet']
before=[p for p in packet['participants'] if p['side']=='before'][0]
after=[p for p in packet['participants'] if p['side']=='after'][0]
print(json.dumps({'proposals':[{'subject_id':before['entity_version_id'],'object_id':after['entity_version_id'],'predicate':'LIKELY_SAME_ENTITY','supports':packet['assertion_ids'][:1],'limitations':[]}],'abstained':False,'limitations':[]}))
"#;

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn retrieval_compatibility_uses_shared_search_and_pinned_reports() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let db = temp.path().join("k.db");
    source(
        &root,
        &[
            ("src/lib.rs", "pub mod payments;\n"),
            ("src/payments.rs", "pub fn charge() {}\n"),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &root, "captured");
    let first = retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).unwrap();
    let second = retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).unwrap();
    assert_eq!(first["operation"], "search");
    assert!(first["report_id"].as_str().is_some());
    assert_eq!(first["items"], second["items"]);
    assert_eq!(first["report_id"], second["report_id"]);
    let retained = briefcase::architecture::service::call(&l, "page",
        &json!({"report_id":first["report_id"],"limit":1000})).unwrap();
    assert_eq!(first["items"], retained["items"]);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn retrieval_rebuild_preserves_identity_and_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let db = temp.path().join("k.db");
    source(
        &root,
        &[
            ("src/lib.rs", "pub mod payments;\n"),
            ("src/payments.rs", "pub fn charge() {}\n"),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &root, "captured");
    let before = retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).unwrap();
    let records = records_snapshot(&db);
    let item_ids: BTreeSet<String> = rows(&before, "items")
        .iter()
        .flat_map(|i| rows(i,"evidence_matches").iter().map(|e|text(e,"assertion_id")))
        .collect();
    Connection::open(&db)
        .unwrap()
        .execute_batch(
            "DELETE FROM search; DELETE FROM retrieval_units; DELETE FROM retrieval_fts;",
        )
        .unwrap();
    assert!(retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).is_err());
    l.rebuild("captured").unwrap();
    let after = retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).unwrap();
    assert_eq!(before["items"], after["items"]);
    assert_eq!(records, records_snapshot(&db));
    let v = l.load("captured").unwrap();
    for id in item_ids {
        assert!(v.eligible.contains(&id));
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn likely_lineage_claim_labels_certainty_without_touching_exact() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let changed = temp.path().join("changed");
    let db = temp.path().join("k.db");
    source(
        &base,
        &[
            ("src/lib.rs", "pub mod old;\n"),
            ("src/old.rs", "pub fn charge() {}\npub fn audit() {}\n"),
        ],
    );
    source(
        &changed,
        &[
            ("src/lib.rs", "pub mod new;\n"),
            (
                "src/new.rs",
                "pub fn charge() {}\n// edited implementation\npub fn audit_log() {}\n",
            ),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &base, "base");
    scan(&mut l, &changed, "changed");
    let o = opts("changed");
    query::prepare_compare(&mut l, "base", &o).unwrap();
    // Edited movement is not exact correspondence.
    let exact = query::history_lineage(&l, "base", &o, "exact").unwrap();
    assert!(rows(&exact["response"], "results").is_empty());
    let provider = lineage_provider(temp.path(), "lineage", PROPOSE_FIRST_PAIR);
    let result = lineage::classify(&mut l, "base", &o, &provider, 65536).unwrap();
    assert_eq!(result["abstained"], false);
    let claim = strings(&result["claim_ids"])[0].clone();
    let likely = query::history_lineage(&l, "base", &o, "likely").unwrap();
    let details: Vec<&Value> = likely["details"].as_object().unwrap().values().collect();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["predicate"], "LIKELY_SAME_ENTITY");
    assert_eq!(details[0]["certainty_class"], "inferred_continuity");
    assert_eq!(details[0]["from"], "old");
    assert_eq!(details[0]["to"], "new");
    assert!(strings(&likely["response"]["results"][0]["assertion_ids"]).contains(&claim));
    // Combined history keeps both classes labeled and distinct.
    let combined = query::history_lineage(&l, "base", &o, "combined").unwrap();
    let classes: BTreeSet<String> = combined["details"]
        .as_object()
        .unwrap()
        .values()
        .map(|d| text(d, "certainty_class"))
        .collect();
    assert!(classes.contains("inferred_continuity"));
    assert!(classes.contains("ambiguous_candidates"));
    assert!(
        !combined["details"]
            .as_object()
            .unwrap()
            .values()
            .any(|d| d["predicate"] == "RENAMED_FROM"
                && d["certainty_class"] == "inferred_continuity")
    );
    // The Claim stays out of the evidence view.
    let evidence = query::dependencies(
        &l,
        &Options {
            include_structure: true,
            ..o.clone()
        },
    )
    .unwrap();
    assert!(!rows(&evidence["response"], "results")
        .iter()
        .any(|r| strings(&r["assertion_ids"]).contains(&claim)));
    // Explanation retains the inference and its retained sources.
    let handle = likely["response"]["results"][0]["explanation_handle"]
        .as_str()
        .unwrap();
    let proof = query::explain(&l, handle, 100, 0).unwrap();
    assert!(rows(&proof, "support")
        .iter()
        .any(|s| s["assertion"]["class"] == "claim"
            && s["assertion"]["proposition"]["predicate"] == "LIKELY_SAME_ENTITY"));
    assert!(rows(&proof, "support")
        .iter()
        .any(|s| !rows(s, "sources").is_empty()));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn lineage_validator_rejects_direction_and_exact_contradictions() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let changed = temp.path().join("changed");
    let db = temp.path().join("k.db");
    source(
        &base,
        &[
            ("src/lib.rs", "pub mod old;\n"),
            ("src/old.rs", "pub fn charge() {}\n"),
        ],
    );
    source(
        &changed,
        &[
            ("src/lib.rs", "pub mod new;\n"),
            ("src/new.rs", "pub fn charge() {}\npub fn extra() {}\n"),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &base, "base");
    scan(&mut l, &changed, "changed");
    let o = opts("changed");
    query::prepare_compare(&mut l, "base", &o).unwrap();
    let bad = lineage_provider(temp.path(), "bad", PROPOSE_WRONG_DIRECTION);
    let error = lineage::classify(&mut l, "base", &o, &bad, 65536).unwrap_err();
    assert!(
        error.to_string().contains("INFERENCE_REJECTED"),
        "unexpected error: {error}"
    );
    // A pure rename stays exact-only: no candidates, no Claim.
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let changed = temp.path().join("changed");
    let db = temp.path().join("k.db");
    source(
        &base,
        &[
            ("src/lib.rs", "pub mod old;\n"),
            ("src/old.rs", "pub fn charge() {}\n"),
        ],
    );
    source(
        &changed,
        &[
            ("src/lib.rs", "pub mod new;\n"),
            ("src/new.rs", "pub fn charge() {}\n"),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &base, "base");
    scan(&mut l, &changed, "changed");
    query::prepare_compare(&mut l, "base", &o).unwrap();
    let exact = query::history_lineage(&l, "base", &o, "exact").unwrap();
    let details: Vec<&Value> = exact["details"].as_object().unwrap().values().collect();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["predicate"], "RENAMED_FROM");
    assert_eq!(details[0]["certainty_class"], "exact_content_move");
    let provider = lineage_provider(temp.path(), "lineage", PROPOSE_FIRST_PAIR);
    let error = lineage::classify(&mut l, "base", &o, &provider, 65536).unwrap_err();
    assert!(
        error.to_string().contains("INPUT_PACKET"),
        "unexpected error: {error}"
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn ambiguous_successors_keep_explicit_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let changed = temp.path().join("changed");
    let db = temp.path().join("k.db");
    source(
        &base,
        &[
            ("src/lib.rs", "pub mod old;\n"),
            ("src/old.rs", "pub fn charge() {}\n"),
        ],
    );
    source(
        &changed,
        &[
            ("src/lib.rs", "pub mod a;\npub mod b;\n"),
            ("src/a.rs", "pub fn charge() {}\npub fn extra_a() {}\n"),
            ("src/b.rs", "pub fn charge() {}\npub fn extra_b() {}\n"),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &base, "base");
    scan(&mut l, &changed, "changed");
    let o = opts("changed");
    query::prepare_compare(&mut l, "base", &o).unwrap();
    // Two plausible successors: no deterministic correspondence.
    let exact = query::history_lineage(&l, "base", &o, "exact").unwrap();
    assert!(rows(&exact["response"], "results").is_empty());
    let ambiguous = query::history_lineage(&l, "base", &o, "ambiguous").unwrap();
    assert!(!rows(&ambiguous["response"], "results").is_empty());
    assert!(ambiguous["details"]
        .as_object()
        .unwrap()
        .values()
        .all(|d| d["certainty_class"] == "ambiguous_candidates"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn reserved_execution_predicates_stay_inadmissible() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let db = temp.path().join("k.db");
    source(
        &root,
        &[
            ("src/lib.rs", "pub mod payments;\n"),
            ("src/payments.rs", "pub fn charge() {}\n"),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &root, "captured");
    let v = l.load("captured").unwrap();
    let payloads = l.payloads(&v.bundle).unwrap();
    for predicate in [
        "METHOD_CALLS",
        "CONSTRUCTS",
        "HANDLES_EVENT",
        "REGISTERS_CALLBACK",
        "FRAMEWORK_ENTRYPOINT",
        "SUBSCRIBES",
    ] {
        let mut bundle = v.bundle.clone();
        let target = rows(&bundle, "assertions")
            .iter()
            .find(|a| a["class"] == "evidence")
            .unwrap()
            .clone();
        let mut forged = target.clone();
        forged["proposition"]["predicate"] = json!(predicate);
        forged["id"] = json!(identity::id("assertion", &{
            let mut input = forged.clone();
            input.as_object_mut().unwrap().remove("id");
            input
        })
        .unwrap());
        bundle["assertions"].as_array_mut().unwrap().push(forged);
        let error = contracts::validate(&bundle, &payloads).unwrap_err();
        assert!(
            error.to_string().contains("PREDICATE"),
            "{predicate}: unexpected error: {error}"
        );
    }
}
