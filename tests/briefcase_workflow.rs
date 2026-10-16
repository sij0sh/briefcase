use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};
use briefcase::architecture::{
    capture::Capture,
    contracts::{rows, text},
    ledger::{Ledger, Prepared},
    mcp,
    query::{self, Options},
    rust::{self, ScanOptions},
};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn produced(source: &str, generation: u64, semantic: bool) -> Prepared {
    let capture = Capture::read(
        &root().join(format!("evaluation/sources/rust/{source}")),
        Some(&root().join("evaluation/sources/policy/architecture.json")),
    )
    .unwrap();
    let exe = std::env::var_os("BRIEFCASE_RUST_ANALYZER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rust-analyzer"));
    rust::produce(
        capture,
        "gold-rust",
        generation,
        &ScanOptions {
            analyzer: semantic.then_some(exe),
            timeout: Duration::from_secs(45),
            features: vec![],
        },
    )
    .unwrap()
}
fn relationships(b: &Value, predicate: &str) -> BTreeSet<(String, String)> {
    let key = |id: &Value| {
        text(
            rows(b, "entity_versions")
                .iter()
                .find(|e| e["id"] == *id)
                .unwrap(),
            "canonical_key",
        )
    };
    rows(b, "assertions")
        .iter()
        .filter(|a| a["proposition"]["predicate"] == predicate)
        .map(|a| {
            (
                key(&a["proposition"]["subject_id"]),
                key(&a["proposition"]["object"]["entity_version_id"]),
            )
        })
        .collect()
}
fn opts(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
#[test]
fn real_rust_two_revision_policy_explanation_retraction_workflow() {
    let temp = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    let g1 = ledger.begin("gold-rust", Some("base")).unwrap();
    let p1 = produced("base", g1, true);
    assert_eq!(
        relationships(&p1.bundle, "IMPORTS"),
        BTreeSet::from([("orders".into(), "payments".into())])
    );
    assert_eq!(
        p1.bundle["runs"][0]["coverage"]["resolution_complete"],
        true
    );
    assert!(rows(&p1.bundle, "assertions")
        .iter()
        .all(|a| a["proposition"]["predicate"] != "CALLS_STATIC"));
    let v1 = ledger.commit(g1, &p1).unwrap();
    ledger.publish(&v1, true).unwrap();
    let base_policy = query::policy(&ledger, &opts("base")).unwrap();
    assert_eq!(base_policy["response"]["results"][0]["conclusion"], "pass");
    let g2 = ledger.begin("gold-rust", Some("changed")).unwrap();
    let p2 = produced("changed", g2, true);
    assert_eq!(
        relationships(&p2.bundle, "IMPORTS"),
        BTreeSet::from([
            ("orders".into(), "payments".into()),
            ("payments".into(), "orders".into())
        ])
    );
    assert_eq!(
        rows(&p2.bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "IN_CYCLE")
            .count(),
        2
    );
    let v2 = ledger.commit(g2, &p2).unwrap();
    ledger.publish(&v2, true).unwrap();
    let component = query::dependencies(
        &ledger,
        &Options {
            component: Some("orders".into()),
            ..opts("changed")
        },
    )
    .unwrap();
    let cycles: Vec<_> = component["details"]
        .as_object()
        .unwrap()
        .values()
        .filter(|d| d["predicate"] == "IN_CYCLE")
        .collect();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0]["component"], "orders");
    let diff = query::compare(&ledger, "base", &opts("changed")).unwrap();
    let details = diff["details"].as_object().unwrap();
    assert_eq!(details.len(), 1);
    let change = details.values().next().unwrap();
    assert_eq!(change["from"], "payments");
    assert_eq!(change["to"], "orders");
    assert_eq!(change["change"], "added");
    let policy = query::policy(&ledger, &opts("changed")).unwrap();
    assert_eq!(policy["response"]["results"][0]["conclusion"], "violation");
    let handle = policy["response"]["results"][0]["explanation_handle"]
        .as_str()
        .unwrap();
    let proof = query::explain(&ledger, handle, 100, 0).unwrap();
    assert_eq!(proof["conflict_count"], 1);
    assert_eq!(
        proof["known_conflicts"][0]["kind"],
        "declared_prohibition_vs_known_import_path"
    );
    assert!(proof["support"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["sources"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["text"]
                .as_str()
                .is_some_and(|t| t.contains("use crate::orders::audit"))))));
    let page = query::dependencies(
        &ledger,
        &Options {
            limit: 1,
            ..opts("changed")
        },
    )
    .unwrap();
    let token = page["response"]["continuation_token"].as_str().unwrap();
    assert_eq!(
        query::continuation(&ledger, token).unwrap()["pinned_views"],
        json!([v2])
    );
    let reverse = rows(&p2.bundle, "assertions")
        .iter()
        .find(|a| {
            a["proposition"]["predicate"] == "IMPORTS"
                && relationships(
                    &json!({"entity_versions":p2.bundle["entity_versions"],"assertions":[a]}),
                    "IMPORTS",
                )
                .contains(&("payments".into(), "orders".into()))
        })
        .unwrap();
    ledger
        .retract(&text(reverse, "id"), "adapter correction")
        .unwrap();
    assert!(query::policy(&ledger, &opts("changed"))
        .unwrap_err()
        .to_string()
        .starts_with("FRESHNESS"));
    ledger.rebuild("changed").unwrap();
    let corrected = query::policy(&ledger, &opts("changed")).unwrap();
    assert_eq!(corrected["response"]["results"][0]["conclusion"], "unknown");
    let historical = query::explain(&ledger, handle, 100, 0).unwrap();
    assert!(historical["support"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["currently_retracted"] == true));
    assert!(query::continuation(&ledger, token).is_err());
}
#[test]
fn syntax_determinism_is_checked_without_the_analyzer() {
    let first = produced("changed", 1, false);
    let second = produced("changed", 2, false);
    assert_eq!(first.bundle["snapshots"], second.bundle["snapshots"]);
    assert_eq!(first.bundle["assertions"], second.bundle["assertions"]);
}
#[test]
#[ignore = "conformance: repeated real-analyzer determinism"]
fn real_rust_repeat_determinism_conformance() {
    let first = produced("changed", 1, true);
    let second = produced("changed", 2, true);
    assert_eq!(first.bundle["snapshots"], second.bundle["snapshots"]);
    assert_eq!(first.bundle["assertions"], second.bundle["assertions"]);
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn syntax_mode_keeps_source_and_policy_without_false_compliance() {
    let temp = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    let generation = ledger.begin("gold-rust", Some("syntax")).unwrap();
    let p = produced("base", generation, false);
    let view = ledger.commit(generation, &p).unwrap();
    ledger.publish(&view, true).unwrap();
    let policy = query::policy(&ledger, &Options::default()).unwrap();
    assert_eq!(policy["response"]["results"][0]["conclusion"], "unknown");
    let unknown = query::unresolved(&ledger, &Options::default()).unwrap();
    assert!(!rows(&unknown["response"], "results").is_empty());
    let handle = unknown["response"]["results"][0]["explanation_handle"]
        .as_str()
        .unwrap();
    let e = query::explain(&ledger, handle, 1, 0).unwrap();
    assert!(e["support"][0]["unresolved_source"]["text"]
        .as_str()
        .unwrap()
        .contains("use crate::payments"));
    let mcp_result = mcp::dispatch(
        &ledger,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"architecture_policy","arguments":{}}}),
    );
    assert_eq!(
        mcp_result["result"]["structuredContent"]["response"],
        policy["response"]
    );
    let denied = mcp::dispatch(
        &ledger,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"commit","arguments":{"capability":"evidence_writer"}}}),
    );
    assert_eq!(denied["result"]["isError"], true);
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn replay_stale_publication_and_partial_preparation_are_durable() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("knowledge.db");
    let mut l = Ledger::open(&db).unwrap();
    let g1 = l.begin("gold-rust", Some("old")).unwrap();
    let p1 = produced("base", g1, false);
    let old = l.commit(g1, &p1).unwrap();
    let mark = l.watermark().unwrap();
    assert_eq!(l.commit(g1, &p1).unwrap(), old);
    assert_eq!(l.watermark().unwrap(), mark);
    let mut conflict = produced("base", g1, false);
    conflict.bundle["fixture_label"] = json!("different bytes");
    assert!(l
        .commit(g1, &conflict)
        .unwrap_err()
        .to_string()
        .starts_with("CONFLICTING_REPLAY"));
    let g2 = l.begin("gold-rust", Some("new")).unwrap();
    let p2 = produced("changed", g2, false);
    let new = l.commit(g2, &p2).unwrap();
    l.publish(&new, true).unwrap();
    assert!(l
        .publish(&old, true)
        .unwrap_err()
        .to_string()
        .starts_with("STALE_PUBLICATION"));
    l.publish(&old, false).unwrap();
    let g3 = l.begin("gold-rust", Some("interrupted")).unwrap();
    drop(l);
    let mut l = Ledger::open(&db).unwrap();
    let recovery = l.recover().unwrap();
    assert_eq!(recovery["status"]["current_view"], new);
    assert_eq!(recovery["status"]["scans"][2]["status"], "interrupted");
    assert_eq!(recovery["status"]["scans"][2]["generation"], g3);
    assert!(query::dependencies(&l, &Options::default()).is_err());
    assert!(query::dependencies(&l, &opts("new")).is_ok());
}
#[test]
fn mcp_frames_are_json_rpc_and_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let l = Ledger::open(&temp.path().join("k.db")).unwrap();
    let input=b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
    let mut output = Vec::new();
    mcp::serve(&l, std::io::Cursor::new(input), &mut output).unwrap();
    let lines: Vec<_> = output
        .split(|b| *b == b'\n')
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let initialized: serde_json::Value = serde_json::from_slice(lines[0]).unwrap();
    assert_eq!(initialized["result"]["serverInfo"]["name"], "briefcase");
}

