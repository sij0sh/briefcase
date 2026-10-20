mod native_language_support;
use briefcase::architecture::{
    capture::Capture,
    contracts::{self, rows, strings, text},
    inference,
    ledger::Ledger,
    mcp, passes,
    query::{self, Options},
    retrieval,
    typescript::{self, ScanOptions},
};
use native_language_support::assert_retained_retrieval;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

fn options(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
fn source(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{"target":"ES2020","module":"CommonJS","strict":true,"allowJs":true,"checkJs":false},"include":["src/**/*"]}"#).unwrap();
    for (name, code) in files {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, code).unwrap();
    }
}
fn scan(l: &mut Ledger, root: &Path, label: &str, calls: bool) -> Value {
    let generation = l.begin("typescript-tests", Some(label)).unwrap();
    let prepared = typescript::produce(
        Capture::read(root, None).unwrap(),
        "typescript-tests",
        generation,
        &ScanOptions {
            calls,
            ..Default::default()
        },
    )
    .unwrap();
    contracts::validate(&prepared.bundle, &prepared.payloads).unwrap();
    let view = l.commit(generation, &prepared).unwrap();
    l.publish(&view, true).unwrap();
    prepared.diagnostics
}
fn pairs(b: &Value, predicate: &str) -> BTreeSet<(String, String)> {
    let key = |id: &str| {
        text(
            rows(b, "entity_versions")
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
    rows(b, "assertions")
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

#[test]
fn compiler_aliases_reexports_commonjs_calls_and_retained_only_extension() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation/sources/typescript/capabilities");
    let capture = Capture::read(&fixture, None).unwrap();
    for (p, b) in capture.files {
        let p = root.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    let diagnostics = scan(&mut l, &root, "base", false);
    assert_eq!(diagnostics["provider"]["version"], "5.9.3");
    assert!(
        rows(&diagnostics, "compiler_diagnostics").is_empty(),
        "{diagnostics}"
    );
    let before = l.load("base").unwrap();
    assert!(pairs(&before.bundle, "CALLS_STATIC").is_empty());
    let imports = pairs(&before.bundle, "IMPORTS");
    for pair in [
        ("src/app.ts", "src/payments.ts"),
        ("src/app.ts", "src/facade.ts"),
        ("src/app.ts", "src/audit.ts"),
        ("src/facade.ts", "src/payments.ts"),
        ("src/payments.ts", "src/audit.ts"),
        ("src/orders.cjs", "src/legacy.cjs"),
    ] {
        assert!(
            imports.contains(&(pair.0.into(), pair.1.into())),
            "Missing {pair:?}: {imports:?}"
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        typescript::analyze(&mut l, &options("base"), &ScanOptions::default()).unwrap()["reused"],
        false
    );
    let v = l.load("base").unwrap();
    assert_eq!(
        pairs(&v.bundle, "CALLS_STATIC"),
        BTreeSet::from([
            (
                "src/app.ts::submit".into(),
                "src/payments.ts::charge".into()
            ),
            ("src/app.ts::submit".into(), "src/app.ts::local".into()),
            ("src/app.ts::submit".into(), "src/audit.ts::record".into()),
            ("src/app.ts::local".into(), "src/audit.ts::record".into()),
            (
                "src/app.ts::recursive".into(),
                "src/app.ts::recursive".into()
            ),
            (
                "src/payments.ts::charge".into(),
                "src/audit.ts::record".into()
            ),
        ])
    );
    let calls: Vec<_> = rows(&v.bundle, "assertions")
        .iter()
        .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
        .collect();
    assert_eq!(calls.len(), 7); // Two alias callsites share endpoints but distinct spans.
    assert!(calls
        .iter()
        .all(|a| a["class"] == "evidence" && a["resolution"]["method"] == "compiler"));
    assert_eq!(
        typescript::analyze(&mut l, &options("base"), &ScanOptions::default()).unwrap()["reused"],
        true
    );
    let report = query::reachability(
        &l,
        &options("base"),
        "src/app.ts::submit",
        "src/audit.ts::record",
    )
    .unwrap();
    assert_eq!(report["response"]["results"][0]["conclusion"], "present");
    let handle = text(&report["response"]["results"][0], "explanation_handle");
    let proof = query::explain(&l, &handle, 1000, 0).unwrap();
    assert!(rows(&proof, "support")
        .iter()
        .any(
            |s| s["assertion"]["proposition"]["predicate"] == "CALLS_STATIC"
                && !rows(s, "sources").is_empty()
        ));
    let mcp_report = mcp::dispatch(
        &l,
        &json!({"id":1,"method":"tools/call","params":{"name":"architecture_reachability","arguments":{"view":"base","from":"src/app.ts::submit","to":"src/audit.ts::record"}}}),
    );
    assert_eq!(
        mcp_report["result"]["structuredContent"]["response"],
        report["response"]
    );
    let retrieved = retrieval::retrieve(&l, "支払い submit", &options("base"), 16384, 1).unwrap();
    assert!(retrieved.to_string().contains("支払い"));
    let ids: BTreeSet<_> = rows(&v.bundle, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect();
    rusqlite::Connection::open(temp.path().join("knowledge.db"))
        .unwrap()
        .execute_batch(
            "DELETE FROM search; DELETE FROM retrieval_units; DELETE FROM retrieval_fts;",
        )
        .unwrap();
    assert!(retrieval::retrieve(&l, "submit", &options("base"), 16384, 1).is_err());
    l.rebuild(&v.id).unwrap();
    assert_retained_retrieval(
        &l,
        &retrieved,
        &retrieval::retrieve(&l, "支払い submit", &options("base"), 16384, 1).unwrap(),
    );
    assert_eq!(
        ids,
        rows(&l.load("base").unwrap().bundle, "assertions")
            .iter()
            .map(|a| text(a, "id"))
            .collect()
    );
    let corrected = rows(&proof, "support")
        .iter()
        .find(|s| s["assertion"]["proposition"]["predicate"] == "CALLS_STATIC")
        .unwrap()["assertion"]["id"]
        .as_str()
        .unwrap();
    l.retract(corrected, "corrected callsite").unwrap();
    l.rebuild(&v.id).unwrap();
    assert!(
        rows(&query::explain(&l, &handle, 1000, 0).unwrap(), "support")
            .iter()
            .any(|s| s["assertion"]["id"] == corrected && s["currently_retracted"] == true)
    );
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "src/other.ts::charge",
            "src/audit.ts::record"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "unknown"
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn complete_project_supports_policy_passes_negative_paths_and_stable_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            (
                "src/orders.ts",
                "import { charge } from './payments'; export function submit(){charge();}",
            ),
            ("src/payments.ts", "export function charge(){}"),
            (
                "src/unused.ts",
                "export function idle(){helper();} export function helper(){}",
            ),
        ],
    );
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    scan(&mut l, &root, "base", false);
    assert!(briefcase::architecture::execution::analyze(
        &mut l,
        &options("base"),
        &briefcase::architecture::rust::ScanOptions::default()
    )
    .is_err());
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "src/unused.ts::idle",
            "src/payments.ts::charge"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "unknown"
    );
    assert_eq!(
        typescript::analyze(&mut l, &options("base"), &ScanOptions::default()).unwrap()["coverage"]
            ["resolution_complete"],
        true
    );
    let original = l.load("base").unwrap();
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "src/unused.ts::idle",
            "src/payments.ts::charge"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "absent"
    );
    let plan = json!({"schema_version":"1.0","view":"base","passes":[{"id":"dep","operation":"dependencies","dependencies":[]},{"id":"scc","operation":"scc","dependencies":["dep"]},{"id":"reach","operation":"reachability","dependencies":[]}]});
    assert_eq!(
        passes::inspect(&mut l, "base", Some(&plan), true).unwrap()["recomputed_and_verified"],
        true
    );
    scan(&mut l, &root, "repeat", false);
    assert_eq!(
        typescript::analyze(&mut l, &options("repeat"), &ScanOptions::default()).unwrap()["reused"],
        false
    );
    let repeat = l.load("repeat").unwrap();
    assert_eq!(
        rows(&original.bundle, "assertions"),
        rows(&repeat.bundle, "assertions")
    );
    std::fs::write(
        root.join("src/orders.ts"),
        "import { charge } from './payments'; export function submit(){const pointer=charge;}",
    )
    .unwrap();
    scan(&mut l, &root, "changed", true);
    assert_eq!(
        query::reachability(
            &l,
            &options("changed"),
            "src/orders.ts::submit",
            "src/payments.ts::charge"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "absent"
    );
    let unaffected = |b: &Value| -> BTreeSet<String> {
        rows(b, "pass_manifests")
            .iter()
            .filter(|m| m["pass_name"] == "static-function-reachability-v1")
            .filter(|m| {
                strings(&m["input_artifact_ids"]).iter().any(|id| {
                    rows(b, "artifacts")
                        .iter()
                        .any(|a| a["id"] == *id && a["path"] == "src/unused.ts")
                })
            })
            .flat_map(|m| strings(&m["output_artifact_ids"]))
            .collect()
    };
    assert!(!unaffected(&original.bundle).is_empty());
    assert_eq!(
        unaffected(&original.bundle),
        unaffected(&l.load("changed").unwrap().bundle)
    );
}

#[test]
fn dynamic_dispatch_shadowing_mutation_and_missing_inputs_remain_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root,&[("src/dynamic.ts", "export function charge(){}\nfunction other(){}\ncharge=other;\nexport function invoke(f:()=>void,obj:{run:()=>void},key:string){charge();f();obj.run();const closure=()=>charge();closure();import(key);}\nfunction require(p:string){return p;}\nexport function shadow(){require('./payments');}\nimport { missing } from './absent';\n"),
        ("src/payments.ts","export function charge(){}"),
        ("src/mutated.js","function arrayTarget(){} function objectTarget(){} function replacement(){}\n[arrayTarget]=[replacement]; ({target:objectTarget}={target:replacement});\nexport function invoke(){arrayTarget();objectTarget();}\nfunction deferred(){} export function outer(){class Inner {field=deferred();} return Inner;}\n")]);
    std::fs::write(root.join("ignored.ts"), "export function ignored(){}").unwrap();
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    let diagnostics = scan(&mut l, &root, "base", true);
    assert_eq!(diagnostics["call_coverage"]["resolution_complete"], false);
    assert_eq!(diagnostics["coverage"]["resolution_complete"], false);
    let v = l.load("base").unwrap();
    assert_eq!(
        pairs(&v.bundle, "CALLS_STATIC"),
        BTreeSet::from([(
            "src/dynamic.ts::shadow".into(),
            "src/dynamic.ts::require".into()
        )])
    );
    assert!(pairs(&v.bundle, "IMPORTS").is_empty());
    assert!(rows(&v.bundle, "unresolved_references")
        .iter()
        .any(|u| u["reason"] == "missing_dependency"));
    assert!(query::callgraph(&l, &options("base"))
        .unwrap()
        .to_string()
        .contains("dynamic_property"));
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "src/payments.ts::charge",
            "src/dynamic.ts::charge"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "unknown"
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn missing_compiler_bad_config_and_project_reference_fail_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[("src/lib.ts", "export function function_name(){}")],
    );
    let missing = ScanOptions {
        compiler: temp.path().join("absent.js"),
        ..Default::default()
    };
    assert!(
        typescript::produce(Capture::read(&root, None).unwrap(), "tests", 1, &missing).is_err()
    );
    for config in [
        "{",
        r#"{"files":["src/lib.ts"],"references":[{"path":"./other"}]}"#,
    ] {
        std::fs::write(root.join("tsconfig.json"), config).unwrap();
        assert!(typescript::produce(
            Capture::read(&root, None).unwrap(),
            "tests",
            1,
            &ScanOptions::default()
        )
        .is_err());
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn javascript_and_overloads_map_to_one_retained_implementation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root,&[("src/app.js","import { charge } from './payments'; export function submit(){return charge(1);}"),
        ("src/payments.ts","export function charge(value:number):number;\nexport function charge(value:string):string;\nexport function charge(value:number|string):number|string{return value;}\n")]);
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    assert_eq!(
        scan(&mut l, &root, "base", true)["call_coverage"]["resolution_complete"],
        true
    );
    let v = l.load("base").unwrap();
    assert_eq!(
        pairs(&v.bundle, "CALLS_STATIC"),
        BTreeSet::from([(
            "src/app.js::submit".into(),
            "src/payments.ts::charge".into()
        )])
    );
    assert!(rows(&v.bundle, "entity_versions")
        .iter()
        .any(|e| e["language"] == "javascript" && e["identity_quality"] == "compiler"));
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "src/app.js::submit",
            "src/payments.ts::charge"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "present"
    );
    std::fs::write(
        root.join("src/app.js"),
        "import { charge } from './payments'; export function submit(fn){return fn();}",
    )
    .unwrap();
    assert_eq!(
        scan(&mut l, &root, "changed", true)["call_coverage"]["resolution_complete"],
        false
    );
    assert!(pairs(&l.load("changed").unwrap().bundle, "CALLS_STATIC").is_empty());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn ts_modules_preserve_snapshot_identity_during_exact_and_ambiguous_movement() {
    for duplicate in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let code = "export function charge(){return 1;}\n";
        source(&root, &[("src/old.ts", code)]);
        if duplicate {
            std::fs::write(root.join("src/twin.ts"), code).unwrap();
        }
        let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
        scan(&mut l, &root, "before", false);
        let before = l.load("before").unwrap();
        std::fs::remove_file(root.join("src/old.ts")).unwrap();
        if duplicate {
            std::fs::remove_file(root.join("src/twin.ts")).unwrap();
        }
        std::fs::write(root.join("src/new.ts"), code).unwrap();
        scan(&mut l, &root, "after", false);
        let after = l.load("after").unwrap();
        let old = rows(&before.bundle, "entity_versions")
            .iter()
            .find(|e| e["kind"] == "module" && e["canonical_key"] == "src/old.ts")
            .unwrap();
        let new = rows(&after.bundle, "entity_versions")
            .iter()
            .find(|e| e["kind"] == "module" && e["canonical_key"] == "src/new.ts")
            .unwrap();
        assert_ne!(old["id"], new["id"]);
        query::prepare_compare(&mut l, "before", &options("after")).unwrap();
        let report = query::compare(&l, "before", &options("after")).unwrap();
        let details = report["details"].as_object().unwrap();
        assert_eq!(
            details.values().any(|d| d["predicate"] == "RENAMED_FROM"),
            !duplicate
        );
        if duplicate {
            assert!(details
                .values()
                .any(|d| d["change"] == "ambiguous_correspondence" && d["exact_match"] == false));
        }
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn cli_tsjs_policy_claim_routing_rebuild_and_retraction_share_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            (
                "src/orders.ts",
                "import { charge } from './payments'; export function submit(){charge();}",
            ),
            ("src/payments.ts", "export function charge(){return 1;}"),
        ],
    );
    let db = temp.path().join("knowledge.db");
    let policy = temp.path().join("policy.json");
    std::fs::write(&policy,json!({"authority":"architecture-owner","revision":"1",
        "assignments":{"src/payments.ts":"CollectionAuthority"},
        "rules":[{"id":"orders-cannot-depend-on-payments","from":"src/orders.ts","to":"src/payments.ts","mode":"report-only"}]}).to_string()).unwrap();
    let cli = |args: &[&str]| -> Value {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_briefcase"))
            .arg("--db")
            .arg(&db)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    cli(&[
        "scan",
        root.to_str().unwrap(),
        "--language",
        "typescript",
        "--calls",
        "--label",
        "base",
        "--policy",
        policy.to_str().unwrap(),
    ]);
    assert_eq!(
        cli(&["policy", "--view", "base"])["response"]["results"][0]["conclusion"],
        "violation"
    );
    let routing = cli(&["retrieve", "CollectionAuthority", "--view", "base"]);
    assert!(routing.to_string().contains("src/payments.ts"));
    assert!(routing.to_string().contains("declaration"));
    let mut l = Ledger::open(&db).unwrap();
    let provider_script = temp.path().join("provider.py");
    std::fs::write(&provider_script,"import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps({'proposals':[{'target_id':r['target_id'],'predicate':'BELONGS_TO_DOMAIN','domain':'Payments','supports':[r['packet']['assertion_ids'][0]],'limitations':['controlled contract test']}],'abstained':False,'limitations':[]}))\n").unwrap();
    let provider = temp.path().join("provider.json");
    std::fs::write(&provider,json!({"schema_version":"1.0","name":"controlled-test-provider","model_name":"contract-test",
        "model_version":"1","settings":{},"command":{"program":"python3","args":[provider_script]},
        "timeout_seconds":10,"opaque":false,"deterministic":true}).to_string()).unwrap();
    let taxonomy = temp.path().join("taxonomy.json");
    std::fs::write(
        &taxonomy,
        json!({"schema_version":"1.0","version":"test-v1","domains":["Payments"]}).to_string(),
    )
    .unwrap();
    let q = Options {
        component: Some("src/payments.ts".into()),
        ..options("base")
    };
    let classified = inference::classify(&mut l, &q, &provider, &taxonomy, 16, 65536).unwrap();
    let claim = strings(&classified["claim_ids"])[0].clone();
    // Fresh from classification, the Claim is a current advisory assignment.
    let domains = query::domains(&l, &q).unwrap();
    assert!(
        rows(&domains["response"], "results")
            .iter()
            .any(
                |r| domains["details"][text(r, "id")]["conclusion"] == "present"
                    || r["conclusion"] == "present"
            ),
        "{domains}"
    );
    let drift = query::drift(&l, &q).unwrap();
    assert!(
        drift.to_string().contains("domain_intent_disagreement"),
        "{drift}"
    );
    assert!(!query::dependencies(&l, &options("base"))
        .unwrap()
        .to_string()
        .contains(&claim));
    let policy_after = query::policy(&l, &options("base")).unwrap();
    assert_eq!(
        policy_after["response"]["results"][0]["conclusion"],
        "violation"
    );
    assert!(!policy_after.to_string().contains(&claim));
    let v = l.load("base").unwrap();
    let ids: BTreeSet<_> = rows(&v.bundle, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect();
    let support = strings(
        &rows(&v.bundle, "assertions")
            .iter()
            .find(|a| a["id"] == claim)
            .unwrap()["supports"],
    )[0]
    .clone();
    let handle = text(&drift["response"]["results"][0], "explanation_handle");
    std::fs::remove_dir_all(root).unwrap();
    l.rebuild(&v.id).unwrap();
    assert_eq!(
        ids,
        rows(&l.load("base").unwrap().bundle, "assertions")
            .iter()
            .map(|a| text(a, "id"))
            .collect()
    );
    assert_eq!(
        query::drift(&l, &q).unwrap()["response"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    l.retract(&support, "source correction").unwrap();
    l.rebuild(&v.id).unwrap();
    assert!(query::drift(&l, &q).unwrap()["response"]["results"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        rows(&query::explain(&l, &handle, 1000, 0).unwrap(), "support")
            .iter()
            .any(|s| s["currently_retracted"] == true)
    );
    assert!(typescript::analyze(&mut l, &options("base"), &ScanOptions::default()).is_err());
}

#[test]
fn workspace_partitions_resolve_manifests_barrels_and_classify_calls() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let files: &[(&str, &str)] = &[
        ("tsconfig.base.json", r#"{"compilerOptions":{"module":"NodeNext","moduleResolution":"NodeNext","allowImportingTsExtensions":true,"strict":true}}"#),
        ("packages/core/package.json", r#"{"name":"@w/core","exports":{".":{"types":"./src/index.ts"},"./util":{"types":"./src/util.ts"}}}"#),
        ("packages/core/tsconfig.json", r#"{"extends":"../../tsconfig.base.json","include":["src"]}"#),
        ("packages/core/src/index.ts", "export * from \"./settings.ts\";\nexport { helper as aliased } from \"./util.ts\";\n"),
        ("packages/core/src/settings.ts", "export const Settings = { a: 1 };\nexport function loadSettings() { return Settings; }\n"),
        ("packages/core/src/util.ts", "export function helper() { return 1; }\n"),
        ("apps/server/package.json", r#"{"name":"server"}"#),
        ("apps/server/tsconfig.json", r#"{"extends":"external-preset/tsconfig.json","include":["src"]}"#),
        ("apps/server/src/server.ts", "import { loadSettings } from \"@w/core\";\nimport { helper } from \"@w/core/util\";\nimport { Effect } from \"effect\";\nimport icon from \"./icon.png\";\nimport { missing } from \"./nothere.ts\";\nexport const run = () => Effect.succeed(loadSettings()).pipe(Effect.map(helper));\nnew Map(); helper();\n"),
        ("apps/server/src/server.test.ts", "import { run } from \"./server.ts\";\nrun();\n"),
        ("packages/broken/tsconfig.json", "{ broken"),
        ("scripts/tool.ts", "export const x = 1;\n"),
    ];
    std::fs::create_dir_all(&root).unwrap();
    for (name, code) in files {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, code).unwrap();
    }
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    let generation = l.begin("typescript-tests", Some("workspace")).unwrap();
    let prepared = typescript::produce(
        Capture::read(&root, None).unwrap(),
        "typescript-tests",
        generation,
        &ScanOptions {
            project: "workspace".into(),
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    contracts::validate(&prepared.bundle, &prepared.payloads).unwrap();
    let imports = pairs(&prepared.bundle, "IMPORTS");
    for (from, to) in [
        (
            "apps/server/src/server.test.ts",
            "apps/server/src/server.ts",
        ),
        ("apps/server/src/server.ts", "packages/core/src/index.ts"),
        ("apps/server/src/server.ts", "packages/core/src/util.ts"),
        // The named import through `export *` reaches the declaring module.
        ("apps/server/src/server.ts", "packages/core/src/settings.ts"),
        (
            "packages/core/src/index.ts",
            "packages/core/src/settings.ts",
        ),
    ] {
        assert!(
            imports.contains(&(from.into(), to.into())),
            "{from} -> {to}: {imports:?}"
        );
    }
    assert!(pairs(&prepared.bundle, "CALLS_STATIC").is_empty());
    let unresolved = rows(&prepared.bundle, "unresolved_references");
    assert_eq!(
        unresolved.len(),
        1,
        "only the missing relative module: {unresolved:?}"
    );
    let d = &prepared.diagnostics;
    assert!(d["call_coverage"].is_null());
    let w = &d["workspace"];
    let status = |config: &str| {
        rows(w, "partitions")
            .into_iter()
            .find(|p| p["config"] == config)
            .unwrap()
    };
    assert_eq!(status("packages/broken/tsconfig.json")["status"], "failed");
    assert_eq!(
        status("apps/server/tsconfig.json")["degraded"],
        "external_extends_unavailable"
    );
    let resolution = &w["import_resolution"];
    assert_eq!(resolution["external_packages"]["effect"], 1);
    assert_eq!(resolution["by_status"]["non_code_unretained"], 1);
    assert_eq!(resolution["by_method"]["workspace_manifest"], 2);
    assert_eq!(w["call_constructs"]["constructor"]["count"], 1);
    assert!(
        w["call_constructs"]["effect_namespace_call"]["count"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert_eq!(d["omitted_source_files"], json!(["scripts/tool.ts"]));
    assert!(strings(&d["limitations"])
        .iter()
        .any(|l| l.contains("1 file(s) under scripts")));
}

#[test]
fn workspace_binding_references_resolve_uses_through_barrels_namespaces_and_heritage() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            ("tsconfig.base.json", r#"{"compilerOptions":{"module":"NodeNext","moduleResolution":"NodeNext","allowImportingTsExtensions":true,"strict":true}}"#),
            ("packages/core/package.json", r#"{"name":"@w/core","exports":{".":{"types":"./src/index.ts"},"./util":{"types":"./src/util.ts"},"./def":{"types":"./src/def.ts"}}}"#),
            ("packages/core/tsconfig.json", r#"{"extends":"../../tsconfig.base.json","include":["src"]}"#),
            ("packages/core/src/index.ts", "export * from \"./settings.ts\";\nexport { helper as aliased } from \"./util.ts\";\n"),
            ("packages/core/src/settings.ts", "export interface Policy { allow(): boolean }\nexport class Base {}\nexport function loadSettings() { return 1; }\n"),
            ("packages/core/src/util.ts", "export function helper() { return 1; }\n"),
            ("packages/core/src/def.ts", "function makeDef() { return 2; }\nexport default makeDef;\n"),
            ("apps/server/package.json", r#"{"name":"server"}"#),
            ("apps/server/tsconfig.json", r#"{"extends":"../../tsconfig.base.json","include":["src"]}"#),
            ("apps/server/src/server.ts", concat!(
                "import { loadSettings, aliased, type Policy, Base } from \"@w/core\";\n",
                "import * as util from \"@w/core/util\";\n",
                "import makeDef from \"@w/core/def\";\n",
                "loadSettings();\n",
                "export function run(cfg: { loadSettings: number }) { return loadSettings() + cfg.loadSettings + aliased(); }\n",
                "export class Impl extends Base implements Policy { allow() { return true; } }\n",
                "export function useUtil() { return util.helper() + makeDef(); }\n",
                "export function shadow(loadSettings: () => number) { return loadSettings(); }\n",
            )),
        ],
    );
    let generation_options = |references| ScanOptions {
        project: "workspace".into(),
        references,
        ..Default::default()
    };
    let mut l = Ledger::open(&temp.path().join("knowledge.db")).unwrap();
    let generation = l.begin("typescript-tests", Some("workspace")).unwrap();
    let plain = typescript::produce(Capture::read(&root, None).unwrap(), "typescript-tests", generation, &generation_options(false)).unwrap();
    assert!(pairs(&plain.bundle, "REFERENCES").is_empty());
    assert_eq!(plain.diagnostics["workspace"]["references"], "not_requested");
    let prepared = typescript::produce(Capture::read(&root, None).unwrap(), "typescript-tests", generation, &generation_options(true)).unwrap();
    contracts::validate(&prepared.bundle, &prepared.payloads).unwrap();
    let server = "apps/server/src/server.ts";
    let expected: BTreeSet<(String, String)> = [
        (server.to_owned(), "packages/core/src/settings.ts::loadSettings"),
        (format!("{server}::run"), "packages/core/src/settings.ts::loadSettings"),
        (format!("{server}::run"), "packages/core/src/util.ts::helper"),
        (format!("{server}::Impl"), "packages/core/src/settings.ts::Base"),
        (format!("{server}::Impl"), "packages/core/src/settings.ts::Policy"),
        (format!("{server}::useUtil"), "packages/core/src/util.ts::helper"),
        (format!("{server}::useUtil"), "packages/core/src/def.ts::makeDef"),
    ]
    .into_iter()
    .map(|(a, b)| (a, b.to_owned()))
    .collect();
    assert_eq!(pairs(&prepared.bundle, "REFERENCES"), expected);
    let counts = &prepared.diagnostics["workspace"]["references"];
    assert_eq!(counts["by_kind"]["implements"], 1);
    assert_eq!(counts["by_kind"]["extends"], 1);
    assert_eq!(counts["shadowed"], 1, "the parameter shadows the import: {counts}");
    let run = rows(&prepared.bundle, "runs")
        .into_iter()
        .find(|r| r["coverage"]["guarantees"][0]["predicate"] == "REFERENCES")
        .unwrap();
    assert_eq!(run["coverage"]["guarantees"][0]["kind"], "syntax_only");
    assert!(text(&run["coverage"]["guarantees"][0], "scope").starts_with("imported binding uses"));
}
