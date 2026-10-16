//! Clang C/C++ acceptance through retained source and the canonical ledger.
mod native_language_support;
use native_language_support::*;
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use briefcase::architecture::{
    capture::Capture,
    clang::{self, Language, ScanOptions},
    contracts::{rows, strings, text},
    history,
    ledger::Ledger,
    mcp, query, recall,
};

fn calls(language: Language) -> ScanOptions {
    ScanOptions {
        language,
        calls: true,
        ..Default::default()
    }
}
fn prepared(
    root: &std::path::Path,
    options: &ScanOptions,
) -> briefcase::architecture::ledger::Prepared {
    clang::produce(capture(root), "native-tests", 1, options).unwrap()
}

#[test]
fn c_includes_declarations_external_linkage_static_functions_and_retained_execution() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("src");
    source(&root, &[("app.c", "#include \"store.h\"\nstatic int helper(void){return 1;}\nint run(void){return save()+helper();}\n"), ("store.h", "int save(void);\n"), ("store.c", "#include \"store.h\"\nint save(void){return 2;}\n")]);
    let p = prepared(&root, &calls(Language::C));
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
    assert!(pairs(&p.bundle, "IMPORTS").contains(&("app.c".into(), "store.h".into())));
    assert!(pairs(&p.bundle, "REFERENCES").contains(&("app.c::run".into(), "store.c::save".into())));
    assert_eq!(
        pairs(&p.bundle, "CALLS_STATIC"),
        BTreeSet::from([
            ("app.c::run".into(), "store.c::save".into()),
            ("app.c::run".into(), "app.c::helper".into())
        ])
    );
    let mut l = Ledger::open(&t.path().join("ledger.db")).unwrap();
    publish(&mut l, "base", |g| {
        clang::produce(capture(&root), "native-tests", g, &Default::default()).unwrap()
    });
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        clang::analyze(&mut l, &options("base"), &Default::default()).unwrap()["reused"],
        false
    );
    assert_eq!(
        clang::analyze(&mut l, &options("base"), &Default::default()).unwrap()["reused"],
        true
    );
    proof_and_retraction(&mut l, "app.c::run", "store.c::save");
}

