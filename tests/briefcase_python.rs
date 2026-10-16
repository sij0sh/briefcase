//! Python producer acceptance through the canonical ledger, queries and CLI.
mod native_language_support;
use native_language_support::assert_retained_retrieval;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path, process::Command};
use briefcase::architecture::{
    capture::Capture,
    contracts::{self, rows, strings, text},
    ledger::Ledger,
    mcp, passes,
    python::{self, ScanOptions},
    query::{self, Options},
    recall, retrieval,
};

fn source(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    for (path, code) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, code).unwrap();
    }
}
fn options(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
fn scan(l: &mut Ledger, root: &Path, label: &str, calls: bool) -> Value {
    let generation = l.begin("python-tests", Some(label)).unwrap();
    let prepared = python::produce(
        Capture::read(root, None).unwrap(),
        "python-tests",
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
fn pairs(bundle: &Value, predicate: &str) -> BTreeSet<(String, String)> {
    let key = |id: &str| {
        text(
            rows(bundle, "entity_versions")
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
    rows(bundle, "assertions")
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
fn cli(root: &Path, args: &[&str]) -> (bool, Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(root)
        .env_remove("BRIEFCASE_DB")
        .args(args)
        .output()
        .unwrap();
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn extensions_packages_relative_alias_reexport_calls_and_retained_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, &[
        ("pyproject.toml", "[project]\nname='demo'\nversion='0.1.0'\n"),
        ("src/shop/__init__.py", "from .payments import charge as pay\n"),
        ("src/shop/payments.py", "from .audit import record\n\ndef charge():\n    return record()\n"),
        ("src/shop/payments.pyi", "def charge() -> None: ...\n"),
        ("src/shop/audit.py", "def record():\n    return None\n"),
        ("src/shop/audit.pyi", "def record() -> None: ...\n"),
        ("app.pyw", "from shop import pay\nimport shop.payments as payment\nimport shop.audit\n# Unicode before a call: café\ndef main():\n    pay()\n    payment.charge()\n    shop.audit.record()\n    return local()\ndef local():\n    return main()\n"),
    ]);
    let mut l = Ledger::open(&temp.path().join("briefcase.db")).unwrap();
    let diagnostics = scan(&mut l, &root, "base", false);
    assert_eq!(diagnostics["provider"]["name"], "cpython-ast-symtable");
    assert_eq!(diagnostics["provider"]["isolated"], true);
    assert_eq!(diagnostics["provider"]["executes_source"], false);
    assert!(
        rows(&diagnostics, "python_diagnostics").is_empty(),
        "{diagnostics}"
    );
    let before = l.load("base").unwrap();
    assert!(pairs(&before.bundle, "CALLS_STATIC").is_empty());
    assert!(rows(&before.bundle, "entity_versions")
        .iter()
        .any(|e| e["kind"] == "package" && e["canonical_key"] == "package::src/shop"));
    assert!(rows(&before.bundle, "entity_versions")
        .iter()
        .any(|e| e["language"] == "python"
            && e["canonical_key"]
                .as_str()
                .is_some_and(|k| k.starts_with("src/shop/payments.pyi::charge"))
            && e["identity_quality"] == "syntax"));
    let imports = pairs(&before.bundle, "IMPORTS");
    for (from, to) in [
        ("app.pyw", "src/shop/__init__.py"),
        ("app.pyw", "src/shop/payments.py"),
        ("app.pyw", "src/shop/audit.py"),
        ("src/shop/__init__.py", "src/shop/payments.py"),
        ("src/shop/payments.py", "src/shop/audit.py"),
    ] {
        assert!(imports.contains(&(from.into(), to.into())), "{imports:?}");
    }
    assert!(!imports.iter().any(|(_, target)| target.ends_with(".pyi")));
    std::fs::remove_dir_all(&root).unwrap();
    let analyzed = python::analyze(&mut l, &options("base"), &ScanOptions::default()).unwrap();
    assert_eq!(analyzed["reused"], false);
    assert_eq!(
        analyzed["coverage"]["resolution_complete"], true,
        "{analyzed}"
    );
    let v = l.load("base").unwrap();
    assert_eq!(
        pairs(&v.bundle, "CALLS_STATIC"),
        BTreeSet::from([
            (
                "app.pyw::main".into(),
                "src/shop/payments.py::charge".into()
            ),
            ("app.pyw::main".into(), "src/shop/audit.py::record".into()),
            ("app.pyw::main".into(), "app.pyw::local".into()),
            ("app.pyw::local".into(), "app.pyw::main".into()),
            (
                "src/shop/payments.py::charge".into(),
                "src/shop/audit.py::record".into()
            ),
        ])
    );
    let calls: Vec<_> = rows(&v.bundle, "assertions")
        .iter()
        .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
        .collect();
    assert_eq!(calls.len(), 6);
    assert!(calls
        .iter()
        .all(|a| a["class"] == "evidence" && a["resolution"]["method"] == "compiler"));
    assert_eq!(
        python::analyze(&mut l, &options("base"), &ScanOptions::default()).unwrap()["reused"],
        true
    );
    let report = query::reachability(
        &l,
        &options("base"),
        "app.pyw::main",
        "src/shop/audit.py::record",
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
        &json!({"id":1,"method":"tools/call","params":{"name":"architecture_reachability","arguments":{"view":"base","from":"app.pyw::main","to":"src/shop/audit.py::record"}}}),
    );
    assert_eq!(
        mcp_report["result"]["structuredContent"]["response"],
        report["response"]
    );
    let recalled = recall::recall(&l, "café", &options("base"), 16384, false).unwrap();
    assert!(recalled.to_string().contains("café"));
    let retrieved = retrieval::retrieve(&l, "main café", &options("base"), 16384, 0).unwrap();
    assert!(retrieved.to_string().contains("café"));
    l.rebuild(&v.id).unwrap();
    assert_retained_retrieval(
        &l,
        &retrieved,
        &retrieval::retrieve(&l, "main café", &options("base"), 16384, 0).unwrap(),
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn nested_async_functions_shadowing_values_methods_and_decorators_are_not_guessed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, &[("app.py", "def target():\n    return None\nasync def asynchronous():\n    target()\ndef outer():\n    def nested():\n        target()\n    nested()\ndef shadow(target):\n    target()\ndef alias():\n    pointer = target\n    pointer()\ndef method(obj):\n    obj.run()\nclass Store:\n    def save(self):\n        target()\ndef wrapper(fn):\n    return fn\n@wrapper\ndef decorated():\n    target()\ndef invokes():\n    decorated()\ndef comprehension():\n    return [target() for x in ()]\n")]);
    let mut l = Ledger::open(&temp.path().join("briefcase.db")).unwrap();
    let diagnostics = scan(&mut l, &root, "base", true);
    assert_eq!(diagnostics["call_coverage"]["resolution_complete"], false);
    assert_eq!(
        pairs(&l.load("base").unwrap().bundle, "CALLS_STATIC"),
        BTreeSet::from([
            ("app.py::asynchronous".into(), "app.py::target".into()),
            ("app.py::outer".into(), "app.py::outer::nested".into()),
            ("app.py::outer::nested".into(), "app.py::target".into()),
        ])
    );
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "app.py::target",
            "app.py::asynchronous"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "unknown"
    );
    assert!(query::callgraph(&l, &options("base"))
        .unwrap()
        .to_string()
        .contains("dynamic_property"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn writes_conditional_imports_wildcards_reflection_and_hooks_are_unknown() {
    let cases = [
        "def target(): pass\ndef other(): pass\ntarget = other\ndef invoke(): target()\n",
        "from lib import target\ndef invoke():\n    target()\n    target = 1\n",
        "from lib import target\ndef invoke():\n    try: pass\n    except Exception as target: pass\n    target()\n",
        "from lib import target\ndef invoke(x):\n    match x:\n        case {'x': target}: pass\n    target()\n",
        "if True:\n    from lib import target\ndef invoke(): target()\n",
        "from lib import *\ndef invoke(): target()\n",
        "import lib\nlib.target = 1\ndef invoke(): lib.target()\n",
        "import lib\nalias = lib\nalias.target = 1\ndef invoke(): lib.target()\n",
        "from lib import target\ndef invoke():\n    global target\n    target = 1\n    target()\n",
        "def target(): pass\ndef outer():\n    def nested():\n        nonlocal target\n        target = 1\n    target = 1\n    target()\n",
        "from lib import target\nimport lib\nlib.target = 1\ndef invoke(): target()\n",
        "from lib import target\ndef invoke(): exec('target = 1'); target()\n",
        "from lib import target\ndef invoke(): globals()['target'] = 1; target()\n",
        "import sys\nfrom lib import target\nsys.path.append('/outside')\ndef invoke(): target()\n",
        "from lib import target\nimport importlib\nimportlib.import_module('lib')\ndef invoke(): target()\n",
        "import lib\nif True:\n    import lib as alias\nalias.target = 1\ndef invoke(): lib.target()\n",
        "from lib import target\nloader = exec\ndef invoke(): loader('x=1'); target()\n",
        "from lib import target\nimport builtins\nloader = builtins.exec\ndef invoke(): loader('x=1'); target()\n",
        "from lib import target\nimport importlib\nloader = importlib.import_module\ndef invoke(): loader('lib'); target()\n",
        "from lib import target\nfrom builtins import exec as execute\ndef invoke(): execute('x=1'); target()\n",
        "from lib import target\nfrom importlib import reload as refresh\ndef invoke(): refresh('lib'); target()\n",
        "from ns import sub\nimport ns\nns.sub.target = 1\ndef invoke(): sub.target()\n",
        "from lib import target\nexecute = exec\nexec = execute\ndef invoke(): exec('x=1'); target()\n",
        "import lib\naliases = [lib]\naliases[0].target = 1\ndef invoke(): lib.target()\n",
    ];
    for (index, code) in cases.iter().enumerate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        source(
            &root,
            &[
                ("app.py", code),
                ("lib.py", "def target(): pass\n"),
                ("ns/sub.py", "def target(): pass\n"),
            ],
        );
        let p = python::produce(
            Capture::read(&root, None).unwrap(),
            "dynamic",
            1,
            &ScanOptions {
                calls: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            pairs(&p.bundle, "CALLS_STATIC").is_empty(),
            "case {index}: {code}\n{:?}",
            pairs(&p.bundle, "CALLS_STATIC")
        );
        assert_eq!(
            p.diagnostics["call_coverage"]["resolution_complete"], false,
            "case {index}"
        );
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn namespace_packages_standalone_stubs_scripts_and_ambiguous_modules() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, &[
        ("ns/util.py", "def work(): pass\n"),
        ("api.pyi", "def declared() -> None: ...\n"),
        ("gui.pyw", "def display(): pass\n"),
        ("app.py", "from ns import util\nfrom api import declared\nimport gui\ndef run():\n    util.work()\n    declared()\n"),
    ]);
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "namespace",
        1,
        &ScanOptions {
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pairs(&p.bundle, "IMPORTS").contains(&("app.py".into(), "ns/util.py".into())));
    assert!(pairs(&p.bundle, "IMPORTS").contains(&("app.py".into(), "api.pyi".into())));
    assert!(!pairs(&p.bundle, "IMPORTS")
        .iter()
        .any(|(_, to)| to == "gui.pyw"));
    assert_eq!(
        pairs(&p.bundle, "CALLS_STATIC"),
        BTreeSet::from([("app.py::run".into(), "ns/util.py::work".into())])
    );
    source(&root, &[("ns.py", "def work(): pass\n")]);
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "namespace",
        2,
        &ScanOptions {
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["predicate"] == "IMPORTS" && u["reason"] == "missing_dependency"));
    source(
        &root,
        &[("ns/__init__.py", "# Regular package competes with ns.py.\n")],
    );
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "namespace",
        3,
        &ScanOptions {
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["status"] == "ambiguous" && !strings(&u["candidate_ids"]).is_empty()));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn syntax_failures_and_explicit_syntax_only_preserve_text_without_complete_guarantees() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            ("bad.py", "def broken(:\n"),
            ("bad.pyi", "class Broken(\n"),
            ("valid.pyw", "def entry(): pass\n"),
            ("lib.py", "def target(): pass\n"),
            ("app.py", "from lib import target\ndef run(): target()\n"),
        ],
    );
    let mut l = Ledger::open(&temp.path().join("briefcase.db")).unwrap();
    let d = scan(&mut l, &root, "partial", true);
    assert_eq!(d["coverage"]["resolution_complete"], false);
    assert_eq!(rows(&d, "python_diagnostics").len(), 2);
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "python-tests",
        2,
        &ScanOptions {
            syntax_only: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pairs(&p.bundle, "IMPORTS").is_empty());
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["coverage"]["resolution_complete"], false);
    assert!(!rows(&p.bundle, "unresolved_references").is_empty());
    assert!(
        recall::recall(&l, "broken", &options("partial"), 16384, false)
            .unwrap()
            .to_string()
            .contains("broken")
    );
    assert!(python::produce(
        Capture::read(&root, None).unwrap(),
        "python-tests",
        2,
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
fn negative_paths_policy_cycles_retraction_and_stable_identity_share_existing_passes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            (
                "orders.py",
                "from payments import charge\ndef submit(): charge()\n",
            ),
            ("payments.py", "def charge(): pass\n"),
            ("unused.py", "def idle(): pass\n"),
        ],
    );
    let policy = temp.path().join("policy.json");
    std::fs::write(&policy, serde_json::to_vec(&json!({"authority":"owner","revision":"1",
        "assignments":{"orders.py":"Checkout"},"rules":[{"id":"reverse","from":"payments.py","to":"orders.py","mode":"report-only"}]})).unwrap()).unwrap();
    let mut l = Ledger::open(&temp.path().join("briefcase.db")).unwrap();
    let publish = |l: &mut Ledger, label: &str| {
        let g = l.begin("python-tests", Some(label)).unwrap();
        let p = python::produce(
            Capture::read(&root, Some(&policy)).unwrap(),
            "python-tests",
            g,
            &ScanOptions {
                calls: true,
                ..Default::default()
            },
        )
        .unwrap();
        let view = l.commit(g, &p).unwrap();
        l.publish(&view, true).unwrap();
    };
    publish(&mut l, "base");
    assert_eq!(
        query::policy(&l, &options("base")).unwrap()["response"]["results"][0]["conclusion"],
        "pass"
    );
    assert_eq!(
        query::reachability(
            &l,
            &options("base"),
            "unused.py::idle",
            "payments.py::charge"
        )
        .unwrap()["response"]["results"][0]["conclusion"],
        "absent"
    );
    assert!(query::domains(&l, &options("base"))
        .unwrap()
        .to_string()
        .contains("Checkout"));
    let original = l.load("base").unwrap().bundle;
    publish(&mut l, "repeat");
    assert_eq!(
        rows(&original, "assertions"),
        rows(&l.load("repeat").unwrap().bundle, "assertions")
    );
    let plan = json!({"schema_version":"1.0","view":"base","passes":[{"id":"dep","operation":"dependencies","dependencies":[]},{"id":"scc","operation":"scc","dependencies":["dep"]},{"id":"reach","operation":"reachability","dependencies":[]}]});
    assert_eq!(
        passes::inspect(&mut l, "base", Some(&plan), true).unwrap()["recomputed_and_verified"],
        true
    );
    source(
        &root,
        &[(
            "payments.py",
            "from orders import submit\ndef charge(): pass\n",
        )],
    );
    publish(&mut l, "changed");
    assert_eq!(
        rows(&l.load("changed").unwrap().bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "IN_CYCLE")
            .count(),
        2
    );
    let report = query::policy(&l, &options("changed")).unwrap();
    assert_eq!(report["response"]["results"][0]["conclusion"], "violation");
    let handle = text(&report["response"]["results"][0], "explanation_handle");
    let proof = query::explain(&l, &handle, 1000, 0).unwrap();
    let corrected = rows(&proof, "support")
        .iter()
        .find(|s| {
            s["assertion"]["proposition"]["predicate"] == "IMPORTS"
                && rows(s, "sources").iter().any(|s| {
                    s["text"]
                        .as_str()
                        .is_some_and(|s| s.contains("from orders"))
                })
        })
        .unwrap();
    let id = text(&corrected["assertion"], "id");
    l.retract(&id, "corrected Python import").unwrap();
    l.rebuild("changed").unwrap();
    assert_eq!(
        query::policy(&l, &options("changed")).unwrap()["response"]["results"][0]["conclusion"],
        "unknown"
    );
    assert!(
        rows(&query::explain(&l, &handle, 1000, 0).unwrap(), "support")
            .iter()
            .any(|s| s["assertion"]["id"] == id && s["currently_retracted"] == true)
    );
    assert!(python::analyze(
        &mut l,
        &Options {
            historical: true,
            ..options("changed")
        },
        &ScanOptions::default()
    )
    .is_err());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn cli_scan_sync_source_roots_and_python_only_autodetection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            (
                "pyproject.toml",
                "[project]\nname='demo'\nversion='0.1.0'\n",
            ),
            (
                "src/app.py",
                "from lib import target\ndef run(): target()\n",
            ),
            ("src/lib.py", "def target(): pass\n"),
        ],
    );
    let db = temp.path().join("scan.db");
    let (ok, scan, error) = cli(
        &root,
        &[
            "--db",
            db.to_str().unwrap(),
            "--json",
            "scan",
            ".",
            "--language",
            "python",
            "--python-source-root",
            "src",
            "--label",
            "python",
        ],
    );
    assert!(ok, "{error}");
    assert_eq!(scan["status"], "published");
    let (ok, analyzed, error) = cli(
        &root,
        &[
            "--db",
            db.to_str().unwrap(),
            "--json",
            "analyze-execution",
            "--language",
            "python",
            "--view",
            "python",
        ],
    );
    assert!(ok, "{error}");
    assert_eq!(analyzed["coverage"]["resolution_complete"], true);
    let args = [
        "--project-root",
        root.to_str().unwrap(),
        "sync",
        "--calls",
        "--require-ready",
    ];
    let (ok, first, error) = cli(&root, &args);
    assert!(ok, "{error} {first}");
    assert_eq!(first["sync"], "refreshed");
    assert_eq!(first["profile"]["language"], "python");
    assert_eq!(cli(&root, &args).1["sync"], "up-to-date");
    source(&root, &[("src/lib.py", "def edited(): pass\n")]);
    assert_eq!(cli(&root, &args).1["sync"], "refreshed");
    source(&root, &[("package.json", "{}")]);
    assert_eq!(cli(&root, &args).1["sync"], "needs_configuration");
    source(
        &root,
        &[(
            ".briefcase/sync.json",
            "{\"language\":\"python\",\"python_source_roots\":[\"src\"]}",
        )],
    );
    assert_eq!(cli(&root, &args).1["sync"], "refreshed");
    std::fs::remove_file(root.join("package.json")).unwrap();
    std::fs::remove_file(root.join("pyproject.toml")).unwrap();
    std::fs::remove_file(root.join(".briefcase/sync.json")).unwrap();
    assert_eq!(cli(&root, &args).1["profile"]["language"], "python");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn isolated_interpreter_never_runs_repository_or_sitecustomize_and_bad_inputs_fail() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let sentinel = temp.path().join("executed");
    let malicious = format!(
        "from pathlib import Path\nPath({:?}).write_text('executed')\n",
        sentinel.to_str().unwrap()
    );
    source(
        &root,
        &[
            ("sitecustomize.py", &malicious),
            ("ast.py", &malicious),
            ("app.pyw", &malicious),
            ("lib.py", "def target(): pass\n"),
        ],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(&root)
        .env("PYTHONPATH", &root)
        .env("PYTHONHOME", &root)
        .args([
            "--db",
            temp.path().join("briefcase.db").to_str().unwrap(),
            "--json",
            "scan",
            ".",
            "--language",
            "python",
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
    assert!(!root.join("__pycache__").exists());
    for options in [
        ScanOptions {
            python: temp.path().join("missing"),
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
        assert!(
            python::produce(Capture::read(&root, None).unwrap(), "invalid", 1, &options).is_err()
        );
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn utf8_bom_crlf_and_annotated_declarations_keep_exact_byte_spans() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let code = "\u{feff}# Unicode separator \u{2028}\r\ndef café():\r\n    return None\r\ndef run():\r\n    café()\r\n";
    source(&root, &[("app.pyw", code)]);
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "unicode",
        1,
        &ScanOptions {
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    contracts::validate(&p.bundle, &p.payloads).unwrap();
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], true);
    let call = rows(&p.bundle, "assertions")
        .iter()
        .find(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
        .unwrap();
    let evidence = rows(&p.bundle, "evidence_refs")
        .iter()
        .find(|e| e["id"] == call["evidence_ref_ids"][0])
        .unwrap();
    let start = evidence["locator"]["start"].as_u64().unwrap() as usize;
    let end = evidence["locator"]["end"].as_u64().unwrap() as usize;
    assert_eq!(&code[start..end], "café()");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn explicit_roots_ambiguous_roots_cyclic_reexports_and_frozen_modules_stay_honest() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            ("src/lib.py", "def target(): pass\n"),
            ("alt/lib.py", "def target(): pass\n"),
            ("app.py", "from lib import target\ndef run(): target()\n"),
        ],
    );
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "roots",
        1,
        &ScanOptions {
            source_roots: vec!["src".into()],
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
    assert!(strings(&p.diagnostics["omitted_source_files"]).contains(&"app.py".into()));
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "roots",
        2,
        &ScanOptions {
            source_roots: vec!["src".into(), "alt".into(), ".".into()],
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pairs(&p.bundle, "CALLS_STATIC").is_empty());
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["status"] == "ambiguous" && strings(&u["candidate_ids"]).len() == 2));
    source(
        &root,
        &[
            ("a.py", "from b import target\n"),
            ("b.py", "from a import target\n"),
            ("os.py", "def target(): pass\n"),
            ("frozen.py", "from os import target\ndef run(): target()\n"),
        ],
    );
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "roots",
        3,
        &ScanOptions {
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!pairs(&p.bundle, "CALLS_STATIC")
        .iter()
        .any(|(_, target)| target == "os.py::target"));
    assert!(rows(&p.bundle, "unresolved_references")
        .iter()
        .any(|u| u["predicate"] == "IMPORTS"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn nested_closure_identity_default_arguments_and_annotations_do_not_leak_callers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, &[("app.py", "def target(): pass\ndef outer():\n    def nested():\n        return target\n    def sibling():\n        return nested()\n    return sibling()\ndef defaults():\n    def deferred(x=target()) -> target():\n        return None\n    return deferred()\n")]);
    let p = python::produce(
        Capture::read(&root, None).unwrap(),
        "closure",
        1,
        &ScanOptions {
            calls: true,
            ..Default::default()
        },
    )
    .unwrap();
    let calls = pairs(&p.bundle, "CALLS_STATIC");
    assert!(calls.contains(&(
        "app.py::outer::sibling".into(),
        "app.py::outer::nested".into()
    )));
    assert!(calls.contains(&("app.py::outer".into(), "app.py::outer::sibling".into())));
    assert!(calls.contains(&("app.py::defaults".into(), "app.py::target".into())));
    // Only the evaluated default can be attributed to defaults, not the return annotation.
    assert_eq!(
        rows(&p.bundle, "assertions")
            .iter()
            .filter(|a| a["proposition"]["predicate"] == "CALLS_STATIC")
            .count(),
        4
    );
    assert_eq!(p.diagnostics["call_coverage"]["resolution_complete"], false);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn python_revision_comparison_uses_retained_python_declarations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, &[("old.py", "def work():\n    return 1\n")]);
    let mut l = Ledger::open(&temp.path().join("briefcase.db")).unwrap();
    scan(&mut l, &root, "base", false);
    std::fs::rename(root.join("old.py"), root.join("new.py")).unwrap();
    scan(&mut l, &root, "move", false);
    let before = l.load("base").unwrap();
    let after = l.load("move").unwrap();
    briefcase::architecture::history::prepare(&mut l, &before, &after).unwrap();
    assert!(query::compare(&l, "base", &options("move"))
        .unwrap()
        .to_string()
        .contains("RENAMED_FROM"));
    std::fs::remove_file(root.join("new.py")).unwrap();
    source(&root, &[("edited.py", "def work():\n    return 2\n")]);
    scan(&mut l, &root, "edited", false);
    let after = l.load("edited").unwrap();
    briefcase::architecture::history::prepare(&mut l, &before, &after).unwrap();
    let report = query::compare(&l, "base", &options("edited")).unwrap();
    assert!(
        report.to_string().contains("possible_edited_move"),
        "{report}"
    );
    assert!(!report.to_string().contains("exact_content_move"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn retained_execution_rejects_changed_roots_wrong_languages_and_source_retractions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            ("lib.py", "def target(): pass\n"),
            ("app.py", "from lib import target\ndef run(): target()\n"),
        ],
    );
    let mut l = Ledger::open(&temp.path().join("briefcase.db")).unwrap();
    scan(&mut l, &root, "base", false);
    assert!(python::analyze(
        &mut l,
        &options("base"),
        &ScanOptions {
            source_roots: vec!["src".into()],
            ..Default::default()
        }
    )
    .is_err());
    assert!(briefcase::architecture::typescript::analyze(
        &mut l,
        &options("base"),
        &Default::default()
    )
    .is_err());
    let v = l.load("base").unwrap();
    let id = rows(&v.bundle, "assertions")
        .iter()
        .find(|a| a["proposition"]["predicate"] == "DECLARES")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    l.retract(id, "corrected Python declaration").unwrap();
    l.rebuild("base").unwrap();
    assert!(python::analyze(&mut l, &options("base"), &ScanOptions::default()).is_err());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn python_cache_directories_are_excluded_and_quota_failure_cannot_publish() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(
        &root,
        &[
            ("lib.py", "def target(): pass\n"),
            (".mypy_cache/generated.py", "def broken(:\n"),
            ("__pycache__/generated.py", "def broken(:\n"),
            ("setup.cfg", "[metadata]\nname=demo\n"),
            ("Pipfile", "[packages]\n"),
        ],
    );
    let c = Capture::read(&root, None).unwrap();
    assert!(!c.files.keys().any(|p| p.contains("cache")));
    assert!(c.files.contains_key("setup.cfg") && c.files.contains_key("Pipfile"));
    let code: String = (0..4097)
        .map(|i| format!("def function_{i}(): pass\n"))
        .collect();
    source(&root, &[("lib.py", &code)]);
    assert!(python::produce(
        Capture::read(&root, None).unwrap(),
        "quota",
        1,
        &ScanOptions::default()
    )
    .err()
    .unwrap()
    .to_string()
    .contains("definition limit"));
}

#[cfg(unix)]
#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn interpreter_timeout_is_bounded_and_creates_no_candidate_batch() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    source(&root, &[("app.py", "def target(): pass\n")]);
    let sleeper = temp.path().join("sleeper");
    std::fs::write(&sleeper, "#!/bin/sh\nexec sleep 5\n").unwrap();
    std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let started = std::time::Instant::now();
    let result = python::produce(
        Capture::read(&root, None).unwrap(),
        "timeout",
        1,
        &ScanOptions {
            python: sleeper,
            timeout: std::time::Duration::from_secs(1),
            ..Default::default()
        },
    );
    assert!(result.err().unwrap().to_string().contains("timed out"));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}
