//! Evolution map: Git change activity and coupling are derived history
//! metrics over retained commits, distinct from captured-snapshot lineage and
//! architecture changes; co-change is never a dependency.
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use briefcase::architecture::{
    capture::Capture,
    contracts::{rows, strings, text},
    evolution::{self, Request},
    git_history,
    ledger::Ledger,
    lineage,
    query::{self, Options},
    rust::{self, ScanOptions},
    service,
};

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let repo = Self {
            root: root.canonicalize().unwrap(),
            db: dir.path().join("briefcase.db"),
            _dir: dir,
        };
        repo.git(&["init", "-q", "-b", "main"], 1);
        repo
    }
    fn write(&self, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = self.root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }
    /// Run git with a fixed author and the given day of January 2026.
    fn git(&self, args: &[&str], day: u32) -> String {
        let date = format!("2026-01-{day:02}T12:00:00Z");
        let out = Command::new("git")
            .current_dir(&self.root)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    /// Stage everything and commit on `day`; returns the commit OID.
    fn commit(&self, message: &str, day: u32) -> String {
        self.git(&["add", "-A"], day);
        self.git(&["commit", "-qm", message], day);
        self.git(&["rev-parse", "HEAD"], day)
    }
    fn cli(&self, args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_briefcase"))
            .arg("--db")
            .arg(&self.db)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn ledger(&self) -> Ledger {
        Ledger::open(&self.db).unwrap()
    }
}
fn scan(l: &mut Ledger, root: &Path, label: &str) {
    let generation = l.begin("evolution-tests", Some(label)).unwrap();
    let prepared = rust::produce(
        Capture::read(root, None).unwrap(),
        "evolution-tests",
        generation,
        &ScanOptions {
            analyzer: None,
            ..Default::default()
        },
    )
    .unwrap();
    let id = l.commit(generation, &prepared).unwrap();
    l.publish(&id, true).unwrap();
}
fn retain_history(l: &Ledger, root: &Path) -> Value {
    git_history::update(l, root, Instant::now() + Duration::from_secs(60)).unwrap()
}
fn opts(view: &str) -> Options {
    Options {
        view: view.into(),
        ..Default::default()
    }
}
fn component(view: &str, component: &str) -> Options {
    Options {
        component: Some(component.into()),
        ..opts(view)
    }
}
fn entries<'a>(report: &'a Value, section: &str) -> &'a [Value] {
    rows(&report["sections"][section], "entries")
}
fn activity(report: &Value, path: &str) -> u64 {
    entries(report, "activity")
        .iter()
        .find(|e| e["path"] == path)
        .map(|e| e["commits"].as_u64().unwrap())
        .unwrap_or(0)
}
fn pair<'a>(report: &'a Value, from: &str, to: &str) -> Option<&'a Value> {
    entries(report, "coupling")
        .iter()
        .find(|e| e["from"] == from && e["to"] == to)
}
const CARGO: &str = "[package]\nname=\"evolution-tests\"\nversion=\"0.1.0\"\nedition=\"2021\"\n";

