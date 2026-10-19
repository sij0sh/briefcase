//! Phase 2 vertical slice: prior session experience through the same
//! Case/Brief findings pipeline as Phase 1 behavior findings.
//! The task wording never names the session's command or configuration
//! key; the join is through retained subjects (paths and `env:` keys).
#[path = "context_support/mod.rs"]
mod context_support;
use briefcase::architecture::{
    contracts::{self, rows},
    session::{self, ImportOptions, Retention},
};
use context_support::Fixture;
use serde_json::{json, Value};

fn fixture() -> Fixture {
    let mut f = Fixture::new();
    std::fs::create_dir_all(f.root.join("config")).unwrap();
    std::fs::write(f.root.join("src/adapter.rs"),
        "pub fn rollback() -> Result<(), String> {\n    if !restore_is_safe() { return Err(\"restore unsafe\".into()); }\n    let _token = std::env::var(\"ROLLBACK_TOKEN\");\n    Ok(())\n}\nfn restore_is_safe() -> bool { false }\n").unwrap();
    std::fs::write(f.root.join("config/app.env"), "ROLLBACK_TOKEN=fixture\n").unwrap();
    f.recapture();
    f
}

fn record(id: &str, parent: &str, message: Value) -> String {
    json!({"type":"message","id":id,"parentId":parent,
        "timestamp":"2026-10-12T10:04:00Z","message":message})
    .to_string()
}
fn call(id: &str, parent: &str, call_id: &str, name: &str, arguments: Value) -> String {
    record(
        id,
        parent,
        json!({"role":"assistant","content":[{"type":"toolCall","id":call_id,"name":name,"arguments":arguments}]}),
    )
}
fn result(id: &str, parent: &str, call_id: &str, error: bool) -> String {
    record(
        id,
        parent,
        json!({"role":"toolResult","toolCallId":call_id,"isError":error,
            "details":{"exitCode":if error {101} else {0}},
            "content":[{"type":"text","text":if error {"failed"} else {"ok"}}]}),
    )
}

/// One Pi transcript: a user requirement, a failed command, a confirmed
/// edit, then the same command succeeding with an env assignment.
fn transcript(root: &str, session_id: &str, user_text: &str, succeed: bool) -> Vec<u8> {
    let mut lines = vec![
        json!({"type":"session","id":session_id,"cwd":root,"timestamp":"2026-10-12T10:04:00Z"})
            .to_string(),
        record("m1", session_id, json!({"role":"user","content":user_text})),
        call("m2", "m1", "c1", "bash", json!({"command":"cargo test"})),
        result("m3", "m2", "c1", true),
        call("m4", "m3", "c2", "edit", json!({"path":"src/adapter.rs"})),
        result("m5", "m4", "c2", false),
    ];
    if succeed {
        lines.push(call(
            "m6",
            "m5",
            "c3",
            "bash",
            json!({"command":"ROLLBACK_TOKEN=s3cr3t-value cargo test"}),
        ));
        lines.push(result("m7", "m6", "c3", false));
    }
    lines.push(record(
        "m8",
        "m7",
        json!({"role":"assistant","content":"You must never skip the checks."}),
    ));
    let mut raw = lines.join("\n");
    raw.push('\n');
    raw.into_bytes()
}

fn import(f: &mut Fixture, raw: &[u8], repository_root: &str) {
    session::import(
        &mut f.ledger,
        raw,
        &ImportOptions {
            profile: "pi-v1".into(),
            repository_root: repository_root.into(),
            label: None,
            repository_id: None,
            source_name: "session.jsonl".into(),
            retention: Retention::Full,
        },
    )
    .unwrap();
}

fn brief_findings(f: &Fixture) -> (Value, Vec<Value>) {
    let packet = f.call(
        "brief",
        json!({"task":"explain the rollback adapter and its conditions",
        "target":"src/adapter.rs","limit":100,"max_bytes":1048576}),
    );
    let retained = f
        .ledger
        .report(packet["case_id"].as_str().unwrap())
        .unwrap();
    contracts::schema(&retained, "task-case", None).unwrap();
    (retained.clone(), rows(&retained, "findings").to_vec())
}

