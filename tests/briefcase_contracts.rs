use serde_json::Value;
use std::path::Path;
use briefcase::architecture::{
    contracts::{self, rows, strings, text, Capability, Payloads},
    identity,
};
fn read(name: &str) -> Value {
    identity::parse(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("evaluation/fixtures")
                .join(name),
        )
        .unwrap(),
    )
    .unwrap()
}
fn payloads(b: &Value) -> Payloads {
    rows(b, "artifacts")
        .iter()
        .filter(|a| a["availability"] == "available")
        .map(|a| {
            (
                text(a, "id"),
                std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(text(a, "path"))).unwrap(),
            )
        })
        .collect()
}
fn capability(name: &str) -> Capability {
    match name {
        "evidence_writer" => Capability::Evidence,
        "derived_writer" => Capability::Derived,
        "claim_writer" => Capability::Claim,
        "declaration_writer" => Capability::Declaration,
        _ => panic!("Unknown test capability"),
    }
}
#[test]
fn all_authored_contracts_pass_in_production_validator() {
    let index = read("index.json");
    for name in strings(&index["valid_bundles"]) {
        let b = read(&name);
        contracts::validate(&b, &payloads(&b)).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    for pair in rows(&index, "valid_queries") {
        let b = read(&text(pair, "bundle"));
        contracts::validate_query(&read(&text(pair, "query")), &b, &payloads(&b))
            .unwrap_or_else(|e| panic!("{}: {e}", pair["query"]));
    }
    for pair in rows(&index, "valid_batches") {
        let b = read(&text(pair, "bundle"));
        contracts::validate_batch(
            &read(&text(pair, "batch")),
            &b,
            &payloads(&b),
            capability(&text(pair, "capability")),
        )
        .unwrap();
    }
}
fn patch(v: &mut Value, path: &str, replacement: Value) {
    let pointer = format!("/{path}");
    if let Some(target) = v.pointer_mut(&pointer) {
        *target = replacement;
    } else {
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        v.pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.to_owned(), replacement);
    }
}
#[test]
fn every_authored_rejection_fails_for_its_expected_reason() {
    for case in read("rejections.json").as_array().unwrap() {
        let mut b = read(&text(case, "bundle"));
        let original_payloads = payloads(&b);
        let mut q = case["query"].as_str().map(read);
        let mut batch = case["batch"].as_str().map(read);
        for p in rows(case, "patches") {
            let path = text(p, "path");
            if let Some(path) = path.strip_prefix("query/") {
                patch(q.as_mut().unwrap(), path, p["value"].clone());
            } else if let Some(path) = path.strip_prefix("batch/") {
                patch(batch.as_mut().unwrap(), path, p["value"].clone());
            } else {
                patch(&mut b, &path, p["value"].clone());
            }
        }
        let result = (|| {
            contracts::validate(&b, &original_payloads)?;
            if let Some(q) = q {
                contracts::validate_query(&q, &b, &original_payloads)?;
            }
            if let Some(batch) = batch {
                contracts::validate_batch(
                    &batch,
                    &b,
                    &original_payloads,
                    capability(&text(case, "capability")),
                )?;
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        })();
        let error = result.expect_err(&format!("Case {} unexpectedly accepted", case["id"]));
        assert!(
            error
                .to_string()
                .starts_with(case["expected_code"].as_str().unwrap()),
            "{}: {error}",
            case["id"]
        );
    }
}
#[test]
fn canonical_identity_vectors_and_invalid_inputs() {
    let vectors = identity::parse(include_bytes!("../contracts/v1/identity-vectors.json")).unwrap();
    for v in vectors.as_array().unwrap() {
        assert_eq!(
            identity::digest(&v["input"]).unwrap(),
            text(v, "expected_digest")
        );
        assert_eq!(
            String::from_utf8(identity::canonical(&v["input"]).unwrap()).unwrap(),
            text(v, "expected_canonical_utf8")
        );
    }
    assert!(identity::digest(&serde_json::json!({"score":0.9})).is_err());
    assert!(
        identity::canonical(&identity::parse("{\"é\":1,\"é\":2}".as_bytes()).unwrap()).is_err()
    );
    assert!(identity::parse(b"{\"x\":1,\"x\":2}").is_err());
    assert!(identity::parse(b"{\"x\":{\"y\":1,\"y\":2}}").is_err());
}

#[test]
fn every_owned_schema_dispatches_and_query_definitions_are_checked() {
    for name in [
        "knowledge",
        "batch",
        "query",
        "session-profile",
        "session",
        "architecture-declaration",
        "session-activity",
        "task-brief",
    ] {
        let error = contracts::schema(&Value::Null, name, None)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("SCHEMA:"), "{name}: {error}");
    }
    assert_eq!(
        contracts::schema(&Value::Null, "unknown", None)
            .unwrap_err()
            .to_string(),
        "Unknown owned schema"
    );
    let mut response = read("query-dependency.json");
    contracts::schema(&response, "query", Some("Response")).unwrap();
    response["operation"] = serde_json::json!("scan");
    assert!(contracts::schema(&response, "query", Some("Response")).is_err());
}