#[test]
#[ignore = "Runs the installed rust-analyzer; required by scripts/verify-mvp"]
fn native_alias_group_reexport_cfg_and_dynamic_capability_gate() {
    let capture =
        Capture::read(&root().join("evaluation/sources/rust/capabilities"), None).unwrap();
    let p = rust::produce(capture, "rust-capabilities", 1, &ScanOptions::default()).unwrap();
    let gold: Value = serde_json::from_slice(&std::fs::read(root().join("evaluation/gold/rust-capability-expectations.json")).expect("Original evaluation fixture missing from supplied archive"))
    .unwrap();
    let expected: BTreeSet<_> = gold["required_imports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p[0].as_str().unwrap().to_owned(),
                p[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(relationships(&p.bundle, "IMPORTS"), expected);
    assert_eq!(
        p.bundle["runs"][0]["coverage"]["resolution_complete"],
        false
    );
    assert!(
        rows(&p.bundle, "unresolved_references")
            .iter()
            .filter(|r| r["reason"] == "trait_dispatch")
            .count()
            >= 2
    );
    assert!(rows(&p.bundle, "assertions")
        .iter()
        .all(|a| a["proposition"]["predicate"] != "CALLS_STATIC"));
    let nested: Vec<_> = rows(&p.bundle, "entity_versions")
        .iter()
        .filter(|e| {
            e["canonical_key"]
                .as_str()
                .is_some_and(|s| s.contains("::same#"))
        })
        .collect();
    assert_eq!(nested.len(), 2);
    assert_ne!(nested[0]["id"], nested[1]["id"]);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn describe_and_large_unicode_source_proof_are_bounded_and_pageable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"proof\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    let code = format!("// {}\npub fn entry() {{}}\n", "é".repeat(3500));
    std::fs::write(root.join("src/lib.rs"), &code).unwrap();
    let mut l = Ledger::open(&temp.path().join("k.db")).unwrap();
    let generation = l.begin("proof", None).unwrap();
    let p = rust::produce(
        Capture::read(&root, None).unwrap(),
        "proof",
        generation,
        &ScanOptions {
            analyzer: None,
            ..Default::default()
        },
    )
    .unwrap();
    let view = l.commit(generation, &p).unwrap();
    l.publish(&view, true).unwrap();
    let described = query::dependencies(
        &l,
        &Options {
            include_structure: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(described["details"]
        .as_object()
        .unwrap()
        .values()
        .any(|d| d["predicate"] == "DECLARES"));
    let handle = described["response"]["results"][0]["explanation_handle"]
        .as_str()
        .unwrap();
    let first = query::explain(&l, handle, 1, 0).unwrap();
    let bytes = &first["support"][0]["sources"][0];
    assert!(bytes["text"].as_str().unwrap().len() <= 4096);
    let next = bytes["next_source_offset"].as_u64().unwrap() as usize;
    let second = query::explain_page(&l, handle, 1, 0, next).unwrap();
    assert_eq!(
        format!(
            "{}{}",
            bytes["text"].as_str().unwrap(),
            second["support"][0]["sources"][0]["text"].as_str().unwrap()
        ),
        code
    );
    let unknown = query::dependencies(
        &l,
        &Options {
            component: Some("absent".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(unknown["response"]["outcome"], "unknown");
    l.begin("proof", Some("pending")).unwrap();
    let partial = query::dependencies(
        &l,
        &Options {
            include_structure: true,
            allow_partial: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_ne!(partial["response"]["outcome"], "complete");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn declared_policy_source_is_bounded_and_pageable() {
    let temp = tempfile::tempdir().unwrap();
    let policy_path = temp.path().join("policy.json");
    let policy_text = format!(
        "{}{}",
        std::fs::read_to_string(root().join("evaluation/sources/policy/architecture.json"))
            .unwrap(),
        " \n".repeat(3000)
    );
    std::fs::write(&policy_path, &policy_text).unwrap();
    let mut l = Ledger::open(&temp.path().join("k.db")).unwrap();
    let generation = l.begin("policy-pages", None).unwrap();
    let p = rust::produce(
        Capture::read(
            &root().join("evaluation/sources/rust/base"),
            Some(&policy_path),
        )
        .unwrap(),
        "policy-pages",
        generation,
        &ScanOptions {
            analyzer: None,
            ..Default::default()
        },
    )
    .unwrap();
    let view = l.commit(generation, &p).unwrap();
    l.publish(&view, true).unwrap();
    let answer = query::policy(&l, &Options::default()).unwrap();
    let handle = answer["response"]["results"][0]["explanation_handle"]
        .as_str()
        .unwrap();
    let proof = query::explain(&l, handle, 1, 0).unwrap();
    let first = &proof["support"][0];
    assert!(first["policy_source"].as_str().unwrap().len() <= 4096);
    let next = first["policy_source_page"]["next_source_offset"]
        .as_u64()
        .unwrap() as usize;
    let second = query::explain_page(&l, handle, 1, 0, next).unwrap();
    assert_eq!(
        format!(
            "{}{}",
            first["policy_source"].as_str().unwrap(),
            second["support"][0]["policy_source"].as_str().unwrap()
        ),
        policy_text
    );
}