fn session_findings(findings: &[Value]) -> Vec<&Value> {
    findings
        .iter()
        .filter(|f| f["payload"]["type"] == "session_experience")
        .collect()
}

#[test]
fn prior_experience_is_joined_by_subject_not_task_wording() {
    let mut f = fixture();
    let root = f.root.to_str().unwrap().to_owned();
    import(
        &mut f,
        &transcript(
            &root,
            "sess-good",
            "Fix the rollback check. You must always keep the token guard.",
            true,
        ),
        &root,
    );
    let (retained, findings) = brief_findings(&f);
    let session = session_findings(&findings);
    let kinds: Vec<&str> = session
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    for expected in [
        "session_failure_recovery",
        "session_environment_observation",
        "session_review_requirement",
    ] {
        assert!(kinds.contains(&expected), "{kinds:?}: {session:?}");
    }
    assert!(
        retained["coverage"]["findings"]["session"]["emitted"]
            .as_u64()
            .unwrap()
            >= 3,
        "{}",
        retained["coverage"]["findings"]
    );
    // Recovery: observations only; causality and necessity stay unverified,
    // and the env value from the recorded command never reaches the Case.
    let recovery = session
        .iter()
        .find(|f| f["kind"] == "session_failure_recovery")
        .unwrap();
    assert_eq!(recovery["payload"]["causality"], "unverified");
    assert_eq!(recovery["payload"]["necessity"], "unverified");
    assert_eq!(recovery["applicability"]["current_guidance"], false);
    assert!(recovery["subjects"]
        .as_array()
        .unwrap()
        .contains(&json!("src/adapter.rs")));
    assert!(!serde_json::to_string(&retained)
        .unwrap()
        .contains("s3cr3t-value"));
    let environment = session
        .iter()
        .find(|f| f["kind"] == "session_environment_observation")
        .unwrap();
    assert_eq!(environment["payload"]["setting_key"], "ROLLBACK_TOKEN");
    assert_eq!(environment["payload"]["value"], "redacted");
    assert!(environment["payload"]["joined_subjects"]
        .as_array()
        .unwrap()
        .contains(&json!("env:ROLLBACK_TOKEN")));
    // The requirement is the recorded user's only: the assistant's later
    // "must never" statement produces no requirement finding.
    let requirements: Vec<_> = session
        .iter()
        .filter(|f| f["kind"] == "session_review_requirement")
        .collect();
    assert_eq!(requirements.len(), 1, "{requirements:?}");
    assert_eq!(requirements[0]["payload"]["actor"], "user");
    assert_eq!(requirements[0]["payload"]["scope"], "stated_future");
    assert_eq!(requirements[0]["payload"]["current_instruction"], false);

    // Same pinned inputs and rules: a second brief yields identical findings.
    let (_, again) = brief_findings(&f);
    assert_eq!(findings, again);

    // The compact manifest carries the session kinds under the same budget,
    // and /findings pages through the retained Case like any other finding.
    let manifest = f.call(
        "agent_context",
        json!({"task":"explain the rollback adapter and its conditions",
        "target":"src/adapter.rs","max_bytes":8000}),
    );
    assert!(rows(&manifest, "findings")
        .iter()
        .any(|f| f["kind"] == "session_failure_recovery"));
    let page = f.call(
        "page",
        json!({"report_id":retained["id"],"section":"/findings","offset":0,"limit":100,"max_bytes":1048576}),
    );
    assert_eq!(rows(&page, "items").len(), findings.len());
}

