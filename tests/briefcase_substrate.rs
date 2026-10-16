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
    identity, inference,
    ledger::{Ledger, Prepared},
    mcp, passes,
    query::{self, Options},
    recall, retrieval,
    rust::{self, ScanOptions},
    session,
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
        "[package]\nname=\"substrate-tests\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    for (name, text) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}
fn scan(l: &mut Ledger, root: &Path, label: &str, policy: Option<&Path>, native: bool) -> Prepared {
    let generation = l.begin("substrate-tests", Some(label)).unwrap();
    let p = rust::produce(
        Capture::read(root, policy).unwrap(),
        "substrate-tests",
        generation,
        &ScanOptions {
            analyzer: native.then(|| {
                std::env::var_os("BRIEFCASE_RUST_ANALYZER")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| "rust-analyzer".into())
            }),
            timeout: Duration::from_secs(45),
            features: vec![],
        },
    )
    .unwrap();
    let view = l.commit(generation, &p).unwrap();
    l.publish(&view, true).unwrap();
    p
}
fn ids(b: &Value) -> BTreeSet<String> {
    rows(b, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect()
}
type RetainedSnapshot = (Vec<(String, String)>, Vec<(String, Vec<u8>)>);
fn table_snapshot(path: &Path) -> RetainedSnapshot {
    let conn = Connection::open(path).unwrap();
    let records = conn
        .prepare("SELECT id,body FROM records ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let payloads = conn
        .prepare("SELECT artifact_id,bytes FROM payloads ORDER BY artifact_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    (records, payloads)
}
fn policy(temp: &Path) -> PathBuf {
    let path = temp.join("policy.json");
    std::fs::write(&path,json!({"authority":"test-owner","revision":"1","assignments":{"payments":"Billing"},"rules":[]}).to_string()).unwrap();
    path
}
#[test]
fn retrieval_recovers_both_indexes_and_never_changes_retained_unicode() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let db = temp.path().join("k.db");
    let code = "// e\u{0301} is retained exactly\npub fn charge() {}\n";
    source(
        &root,
        &[
            ("src/lib.rs", "pub mod payments;\n"),
            ("src/payments.rs", code),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &root, "captured", Some(&policy(temp.path())), false);
    let before = table_snapshot(&db);
    let answer = retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).unwrap();
    assert!(rows(&answer, "items")
        .iter()
        .any(|item| text(item, "text") == code
            && !rows(item,"evidence_matches").is_empty()));
    assert!(retrieval::retrieve(&l, "charge", &opts("captured"), 17, 1)
        .unwrap_err()
        .to_string()
        .contains("Response budget"));
    Connection::open(&db)
        .unwrap()
        .execute_batch(
            "DELETE FROM search; DELETE FROM retrieval_units; DELETE FROM retrieval_fts;",
        )
        .unwrap();
    assert!(retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).is_err());
    l.recover().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    let answer = retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).unwrap();
    assert!(rows(&answer, "items")
        .iter()
        .any(|item| text(item, "text") == code));
    assert_eq!(table_snapshot(&db), before);
    let changed = Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE retrieval_fts SET evidence_text='fabricated source'",
            [],
        )
        .unwrap();
    assert!(changed > 0);
    assert!(retrieval::retrieve(&l, "fabricated", &opts("captured"), 16384, 1).is_err());
    l.rebuild("captured").unwrap();
    assert!(retrieval::retrieve(&l, "charge", &opts("captured"), 16384, 1).is_ok());
}
#[test]
fn retrieve_keeps_report_provenance_and_source_within_budget() {
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
    scan(&mut l, &root, "captured", Some(&policy(temp.path())), false);
    let answer = retrieval::retrieve(
        &l,
        "charge",
        &Options {
            limit: 2,
            ..opts("captured")
        },
        5500,
        0,
    )
    .unwrap();
    assert_eq!(answer["operation"],"search");
    assert!(answer["report_id"].as_str().is_some());
    assert!(rows(&answer,"items").iter().any(|i|i["text"].as_str().is_some_and(|t|t.contains("charge"))));
    let proof=briefcase::architecture::service::call(&l,"explain",&json!({
        "report_id":answer["report_id"],"ref":"/items/0","max_bytes":5500})).unwrap();
    assert!(proof["text"].as_str().unwrap().contains("charge"));
    let serialized = serde_json::to_string(&answer).unwrap();
    assert!(
        serialized.len() + 1 <= 5500,
        "retrieve response must stay bounded, got {} bytes",
        serialized.len()
    );
}