/// c0..c4 on days 1..5: A, A+B, A+B (+lock file), C. `c.rs` uses `b`;
/// `a.rs` and `b.rs` never import one another. c5 renames c.rs to d.rs
/// unchanged; c6 and c7 edit d.rs with b.rs, and c6 makes d use a.
struct Story {
    repo: Repo,
    oids: Vec<String>,
}
fn story() -> Story {
    let repo = Repo::new();
    let mut oids = Vec::new();
    repo.write(&[
        ("Cargo.toml", CARGO),
        ("Cargo.lock", "# lock 0\n"),
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\n"),
        ("src/a.rs", "pub fn alpha() -> u32 { 0 }\n"),
        ("src/b.rs", "pub fn beta() -> u32 { 0 }\n"),
        (
            "src/c.rs",
            "use crate::b;\npub fn gamma() -> u32 { b::beta() }\n",
        ),
    ]);
    oids.push(repo.commit("Initial layout", 1));
    repo.write(&[("src/a.rs", "pub fn alpha() -> u32 { 1 }\n")]);
    oids.push(repo.commit("Tune alpha", 2));
    repo.write(&[
        ("src/a.rs", "pub fn alpha() -> u32 { 2 }\n"),
        ("src/b.rs", "pub fn beta() -> u32 { 2 }\n"),
    ]);
    oids.push(repo.commit("Change alpha and beta together", 3));
    repo.write(&[
        ("src/a.rs", "pub fn alpha() -> u32 { 3 }\n"),
        ("src/b.rs", "pub fn beta() -> u32 { 3 }\n"),
        ("Cargo.lock", "# lock 3\n"),
    ]);
    oids.push(repo.commit("Change alpha and beta again", 4));
    repo.write(&[(
        "src/c.rs",
        "use crate::b;\npub fn gamma() -> u32 { b::beta() + 1 }\n",
    )]);
    oids.push(repo.commit("Tune gamma", 5));
    let mut l = repo.ledger();
    scan(&mut l, &repo.root, "before");
    repo.git(&["mv", "src/c.rs", "src/d.rs"], 6);
    repo.write(&[("src/lib.rs", "pub mod a;\npub mod b;\npub mod d;\n")]);
    oids.push(repo.commit("Rename c to d", 6));
    scan(&mut l, &repo.root, "renamed");
    repo.write(&[
        (
            "src/d.rs",
            "use crate::a;\nuse crate::b;\npub fn gamma() -> u32 { b::beta() + a::alpha() }\n",
        ),
        ("src/b.rs", "pub fn beta() -> u32 { 6 }\n"),
    ]);
    oids.push(repo.commit("Gamma reads alpha", 7));
    repo.write(&[
        (
            "src/d.rs",
            "use crate::a;\nuse crate::b;\npub fn gamma() -> u32 { b::beta() * a::alpha() }\n",
        ),
        ("src/b.rs", "pub fn beta() -> u32 { 7 }\n"),
    ]);
    oids.push(repo.commit("Gamma multiplies", 8));
    scan(&mut l, &repo.root, "after");
    assert_eq!(retain_history(&l, &repo.root)["added"], 8);
    Story { repo, oids }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn activity_and_coupling_are_exact_and_never_a_dependency() {
    let s = story();
    let l = s.repo.ledger();
    // Days 2..5 (since inclusive, until exclusive): A, A+B, A+B+lock, C.
    let window = Request {
        since: Some("2026-01-02".into()),
        until: Some("2026-01-06".into()),
        ..Default::default()
    };
    let report = evolution::evolution(&l, &opts("after"), &window).unwrap();
    assert_eq!(report["operation"], "evolution");
    assert_eq!(
        report["coverage"]["change_activity"]["commits_considered"],
        4
    );
    assert_eq!(
        (
            activity(&report, "src/a.rs"),
            activity(&report, "src/b.rs"),
            activity(&report, "src/c.rs")
        ),
        (3, 2, 1)
    );
    // The lock file is noise, counted in coverage and never measured.
    assert_eq!(activity(&report, "Cargo.lock"), 0);
    assert_eq!(report["coverage"]["excluded_paths"]["lock_file"], 1);
    let ab = pair(&report, "src/a.rs", "src/b.rs").expect("A and B co-change");
    assert_eq!(ab["shared_commits"], 2);
    assert_eq!(
        (ab["from_commits"].as_u64(), ab["to_commits"].as_u64()),
        (Some(3), Some(2))
    );
    assert_eq!(ab["from_to"], json!(0.6667));
    assert_eq!(ab["to_from"], json!(1.0));
    assert_eq!(
        strings(&ab["witness_commits"]),
        [s.oids[3].clone(), s.oids[2].clone()]
    );
    assert_eq!(ab["class"], "derived_history_metric");
    assert_eq!(ab["method"], "change-coupling-v1");
    // A and B never import each other: coupling present, dependency not established.
    assert_eq!(ab["static_dependency"]["status"], "not_established");
    let deps = query::dependencies(&l, &opts("after")).unwrap();
    assert!(!deps["details"]
        .as_object()
        .unwrap()
        .values()
        .any(|d| d["predicate"] == "DEPENDS_ON"
            && [&d["from"], &d["to"]].iter().all(|k| {
                let k = k.as_str().unwrap();
                k == "a" || k == "b"
            })));
    assert!(strings(&report["sections"]["coupling"]["limitations"])
        .iter()
        .any(|x| x.contains("not an architectural dependency")));
    // Threshold: no pair reaches three shared commits.
    let strict = evolution::evolution(
        &l,
        &opts("after"),
        &Request {
            min_shared: 3,
            ..window.clone()
        },
    )
    .unwrap();
    assert!(entries(&strict, "coupling").is_empty());
    assert_eq!(
        strict["sections"]["coupling"]["status"],
        "no_changes_in_scope"
    );
    // The full history: b.rs and d.rs change together twice, and d uses b.
    let all = evolution::evolution(&l, &opts("after"), &Request::default()).unwrap();
    // Every retained commit was usable: complete for the disclosed retained scope.
    assert_eq!(all["outcome"], "complete");
    assert_eq!(
        all["coverage"]["change_coupling"]["complete_for_retained_history"],
        true
    );
    // Syntax-only Rust resolves no imports, so no dependency is established.
    let bd = pair(&all, "src/b.rs", "src/d.rs").unwrap();
    assert_eq!(bd["shared_commits"], 2);
    assert_eq!(bd["static_dependency"]["status"], "not_established");
    // A coupling entry explains to its witness commits.
    let handle = text(bd, "explanation_handle");
    let proof = query::explain(&l, &handle, 10, 0).unwrap();
    assert_eq!(proof["detail"]["witness_commits"], bd["witness_commits"]);
    assert_eq!(proof["conclusion"], "derived_history_metric");
    // Deterministic: the same inputs give the same content-addressed report.
    let again = evolution::evolution(&l, &opts("after"), &Request::default()).unwrap();
    assert_eq!(again["id"], all["id"]);
    // The text names what coupling is not.
    let text = evolution::render_text(&all);
    assert!(text.contains("[coupling]") && text.contains("not dependency"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn components_scope_focus_and_unknown_resolution() {
    let s = story();
    let l = s.repo.ledger();
    // A file path: the focus is the file, coupling is oriented from it.
    let a = evolution::evolution(&l, &component("after", "src/a.rs"), &Request::default()).unwrap();
    assert_eq!(a["resolution"]["status"], "path");
    assert_eq!(a["sections"]["activity"]["component_total"]["commits"], 4);
    assert!(entries(&a, "coupling")
        .iter()
        .all(|e| e["from"] == "src/a.rs"));
    assert_eq!(
        pair(&a, "src/a.rs", "src/b.rs").unwrap()["shared_commits"],
        3
    );
    // A symbol resolves to the file that declares it.
    let alpha =
        evolution::evolution(&l, &component("after", "alpha"), &Request::default()).unwrap();
    assert_eq!(alpha["resolution"]["status"], "entity");
    assert_eq!(alpha["resolution"]["focus"], "src/a.rs");
    assert_eq!(
        entries(&alpha, "coupling").len(),
        entries(&a, "coupling").len()
    );
    // A subtree folds its files into one node and counts each commit once.
    let src = evolution::evolution(&l, &component("after", "src"), &Request::default()).unwrap();
    assert_eq!(src["resolution"]["status"], "subtree");
    assert_eq!(src["sections"]["activity"]["component_total"]["commits"], 8);
    assert!(entries(&src, "coupling")
        .iter()
        .all(|e| e["from"] == "src" && !text(e, "to").starts_with("src/")));
    // A retained Git path that is not in the view is matched as history only.
    let gone =
        evolution::evolution(&l, &component("after", "src/c.rs"), &Request::default()).unwrap();
    assert_eq!(gone["resolution"]["status"], "retained_history_path");
    // Activity counts the paths a commit wrote; the rename away is listed, not counted.
    assert_eq!(
        gone["sections"]["activity"]["component_total"]["commits"],
        2
    );
    assert!(
        rows(&gone["sections"]["activity"]["git_path_renames"], "entries")
            .iter()
            .any(|g| g["from"] == "src/c.rs" && g["to"] == "src/d.rs")
    );
    // An unknown component is unknown, not empty.
    let none =
        evolution::evolution(&l, &component("after", "nowhere"), &Request::default()).unwrap();
    assert_eq!(none["outcome"], "unknown");
    assert_eq!(none["sections"]["activity"]["status"], "not_evaluated");
    assert!(rows(&none, "unknowns")
        .iter()
        .any(|u| u["kind"] == "component_resolution"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn git_renames_stay_distinct_from_captured_lineage_and_changes() {
    let s = story();
    let mut l = s.repo.ledger();
    let request = Request {
        base: Some("before".into()),
        ..Default::default()
    };
    // Read-only: unprepared correspondence is reported, never created.
    let unprepared = evolution::evolution(&l, &opts("renamed"), &request).unwrap();
    assert_eq!(
        unprepared["sections"]["lineage"]["status"],
        "correspondence_not_prepared"
    );
    assert!(rows(&unprepared, "unknowns")
        .iter()
        .any(|u| u["kind"] == "correspondence_not_prepared"));
    query::prepare_compare(&mut l, "before", &opts("renamed")).unwrap();
    let report = evolution::evolution(&l, &opts("renamed"), &request).unwrap();
    let lineage = &report["sections"]["lineage"];
    assert_eq!(lineage["status"], "available");
    assert_eq!(lineage["class"], "captured_snapshot_lineage");
    let renamed = rows(lineage, "entries")
        .iter()
        .find(|e| e["predicate"] == "RENAMED_FROM")
        .expect("exact captured rename");
    assert_eq!(renamed["certainty_class"], "exact_content_move");
    assert!(renamed["from"] == "c" && renamed["to"] == "d", "{renamed}");
    assert!(!strings(&renamed["assertion_ids"]).is_empty());
    let proof = query::explain(&l, &text(renamed, "explanation_handle"), 10, 0).unwrap();
    assert!(rows(&proof, "support")
        .iter()
        .any(|x| x["assertion"]["proposition"]["predicate"] == "RENAMED_FROM"));
    // Git's rename of the same file is listed separately and labeled.
    let git = rows(
        &report["sections"]["activity"]["git_path_renames"],
        "entries",
    );
    let g = git.iter().find(|g| g["to"] == "src/d.rs").unwrap();
    assert_eq!(
        (g["from"].as_str(), g["kind"].as_str()),
        (Some("src/c.rs"), Some("rename"))
    );
    assert_eq!(g["class"], "git_path_rename");
    assert!(g.get("predicate").is_none());
    // Activity is not merged across the Git rename.
    assert_eq!(activity(&report, "src/c.rs"), 2);
    assert_eq!(activity(&report, "src/d.rs"), 3);
    // Lineage is scoped by the files that hold its keys in either view.
    let scoped = evolution::evolution(&l, &component("renamed", "src/d.rs"), &request).unwrap();
    assert!(entries(&scoped, "lineage")
        .iter()
        .any(|e| e["predicate"] == "RENAMED_FROM"));
    let other = evolution::evolution(&l, &component("renamed", "src/a.rs"), &request).unwrap();
    assert!(entries(&other, "lineage").is_empty());
    assert_eq!(
        report["sections"]["architecture_changes"]["class"],
        "captured_snapshot_comparison"
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn merges_and_truncated_commits_are_excluded_from_coupling() {
    let repo = Repo::new();
    repo.write(&[("Cargo.toml", CARGO), ("src/lib.rs", "pub fn root() {}\n")]);
    repo.commit("Initial", 1);
    // A side branch changes two documents together once; the merge's
    // first-parent diff would count that pair a second time.
    repo.git(&["checkout", "-qb", "side"], 2);
    repo.write(&[("docs/x.md", "x\n"), ("docs/y.md", "y\n")]);
    repo.commit("Document x and y", 2);
    repo.git(&["checkout", "-q", "main"], 3);
    repo.write(&[("src/lib.rs", "pub fn root() { }\n")]);
    repo.commit("Touch main", 3);
    repo.git(&["merge", "-q", "--no-ff", "-m", "Merge side", "side"], 4);
    // A mechanical commit over the per-commit retention bound.
    let mut files: Vec<(String, String)> = (0..70)
        .map(|i| (format!("gen/f{i:02}.txt"), format!("{i}\n")))
        .collect();
    files.push(("docs/x.md".into(), "x2\n".into()));
    files.push(("docs/y.md".into(), "y2\n".into()));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    repo.write(&borrowed);
    repo.commit("Regenerate fixtures", 5);
    let mut l = repo.ledger();
    scan(&mut l, &repo.root, "app");
    retain_history(&l, &repo.root);
    let report = evolution::evolution(&l, &opts("app"), &Request::default()).unwrap();
    let coverage = &report["coverage"]["change_coupling"];
    assert_eq!(coverage["excluded_merge_commits"], 1);
    assert_eq!(coverage["excluded_truncated_commits"], 1);
    assert_eq!(coverage["used_commits"], 3);
    assert_eq!(coverage["complete_for_retained_history"], false);
    // Neither the merge nor the truncated commit contributes the x/y pair.
    assert!(pair(&report, "docs/x.md", "docs/y.md").is_none());
    let loose = evolution::evolution(
        &l,
        &opts("app"),
        &Request {
            min_shared: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        pair(&loose, "docs/x.md", "docs/y.md").unwrap()["shared_commits"],
        1
    );
    assert_eq!(report["outcome"], "partial");
    assert!(rows(&report, "unknowns")
        .iter()
        .any(|u| u["kind"] == "incomplete_commits"));
    assert_eq!(report["sections"]["coupling"]["outcome"], "partial");
    assert_eq!(report["coverage"]["history"]["max_files_per_commit"], 64);
}

/// The M6 TypeScript reference in a Git repository: day 1 adds it; days 2
/// and 3 change routes.ts, service.ts (which routes imports) and the
/// unrelated admin/reports.ts together. Scanned through the CLI, which
/// resolves TypeScript imports.
pub(crate) fn typescript_story() -> Repo {
    let repo = Repo::new();
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation/sources/m6/reference");
    copy_dir(&reference, &repo.root);
    repo.commit("Reference service", 1);
    repo.cli(&[
        "scan",
        repo.root.to_str().unwrap(),
        "--language",
        "typescript",
        "--calls",
        "--label",
        "base",
    ]);
    for day in [2, 3] {
        for file in [
            "src/orders/routes.ts",
            "src/orders/service.ts",
            "src/admin/reports.ts",
        ] {
            let path = repo.root.join(file);
            let mut text = std::fs::read_to_string(&path).unwrap();
            text.push_str(&format!("// revision {day}\n"));
            if day == 2 && file == "src/admin/reports.ts" {
                text.push_str("import { getOrder } from '../orders/service';\nexport const lookup = getOrder;\n");
            }
            std::fs::write(path, text).unwrap();
        }
        repo.commit(&format!("Revise orders and reports {day}"), day);
    }
    repo.cli(&[
        "scan",
        repo.root.to_str().unwrap(),
        "--language",
        "typescript",
        "--calls",
        "--label",
        "app",
    ]);
    retain_history(&repo.ledger(), &repo.root);
    repo
}
fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn coupling_states_whether_a_static_dependency_is_established() {
    let repo = typescript_story();
    let l = repo.ledger();
    let report = evolution::evolution(
        &l,
        &component("app", "src/orders/routes.ts"),
        &Request {
            since: Some("2026-01-02".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let service = pair(&report, "src/orders/routes.ts", "src/orders/service.ts").unwrap();
    assert_eq!(service["shared_commits"], 2);
    assert_eq!(service["static_dependency"]["status"], "present");
    assert_eq!(service["static_dependency"]["direction"], "from_to");
    let ids = strings(&service["static_dependency"]["assertion_ids"]);
    let deps = query::dependencies(&l, &component("app", "src/orders/routes.ts")).unwrap();
    assert!(rows(&deps["response"], "results")
        .iter()
        .any(|r| strings(&r["assertion_ids"])
            .iter()
            .any(|id| ids.contains(id))));
    // Same co-change, no import: correlation only.
    let reports = pair(&report, "src/orders/routes.ts", "src/admin/reports.ts").unwrap();
    assert_eq!(reports["shared_commits"], 2);
    assert_eq!(reports["static_dependency"]["status"], "not_established");
    // Captured architecture change: reports.ts started importing service.ts.
    let changed = repo.cli(&[
        "evolution",
        "--view",
        "app",
        "--base",
        "base",
        "--component",
        "src/admin/reports.ts",
    ]);
    let added = &changed["sections"]["architecture_changes"];
    assert_eq!(added["class"], "captured_snapshot_comparison");
    assert!(
        rows(added, "entries").iter().any(|e| {
            ["added", "possibly_added"].contains(&e["change"].as_str().unwrap())
                && e["from"] == "src/admin/reports.ts"
                && e["to"] == "src/orders/service.ts"
                && !strings(&e["assertion_ids"]).is_empty()
        }),
        "{added}"
    );
    assert!(rows(added, "entries")
        .iter()
        .all(|e| e["from"] == "src/admin/reports.ts" || e["to"] == "src/admin/reports.ts"));
}

fn source(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("Cargo.toml"), CARGO).unwrap();
    for (name, text) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}
const PROPOSE_FIRST_PAIR: &str = r#"import json, sys
request=json.load(sys.stdin)
packet=request['packet']
before=[p for p in packet['participants'] if p['side']=='before'][0]
after=[p for p in packet['participants'] if p['side']=='after'][0]
print(json.dumps({'proposals':[{'subject_id':after['entity_version_id'],'object_id':before['entity_version_id'],'predicate':'LIKELY_SAME_ENTITY','supports':packet['assertion_ids'][:2],'score':{'value':0.7,'semantics':'uncalibrated'},'limitations':['test only']}],'abstained':False,'limitations':[]}))
"#;

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn without_git_history_lineage_still_answers_with_labeled_certainty() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let changed = temp.path().join("changed");
    let split = temp.path().join("split");
    source(
        &base,
        &[
            ("src/lib.rs", "pub mod old;\n"),
            ("src/old.rs", "pub fn charge() {}\npub fn audit() {}\n"),
        ],
    );
    source(
        &changed,
        &[
            ("src/lib.rs", "pub mod new;\n"),
            (
                "src/new.rs",
                "pub fn charge() {}\n// edited implementation\npub fn audit_log() {}\n",
            ),
        ],
    );
    source(
        &split,
        &[
            ("src/lib.rs", "pub mod charging;\npub mod auditing;\n"),
            ("src/charging.rs", "pub fn charge() {}\npub fn retry() {}\n"),
            ("src/auditing.rs", "pub fn audit() {}\npub fn export() {}\n"),
        ],
    );
    let mut l = Ledger::open(&temp.path().join("k.db")).unwrap();
    scan(&mut l, &base, "base");
    scan(&mut l, &changed, "changed");
    scan(&mut l, &split, "split");
    // Inferred continuity from an explicitly selected local provider.
    query::prepare_compare(&mut l, "base", &opts("changed")).unwrap();
    let program = temp.path().join("lineage.py");
    std::fs::write(&program, PROPOSE_FIRST_PAIR).unwrap();
    let provider = temp.path().join("lineage.json");
    std::fs::write(&provider, json!({"schema_version":"1.0","name":"lineage","model_name":"lineage-test","model_version":"1","settings":{},"command":{"program":"/usr/bin/python3","args":[program]},"timeout_seconds":10,"opaque":false,"deterministic":false}).to_string()).unwrap();
    lineage::classify(&mut l, "base", &opts("changed"), &provider, 65536).unwrap();
    let request = Request {
        base: Some("base".into()),
        ..Default::default()
    };
    let report = evolution::evolution(&l, &opts("changed"), &request).unwrap();
    let likely = entries(&report, "lineage")
        .iter()
        .find(|e| e["predicate"] == "LIKELY_SAME_ENTITY")
        .unwrap();
    assert_eq!(likely["certainty_class"], "inferred_continuity");
    assert!(entries(&report, "lineage")
        .iter()
        .all(|e| e["predicate"] != "RENAMED_FROM"));
    // No Git history: metrics are unavailable and say so; lineage still answers.
    assert_eq!(report["sections"]["activity"]["status"], "unavailable");
    assert_eq!(report["sections"]["coupling"]["status"], "unavailable");
    assert_eq!(report["coverage"]["history"]["retained"], false);
    assert!(rows(&report, "unknowns")
        .iter()
        .any(|u| u["kind"] == "history_unavailable"));
    assert_eq!(report["outcome"], "partial");
    // Without history or a base there is nothing to establish.
    let empty = evolution::evolution(&l, &opts("changed"), &Request::default()).unwrap();
    assert_eq!(empty["outcome"], "unknown");
    // A split stays ambiguous: candidates are shown, none is promoted.
    query::prepare_compare(&mut l, "base", &opts("split")).unwrap();
    let split = evolution::evolution(&l, &opts("split"), &request).unwrap();
    let lineage = entries(&split, "lineage");
    assert!(!lineage.is_empty());
    assert!(lineage
        .iter()
        .all(|e| e["predicate"].is_null() && e["certainty_class"] == "ambiguous_candidates"));
    assert!(lineage
        .iter()
        .any(|e| e["change"] == "possible_split" && e["to"].as_array().unwrap().len() == 2));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn the_catalog_cli_and_mcp_share_the_operation() {
    let s = story();
    let l = s.repo.ledger();
    let op = service::find("architecture_evolution").unwrap();
    assert_eq!(op.name, "evolution");
    let via = service::call(
        &l,
        "evolution",
        &json!({"view":"after","since":"2026-01-02","until":"2026-01-06"}),
    )
    .unwrap();
    assert_eq!(
        pair(&via, "src/a.rs", "src/b.rs").unwrap()["shared_commits"],
        2
    );
    let llm = service::call(
        &l,
        "architecture_evolution",
        &json!({"view":"after","format":"llm"}),
    )
    .unwrap();
    assert_eq!(llm["format"], "llm");
    assert!(llm["text"].as_str().unwrap().starts_with("EVOLUTION "));
    for bad in [
        json!({"min_shared":0}),
        json!({"since":"January"}),
        json!({"format":"svg"}),
        json!({"since":"2026-02-01","until":"2026-01-01"}),
    ] {
        assert!(service::call(&l, "evolution", &bad).is_err(), "{bad}");
    }
    // The CLI prepares correspondence for --base, as compare does.
    let out = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .args(["--db"])
        .arg(&s.repo.db)
        .args([
            "--json",
            "evolution",
            "--view",
            "renamed",
            "--base",
            "before",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cli: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli["sections"]["lineage"]["status"], "available");
    let text = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .args(["--db"])
        .arg(&s.repo.db)
        .args(["evolution", "--view", "after", "--component", "src/a.rs"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("src/a.rs ~ src/b.rs"), "{text}");
}

fn replay(script: &Value) -> Repo {
    let repo = Repo::new();
    let mut l = repo.ledger();
    for step in rows(script, "steps") {
        let day = step["day"].as_u64().unwrap() as u32;
        if let Some(branch) = step["checkout"].as_str() {
            match branch.strip_prefix("new:") {
                Some(name) => repo.git(&["checkout", "-qb", name], day),
                None => repo.git(&["checkout", "-q", branch], day),
            };
        }
        if let Some(branch) = step["merge"].as_str() {
            repo.git(
                &[
                    "merge",
                    "-q",
                    "--no-ff",
                    "-m",
                    &text(step, "message"),
                    branch,
                ],
                day,
            );
        } else {
            for pair in rows(step, "move") {
                repo.git(
                    &["mv", pair[0].as_str().unwrap(), pair[1].as_str().unwrap()],
                    day,
                );
            }
            for path in strings(&step["delete"]) {
                repo.git(&["rm", "-q", &path], day);
            }
            if let Some(g) = step["generate"].as_object() {
                for i in 0..g["count"].as_u64().unwrap() {
                    let path = format!("{}{i:02}.txt", g["prefix"].as_str().unwrap());
                    repo.write(&[(&path, &format!("{i}\n"))]);
                }
            }
            for (path, content) in step["write"].as_object().into_iter().flatten() {
                repo.write(&[(path, content.as_str().unwrap())]);
            }
            repo.commit(&text(step, "message"), day);
        }
        if let Some(label) = step["scan"].as_str() {
            scan(&mut l, &repo.root, label);
        }
    }
    retain_history(&l, &repo.root);
    repo
}

#[test]
#[ignore = "evaluation: requires evaluation fixtures absent from this checkout"]
fn authored_history_matches_the_evolution_gold() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation");
    let read =
        |p: PathBuf| -> Value { serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap() };
    let script = read(dir.join("sources/evolution/commits.json"));
    let gold = read(dir.join("evolution-gold.json"));
    let repo = replay(&script);
    let mut l = repo.ledger();
    let report = evolution::evolution(
        &l,
        &Options {
            limit: 1000,
            ..opts("final")
        },
        &Request::default(),
    )
    .unwrap();
    let c = &report["coverage"];
    let g = &gold["coverage"];
    assert_eq!(
        c["change_activity"]["commits_considered"],
        g["commits_considered"]
    );
    assert_eq!(
        c["change_coupling"]["excluded_merge_commits"],
        g["excluded_merge_commits"]
    );
    assert_eq!(
        c["change_coupling"]["excluded_truncated_commits"],
        g["excluded_truncated_commits"]
    );
    assert_eq!(c["change_coupling"]["used_commits"], g["used_commits"]);
    for (path, count) in gold["activity"].as_object().unwrap() {
        assert_eq!(json!(activity(&report, path)), *count, "activity {path}");
    }
    let found: Vec<Value> = entries(&report, "coupling")
        .iter()
        .map(|e| {
            json!({"from":e["from"],"to":e["to"],"shared_commits":e["shared_commits"],
            "from_commits":e["from_commits"],"to_commits":e["to_commits"]})
        })
        .collect();
    assert_eq!(found, gold["coupling"].as_array().unwrap().clone());
    for check in rows(&gold, "exclusion_checks") {
        let shared = pair(&report, &text(check, "from"), &text(check, "to"))
            .map(|e| e["shared_commits"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(shared, check["shared_commits"], "{check}");
    }
    let renames: Vec<Value> = rows(
        &report["sections"]["activity"]["git_path_renames"],
        "entries",
    )
    .iter()
    .map(|g| json!({"from":g["from"],"to":g["to"],"kind":g["kind"]}))
    .collect();
    assert_eq!(
        renames,
        gold["git_path_renames"].as_array().unwrap().clone()
    );
    for case in rows(&gold, "lineage") {
        let (base, view) = (text(case, "base"), text(case, "view"));
        query::prepare_compare(&mut l, &base, &opts(&view)).unwrap();
        let r = evolution::evolution(
            &l,
            &opts(&view),
            &Request {
                base: Some(base.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let lineage = entries(&r, "lineage");
        let matching: Vec<&Value> = lineage
            .iter()
            .filter(|e| e["certainty_class"] == case["certainty_class"])
            .collect();
        assert_eq!(matching.len(), 1, "{base} -> {view}: {lineage:?}");
        let e = matching[0];
        assert_eq!(e["predicate"], case["predicate"], "{base} -> {view}");
        if case["binary_lineage"] == false {
            let set = |v: &Value| -> std::collections::BTreeSet<String> {
                strings(v).into_iter().collect()
            };
            assert_eq!(e["change"], case["change"]);
            assert_eq!(set(&e["from"]), set(&case["from"]));
            assert_eq!(set(&e["to"]), set(&case["to"]));
            assert!(
                lineage.iter().all(|e| e["predicate"].is_null()),
                "no binary lineage"
            );
        } else {
            assert_eq!((&e["from"], &e["to"]), (&case["from"], &case["to"]));
        }
    }
}