#[test]
fn wrong_project_and_unresolved_sessions_do_not_become_guidance() {
    // Wrong project: identical transcript recorded against another root.
    let mut f = fixture();
    let other = f.dir.path().join("other").to_str().unwrap().to_owned();
    import(
        &mut f,
        &transcript(
            &other,
            "sess-foreign",
            "Fix it. You must always keep the token guard.",
            true,
        ),
        &other,
    );
    let (retained, findings) = brief_findings(&f);
    assert!(session_findings(&findings).is_empty(), "{findings:?}");
    assert!(
        retained["coverage"]["findings"]["session"]["excluded_wrong_repository"]
            .as_u64()
            .unwrap()
            > 0
    );

    // Unresolved outcome: the command failed and was never recorded succeeding.
    let mut f = fixture();
    let root = f.root.to_str().unwrap().to_owned();
    import(
        &mut f,
        &transcript(&root, "sess-unresolved", "Fix the rollback check.", false),
        &root,
    );
    let (_, findings) = brief_findings(&f);
    assert!(
        session_findings(&findings)
            .iter()
            .all(|f| f["kind"] != "session_failure_recovery"),
        "{findings:?}"
    );
}

#[test]
fn sessions_facet_opt_out_and_changed_source_keep_history_advisory() {
    let mut f = fixture();
    let root = f.root.to_str().unwrap().to_owned();
    import(
        &mut f,
        &transcript(
            &root,
            "sess-toggle",
            "Fix it. You must always keep the token guard.",
            true,
        ),
        &root,
    );
    // The sessions facet can be switched off like every other facet.
    let packet = f.call(
        "brief",
        json!({"task":"explain the rollback adapter and its conditions",
        "target":"src/adapter.rs","exclude_facets":["sessions"],
        "limit":100,"max_bytes":1048576}),
    );
    let retained = f
        .ledger
        .report(packet["case_id"].as_str().unwrap())
        .unwrap();
    assert!(session_findings(&rows(&retained, "findings")).is_empty());

    // Source moved after the recording: the history stays advisory with
    // unverified current applicability, never current guidance.
    std::fs::write(f.root.join("src/adapter.rs"),
        "pub fn rollback() -> Result<(), String> {\n    if !restore_is_safe() { return Err(\"restore unsafe\".into()); }\n    let _token = std::env::var(\"ROLLBACK_TOKEN\");\n    Ok(())\n}\nfn restore_is_safe() -> bool { true }\n").unwrap();
    f.recapture();
    let (_, findings) = brief_findings(&f);
    let session = session_findings(&findings);
    assert!(!session.is_empty(), "{findings:?}");
    for finding in session {
        assert_eq!(finding["applicability"]["current_guidance"], false);
        assert_eq!(
            finding["lifecycle"]["freshness"],
            "historical_recording_current_applicability_unverified"
        );
    }
}

#[test]
fn unrelated_and_partial_sessions_stay_out_or_stay_unknown() {
    // A session that only touched another file does not join this task.
    let mut f = fixture();
    let root = f.root.to_str().unwrap().to_owned();
    let raw = [
        json!({"type":"session","id":"sess-other","cwd":root,"timestamp":"2026-10-12T10:04:00Z"})
            .to_string(),
        record(
            "m1",
            "sess-other",
            json!({"role":"user","content":"Read the library."}),
        ),
        call("m2", "m1", "c1", "read", json!({"path":"src/lib.rs"})),
        result("m3", "m2", "c1", false),
    ]
    .join("\n");
    import(&mut f, format!("{raw}\n").as_bytes(), &root);
    let (_, findings) = brief_findings(&f);
    assert!(session_findings(&findings).is_empty(), "{findings:?}");

    // Partial capture (an unrecognized record): findings may surface only
    // as advisory context with unknown applicability, never current guidance.
    let mut f = fixture();
    let root = f.root.to_str().unwrap().to_owned();
    let mut raw = transcript(
        &root,
        "sess-partial",
        "Fix it. You must always keep the token guard.",
        true,
    );
    raw.extend_from_slice(b"{\"type\":\"telemetry\",\"n\":1}\n");
    import(&mut f, &raw, &root);
    let (_, findings) = brief_findings(&f);
    let session = session_findings(&findings);
    assert!(!session.is_empty(), "{findings:?}");
    for finding in session {
        assert_eq!(finding["applicability"]["status"], "unknown", "{finding}");
        assert_eq!(finding["applicability"]["current_guidance"], false);
        assert_eq!(
            finding["payload"]["verification"][0]["assessment"],
            "unknown"
        );
    }
}