#[test]
fn cpp_namespaces_overloads_types_methods_final_qualified_calls_and_constructor_boundaries() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(), &[("api.hpp", "namespace Store { int save(int); int save(double); struct Base {virtual int work(){return 1;}}; struct Final final:Base {int work() override {return 2;} int plain(){return 3;} }; }\n"), ("store.cc", "#include \"api.hpp\"\nnamespace Store { int save(int i){return i;} int save(double d){return 1;} }\n"), ("app.cpp", "#include \"api.hpp\"\nint run(Store::Final &f){return Store::save(1)+Store::save(1.0)+f.work()+f.plain()+f.Base::work();}\n")]);
    let p = prepared(t.path(), &calls(Language::Cpp));
    let graph = pairs(&p.bundle, "CALLS_STATIC");
    for to in [
        "store.cc::Store::save",
        "api.hpp::Store::Final::work",
        "api.hpp::Store::Final::plain",
        "api.hpp::Store::Base::work",
    ] {
        assert!(
            graph.contains(&("app.cpp::run".into(), to.into())),
            "{graph:?}"
        );
    }
    assert_eq!(
        rows(&p.bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
            .count(),
        5
    );
    assert!(pairs(&p.bundle, "IMPORTS").contains(&("app.cpp".into(), "api.hpp".into())));
    assert!(!pairs(&p.bundle, "REFERENCES").is_empty());
    source(
        t.path(),
        &[(
            "app.cpp",
            "struct Obj { Obj(int){} ~Obj(){} }; int run(){Obj x(1); return 2;}\n",
        )],
    );
    let p = prepared(t.path(), &calls(Language::Cpp));
    assert_eq!(
        p.diagnostics["call_coverage"]["resolution_complete"], false,
        "Implicit construction/destruction are not complete execution"
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn headers_template_include_files_and_dependent_instantiations_are_retained_without_exact_calls() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "app.cpp",
                "#include \"api.hpp\"\nint run(){return identity<int>(1);}\n",
            ),
            (
                "api.hpp",
                "#pragma once\ntemplate<class T> T identity(T);\n#include \"identity.tpp\"\n",
            ),
            (
                "identity.tpp",
                "template<class T> T identity(T x){return x;}\n",
            ),
            ("types.hxx", "struct Standalone {};\n"),
            ("names.inc", "enum Code { A, B };\n"),
        ],
    );
    let p = prepared(t.path(), &calls(Language::Cpp));
    assert_eq!(p.diagnostics["files_attempted"], 5);
    assert!(pairs(&p.bundle, "IMPORTS").contains(&("api.hpp".into(), "identity.tpp".into())));
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(rows(&p.bundle, "entity_versions")
        .iter()
        .any(|e| text(e, "canonical_key").starts_with("identity.tpp::identity")));
    assert!(rows(&p.bundle, "entity_versions")
        .iter()
        .any(|e| e["canonical_key"] == "types.hxx"));
    assert!(
        ["ipp", "tpp", "inl", "inc", "tcc", "txx", "hh", "hpp", "hxx", "h++", "cppm", "ixx"]
            .iter()
            .all(
                |extension| briefcase::architecture::source::code_extension(&format!(
                    "file.{extension}"
                ))
                .is_some()
            )
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn macros_conditionals_indirect_virtual_lambda_unevaluated_and_external_targets_are_unknown() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(), &[("app.c", "int target(void){return 1;}\n#define CALL() target()\nint run(int (*callback)(void)){return CALL()+callback()+sizeof(target());}\n")]);
    let p = prepared(t.path(), &calls(Language::C));
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    for reason in [
        "unsupported_generated_source",
        "dynamic_property",
        "unsupported_context",
    ] {
        assert!(rows(&p.bundle, "unresolved_references")
            .iter()
            .any(|u| u["predicate"] == "CALLS_STATIC" && u["reason"] == reason));
    }
    std::fs::remove_file(t.path().join("app.c")).unwrap();
    source(t.path(), &[("app.cpp", "struct Base {virtual int work(){return 1;}}; int target(){return 1;} int run(Base &b){auto callback=[](){return target();}; return b.work()+callback();}\n")]);
    let p = prepared(t.path(), &calls(Language::Cpp));
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["reason"] == "trait_dispatch"));
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["reason"] == "dynamic_property"));
    source(
        t.path(),
        &[(
            "app.cpp",
            "#include <not_retained_external>\nint run(){return 1;}\n",
        )],
    );
    let p = prepared(t.path(), &calls(Language::Cpp));
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn duplicate_bodies_conflicting_signatures_internal_linkage_and_c_linkage_are_not_guessed() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "app.c",
                "int target(void); int run(void){return target();}\n",
            ),
            ("a.c", "int target(void){return 1;}\n"),
            ("b.c", "int target(void){return 2;}\n"),
        ],
    );
    let p = prepared(t.path(), &calls(Language::C));
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["predicate"] == "CALLS_STATIC"
            && u["status"] == "ambiguous"
            && strings(&u["candidate_ids"]).len() == 2));
    std::fs::remove_file(t.path().join("b.c")).unwrap();
    source(t.path(), &[("a.c", "double target(double x){return x;}\n")]);
    let p = prepared(t.path(), &calls(Language::C));
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    source(
        t.path(),
        &[
            (
                "app.c",
                "static int target(void){return 1;} int run(void){return target();}\n",
            ),
            (
                "a.c",
                "static int target(void){return 2;} int other(void){return target();}\n",
            ),
        ],
    );
    let p = prepared(t.path(), &calls(Language::C));
    assert_eq!(
        pairs(&p.bundle, "CALLS_STATIC"),
        BTreeSet::from([
            ("app.c::run".into(), "app.c::target".into()),
            ("a.c::other".into(), "a.c::target".into())
        ])
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn include_roots_source_selection_defines_target_standard_syntax_and_utf8_spans_are_pinned() {
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "src/app.c",
                "// é\r\n#include <api.h>\r\nint run(void){return target();}\r\n",
            ),
            (
                "include/api.h",
                "static inline int target(void){return VALUE;}\n",
            ),
            ("other.c", "int other(void){return 1;}\n"),
        ],
    );
    let o = ScanOptions {
        source_roots: vec!["src".into()],
        include_roots: vec!["include".into()],
        defines: vec!["VALUE=1".into()],
        calls: true,
        target: Some("x86_64-unknown-linux-gnu".into()),
        standard: Some("c11".into()),
        ..Default::default()
    };
    let p = prepared(t.path(), &o);
    assert!(pairs(&p.bundle, "CALLS_STATIC")
        .contains(&("src/app.c::run".into(), "include/api.h::target".into())));
    assert_eq!(p.diagnostics["provider"]["standard"], "c11");
    assert!(strings(&p.diagnostics["omitted_source_files"]).contains(&"other.c".into()));
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
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
    let input = capture(t.path());
    let path = text(artifact, "path");
    let start = proof["locator"]["start"].as_u64().unwrap() as usize;
    let end = proof["locator"]["end"].as_u64().unwrap() as usize;
    assert!(std::str::from_utf8(&input.files[&path][start..end]).is_ok());
    source(t.path(), &[("include/api.h", "#if FEATURE\nstatic inline int target(void){return 1;}\n#else\nstatic inline int target(void){return 2;}\n#endif\n")]);
    let p = prepared(
        t.path(),
        &ScanOptions {
            defines: vec!["FEATURE=1".into()],
            ..o.clone()
        },
    );
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(clang::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            include_roots: vec!["../escape".into()],
            ..o.clone()
        }
    )
    .is_err());
    assert!(clang::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            defines: vec!["X=\"evil\"".into()],
            ..o.clone()
        }
    )
    .is_err());
    assert!(clang::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            standard: Some("gnu17".into()),
            ..o.clone()
        }
    )
    .is_err());
    let p = prepared(
        t.path(),
        &ScanOptions {
            syntax_only: true,
            calls: false,
            ..o
        },
    );
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn c_cpp_policy_comparison_recall_negative_paths_and_corrections_share_existing_services() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("source");
    source(
        &root,
        &[
            (
                "app.c",
                "int target(void); int run(void){return target();} int idle(void){return 0;}\n",
            ),
            ("target.c", "int target(void){return 1;}\n"),
            ("README.md", "retained-clang-source-token"),
        ],
    );
    let policy = root.join("policy.json");
    std::fs::write(&policy, json!({"authority":"owner","revision":"1","assignments":{"app.c":"App","target.c":"Store"},"rules":[{"id":"reverse","from":"target.c","to":"app.c","mode":"report-only"}]}).to_string()).unwrap();
    let mut l = Ledger::open(&t.path().join("ledger.db")).unwrap();
    let produce = |g| {
        clang::produce(
            Capture::read(&root, Some(&policy)).unwrap(),
            "native-tests",
            g,
            &calls(Language::C),
        )
        .unwrap()
    };
    publish(&mut l, "base", produce);
    assert_eq!(
        query::policy(&l, &options("base")).unwrap()["response"]["results"][0]["conclusion"],
        "pass"
    );
    assert_eq!(
        query::reachability(&l, &options("base"), "app.c::idle", "target.c::target").unwrap()
            ["response"]["results"][0]["conclusion"],
        "absent"
    );
    assert!(recall::recall(
        &l,
        "retained-clang-source-token",
        &options("base"),
        4000,
        false
    )
    .unwrap()
    .to_string()
    .contains("README.md"));
    let before = l.load("base").unwrap();
    std::fs::rename(root.join("target.c"), root.join("moved.c")).unwrap();
    publish(&mut l, "moved", produce);
    let after = l.load("moved").unwrap();
    history::prepare(&mut l, &before, &after).unwrap();
    assert!(query::compare(&l, "base", &options("moved"))
        .unwrap()
        .to_string()
        .contains("RENAMED_FROM"));
    let answer = mcp::dispatch(
        &l,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"architecture_callgraph","arguments":{"view":"base"}}}),
    );
    assert!(answer["result"]["structuredContent"].is_object());
    proof_and_retraction(&mut l, "app.c::run", "target.c::target");
    let removed = rows(&l.load("moved").unwrap().bundle, "assertions")
        .iter()
        .find(|a| a["proposition"]["predicate"] == "DECLARES")
        .map(|a| text(a, "id"))
        .unwrap();
    l.retract(&removed, "corrected declaration").unwrap();
    l.rebuild("moved").unwrap();
    assert!(clang::analyze(&mut l, &options("moved"), &Default::default()).is_err());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn cli_c_and_cpp_scan_sync_replay_and_mixed_markers_need_configuration() {
    let t = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(t.path())
        .status()
        .unwrap()
        .success());
    source(t.path(), &[("app.c", "int target(void){return 1;} int run(void){return target();}\n"), ("Makefile", "all:\n\ttouch EXECUTED\n"), ("compile_commands.json", "[{\"directory\":\"/\",\"file\":\"app.c\",\"arguments\":[\"/evil/plugin\",\"-Xclang\",\"-load\",\"plugin.so\"]}]")]);
    let (ok, scan, error) = cli(
        t.path(),
        &[
            "--json",
            "scan",
            ".",
            "--language",
            "c",
            "--label",
            "base",
            "--cc-standard",
            "c11",
        ],
    );
    assert!(ok, "{error}");
    assert_eq!(scan["status"], "published");
    let (ok, _, error) = cli(
        t.path(),
        &[
            "--json",
            "analyze-execution",
            "--view",
            "base",
            "--language",
            "c",
        ],
    );
    assert!(ok, "{error}");
    let (ok, _, _) = cli(
        t.path(),
        &[
            "--json",
            "analyze-execution",
            "--view",
            "base",
            "--language",
            "cpp",
        ],
    );
    assert!(!ok);
    let (ok, sync, error) = cli(t.path(), &["sync", "--calls", "--require-ready"]);
    assert!(ok, "{error}");
    assert_eq!(sync["profile"]["language"], "c");
    let (_, sync, _) = cli(t.path(), &["sync", "--calls"]);
    assert_eq!(sync["sync"], "up-to-date");
    assert!(!t.path().join("EXECUTED").exists());
    source(t.path(), &[("app.cpp", "int cpp(){return 1;}\n")]);
    let (_, sync, _) = cli(t.path(), &["sync"]);
    assert_eq!(sync["sync"], "needs_configuration");
    source(
        t.path(),
        &[(
            ".briefcase/sync.json",
            "{\"language\":\"cpp\",\"calls\":true,\"cc_standard\":\"c++20\"}",
        )],
    );
    let (ok, sync, error) = cli(t.path(), &["sync", "--require-ready"]);
    assert!(ok, "{error}");
    assert_eq!(sync["profile"]["language"], "cpp");
    source(
        t.path(),
        &[(
            ".briefcase/sync.json",
            "{\"language\":\"c\",\"clang\":\"./evil\"}",
        )],
    );
    let (_, sync, _) = cli(t.path(), &["sync"]);
    assert_eq!(sync["sync"], "needs_configuration");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn compile_time_contexts_implicit_operators_constructors_and_modules_remain_bounded() {
    let t = tempfile::tempdir().unwrap();
    source(t.path(), &[("app.cpp", "int target(){return 1;} struct Op {int operator()(){return 1;}}; int run(Op &op){static_assert(sizeof(target())>0); if constexpr(false){return target();} return op()+noexcept(target());}")]);
    let p = prepared(
        t.path(),
        &ScanOptions {
            standard: Some("c++20".into()),
            ..calls(Language::Cpp)
        },
    );
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    source(
        t.path(),
        &[(
            "app.cpp",
            "struct Obj {Obj(int){} }; int run(){return Obj(1),0;}",
        )],
    );
    let p = prepared(t.path(), &calls(Language::Cpp));
    assert!(pairs(&p.bundle, "CALLS_STATIC")
        .contains(&("app.cpp::run".into(), "app.cpp::Obj::Obj".into())));
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    source(
        t.path(),
        &[(
            "part.cppm",
            "export module Part; export int target(){return 1;}",
        )],
    );
    let p = prepared(t.path(), &calls(Language::Cpp));
    assert!(strings(&p.diagnostics["omitted_source_files"]).contains(&"part.cppm".into()));
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    assert!(clang::produce(
        capture(t.path()),
        "native-tests",
        1,
        &ScanOptions {
            defines: vec!["VALUE=1".into(), "VALUE=2".into()],
            ..calls(Language::Cpp)
        }
    )
    .is_err());
}

