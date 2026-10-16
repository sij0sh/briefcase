//! Java compiler adapter acceptance through the ledger, queries and CLI.
mod native_language_support;
use native_language_support::*;
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use briefcase::architecture::{
    capture::Capture,
    contracts::{rows, strings, text},
    go, history,
    java::{self, ScanOptions},
    ledger::Ledger,
    mcp, passes, query, recall, retrieval,
};
fn calls() -> ScanOptions {
    ScanOptions {
        calls: true,
        release: Some(17),
        ..Default::default()
    }
}
fn prepared(
    root: &std::path::Path,
    options: &ScanOptions,
) -> briefcase::architecture::ledger::Prepared {
    java::produce(capture(root), "native-tests", 1, options).unwrap()
}
#[test]
fn packages_static_imports_overloads_nested_types_and_retained_execution() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("source");
    source(&root,&[
        ("pom.xml","<project/>\n"),
        ("src/main/java/demo/App.java","package demo; import demo.store.Store; import static demo.store.Store.save; public class App { // café\npublic static void main(){save(1);Store.save();Nested.work();}\nstatic class Nested {static void work(){Store.save();}}}\n"),
        ("src/main/java/demo/store/Store.java","package demo.store; public class Store {public static void save(){} public static void save(int n){save();}}\n"),
    ]);
    let mut l = Ledger::open(&t.path().join("briefcase.db")).unwrap();
    let d = publish(&mut l, "base", |g| {
        java::produce(
            capture(&root),
            "native-tests",
            g,
            &ScanOptions {
                release: Some(17),
                ..Default::default()
            },
        )
        .unwrap()
    });
    assert_eq!(d["provider"]["name"], "jdk-compiler-api");
    assert_eq!(d["provider"]["annotation_processing"], false);
    assert!(rows(&d, "java_diagnostics").is_empty(), "{d}");
    assert!(pairs(&l.load("base").unwrap().bundle, "CALLS_STATIC").is_empty());
    assert!(
        pairs(&l.load("base").unwrap().bundle, "IMPORTS").contains(&(
            "src/main/java/demo/App.java".into(),
            "src/main/java/demo/store/Store.java".into()
        ))
    );
    std::fs::remove_dir_all(&root).unwrap();
    let r = java::analyze(&mut l, &options("base"), &Default::default()).unwrap();
    assert_eq!(r["coverage"]["resolution_complete"], true, "{r}");
    let bundle = l.load("base").unwrap().bundle;
    assert_eq!(
        pairs(&bundle, "CALLS_STATIC"),
        BTreeSet::from([
            (
                "src/main/java/demo/App.java::App::main".into(),
                "src/main/java/demo/store/Store.java::Store::save".into()
            ),
            (
                "src/main/java/demo/App.java::App::main".into(),
                "src/main/java/demo/App.java::App::Nested::work".into()
            ),
            (
                "src/main/java/demo/App.java::App::Nested::work".into(),
                "src/main/java/demo/store/Store.java::Store::save".into()
            ),
            (
                "src/main/java/demo/store/Store.java::Store::save".into(),
                "src/main/java/demo/store/Store.java::Store::save".into()
            ),
        ])
    );
    assert_eq!(
        rows(&bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
            .count(),
        5
    );
    assert_eq!(
        java::analyze(&mut l, &options("base"), &Default::default()).unwrap()["reused"],
        true
    );
    assert!(go::analyze(&mut l, &options("base"), &Default::default()).is_err());
    assert!(recall::recall(&l, "café", &options("base"), 16384, false)
        .unwrap()
        .to_string()
        .contains("café"));
    let retrieve = retrieval::retrieve(&l, "main café", &options("base"), 16384, 0).unwrap();
    l.rebuild("base").unwrap();
    assert_retained_retrieval(
        &l,
        &retrieve,
        &retrieval::retrieve(&l, "main café", &options("base"), 16384, 0).unwrap(),
    );
    let mcp = mcp::dispatch(
        &l,
        &json!({"id":1,"method":"tools/call","params":{"name":"architecture_reachability","arguments":{"view":"base","from":"src/main/java/demo/App.java::App::main","to":"src/main/java/demo/App.java::App::Nested::work"}}}),
    );
    assert_eq!(
        mcp["result"]["structuredContent"]["response"]["results"][0]["conclusion"],
        "present"
    );
    proof_and_retraction(
        &mut l,
        "src/main/java/demo/App.java::App::main",
        "src/main/java/demo/App.java::App::Nested::work",
    );
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn exact_private_final_super_and_explicit_constructor_calls() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(),&[
        ("App.java","class App {private void secret(){} final void fixed(){} void run(){secret();fixed();}}\n"),
        ("FinalStore.java","final class FinalStore {FinalStore(){helper();} private void helper(){} void save(){} static void run(FinalStore s){s.save();new FinalStore();}}\n"),
        ("Base.java","class Base {void work(){}}\n"),
        ("Child.java","class Child extends Base {void run(){super.work();}}\n"),
    ]);
    let p = prepared(t.path(), &calls());
    let edges = pairs(&p.bundle, "CALLS_STATIC");
    for edge in [
        ("App.java::App::run", "App.java::App::secret"),
        ("App.java::App::run", "App.java::App::fixed"),
        (
            "FinalStore.java::FinalStore::run",
            "FinalStore.java::FinalStore::save",
        ),
        (
            "FinalStore.java::FinalStore::run",
            "FinalStore.java::FinalStore::<init>",
        ),
        (
            "FinalStore.java::FinalStore::<init>",
            "FinalStore.java::FinalStore::helper",
        ),
        ("Child.java::Child::run", "Base.java::Base::work"),
    ] {
        assert!(edges.contains(&(edge.0.into(), edge.1.into())), "{edges:?}");
    }
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn virtual_interface_lambda_method_references_external_and_ambiguous_targets_stay_unknown() {
    let cases = [
        "class App {void target(){} void run(){target();}}\n",
        "interface Store {void save();} class App {static void run(Store s){s.save();}}\n",
        "class App {static void target(){} static void run(){Runnable r=()->target();r.run();}}\n",
        "class App {static void target(){} static void run(){Runnable r=App::target;r.run();}}\n",
        "class App {static void run(){System.out.println(1);}}\n",
        "class App {static void target(){} static {target();}}\n",
        "class App {static void target(){} int field=target();}\n",
    ];
    for (i, code) in cases.iter().enumerate() {
        let t = tempfile::tempdir().unwrap();
        source(t.path(), &[("App.java", code)]);
        let p = prepared(t.path(), &calls());
        assert!(
            pairs(&p.bundle, "CALLS_STATIC").is_empty(),
            "case {i}: {:?}",
            pairs(&p.bundle, "CALLS_STATIC")
        );
        assert_eq!(
            p.diagnostics["call_coverage"]["resolution_complete"], false,
            "case {i}"
        );
    }
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "a/Store.java",
                "package demo; public class Store {public static void save(){}}",
            ),
            (
                "b/Store.java",
                "package demo; public class Store {public static void save(){}}",
            ),
            (
                "App.java",
                "import demo.Store; class App {static void run(){Store.save();}}",
            ),
        ],
    );
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["status"] == "ambiguous" && strings(&u["candidate_ids"]).len() == 2));
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn records_enums_generics_varargs_annotations_and_source_release() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(),&[
        ("App.java","@Deprecated class App {static <T> T identity(T value){return value;} static void variadic(int... values){} static void run(){identity(1);variadic(1,2);State.work();Point.work();}}\n"),
        ("State.java","enum State {READY; static void work(){}}\n"),
        ("Point.java","record Point(int x) {static void work(){}}\n"),
    ]);
    let p = prepared(t.path(), &calls());
    let edges = pairs(&p.bundle, "CALLS_STATIC");
    for target in [
        "App.java::App::identity",
        "App.java::App::variadic",
        "State.java::State::work",
        "Point.java::Point::work",
    ] {
        assert!(
            edges.contains(&("App.java::App::run".into(), target.into())),
            "{edges:?}"
        );
    }
    assert!(rows(&p.bundle, "entity_versions")
        .iter()
        .any(|e| e["canonical_key"]
            .as_str()
            .is_some_and(|k| k.starts_with("Point.java::Point#"))));
    let old = prepared(
        t.path(),
        &ScanOptions {
            release: Some(11),
            ..calls()
        },
    );
    assert_eq!(old.diagnostics["coverage"]["resolution_complete"], false);
    assert!(!rows(&old.diagnostics, "java_diagnostics").is_empty());
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn syntax_roots_module_descriptors_and_utf8_unicode_escape_spans() {
    let t = tempfile::tempdir().unwrap();
    let code =
        "// café 😀\r\nclass App {static void café(){} static void run(){caf\\u00e9();}}\r\n";
    source(t.path(), &[("App.java", code)]);
    let p = prepared(t.path(), &calls());
    assert_eq!(
        p.diagnostics["call_coverage"]["resolution_complete"], true,
        "{}",
        p.diagnostics
    );
    let call = rows(&p.bundle, "assertions")
        .iter()
        .find(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
        .unwrap();
    let evidence = rows(&p.bundle, "evidence_refs")
        .iter()
        .find(|e| e["id"] == call["evidence_ref_ids"][0])
        .unwrap();
    assert_eq!(
        &code[evidence["locator"]["start"].as_u64().unwrap() as usize
            ..evidence["locator"]["end"].as_u64().unwrap() as usize],
        "caf\\u00e9()"
    );
    let p = prepared(
        t.path(),
        &ScanOptions {
            syntax_only: true,
            ..Default::default()
        },
    );
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    let p = prepared(
        t.path(),
        &ScanOptions {
            source_roots: vec!["elsewhere".into()],
            ..calls()
        },
    );
    assert!(!strings(&p.diagnostics["omitted_source_files"]).is_empty());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    source(t.path(), &[("module-info.java", "module demo {}\n")]);
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    for options in [
        ScanOptions {
            release: Some(99),
            ..Default::default()
        },
        ScanOptions {
            source_roots: vec!["../outside".into()],
            ..Default::default()
        },
        ScanOptions {
            source_roots: vec!["src".into(), "src".into()],
            ..Default::default()
        },
    ] {
        assert!(java::produce(capture(t.path()), "invalid", 1, &options).is_err());
    }
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn policy_negative_paths_passes_stable_identity_and_revision_comparison() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(),&[("App.java","class App {static void run(){target();} static void target(){} static void idle(){}}\n")]);
    let policy = policy(t.path());
    std::fs::write(&policy, serde_json::to_vec(&json!({"authority":"owner","revision":"1", "assignments":{"App.java":"Checkout"}, "rules":[{"id":"reverse","from":"App.java","to":"App.java","mode":"report-only"}]})).unwrap()).unwrap();
    let mut l = Ledger::open(&t.path().join("briefcase.db")).unwrap();
    let produce = |g| {
        java::produce(
            Capture::read(t.path(), Some(&policy)).unwrap(),
            "native-tests",
            g,
            &calls(),
        )
        .unwrap()
    };
    publish(&mut l, "base", produce);
    assert_eq!(
        query::policy(&l, &options("base")).unwrap()["response"]["results"][0]["conclusion"],
        "pass"
    );
    assert!(query::domains(&l, &options("base"))
        .unwrap()
        .to_string()
        .contains("Checkout"));
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "App.java::App::idle",
            "App.java::App::target"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "absent"
    );
    let plan = json!({"schema_version":"1.0","view":"base","passes":[{"id":"dep","operation":"dependencies","dependencies":[]},{"id":"scc","operation":"scc","dependencies":["dep"]},{"id":"reach","operation":"reachability","dependencies":[]}]});
    assert_eq!(
        passes::inspect(&mut l, "base", Some(&plan), true).unwrap()["recomputed_and_verified"],
        true
    );
    publish(&mut l, "repeat", produce);
    assert_eq!(
        rows(&l.load("base").unwrap().bundle, "assertions"),
        rows(&l.load("repeat").unwrap().bundle, "assertions")
    );
    std::fs::rename(t.path().join("App.java"), t.path().join("Moved.java")).unwrap();
    publish(&mut l, "moved", produce);
    let before = l.load("base").unwrap();
    let after = l.load("moved").unwrap();
    history::prepare(&mut l, &before, &after).unwrap();
    assert!(query::compare(&l, "base", &options("moved"))
        .unwrap()
        .to_string()
        .contains("RENAMED_FROM"));
    let id = rows(&after.bundle, "assertions")
        .iter()
        .find(|a| a["proposition"]["predicate"] == "DECLARES")
        .map(|a| text(a, "id"))
        .unwrap();
    l.retract(&id, "corrected Java declaration").unwrap();
    l.rebuild("moved").unwrap();
    assert!(java::analyze(&mut l, &options("moved"), &Default::default()).is_err());
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn cli_scan_sync_markers_and_retained_roots_release() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("source");
    source(
        &root,
        &[
            (
                "build.gradle.kts",
                "throw RuntimeException(\"never run\")\n",
            ),
            (
                "src/main/java/App.java",
                "class App {static void run(){target();} static void target(){}}\n",
            ),
        ],
    );
    let db = t.path().join("scan.db");
    let (ok, _, err) = cli(
        &root,
        &[
            "--db",
            db.to_str().unwrap(),
            "--json",
            "scan",
            ".",
            "--language",
            "java",
            "--java-release",
            "17",
            "--java-source-root",
            "src/main/java",
            "--label",
            "base",
        ],
    );
    assert!(ok, "{err}");
    let (ok, r, err) = cli(
        &root,
        &[
            "--db",
            db.to_str().unwrap(),
            "--json",
            "analyze-execution",
            "--language",
            "java",
            "--view",
            "base",
        ],
    );
    assert!(ok, "{err}");
    assert_eq!(r["coverage"]["resolution_complete"], true);
    let args = [
        "--project-root",
        root.to_str().unwrap(),
        "sync",
        "--calls",
        "--require-ready",
    ];
    let (ok, r, err) = cli(&root, &args);
    assert!(ok, "{err} {r}");
    assert_eq!(r["profile"]["language"], "java");
    assert_eq!(cli(&root, &args).1["sync"], "up-to-date");
    source(
        &root,
        &[(
            "src/main/java/App.java",
            "class App {static void run(){target();} static void target(){ }}\n",
        )],
    );
    assert_eq!(cli(&root, &args).1["sync"], "refreshed");
    source(&root, &[("go.mod", "module mixed\n")]);
    assert_eq!(cli(&root, &args).1["sync"], "needs_configuration");
    source(
        &root,
        &[(
            ".briefcase/sync.json",
            "{\"language\":\"java\",\"java_release\":17,\"java_source_roots\":[\"src/main/java\"]}",
        )],
    );
    assert_eq!(cli(&root, &args).1["sync"], "refreshed");
    let mut l = Ledger::open_existing(&db).unwrap();
    assert!(java::analyze(
        &mut l,
        &options("base"),
        &ScanOptions {
            release: Some(11),
            ..Default::default()
        }
    )
    .is_err());
}
#[cfg(unix)]
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn environment_and_processor_isolation_and_compiler_timeout() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let sentinel = t.path().join("executed");
    source(t.path(),&[
        ("App.java",&format!("class App {{static {{try {{java.nio.file.Files.writeString(java.nio.file.Path.of({:?}),\"executed\");}}catch(Exception e){{}}}} static void run(){{}}}}\n",sentinel.to_str().unwrap())),
        ("build.gradle","throw new RuntimeException('never run')\n"),
        ("META-INF/services/javax.annotation.processing.Processor","EvilProcessor\n"),
    ]);
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(t.path())
        .env("JAVA_TOOL_OPTIONS", "-javaagent:/missing/evil.jar")
        .env("JDK_JAVA_OPTIONS", "-javaagent:/missing/evil.jar")
        .env("_JAVA_OPTIONS", "-javaagent:/missing/evil.jar")
        .env("CLASSPATH", t.path())
        .args([
            "--db",
            t.path().join("briefcase.db").to_str().unwrap(),
            "--json",
            "scan",
            ".",
            "--language",
            "java",
            "--calls",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!sentinel.exists());
    assert!(!t.path().join("App.class").exists());
    std::fs::write(t.path().join("java"), "unused sibling").unwrap();
    let sleeper = t.path().join("sleeper");
    std::fs::write(&sleeper, "#!/bin/sh\nexec sleep 5\n").unwrap();
    std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let start = std::time::Instant::now();
    assert!(java::produce(
        capture(t.path()),
        "timeout",
        1,
        &ScanOptions {
            javac: sleeper,
            timeout: Duration::from_secs(1),
            ..Default::default()
        }
    )
    .err()
    .unwrap()
    .to_string()
    .contains("timed out"));
    assert!(start.elapsed() < Duration::from_secs(3));
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn same_package_qualified_dependencies_cycles_and_wildcards_keep_native_support() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "App.java",
                "class App {Store store; static void run(){Store.save();}}\n",
            ),
            (
                "Store.java",
                "class Store {App app; static void save(){}}\n",
            ),
        ],
    );
    let p = prepared(t.path(), &calls());
    let deps = pairs(&p.bundle, "DEPENDS_ON");
    assert!(deps.contains(&("App.java".into(), "Store.java".into())));
    assert!(deps.contains(&("Store.java".into(), "App.java".into())));
    assert_eq!(
        rows(&p.bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "IN_CYCLE")
            .count(),
        2
    );
    source(
        t.path(),
        &[
            (
                "App.java",
                "import demo.*; class App {static void run(){demo.Store.save();}}\n",
            ),
            (
                "Store.java",
                "package demo; public class Store {public static void save(){}}\n",
            ),
        ],
    );
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "DEPENDS_ON").contains(&("App.java".into(), "Store.java".into())));
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["predicate"] == "IMPORTS" && u["reason"] == "unsupported_operation"));
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn declaration_quota_and_parse_failure_preserve_honest_boundaries() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[("App.java", "class App {static void broken( }\n")],
    );
    let p = prepared(t.path(), &calls());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    let quota = (0..4097)
        .map(|i| format!("static void f{i}(){{}}\n"))
        .collect::<String>();
    source(
        t.path(),
        &[("App.java", &format!("class App {{\n{quota}}}\n"))],
    );
    assert!(
        java::produce(capture(t.path()), "quota", 1, &Default::default())
            .err()
            .unwrap()
            .to_string()
            .contains("definition limit")
    );
}
