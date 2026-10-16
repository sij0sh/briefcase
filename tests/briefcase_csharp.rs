//! Roslyn acceptance through retained source, canonical ledger and shipped CLI.
mod native_language_support;
use native_language_support::*;
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use briefcase::architecture::{
    capture::Capture,
    contracts::{rows, strings, text},
    csharp::{self, ScanOptions},
    history,
    ledger::Ledger,
    mcp, query, recall,
};

fn calls() -> ScanOptions {
    ScanOptions {
        calls: true,
        ..Default::default()
    }
}
fn prepared(
    root: &std::path::Path,
    options: &ScanOptions,
) -> briefcase::architecture::ledger::Prepared {
    csharp::produce(capture(root), "native-tests", 1, options).unwrap()
}

#[test]
fn namespaces_usings_aliases_overloads_generics_extension_methods_and_retained_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("src");
    source(&root, &[
        ("App.cs", "using Alias = Store.Data; using static Store.Data; namespace App; public class Entry { public static int Run(){ return Alias.Save(1) + Save(\"x\") + Alias.Id<int>(2) + 3.Double(); } }"),
        ("Data.cs", "namespace Store; public static class Data { public static int Save(int x){return x;} public static int Save(string x){return 1;} public static T Id<T>(T x){return x;} public static int Double(this int x){return x*2;} }"),
        ("project.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"><Target Name=\"Run\"><Exec Command=\"touch EXECUTED\"/></Target></Project>"),
    ]);
    let p = prepared(&root, &calls());
    assert_eq!(
        p.diagnostics["coverage"]["resolution_complete"], true,
        "{}",
        p.diagnostics
    );
    assert_eq!(
        p.diagnostics["call_coverage"]["resolution_complete"], true,
        "{}",
        p.diagnostics
    );
    let targets: BTreeSet<_> = pairs(&p.bundle, "CALLS_STATIC")
        .into_iter()
        .map(|(_, to)| to)
        .collect();
    assert_eq!(
        targets,
        BTreeSet::from([
            "Data.cs::Store::Data::Save".into(),
            "Data.cs::Store::Data::Id".into(),
            "Data.cs::Store::Data::Double".into()
        ])
    );
    assert_eq!(
        rows(&p.bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
            .count(),
        4
    );
    assert!(pairs(&p.bundle, "IMPORTS").contains(&("App.cs".into(), "Data.cs".into())));
    assert!(!root.join("EXECUTED").exists());
    let mut ledger = Ledger::open(&temp.path().join("ledger.db")).unwrap();
    publish(&mut ledger, "base", |generation| {
        csharp::produce(
            capture(&root),
            "native-tests",
            generation,
            &Default::default(),
        )
        .unwrap()
    });
    std::fs::remove_dir_all(&root).unwrap();
    let execution = csharp::analyze(&mut ledger, &options("base"), &Default::default()).unwrap();
    assert_eq!(execution["reused"], false);
    assert_eq!(
        csharp::analyze(&mut ledger, &options("base"), &Default::default()).unwrap()["reused"],
        true
    );
    let report = query::callgraph(&ledger, &options("base")).unwrap();
    assert_eq!(report["response"]["results"].as_array().unwrap().len(), 4);
    proof_and_retraction(
        &mut ledger,
        "App.cs::App::Entry::Run",
        "Data.cs::Store::Data::Double",
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn exact_instance_private_sealed_base_constructors_and_local_functions() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(), &[("App.cs", "public class Base { public Base(){} public virtual int Work(){return 1;} public int Plain(){return 2;} } public sealed class App:Base { public App():base(){} private int Private(){return 3;} public override int Work(){return 4;} public int Run(){ int Local(){return Private();} return Local()+base.Work()+new App().Work()+Plain();} }")]);
    let p = prepared(t.path(), &calls());
    let calls = pairs(&p.bundle, "CALLS_STATIC");
    for pair in [
        ("App.cs::App::Run", "App.cs::App::Run::Local"),
        ("App.cs::App::Run::Local", "App.cs::App::Private"),
        ("App.cs::App::Run", "App.cs::Base::Work"),
        ("App.cs::App::Run", "App.cs::App::Work"),
        ("App.cs::App::Run", "App.cs::App::.ctor"),
        ("App.cs::App::.ctor", "App.cs::Base::.ctor"),
    ] {
        assert!(calls.contains(&(pair.0.into(), pair.1.into())), "{calls:?}");
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn virtual_interface_delegate_dynamic_lambda_properties_implicit_operations_and_external_targets_are_unknown(
) {
    let t = tempfile::tempdir().unwrap();
    source(t.path(), &[("App.cs", "public interface I { int Work(); } public class Base { public virtual int Work(){return 1;} } public class App { public int Value {get;set;} public static int Target(){return 1;} public int Run(Base b, I i, dynamic d){ System.Func<int> callback = Target; System.Func<int> closure = () => Target(); return b.Work()+i.Work()+callback()+closure()+d.Work()+Value+System.Math.Abs(1); } }")]);
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    for reason in [
        "trait_dispatch",
        "dynamic_property",
        "unsupported_context",
        "unsupported_operation",
        "missing_dependency",
    ] {
        assert!(
            rows(&p.bundle, "unresolved_references")
                .iter()
                .any(|u| u["predicate"] == "CALLS_STATIC" && u["reason"] == reason),
            "{}",
            p.diagnostics
        );
    }
    source(
        t.path(),
        &[(
            "App.cs",
            "public class App { public App(){} public static App Make(){return new App();} }",
        )],
    );
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC")
        .contains(&("App.cs::App::Make".into(), "App.cs::App::.ctor".into())));
    assert_eq!(
        p.diagnostics["call_coverage"]["resolution_complete"], false,
        "Implicit base construction cannot be complete"
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn partial_declarations_duplicate_types_and_multi_project_universes_do_not_guess() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(), &[("A.cs", "public partial class App { public static int Run(){return Work();} private static partial int Work(); }"), ("B.cs", "public partial class App { private static partial int Work(){return 1;} }")]);
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC")
        .contains(&("A.cs::App::Run".into(), "B.cs::App::Work".into())));
    source(
        t.path(),
        &[(
            "B.cs",
            "public class App { public static int Work(){return 1;} }",
        )],
    );
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    source(
        t.path(),
        &[
            (
                "A.cs",
                "public class App { public static int Run(){return Other.Work();} }",
            ),
            (
                "B.cs",
                "public class Other {public static int Work(){return 1;}}",
            ),
            ("A.csproj", "<Project/>"),
            ("B.csproj", "<Project/>"),
        ],
    );
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn roots_defines_versions_generated_sources_syntax_errors_and_utf8_spans() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "src/App.cs",
                "// é\r\npublic class App {public static int Run(){return Other.Work();}}",
            ),
            (
                "src/Other.cs",
                "public class Other {public static int Work(){return 1;}}",
            ),
            ("tests/App.cs", "public class App {}"),
            ("src/Helper.g.cs", "public class Generated {}"),
        ],
    );
    let selection = ScanOptions {
        source_roots: vec!["src".into()],
        calls: true,
        language_version: Some("12.0".into()),
        ..Default::default()
    };
    let p = prepared(t.path(), &selection);
    assert!(!pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(strings(&p.diagnostics["omitted_source_files"]).contains(&"src/Helper.g.cs".into()));
    let input = capture(t.path());
    let proof = rows(&p.bundle, "evidence_refs")
        .iter()
        .find(|r| {
            r["locator"]["kind"] == "span"
                && r["locator"]["start"]
                    .as_u64()
                    .is_some_and(|start| start > 6)
        })
        .unwrap();
    let artifact = rows(&p.bundle, "artifacts")
        .iter()
        .find(|a| a["id"] == proof["artifact_id"])
        .unwrap();
    let path = text(artifact, "path");
    let start = proof["locator"]["start"].as_u64().unwrap() as usize;
    let end = proof["locator"]["end"].as_u64().unwrap() as usize;
    assert!(std::str::from_utf8(&input.files[&path][start..end]).is_ok());
    source(t.path(), &[("src/Other.cs", "public class Other {\n#if FEATURE\npublic static int Work(){return 1;}\n#else\npublic static int Work(){return 2;}\n#endif\n}")]);
    let p = prepared(
        t.path(),
        &ScanOptions {
            defines: vec!["FEATURE".into()],
            ..selection.clone()
        },
    );
    assert_eq!(p.diagnostics["provider"]["defines"], json!(["FEATURE"]));
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(csharp::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            source_roots: vec!["../bad".into()],
            ..Default::default()
        }
    )
    .is_err());
    assert!(csharp::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            language_version: Some("preview".into()),
            ..Default::default()
        }
    )
    .is_err());
    source(t.path(), &[("src/Other.cs", "public class Other { broken")]);
    let p = prepared(
        t.path(),
        &ScanOptions {
            source_roots: vec!["src".into()],
            syntax_only: true,
            ..Default::default()
        },
    );
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert!(csharp::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            syntax_only: true,
            calls: true,
            ..Default::default()
        }
    )
    .is_err());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn policy_comparison_recall_and_retraction_reuse_existing_contracts() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("src");
    source(
        &root,
        &[
            (
                "App.cs",
                "public class App {public static int Run(){return Store.Work();}}",
            ),
            (
                "Store.cs",
                "public class Store {public static int Work(){return 1;}}",
            ),
            ("README.md", "retained-csharp-contract-token"),
        ],
    );
    let policy = root.join("policy.json");
    std::fs::write(&policy, json!({"authority":"owner","revision":"1","assignments":{"App.cs":"App","Store.cs":"Store"},"rules":[{"id":"forward","from":"App.cs","to":"Store.cs","mode":"report-only"},{"id":"reverse","from":"Store.cs","to":"App.cs","mode":"report-only"}]}).to_string()).unwrap();
    let mut l = Ledger::open(&t.path().join("ledger.db")).unwrap();
    publish(&mut l, "base", |g| {
        csharp::produce(
            Capture::read(&root, Some(&policy)).unwrap(),
            "native-tests",
            g,
            &calls(),
        )
        .unwrap()
    });
    let report = query::policy(&l, &options("base")).unwrap();
    let results = report["response"]["results"].as_array().unwrap();
    assert!(results.iter().any(|r| r["conclusion"] == "violation"));
    assert!(results.iter().any(|r| r["conclusion"] == "pass"));
    assert!(recall::recall(
        &l,
        "retained-csharp-contract-token",
        &options("base"),
        4000,
        false
    )
    .unwrap()
    .to_string()
    .contains("README.md"));
    let before = l.load("base").unwrap();
    std::fs::rename(root.join("Store.cs"), root.join("Moved.cs")).unwrap();
    publish(&mut l, "moved", |g| {
        csharp::produce(
            Capture::read(&root, Some(&policy)).unwrap(),
            "native-tests",
            g,
            &calls(),
        )
        .unwrap()
    });
    let after = l.load("moved").unwrap();
    history::prepare(&mut l, &before, &after).unwrap();
    assert!(query::compare(&l, "base", &options("moved"))
        .unwrap()
        .to_string()
        .contains("renamed"));
    let m = mcp::dispatch(
        &l,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"architecture_callgraph","arguments":{"view":"base"}}}),
    );
    assert!(m["result"]["structuredContent"].is_object());
    proof_and_retraction(&mut l, "App.cs::App::Run", "Store.cs::Store::Work");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn cli_scan_sync_retained_pins_and_hostile_build_inputs() {
    let t = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(t.path())
        .status()
        .unwrap()
        .success());
    source(t.path(), &[("App.cs", "public class App {public static int Run(){return Store.Work();}}"), ("Store.cs", "public class Store {public static int Work(){return 1;}}"), ("App.csproj", "<Project><Target Name=\"BeforeBuild\"><Exec Command=\"touch EXECUTED\"/></Target></Project>"), ("Directory.Build.targets", "<Project><Import Project=\"https://hostile.invalid/plugin.targets\"/></Project>"), ("global.json", "{\"sdk\":{\"version\":\"0.0.0\"}}")]);
    let (ok, scanned, error) = cli(
        t.path(),
        &[
            "--json",
            "scan",
            ".",
            "--language",
            "csharp",
            "--label",
            "scan",
            "--csharp-language-version",
            "12.0",
        ],
    );
    assert!(ok, "{error}");
    assert_eq!(scanned["status"], "published");
    let (ok, _, error) = cli(
        t.path(),
        &[
            "--json",
            "analyze-execution",
            "--view",
            "scan",
            "--language",
            "csharp",
        ],
    );
    assert!(ok, "{error}");
    let (ok, _, _) = cli(
        t.path(),
        &[
            "--json",
            "analyze-execution",
            "--view",
            "scan",
            "--language",
            "csharp",
            "--csharp-define",
            "DIFFERENT",
        ],
    );
    assert!(!ok);
    let (ok, sync, error) = cli(t.path(), &["sync", "--calls", "--require-ready"]);
    assert!(ok, "{error}");
    assert_eq!(sync["profile"]["language"], "csharp");
    let (ok, sync, error) = cli(t.path(), &["sync", "--calls", "--require-ready"]);
    assert!(ok, "{error}");
    assert_eq!(sync["sync"], "up-to-date");
    assert!(!t.path().join("EXECUTED").exists());
    std::fs::write(
        t.path().join(".briefcase/sync.json"),
        "{\"language\":\"csharp\",\"dotnet\":\"./evil\"}",
    )
    .unwrap();
    let (_, result, _) = cli(t.path(), &["sync"]);
    assert_eq!(result["sync"], "needs_configuration");
}

