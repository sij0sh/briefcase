//! Phase 3: one cohort rule and finding comparison, end to end through the
//! Python dependency producer and a retained architecture declaration.
//! Prevalence is an observed convention candidate, never adopted policy;
//! declarations outrank the majority; unresolved peers are never conforming.
use briefcase::architecture::{
    capture::Capture,
    contracts::{self, rows, text},
    python::{self, ScanOptions},
};
#[path = "context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use serde_json::{json, Value};

const HANDLERS: &[&str] = &["a", "b", "c", "d"];

fn fixture(policy: Value, extra: &[(&str, &str)]) -> Fixture {
    let mut f = Fixture::new();
    let write = |path: &str, code: &str| {
        let path = f.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, code).unwrap();
    };
    write("lib/audit.py", "def record(event): return event\n");
    for h in HANDLERS {
        write(
            &format!("handlers/{h}.py"),
            &format!("from lib import audit\ndef handle_{h}(event):\n    audit.record(event)\n    return event\n"),
        );
    }
    write("handlers/e.py", "def handle_e(event):\n    return event\n");
    for (path, code) in extra {
        write(path, code);
    }
    let policy_path = f.dir.path().join("architecture.json");
    std::fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let repo = f.ledger.repository().unwrap();
    let generation = f.ledger.begin(&repo, None).unwrap();
    let prepared = python::produce(
        Capture::read(&f.root, Some(&policy_path)).unwrap(),
        &repo,
        generation,
        &ScanOptions::default(),
    )
    .unwrap();
    f.view = f.ledger.commit(generation, &prepared).unwrap();
    f.ledger.publish(&f.view, true).unwrap();
    f
}

fn policy(rules: Value, exceptions: Value) -> Value {
    json!({"schema_version":"1.0","authority":"fixture-architecture","revision":"r1",
        "assignments":{},"rules":rules,"convention_exceptions":exceptions})
}

fn case(f: &Fixture, target: &str, extra: Value) -> Value {
    let mut args = json!({"task":"change how the event handler records its result",
        "target":target,"limit":100,"max_bytes":1048576});
    if let Some(extra) = extra.as_object() {
        for (k, v) in extra {
            args[k] = v.clone();
        }
    }
    let packet = f.call("brief", args);
    let retained = f.ledger.report(packet["case_id"].as_str().unwrap()).unwrap();
    contracts::schema(&retained, "task-case", None).unwrap();
    retained
}

fn convergence(case: &Value) -> Vec<Value> {
    rows(case, "findings")
        .iter()
        .filter(|f| f["kind"] == "architectural_convergence")
        .cloned()
        .collect()
}

#[test]
fn deviating_subject_is_a_task_local_convention_candidate() {
    let f = fixture(policy(json!([]), json!([])), &[]);
    let retained = case(&f, "handlers/e.py", json!({}));
    let found = convergence(&retained);
    assert_eq!(found.len(), 1, "{:#?}", found.iter().map(|f| &f["claim"]).collect::<Vec<_>>());
    let finding = &found[0];
    let p = &finding["payload"];
    assert_eq!(p["status"], "candidate");
    assert_eq!(p["convention"]["target"], "lib/audit.py");
    assert_eq!(p["cohort"]["analyzed"], 4);
    assert_eq!(p["cohort"]["conforming"], 4);
    assert_eq!(p["cohort"]["unresolved"], 0);
    assert_eq!(p["decision"]["automatic_change"], false);
    assert_eq!(finding["basis"]["authority"], "observed_convention");
    let claim = text(finding, "claim");
    assert!(claim.starts_with("Observed convention candidate: 4 of 4 analyzed"), "{claim}");
    assert!(claim.contains("not intent or design quality"));
    // Every conforming witness resolves through the pinned index.
    for evidence in rows(&finding["basis"], "evidence") {
        let link = evidence["link_id"].as_str().unwrap();
        let explained = f.call("explain", json!({"report_id":retained["id"],"ref":link,"max_bytes":8000}));
        assert_eq!(explained["link_id"], link, "{explained:#}");
    }
    // Compact delivery keeps analyzed and unresolved side by side.
    let packet = f.call(
        "agent_context",
        json!({"task":"change how the event handler records its result","target":"handlers/e.py"}),
    );
    let compact = rows(&packet, "findings")
        .iter()
        .find(|c| c["kind"] == "architectural_convergence")
        .cloned()
        .unwrap_or_else(|| panic!("{:#}", packet["findings"]));
    assert_eq!(compact["convergence"]["analyzed"], 4);
    assert_eq!(compact["convergence"]["unresolved"], 0);
}