#[test]
fn status_summary_inventories_the_captured_view_in_one_call() {
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
    scan(&mut l, &root, "captured", Some(&policy(temp.path())), false);
    let digest =
        briefcase::architecture::service::call(&l, "status", &json!({"summary": true})).unwrap();
    assert_eq!(digest["summary"], true);
    let inventory = &digest["inventory"];
    assert!(
        inventory["captured_files"].as_u64().unwrap() >= 3,
        "{inventory}"
    );
    assert!(inventory["entities_by_kind"]["module"].as_u64().unwrap() >= 1);
    assert!(
        inventory["assertions_by_class"]["evidence"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(inventory["sessions_imported"], 0);
    // No call, data, deployment or runtime analysis ran on this capture.
    let not_analysed: Vec<&str> = inventory["not_analysed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(not_analysed.contains(&"static_calls"), "{not_analysed:?}");
    assert!(
        not_analysed.contains(&"observed_runtime"),
        "{not_analysed:?}"
    );
    // The full diagnostics form is unchanged without the flag.
    let full = briefcase::architecture::service::call(&l, "status", &json!({})).unwrap();
    assert!(full.get("scans").is_some(), "{full}");
}

#[test]
fn recall_session_hits_are_opt_in_labelled_and_freshness_is_stamped() {
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
    scan(&mut l, &root, "captured", Some(&policy(temp.path())), false);
    let raw = std::fs::read("evaluation/sources/sessions/pi/session.jsonl").unwrap();
    session::import(
        &mut l,
        &raw,
        &session::ImportOptions {
            profile: "pi-v1".into(),
            repository_root: "/work/shop".into(),
            label: None,
            repository_id: None,
            source_name: "session.jsonl".into(),
            retention: session::Retention::Full,
        },
    )
    .unwrap();
    let without =
        recall::recall_with_sessions(&l, "rounding", &opts("captured"), 20000, false, false)
            .unwrap();
    assert!(
        without["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["source_kind"] != "session"),
        "{without}"
    );
    assert_eq!(without["coverage"]["sessions"]["included"], false);
    let with = recall::recall_with_sessions(&l, "rounding", &opts("captured"), 20000, false, true)
        .unwrap();
    let hit = with["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["source_kind"] == "session")
        .expect("labelled session hit");
    assert!(text(hit, "text").contains("rounding"), "{hit}");
    assert!(text(hit, "path").starts_with("session:"), "{hit}");
    assert_eq!(with["coverage"]["sessions"]["included"], true);
    assert!(
        with["coverage"]["sessions"]["matched_requests"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert_eq!(with["freshness"]["view_id"], with["view_id"]);
    assert_eq!(with["freshness"]["stale"], false);
    assert!(with["coverage"]["terms"]["matched"]
        .as_array()
        .unwrap()
        .contains(&json!("rounding")));
    // The reading list covers source/commit hits in rank order.
    let sourced = recall::recall(&l, "charge", &opts("captured"), 20000, false).unwrap();
    assert_eq!(sourced["reading_list"][0]["path"], "src/payments.rs");
    // A term absent from retained text is reported, never silently missed.
    let absent = recall::recall_with_sessions(
        &l,
        "rounding zqxwkv",
        &opts("captured"),
        20000,
        false,
        false,
    )
    .unwrap();
    assert!(absent["coverage"]["terms"]["unmatched"]
        .as_array()
        .unwrap()
        .contains(&json!("zqxwkv")));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn module_source_binding_never_uses_a_parent_mod_declaration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            ("src/lib.rs", "pub mod payments;\n"),
            ("src/payments.rs", "pub fn charge() {}\n"),
        ],
    );
    let mut l = Ledger::open(&temp.path().join("k.db")).unwrap();
    let p = scan(&mut l, &root, "captured", None, false);
    let e = rows(&p.bundle, "entity_versions")
        .iter()
        .find(|e| e["kind"] == "module" && e["canonical_key"] == "payments")
        .unwrap();
    let er = rows(&p.bundle, "evidence_refs")
        .iter()
        .find(|er| er["id"] == e["source_evidence_ref_id"])
        .unwrap();
    let artifact = rows(&p.bundle, "artifacts")
        .iter()
        .find(|a| a["id"] == er["artifact_id"])
        .unwrap();
    assert_eq!(artifact["path"], "src/payments.rs");
}
#[test]
fn plans_reject_cycles_mutable_pins_wrong_contracts_and_partial_inputs() {
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
    let p = scan(&mut l, &root, "captured", None, false);
    let before = table_snapshot(&db);
    let cycle = json!({"schema_version":"1.0","view":"captured","passes":[{"id":"dep","operation":"dependencies","dependencies":["scc"]},{"id":"scc","operation":"scc","dependencies":["dep"]}]});
    assert!(passes::validate_plan(&cycle, &p.bundle, "captured")
        .unwrap_err()
        .to_string()
        .starts_with("PASS_CYCLE"));
    let unpinned = json!({"schema_version":"1.0","view":"captured","passes":[{"id":"scc","operation":"scc","dependencies":[{"pass":"dependencies","artifact_id":"latest"}]}]});
    assert!(passes::validate_plan(&unpinned, &p.bundle, "captured")
        .unwrap_err()
        .to_string()
        .starts_with("UNPINNED"));
    let external = text(
        rows(&p.bundle, "pass_manifests")
            .iter()
            .find(|m| m["pass_name"] == "strongly-connected-modules-v1")
            .unwrap(),
        "id",
    );
    let wrong = json!({"schema_version":"1.0","view":"captured","passes":[{"id":"scc","operation":"scc","dependencies":[{"pass":"dependencies","artifact_id":external}]}]});
    assert!(passes::validate_plan(&wrong, &p.bundle, "captured").is_err());
    assert!(passes::inspect(&mut l, "captured", None, true).is_err());
    assert_eq!(table_snapshot(&db), before);
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn requested_failed_analyzer_is_not_a_syntax_success() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, &[("src/lib.rs", "pub fn charge() {}\n")]);
    assert!(rust::produce(
        Capture::read(&root, None).unwrap(),
        "substrate-tests",
        1,
        &ScanOptions {
            analyzer: Some(root.join("nonexistent")),
            ..Default::default()
        }
    )
    .is_err());
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn native_pass_identity_reuse_and_valid_plan_recompute() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let changed = temp.path().join("changed");
    let files = [
        (
            "src/lib.rs",
            "pub mod orders;\npub mod payments;\npub mod inventory;\npub mod catalog;\n",
        ),
        (
            "src/orders.rs",
            "use crate::payments::charge;\npub fn audit() {}\npub fn checkout() {charge();}\n",
        ),
        (
            "src/payments.rs",
            "use crate::orders::audit;\npub fn charge() {}\n",
        ),
        (
            "src/inventory.rs",
            "use crate::catalog::lookup;\npub fn stock() {lookup();}\n",
        ),
        ("src/catalog.rs", "pub fn lookup() {}\n"),
    ];
    source(&root, &files);
    let mut l = Ledger::open(&temp.path().join("k.db")).unwrap();
    let a = scan(&mut l, &root, "before", None, true);
    let repeat = scan(&mut l, &root, "repeat", None, true);
    assert_eq!(ids(&a.bundle), ids(&repeat.bundle));
    assert_eq!(a.bundle["pass_manifests"], repeat.bundle["pass_manifests"]);
    let plan = json!({"schema_version":"1.0","view":"repeat","passes":[{"id":"dependencies","operation":"dependencies","dependencies":[]},{"id":"scc","operation":"scc","dependencies":["dependencies"]}]});
    let run = passes::inspect(&mut l, "repeat", Some(&plan), true).unwrap();
    assert_eq!(run["recomputed_and_verified"], true);
    let rerun = passes::inspect(&mut l, "repeat", None, true).unwrap();
    assert_ne!(run["execution_id"], rerun["execution_id"]);
    let mut newfiles = files;
    newfiles[2] = ("src/payments.rs", "pub fn charge() {}\n");
    source(&changed, &newfiles);
    let b = scan(&mut l, &changed, "after", None, true);
    assert!(!rows(&b.bundle, "assertions")
        .iter()
        .any(|a| a["proposition"]["predicate"] == "IN_CYCLE"));
    let report = passes::inspect(&mut l, "after", None, true).unwrap();
    assert!(!strings(&report["reused_artifact_ids"]).is_empty());
    assert!(!strings(&report["invalidated_artifact_ids"]).is_empty());
    assert_ne!(
        a.bundle["snapshots"][0]["id"],
        b.bundle["snapshots"][0]["id"]
    );
}
fn history_fixture(l: &mut Ledger, temp: &Path, ambiguous: bool) {
    let before = temp.join("before");
    let after = temp.join("after");
    let payment = "pub struct PaymentService;\npub fn charge() {}\n";
    source(
        &before,
        &[
            ("src/lib.rs", "pub mod payment;\npub mod orders;\n"),
            ("src/payment.rs", payment),
            (
                "src/orders.rs",
                "use crate::payment::charge;\npub fn checkout() {charge();}\n",
            ),
        ],
    );
    if ambiguous {
        source(
            &after,
            &[
                (
                    "src/lib.rs",
                    "pub mod payment_v2;\npub mod checkout_payment;\npub mod orders;\n",
                ),
                ("src/payment_v2.rs", payment),
                ("src/checkout_payment.rs", payment),
                (
                    "src/orders.rs",
                    "use crate::payment_v2::charge;\npub fn checkout() {charge();}\n",
                ),
            ],
        );
    } else {
        source(&after,&[("src/lib.rs","pub mod checkout;\npub mod orders;\n"),("src/checkout/mod.rs","pub mod payment_service;\n"),("src/checkout/payment_service.rs",payment),("src/orders.rs","use crate::checkout::payment_service::charge;\npub fn checkout() {charge();}\n")]);
    }
    scan(l, &before, "before", None, false);
    scan(l, &after, "after", None, false);
}
#[test]
fn exact_history_is_retained_and_retraction_does_not_rewrite_old_proof() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("k.db");
    let mut l = Ledger::open(&db).unwrap();
    history_fixture(&mut l, temp.path(), false);
    query::prepare_compare(&mut l, "before", &opts("after")).unwrap();
    let report = query::compare(&l, "before", &opts("after")).unwrap();
    let (row, detail) = report["details"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, d)| d["predicate"] == "RENAMED_FROM")
        .unwrap();
    let lineage = text(detail, "assertion_id");
    let handle = rows(&report["response"], "results")
        .iter()
        .find(|r| r["id"] == *row)
        .unwrap()["explanation_handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = table_snapshot(&db);
    l.rebuild("after").unwrap();
    query::prepare_compare(&mut l, "before", &opts("after")).unwrap();
    assert_eq!(table_snapshot(&db), before);
    l.retract(&lineage, "correspondence correction").unwrap();
    l.rebuild("after").unwrap();
    l.rebuild("before").unwrap();
    let current = query::compare(&l, "before", &opts("after")).unwrap();
    assert!(!rows(&current["response"], "results")
        .iter()
        .any(|r| strings(&r["assertion_ids"]).contains(&lineage)));
    let proof = query::explain(&l, &handle, 100, 0).unwrap();
    assert!(rows(&proof, "support")
        .iter()
        .any(|s| s["assertion"]["id"] == lineage && s["currently_retracted"] == true));
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn duplicate_content_stays_ambiguous_without_exact_lineage() {
    let temp = tempfile::tempdir().unwrap();
    let mut l = Ledger::open(&temp.path().join("k.db")).unwrap();
    history_fixture(&mut l, temp.path(), true);
    query::prepare_compare(&mut l, "before", &opts("after")).unwrap();
    let report = query::compare(&l, "before", &opts("after")).unwrap();
    let finding = report["details"]
        .as_object()
        .unwrap()
        .values()
        .find(|d| d["change"] == "ambiguous_correspondence")
        .unwrap();
    assert_eq!(strings(&finding["candidate_entity_version_ids"]).len(), 2);
    assert_eq!(finding["exact_match"], false);
    assert!(!report["details"]
        .as_object()
        .unwrap()
        .values()
        .any(|d| d["predicate"] == "RENAMED_FROM"));
}
fn provider(temp: &Path) -> (PathBuf, PathBuf) {
    let program = temp.join("provider.py");
    std::fs::write(&program,r#"import json, sys
request=json.load(sys.stdin)
packet=request['packet']
print(json.dumps({'proposals':[{'target_id':request['target_id'],'predicate':'BELONGS_TO_DOMAIN','domain':'Checkout','supports':packet['assertion_ids'][:1],'score':{'value':0.99,'semantics':'uncalibrated'},'limitations':['test only']}],'abstained':False,'limitations':['not model quality']}))
"#).unwrap();
    let config = temp.join("provider.json");
    std::fs::write(&config,json!({"schema_version":"1.0","name":"test-local","model_name":"domain-test","model_version":"1","settings":{"score_semantics":"uncalibrated"},"command":{"program":"/usr/bin/python3","args":[program]},"timeout_seconds":5,"opaque":false,"deterministic":false}).to_string()).unwrap();
    let taxonomy = temp.join("taxonomy.json");
    std::fs::write(
        &taxonomy,
        json!({"schema_version":"1.0","version":"test-v1","domains":["Checkout","Billing"]})
            .to_string(),
    )
    .unwrap();
    (config, taxonomy)
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn domain_claim_retains_exact_packet_and_stays_out_of_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let db = temp.path().join("k.db");
    let code = "// e\u{0301}: source must not be normalized\npub fn charge() {}\n";
    source(
        &root,
        &[
            ("src/lib.rs", "pub mod payments;\n"),
            ("src/payments.rs", code),
        ],
    );
    let mut l = Ledger::open(&db).unwrap();
    scan(&mut l, &root, "captured", Some(&policy(temp.path())), false);
    let (provider, taxonomy) = provider(temp.path());
    let o = Options {
        component: Some("payments".into()),
        ..opts("captured")
    };
    let result = inference::classify(&mut l, &o, &provider, &taxonomy, 16, 16384).unwrap();
    let claim = strings(&result["claim_ids"])[0].clone();
    let v = l.load("captured").unwrap();
    let payloads = l.payloads(&v.bundle).unwrap();
    contracts::validate(&v.bundle, &payloads).unwrap();
    let packet = rows(&v.bundle, "input_packets")
        .iter()
        .find(|p| p["id"] == result["packet_id"])
        .unwrap();
    let wire = identity::parse(&payloads[&text(packet, "artifact_id")]).unwrap();
    let inference = rows(&v.bundle, "inferences")
        .iter()
        .find(|i| i["id"] == result["inference_id"])
        .unwrap();
    let prompt_bytes = &payloads[&text(inference, "prompt_artifact_id")];
    let prompt = std::str::from_utf8(prompt_bytes).unwrap();
    assert_eq!(
        wire["prompt_version"],
        "briefcase-domain-classification-v2"
    );
    assert!(prompt.starts_with("briefcase-domain-classification-v2\n"));
    assert!(prompt.contains("source-level business responsibility"));
    assert!(prompt.contains("Abstain for generic helpers"));
    assert_eq!(
        inference["prompt_digest"],
        identity::bytes_digest(prompt_bytes)
    );
    let request = identity::parse(&payloads[&text(inference, "request_artifact_id")]).unwrap();
    assert_eq!(request["prompt"], prompt);
    assert!(rows(&wire, "sources")
        .iter()
        .any(|s| text(s, "evidence_text") == code));
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
    let domains = query::domains(&l, &o).unwrap();
    assert!(rows(&domains["response"], "results")
        .iter()
        .any(|r| strings(&r["assertion_ids"]).contains(&claim)));
    let drift = query::drift(&l, &o).unwrap();
    assert!(!rows(&drift["response"], "results").is_empty());
    let handle = drift["response"]["results"][0]["explanation_handle"]
        .as_str()
        .unwrap();
    let proof = query::explain(&l, handle, 100, 0).unwrap();
    assert!(rows(&proof, "support")
        .iter()
        .any(|s| s["assertion"]["class"] == "claim"));
    let mcp = mcp::dispatch(
        &l,
        &json!({"id":1,"method":"tools/call","params":{"name":"architecture_drift","arguments":{"view":"captured","component":"payments"}}}),
    );
    assert_eq!(
        mcp["result"]["structuredContent"]["response"],
        drift["response"]
    );
    assert!(identity::digest(&json!({"score":0.99})).is_err());
    let mut broken = v.bundle.clone();
    broken["assertions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|a| a["id"] == claim)
        .unwrap()["model_score"]["value"] = json!(0.5);
    assert!(contracts::validate(&broken, &payloads).is_err());
    let support = strings(
        &rows(&v.bundle, "assertions")
            .iter()
            .find(|a| a["id"] == claim)
            .unwrap()["supports"],
    )[0]
    .clone();
    l.retract(&support, "support correction").unwrap();
    l.rebuild("captured").unwrap();
    assert!(rows(&query::drift(&l, &o).unwrap()["response"], "results").is_empty());
    assert!(
        rows(&query::explain(&l, handle, 100, 0).unwrap(), "support")
            .iter()
            .any(|s| s["assertion"]["id"] == claim && s["currently_retracted"] == true)
    );
}
