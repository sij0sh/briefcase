use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path, time::Duration};
use briefcase::architecture::{
    capture::Capture,
    contracts::{self, rows, strings, text},
    execution,
    ledger::Ledger,
    mcp, passes,
    query::{self, Options},
    rust::{self, ScanOptions},
};

fn options(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
fn native() -> ScanOptions {
    ScanOptions {
        analyzer: Some(
            std::env::var_os("BRIEFCASE_RUST_ANALYZER")
                .map(Into::into)
                .unwrap_or_else(|| "rust-analyzer".into()),
        ),
        timeout: Duration::from_secs(60),
        features: vec![],
    }
}
fn source(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"execution-tests\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    for (name, code) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, code).unwrap();
    }
}
fn scan(l: &mut Ledger, root: &Path, label: &str) {
    let generation = l.begin("execution-tests", Some(label)).unwrap();
    let prepared = rust::produce(
        Capture::read(root, None).unwrap(),
        "execution-tests",
        generation,
        &native(),
    )
    .unwrap();
    assert!(!rows(&prepared.bundle, "assertions")
        .iter()
        .any(|a| a["proposition"]["predicate"] == "CALLS_STATIC"));
    let view = l.commit(generation, &prepared).unwrap();
    l.publish(&view, true).unwrap();
}
const FILES: &[(&str,&str)] = &[
    ("src/lib.rs","pub mod app;\npub mod payments;\npub mod audit;\npub mod facade;\npub mod inventory;\npub mod catalog;\n"),
    ("src/app.rs","use crate::payments::charge as debit;\nuse crate::facade::charge as facade;\npub fn submit() { debit(); facade(); local(); }\npub fn local() { crate::audit::record(); }\npub fn recursive() { recursive(); }\npub fn reference() { let _f = crate::payments::charge; let _s = \"charge()\"; }\n"),
    ("src/payments.rs","pub fn charge() { crate::audit::record(); }\n"),
    ("src/audit.rs","pub fn record() {}\n"),
    ("src/facade.rs","pub use crate::payments::charge;\n"),
    ("src/inventory.rs","use crate::catalog::lookup;\npub fn stock() { lookup(); }\n"),
    ("src/catalog.rs","pub fn lookup() {}\n"),
];
fn semantic_ids(bundle: &Value) -> BTreeSet<String> {
    rows(bundle, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect()
}
fn proof(l: &Ledger, report: &Value) -> (String, Value) {
    let handle = text(&report["response"]["results"][0], "explanation_handle");
    let value = query::explain(l, &handle, 1000, 0).unwrap();
    (handle, value)
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn native_calls_use_retained_source_and_typed_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, FILES);
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    scan(&mut l, &root, "captured");
    std::fs::remove_dir_all(root).unwrap();
    execution::analyze(&mut l, &options("captured"), &native()).unwrap();
    let v = l.load("captured").unwrap();
    let payloads = l.payloads(&v.bundle).unwrap();
    contracts::validate(&v.bundle, &payloads).unwrap();
    let calls: Vec<_> = rows(&v.bundle, "assertions")
        .iter()
        .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
        .collect();
    assert_eq!(calls.len(), 7);
    assert!(calls
        .iter()
        .all(|a| a["class"] == "evidence" && a["resolution"]["method"] == "compiler"));
    let report =
        query::reachability(&l, &options("captured"), "app::submit", "audit::record").unwrap();
    assert_eq!(report["response"]["results"][0]["conclusion"], "present");
    assert_eq!(
        report["response"]["required_predicates"],
        json!(["CALLS_STATIC"])
    );
    let (_, explanation) = proof(&l, &report);
    assert!(rows(&explanation, "support")
        .iter()
        .any(
            |s| s["assertion"]["proposition"]["predicate"] == "CALLS_STATIC"
                && !rows(s, "sources").is_empty()
        ));
    assert!(rows(&explanation, "support")
        .iter()
        .any(|s| strings(&s["pass_manifest"]["coverage_requirements"])
            .contains(&"CALLS_STATIC:exact_supported".into())));
    let repeat = execution::analyze(&mut l, &options("captured"), &native()).unwrap();
    assert_eq!(repeat["reused"], true);
    assert_eq!(
        semantic_ids(&v.bundle),
        semantic_ids(&l.load("captured").unwrap().bundle)
    );
    let recursive =
        query::reachability(&l, &options("captured"), "app::recursive", "app::recursive").unwrap();
    assert_eq!(recursive["response"]["results"][0]["conclusion"], "present");
    let absent = query::reachability(
        &l,
        &options("captured"),
        "inventory::stock",
        "audit::record",
    )
    .unwrap();
    assert_eq!(absent["response"]["results"][0]["conclusion"], "absent");
    let via_mcp = mcp::dispatch(
        &l,
        &json!({"id":1,"method":"tools/call","params":{"name":"architecture_reachability","arguments":{"view":"captured","from":"app::submit","to":"audit::record"}}}),
    );
    assert_eq!(
        via_mcp["result"]["structuredContent"]["response"],
        report["response"]
    );
    let plan = json!({"schema_version":"1.0","view":"captured","passes":[{"id":"reach","operation":"reachability","dependencies":[]}]});
    assert_eq!(
        passes::inspect(&mut l, "captured", Some(&plan), true).unwrap()["recomputed_and_verified"],
        true
    );
    let imports = json!({"schema_version":"1.0","view":"captured","passes":[{"id":"dep","operation":"dependencies","dependencies":[]},{"id":"scc","operation":"scc","dependencies":["dep"]}]});
    assert_eq!(
        passes::inspect(&mut l, "captured", Some(&imports), true).unwrap()
            ["recomputed_and_verified"],
        true
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn call_only_changes_reuse_unaffected_content_and_corrections_retain_proof() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, FILES);
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    scan(&mut l, &root, "before");
    execution::analyze(&mut l, &options("before"), &native()).unwrap();
    let before = l.load("before").unwrap();
    let report =
        query::reachability(&l, &options("before"), "app::local", "audit::record").unwrap();
    let (handle, _) = proof(&l, &report);
    scan(&mut l, &root, "repeat");
    execution::analyze(&mut l, &options("repeat"), &native()).unwrap();
    assert_eq!(
        semantic_ids(&before.bundle),
        semantic_ids(&l.load("repeat").unwrap().bundle)
    );
    std::fs::write(
        root.join("src/app.rs"),
        FILES[1]
            .1
            .replace("debit(); facade(); local();", "local();")
            .replace("crate::audit::record();", ""),
    )
    .unwrap();
    scan(&mut l, &root, "after");
    execution::analyze(&mut l, &options("after"), &native()).unwrap();
    let changes = passes::inspect(&mut l, "after", None, false).unwrap();
    assert!(!strings(&changes["invalidated_artifact_ids"]).is_empty());
    assert!(!strings(&changes["reused_artifact_ids"]).is_empty());
    let gone = query::reachability(&l, &options("after"), "app::submit", "audit::record").unwrap();
    assert!(!rows(&gone["response"], "results")
        .iter()
        .any(|r| r["conclusion"] == "present"));
    let call = rows(&before.bundle, "assertions")
        .iter()
        .find(|a| {
            a["proposition"]["predicate"] == "CALLS_STATIC"
                && rows(&before.bundle, "entity_versions").iter().any(|e| {
                    e["id"] == a["proposition"]["subject_id"]
                        && text(e, "canonical_key").starts_with("app::local#")
                })
        })
        .unwrap();
    l.retract(&text(call, "id"), "callsite correction").unwrap();
    l.rebuild("before").unwrap();
    let corrected =
        query::reachability(&l, &options("before"), "app::local", "audit::record").unwrap();
    assert_eq!(corrected["response"]["results"][0]["conclusion"], "unknown");
    let old = query::explain(&l, &handle, 1000, 0).unwrap();
    assert!(rows(&old, "support")
        .iter()
        .any(|s| s["assertion"]["id"] == call["id"] && s["currently_retracted"] == true));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn unsupported_dispatch_does_not_inherit_import_completeness() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root,&[("src/lib.rs","pub fn charge() {}\npub fn shadow(charge: fn()) { charge(); }\npub fn closure() { let handler = || {}; handler(); }\npub fn function_value() { let handler = charge; handler(); }\n")]);
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    scan(&mut l, &root, "captured");
    assert_eq!(
        l.load("captured").unwrap().diagnostics["coverage"]["resolution_complete"],
        true
    );
    let result = execution::analyze(&mut l, &options("captured"), &native()).unwrap();
    assert_eq!(result["coverage"]["resolution_complete"], false);
    let v = l.load("captured").unwrap();
    assert!(!rows(&v.bundle, "assertions")
        .iter()
        .any(|a| a["proposition"]["predicate"] == "CALLS_STATIC"));
    let calls = query::callgraph(&l, &options("captured")).unwrap();
    assert_eq!(calls["response"]["resolution_complete"], false);
    assert!(!rows(&calls["response"], "results").is_empty());
    assert!(rows(&calls["response"], "results")
        .iter()
        .all(|r| r["conclusion"] == "unknown"));
    let (_, explanation) = proof(&l, &calls);
    assert!(rows(&explanation, "support")
        .iter()
        .any(|s| s["unresolved_source"]["text"].is_string()));
    let reach = query::reachability(&l, &options("captured"), "shadow", "charge").unwrap();
    assert_eq!(reach["response"]["results"][0]["conclusion"], "unknown");
}