#[test]
fn declared_exception_is_preserved_not_flagged_for_cleanup() {
    let exceptions = json!([{"id":"e-skips-audit","module":"handlers/e.py","target":"lib/audit.py",
        "reason":"handler e only echoes health checks"}]);
    let f = fixture(policy(json!([]), exceptions), &[]);
    let found = convergence(&case(&f, "handlers/e.py", json!({})));
    assert_eq!(found[0]["payload"]["status"], "permitted_exception");
    assert!(text(&found[0]["payload"]["decision"], "candidate").starts_with("Preserve"));
}

#[test]
fn adopted_declaration_outranks_the_observed_majority() {
    let rules = json!([{"id":"handlers-no-audit","mode":"report-only","from":"handlers/a.py","to":"lib/audit.py"}]);
    let f = fixture(policy(rules, json!([])), &[]);
    let found = convergence(&case(&f, "handlers/e.py", json!({})));
    let finding = &found[0];
    assert_eq!(finding["payload"]["status"], "contradicted_by_declaration");
    assert!(rows(finding, "counterevidence")
        .iter()
        .any(|c| c["role"] == "contradicting_declaration"));
    assert!(text(&finding["payload"]["decision"], "candidate").starts_with("Do not adopt"));
}

#[test]
fn peers_without_producer_coverage_stay_unresolved() {
    // A peer the producer cannot resolve imports for is reported as
    // "4 of 4 analyzed; 1 unresolved", never "5 of 5".
    let f = fixture(
        policy(json!([]), json!([])),
        &[("handlers/f.py", "from lib import audit\ndef broken(:\n")],
    );
    let found = convergence(&case(&f, "handlers/e.py", json!({})));
    let finding = &found[0];
    let cohort = &finding["payload"]["cohort"];
    assert_eq!(
        (&cohort["eligible"], &cohort["analyzed"], &cohort["conforming"], &cohort["unresolved"]),
        (&json!(5), &json!(4), &json!(4), &json!(1)),
        "{cohort:#}"
    );
    assert_eq!(finding["payload"]["unresolved_members"][0]["path"], "handlers/f.py");
    assert_eq!(finding["applicability"]["cohort_status"], "partial");
    assert!(text(finding, "claim").contains("4 of 4 analyzed"));
    assert!(text(finding, "claim").contains("1 peer(s) unresolved of 5 eligible"));
}

#[test]
fn unrelated_task_and_opt_out_carry_no_architectural_cleanup() {
    let f = fixture(policy(json!([]), json!([])), &[]);
    // A conforming subject is not burdened with the cohort's state.
    let conforming = case(&f, "handlers/a.py", json!({}));
    assert!(convergence(&conforming).is_empty(), "{:#?}", convergence(&conforming).iter().map(|f| &f["claim"]).collect::<Vec<_>>());
    let off = case(&f, "handlers/e.py", json!({"exclude_facets":["convergence"]}));
    assert!(convergence(&off).is_empty());
    assert_eq!(off["coverage"]["findings"]["convergence"]["status"], "disabled_by_request");
}

#[test]
fn lost_exception_on_recapture_is_not_improved_conformity() {
    let exceptions = json!([{"id":"d-skips-audit","module":"handlers/d.py","target":"lib/audit.py",
        "reason":"handler d forwards to an audited queue"}]);
    let d = ("handlers/d.py", "def handle_d(event):\n    return event\n");
    let before = fixture(policy(json!([]), exceptions), &[d]);
    let initial = case(&before, "handlers/e.py", json!({}));
    let after = fixture(policy(json!([]), json!([])), &[d]);
    let review = case(&after, "handlers/e.py", json!({}));
    let a = convergence(&initial);
    let b = convergence(&review);
    assert_eq!(a[0]["payload"]["cohort"]["permitted_exceptions"], 1);
    assert_eq!(a[0]["comparison_key"], b[0]["comparison_key"]);
    let changes = briefcase::architecture::task::compare_findings(rows(&initial, "findings"), rows(&review, "findings"));
    assert_eq!(changes["cohort_changes"][0]["assessment"], "declared_exception_lost", "{changes:#}");
}

#[test]
fn supplied_policy_is_not_reported_as_a_deleted_live_path() {
    let f = fixture(policy(json!([]), json!([])), &[]);
    let packet = f.call("agent_context", json!({"task":"change how the event handler records its result","target":"handlers/e.py"}));
    let worktree = || {
        briefcase::architecture::service::call_with_context(
            &f.ledger, "worktree", &json!({"case_id":packet["case_id"]}), Some(&f.root), false).unwrap()
    };
    let check = worktree();
    assert_eq!(check["changed_paths"], 0, "{check}");
    assert_ne!(check["alignment"], "changed");
    std::fs::write(f.root.join("handlers/e.py"), "def handle_e(event):\n    return None\n").unwrap();
    let after = worktree();
    assert_eq!(after["alignment"], "changed");
    assert_eq!(after["changed_paths"], 1);
}
