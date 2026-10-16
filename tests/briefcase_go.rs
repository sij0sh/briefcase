//! Go producer acceptance through native toolchains, canonical ledger and CLI.
mod native_language_support;
use native_language_support::*;
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use briefcase::architecture::{
    capture::Capture,
    contracts::{rows, strings, text},
    go::{self, ScanOptions},
    history, java,
    ledger::Ledger,
    mcp, passes, query, recall, retrieval,
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
    go::produce(capture(root), "native-tests", 1, options).unwrap()
}
#[test]
fn packages_aliases_generics_methods_go_defer_and_retained_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root,&[
        ("go.mod","module demo\ngo 1.22\n"),
        ("app.go","package main\nimport s \"demo/store\"\n// café before a call\nfunc main(){s.Save(); go s.Save(); defer s.Save(); local[int](1)}\nfunc local[T any](value T){}\n"),
        ("store/save.go","package store\nfunc Save(){helper()}\n"),
        ("store/helper.go","package store\nfunc helper(){}\n"),
        ("store/method.go","package store\ntype Store struct{}\nfunc (s *Store) Commit(){Save()}\nfunc concrete(s *Store){s.Commit()}\n"),
    ]);
    let mut l = Ledger::open(&temp.path().join("briefcase.db")).unwrap();
    let d = publish(&mut l, "base", |g| {
        go::produce(capture(&root), "native-tests", g, &ScanOptions::default()).unwrap()
    });
    assert_eq!(d["provider"]["name"], "go-ast-types");
    assert_eq!(d["provider"]["executes_source"], false);
    let before = l.load("base").unwrap();
    assert!(pairs(&before.bundle, "CALLS_STATIC").is_empty());
    assert!(pairs(&before.bundle, "DEPENDS_ON").contains(&(
        "go-package::.::main".into(),
        "go-package::store::store".into()
    )));
    assert!(pairs(&before.bundle, "IMPORTS")
        .contains(&("app.go".into(), "go-package::store::store".into())));
    std::fs::remove_dir_all(&root).unwrap();
    let report = go::analyze(&mut l, &options("base"), &ScanOptions::default()).unwrap();
    assert_eq!(report["coverage"]["resolution_complete"], true, "{report}");
    let bundle = l.load("base").unwrap().bundle;
    assert_eq!(
        pairs(&bundle, "CALLS_STATIC"),
        BTreeSet::from([
            ("app.go::main".into(), "store/save.go::Save".into()),
            ("app.go::main".into(), "app.go::local".into()),
            (
                "store/save.go::Save".into(),
                "store/helper.go::helper".into()
            ),
            (
                "store/method.go::Store::Commit".into(),
                "store/save.go::Save".into()
            ),
            (
                "store/method.go::concrete".into(),
                "store/method.go::Store::Commit".into()
            ),
        ])
    );
    assert_eq!(
        rows(&bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
            .count(),
        7
    );
    assert_eq!(
        go::analyze(&mut l, &options("base"), &ScanOptions::default()).unwrap()["reused"],
        true
    );
    assert!(java::analyze(&mut l, &options("base"), &Default::default()).is_err());
    let mcp_report = mcp::dispatch(
        &l,
        &json!({"id":1,"method":"tools/call","params":{"name":"architecture_reachability","arguments":{"view":"base","from":"app.go::main","to":"store/helper.go::helper"}}}),
    );
    assert_eq!(
        mcp_report["result"]["structuredContent"]["response"]["results"][0]["conclusion"],
        "present"
    );
    assert!(recall::recall(&l, "café", &options("base"), 16384, false)
        .unwrap()
        .to_string()
        .contains("café"));
    let retrieved = retrieval::retrieve(&l, "main café", &options("base"), 16384, 0).unwrap();
    l.rebuild("base").unwrap();
    assert_retained_retrieval(
        &l,
        &retrieved,
        &retrieval::retrieve(&l, "main café", &options("base"), 16384, 0).unwrap(),
    );
    proof_and_retraction(&mut l, "app.go::main", "store/helper.go::helper");
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn indirect_interface_closure_builtin_and_external_calls_remain_unknown() {
    let cases = [
        "package app\nfunc target(){}\nfunc run(target func()){target()}\n",
        "package app\nfunc target(){}\nfunc run(){pointer:=target;pointer()}\n",
        "package app\ntype Store interface{Save()}\nfunc run(s Store){s.Save()}\n",
        "package app\nfunc target(){}\nfunc run(){f:=func(){target()};f()}\n",
        "package app\nfunc target(){}\nvar value=target()\n",
        "package app\nimport \"fmt\"\nfunc run(){fmt.Println(1)}\n",
        "package app\nfunc run(){println(1)}\n",
    ];
    for (i, code) in cases.iter().enumerate() {
        let t = tempfile::tempdir().unwrap();
        source(t.path(), &[("app.go", code)]);
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
        assert!(!rows(&p.bundle, "unresolved_references").is_empty());
    }
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn build_constraints_platforms_tests_and_roots_pin_selection() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            ("go.mod", "module demo\ngo 1.22\n"),
            ("app.go", "package app\nfunc run(){target()}\n"),
            ("target_linux.go", "package app\nfunc target(){}\n"),
            ("target_windows.go", "package app\nfunc target(){}\n"),
            (
                "feature.go",
                "//go:build feature\n\npackage app\nfunc extra(){target()}\n",
            ),
            ("app_test.go", "package app\nfunc TestLocal(){target()}\n"),
            (
                "external_test.go",
                "package app_test\nimport \"demo\"\nfunc test(){app.Export()}\n",
            ),
        ],
    );
    let a = prepared(
        t.path(),
        &ScanOptions {
            goos: Some("linux".into()),
            ..calls()
        },
    );
    assert!(pairs(&a.bundle, "CALLS_STATIC")
        .contains(&("app.go::run".into(), "target_linux.go::target".into())));
    assert_eq!(a.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(strings(&a.diagnostics["omitted_source_files"]).contains(&"target_windows.go".into()));
    let b = prepared(
        t.path(),
        &ScanOptions {
            goos: Some("windows".into()),
            tags: vec!["feature".into()],
            ..calls()
        },
    );
    assert!(pairs(&b.bundle, "CALLS_STATIC")
        .contains(&("app.go::run".into(), "target_windows.go::target".into())));
    assert!(pairs(&b.bundle, "CALLS_STATIC").contains(&(
        "feature.go::extra".into(),
        "target_windows.go::target".into()
    )));
    assert_ne!(a.bundle["build_contexts"], b.bundle["build_contexts"]);
    let c = prepared(
        t.path(),
        &ScanOptions {
            goos: Some("linux".into()),
            tests: true,
            ..calls()
        },
    );
    assert!(pairs(&c.bundle, "CALLS_STATIC").contains(&(
        "app_test.go::TestLocal".into(),
        "target_linux.go::target".into()
    )));
    let d = prepared(
        t.path(),
        &ScanOptions {
            source_roots: vec!["src".into()],
            ..calls()
        },
    );
    assert!(pairs(&d.bundle, "CALLS_STATIC").is_empty());
    assert!(!strings(&d.diagnostics["omitted_source_files"]).is_empty());
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn workspace_modules_ambiguity_replacements_cgo_and_linkname_are_honest() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            ("go.work", "go 1.22\nuse (\n ./a\n ./b\n)\n"),
            ("a/go.mod", "module example/a\ngo 1.22\n"),
            ("b/go.mod", "module example/b\ngo 1.22\n"),
            (
                "a/app.go",
                "package a\nimport \"example/b\"\nfunc Run(){b.Save()}\n",
            ),
            ("b/save.go", "package b\nfunc Save(){}\n"),
        ],
    );
    let a = prepared(t.path(), &calls());
    assert!(pairs(&a.bundle, "CALLS_STATIC")
        .contains(&("a/app.go::Run".into(), "b/save.go::Save".into())));
    source(
        t.path(),
        &[(
            "a/go.mod",
            "module example/a\ngo 1.22\nreplace example/b => ../../outside\n",
        )],
    );
    let b = prepared(t.path(), &calls());
    assert!(pairs(&b.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(b.diagnostics["call_coverage"]["resolution_complete"], false);
    let cases=[
        &[("app.go","package app\nimport \"C\"\nfunc target(){}\n")][..],
        &[("app.go","package app\n//go:linkname target external.target\nfunc target(){}\nfunc run(){target()}\n")][..],
        &[("a.go","package app\nfunc target(){}\nfunc run(){target()}\n"),("b.go","package other\nfunc target(){}\n")][..],
        &[("a.go","package app\nfunc target(){}\nfunc run(){target()}\n"),("b.go","package app\nfunc broken(\n")][..],
    ];
    for files in cases {
        let t = tempfile::tempdir().unwrap();
        source(t.path(), files);
        let p = prepared(t.path(), &calls());
        assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
        assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    }
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            ("go.work", "go 1.22\nuse (\n ./a\n ./b\n ./c\n)\n"),
            ("a/go.mod", "module shared\n"),
            ("a/save.go", "package shared\nfunc Save(){}\n"),
            ("b/go.mod", "module shared\n"),
            ("b/save.go", "package shared\nfunc Save(){}\n"),
            ("c/go.mod", "module client\n"),
            (
                "c/app.go",
                "package client\nimport \"shared\"\nfunc Run(){shared.Save()}\n",
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
fn syntax_failures_syntax_only_utf8_spans_and_limits() {
    let t = tempfile::tempdir().unwrap();
    let code = "\u{feff}// café\r\npackage app\r\nfunc café(){}\r\nfunc run(){café()}\r\n";
    source(t.path(), &[("app.go", code)]);
    let p = prepared(t.path(), &calls());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], true);
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
        "café()"
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
    for options in [
        ScanOptions {
            source_roots: vec!["../outside".into()],
            ..Default::default()
        },
        ScanOptions {
            tags: vec!["x".into(), "x".into()],
            ..Default::default()
        },
        ScanOptions {
            goarch: Some("invalid".into()),
            ..Default::default()
        },
    ] {
        assert!(go::produce(capture(t.path()), "invalid", 1, &options).is_err());
    }
    let quota = (0..4097)
        .map(|i| format!("func f{i}(){{}}\n"))
        .collect::<String>();
    source(t.path(), &[("app.go", &format!("package app\n{quota}"))]);
    assert!(
        go::produce(capture(t.path()), "quota", 1, &Default::default())
            .err()
            .unwrap()
            .to_string()
            .contains("definition limit")
    );
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn range_function_iteration_is_not_silently_complete() {
    let version = std::process::Command::new(
        std::env::var_os("BRIEFCASE_GO").unwrap_or_else(|| "go".into()),
    )
    .arg("version")
    .output()
    .unwrap();
    let version = String::from_utf8_lossy(&version.stdout);
    let minor = version
        .split_whitespace()
        .find_map(|s| s.strip_prefix("go1."))
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    if minor < 23 {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    source(t.path(),&[
        ("go.mod","module demo\ngo 1.23\n"),
        ("app.go","package app\nfunc iterator(yield func(int) bool){}\nfunc run(){for range iterator {}}\n"),
    ]);
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["predicate"] == "CALLS_STATIC" && u["reason"] == "unsupported_operation"));
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn import_cycles_retain_dependency_evidence_without_invented_calls() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            ("go.mod", "module demo\ngo 1.22\n"),
            ("a/a.go", "package a\nimport \"demo/b\"\nfunc A(){b.B()}\n"),
            ("b/b.go", "package b\nimport \"demo/a\"\nfunc B(){a.A()}\n"),
        ],
    );
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(
        rows(&p.bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "IN_CYCLE")
            .count(),
        2
    );
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn internal_visibility_module_versions_standard_shadowing_and_dot_imports() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            ("go.mod", "module demo\ngo 1.22\n"),
            (
                "lib/private.go",
                "package lib\nimport . \"demo/lib/internal/store\"\nfunc Run(){Save()}\n",
            ),
            (
                "lib/internal/store/save.go",
                "package store\nfunc Save(){}\n",
            ),
            (
                "app.go",
                "package app\nimport \"demo/lib/internal/store\"\nfunc run(){store.Save()}\n",
            ),
        ],
    );
    let p = prepared(t.path(), &calls());
    let edges = pairs(&p.bundle, "CALLS_STATIC");
    assert!(edges.contains(&(
        "lib/private.go::Run".into(),
        "lib/internal/store/save.go::Save".into()
    )));
    assert!(!edges.iter().any(|(from, _)| from == "app.go::run"));
    source(t.path(), &[("go.mod", "module demo\ngo 1.99\n")]);
    assert!(go::produce(capture(t.path()), "future", 1, &calls()).is_err());
    let t = tempfile::tempdir().unwrap();
    source(t.path(),&[
        ("go.mod","module fmt\ngo 1.22\n"),
        ("printf.go","package fmt\nfunc Println(...int){}\n"),
        ("cmd/main.go","package main\nimport \"fmt\"\nfunc main(){fmt.Println(1);local()}\nfunc local(){}\n"),
    ]);
    let p = prepared(t.path(), &calls());
    assert!(pairs(&p.bundle, "CALLS_STATIC")
        .contains(&("cmd/main.go::main".into(), "cmd/main.go::local".into())));
    assert!(!pairs(&p.bundle, "CALLS_STATIC")
        .iter()
        .any(|(_, to)| to == "printf.go::Println"));
    assert!(!pairs(&p.bundle, "REFERENCES")
        .iter()
        .any(|(_, to)| to == "go-package::.::fmt"));
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn policy_negative_paths_passes_retraction_and_revision_comparison() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[(
            "app.go",
            "package app\nfunc run(){target()}\nfunc target(){}\nfunc idle(){}\n",
        )],
    );
    source(t.path(), &[("store.go", "package app\nfunc store(){}\n")]);
    let policy = policy(t.path());
    let mut l = Ledger::open(&t.path().join("briefcase.db")).unwrap();
    let produce = |g| {
        go::produce(
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
    assert_eq!(
        query::reachability(&l, &options("base"), "app.go::idle", "app.go::target").unwrap()
            ["response"]["results"][0]["conclusion"],
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
    std::fs::rename(t.path().join("app.go"), t.path().join("new.go")).unwrap();
    publish(&mut l, "moved", produce);
    let before = l.load("base").unwrap();
    let after = l.load("moved").unwrap();
    history::prepare(&mut l, &before, &after).unwrap();
    assert!(query::compare(&l, "base", &options("moved"))
        .unwrap()
        .to_string()
        .contains("RENAMED_FROM"));
    let id = rows(&l.load("moved").unwrap().bundle, "assertions")
        .iter()
        .find(|a| a["proposition"]["predicate"] == "DECLARES")
        .map(|a| text(a, "id"))
        .unwrap();
    l.retract(&id, "corrected declaration").unwrap();
    l.rebuild("moved").unwrap();
    assert!(go::analyze(&mut l, &options("moved"), &Default::default()).is_err());
}
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn cli_scan_sync_autodetection_and_retained_configuration() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("source");
    source(
        &root,
        &[
            ("go.mod", "module demo\ngo 1.22\n"),
            (
                "app.go",
                "package app\nfunc run(){target()}\nfunc target(){}\n",
            ),
        ],
    );
    let db = t.path().join("scan.db");
    let args = [
        "--db",
        db.to_str().unwrap(),
        "--json",
        "scan",
        ".",
        "--language",
        "go",
        "--label",
        "base",
        "--goos",
        "linux",
    ];
    let (ok, _, err) = cli(&root, &args);
    assert!(ok, "{err}");
    let (ok, report, err) = cli(
        &root,
        &[
            "--db",
            db.to_str().unwrap(),
            "--json",
            "analyze-execution",
            "--language",
            "go",
            "--view",
            "base",
        ],
    );
    assert!(ok, "{err}");
    assert_eq!(report["coverage"]["resolution_complete"], true);
    let args = [
        "--project-root",
        root.to_str().unwrap(),
        "sync",
        "--calls",
        "--require-ready",
    ];
    let (ok, first, err) = cli(&root, &args);
    assert!(ok, "{err} {first}");
    assert_eq!(first["profile"]["language"], "go");
    assert_eq!(cli(&root, &args).1["sync"], "up-to-date");
    source(
        &root,
        &[(
            "app.go",
            "package app\nfunc run(){target()}\nfunc target(){ }\n",
        )],
    );
    assert_eq!(cli(&root, &args).1["sync"], "refreshed");
    source(&root, &[("pom.xml", "<project/>\n")]);
    assert_eq!(cli(&root, &args).1["sync"], "needs_configuration");
    source(
        &root,
        &[(
            ".briefcase/sync.json",
            "{\"language\":\"go\",\"goos\":\"linux\",\"go_tags\":[\"feature\"]}",
        )],
    );
    assert_eq!(cli(&root, &args).1["sync"], "refreshed");
    let mut l = Ledger::open_existing(&db).unwrap();
    assert!(go::analyze(
        &mut l,
        &options("base"),
        &ScanOptions {
            goos: Some("windows".into()),
            ..Default::default()
        }
    )
    .is_err());
}
#[cfg(unix)]
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn toolchain_environment_isolation_and_timeout_leave_no_candidate() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let sentinel = t.path().join("executed");
    source(
        t.path(),
        &[
            ("go.mod", "module demo\ngo 1.22\n"),
            (
                "app.go",
                &format!(
                    "package app\nimport \"os\"\nfunc init(){{os.WriteFile({:?},nil,0600)}}\n",
                    sentinel.to_str().unwrap()
                ),
            ),
            ("go.env", "GOTOOLCHAIN=evil\n"),
        ],
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(t.path())
        .env("GOENV", t.path().join("go.env"))
        .env("GOFLAGS", "-toolexec=/missing")
        .env("GOTOOLCHAIN", "evil")
        .args([
            "--db",
            t.path().join("briefcase.db").to_str().unwrap(),
            "--json",
            "scan",
            ".",
            "--language",
            "go",
            "--calls",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!sentinel.exists());
    assert!(!t.path().join("go.sum").exists());
    let sleeper = t.path().join("sleeper");
    std::fs::write(&sleeper, "#!/bin/sh\nexec sleep 5\n").unwrap();
    std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let start = std::time::Instant::now();
    assert!(go::produce(
        capture(t.path()),
        "timeout",
        1,
        &ScanOptions {
            go: sleeper,
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