#[cfg(unix)]
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn hostile_dotnet_startup_environment_and_implicit_conversions_are_isolated() {
    use std::process::Command;
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "App.cs",
                "public class App { public static int Run(){return Store.Work();}}",
            ),
            (
                "Store.cs",
                "public class Store {public static int Work(){return 1;}}",
            ),
        ],
    );
    let result = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(t.path())
        .env_remove("BRIEFCASE_DB")
        .env("DOTNET_STARTUP_HOOKS", t.path().join("hostile.dll"))
        .env("DOTNET_ADDITIONAL_DEPS", t.path().join("hostile.deps.json"))
        .env("DOTNET_SHARED_STORE", t.path())
        .env("DOTNET_ROOT", t.path())
        .env("CORECLR_ENABLE_PROFILING", "1")
        .env("CORECLR_PROFILER", "{BAD-PROFILER}")
        .env("CORECLR_PROFILER_PATH", t.path().join("hostile.so"))
        .args([
            "--json",
            "--db",
            t.path().join("test.db").to_str().unwrap(),
            "scan",
            ".",
            "--language",
            "csharp",
            "--calls",
            "--label",
            "base",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        output["diagnostics"]["call_coverage"]["resolution_complete"],
        true
    );
    source(t.path(), &[("App.cs", "public struct Value { public static implicit operator int(Value v){return 1;} public static Value operator ++(Value v){return v;} } public class App { public static int Run(){Value value=default; value++; int result=value; return result;} }")]);
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["predicate"] == "CALLS_STATIC" && u["reason"] == "unsupported_operation"));
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn quotas_toolchain_failure_and_timeout_create_no_candidate() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(), &[("App.cs", "public class App {}")]);
    assert!(csharp::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            dotnet: t.path().join("missing"),
            ..Default::default()
        }
    )
    .is_err());
    let many = (0..4097)
        .map(|i| format!("class Type{i} {{}}\n"))
        .collect::<String>();
    source(t.path(), &[("App.cs", &many)]);
    assert!(csharp::produce(capture(t.path()), "native-tests", 1, &Default::default()).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = t.path().join("slow-dotnet");
        std::fs::write(&executable, "#!/bin/sh\n/usr/bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let start = std::time::Instant::now();
        let failure = csharp::produce(
            capture(t.path()),
            "native-tests",
            1,
            &ScanOptions {
                dotnet: executable,
                timeout: Duration::from_secs(1),
                ..Default::default()
            },
        )
        .err()
        .unwrap()
        .to_string();
        assert!(failure.contains("timed out"), "{failure}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