#[cfg(unix)]
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn compiler_environment_flags_and_repository_response_files_are_never_loaded() {
    use std::process::Command;
    let t = tempfile::tempdir().unwrap();
    source(
        t.path(),
        &[
            (
                "app.c",
                "int target(void){return 1;} int run(void){return target();}",
            ),
            ("evil.rsp", "-Xclang -load hostile.so"),
            ("CMakeLists.txt", "execute_process(COMMAND touch EXECUTED)"),
        ],
    );
    let result = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(t.path())
        .env_remove("BRIEFCASE_DB")
        .env("CPATH", t.path())
        .env("C_INCLUDE_PATH", t.path())
        .env("CPLUS_INCLUDE_PATH", t.path())
        .env("CCC_OVERRIDE_OPTIONS", "^-Xclang^-load^hostile.so")
        .env("CLANG_CONFIG_FILE_USER_DIR", t.path())
        .env("CLANG_CONFIG_FILE_SYSTEM_DIR", t.path())
        .env("CC", "./evil")
        .env("CXX", "./evil")
        .env("CFLAGS", "@evil.rsp")
        .env("CXXFLAGS", "@evil.rsp")
        .args([
            "--json",
            "--db",
            t.path().join("test.db").to_str().unwrap(),
            "scan",
            ".",
            "--language",
            "c",
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
    assert!(!t.path().join("EXECUTED").exists());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn unsafe_inputs_system_reads_parse_errors_and_quotas_fail_conservatively() {
    let t = tempfile::tempdir().unwrap();
    let outside = t.path().join("outside");
    source(&outside, &[("private.h", "int stolen(void){return 1;}\n")]);
    let root = t.path().join("source");
    source(
        &root,
        &[(
            "app.c",
            &format!(
                "#include \"{}\"\nint run(void){{return stolen();}}\n",
                outside.join("private.h").display()
            ),
        )],
    );
    let p = prepared(&root, &calls(Language::C));
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    source(
        &root,
        &[("app.c", "#include <stdio.h>\nint run(void){return 0;}\n")],
    );
    let p = prepared(&root, &calls(Language::C));
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    source(&root, &[("app.c", "int broken( {\n")]);
    let p = prepared(&root, &calls(Language::C));
    assert_eq!(p.diagnostics["files_completed"], 0);
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    source(
        &root,
        &[(
            "app.c",
            &(0..4097)
                .map(|i| format!("int field{i};\n"))
                .collect::<String>(),
        )],
    );
    assert!(clang::produce(capture(&root), "native-tests", 1, &Default::default()).is_err());
    assert!(clang::produce(
        capture(&root),
        "native-tests",
        1,
        &ScanOptions {
            clang: root.join("missing"),
            ..Default::default()
        }
    )
    .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = t.path().join("slow-clang");
        std::fs::write(&executable, "#!/bin/sh\n/usr/bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let start = std::time::Instant::now();
        let e = clang::produce(
            capture(&root),
            "native-tests",
            1,
            &ScanOptions {
                clang: executable,
                timeout: Duration::from_secs(1),
                ..Default::default()
            },
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("timed out"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
