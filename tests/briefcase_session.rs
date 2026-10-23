//! Agent sessions as retained evidence (ADR-0027): JSONL transport, the
//! declarative profile engine, normalization, exact call-ID pairing,
//! attempt/confirmed separation, path resolution, the session substrate and
//! named session queries. Every gate runs offline with no model.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use briefcase::architecture::{
    contracts::{rows, strings, text},
    session::{self, jsonl, normalize, profile},
};

const ROOT: &str = "/work/shop";

fn sources() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation/sources/sessions")
}
fn generic_profile() -> String {
    sources()
        .join("generic/profile.json")
        .to_string_lossy()
        .into_owned()
}
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(sources().join(name).join("session.jsonl")).unwrap()
}
fn normalized(name: &str, profile: &str) -> normalize::Normalized {
    let p = profile::load(profile).unwrap();
    normalize::normalize(&jsonl::read(&fixture(name)), &p.profile, ROOT)
}
fn kinds<'a>(n: &'a normalize::Normalized, kind: &str) -> Vec<&'a Value> {
    n.events.iter().filter(|e| e["kind"] == kind).collect()
}
fn paths(n: &normalize::Normalized, kind: &str) -> Vec<String> {
    kinds(n, kind)
        .iter()
        .map(|e| text(&e["path"], "path"))
        .collect()
}

struct Db {
    _dir: tempfile::TempDir,
    db: PathBuf,
}
impl Db {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("briefcase.db");
        Self { _dir: dir, db }
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_briefcase"))
            .arg("--db")
            .arg(&self.db)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn cli(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn import(&self, name: &str, profile: &str, label: &str) -> Value {
        let path = sources().join(name).join("session.jsonl");
        self.cli(&[
            "sessions",
            "import",
            path.to_str().unwrap(),
            "--profile",
            profile,
            "--repository-root",
            ROOT,
            "--label",
            label,
        ])
    }
}
fn details(report: &Value) -> Vec<(&Value, &Value)> {
    rows(&report["response"], "results")
        .iter()
        .map(|r| (r, &report["details"][r["id"].as_str().unwrap()]))
        .collect()
}

// ---- S1: transport ---------------------------------------------------------------------

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn transport_retains_ranges_and_makes_failures_visible() {
    let raw = fixture("malformed");
    let s = jsonl::read(&raw);
    assert_eq!(s.records.len(), 6);
    assert_eq!(
        s.malformed(),
        2,
        "a truncated middle line and a duplicate key"
    );
    for r in &s.records {
        // Every record explains back to exactly one line of the retained bytes.
        let line = &raw[r.byte_start..r.byte_end];
        assert!(!line.contains(&b'\n'));
        assert_eq!(raw.split(|b| *b == b'\n').nth(r.ordinal - 1).unwrap(), line);
    }
    assert!(!s.truncated_final_record);
    let partial = jsonl::read(&fixture("partial"));
    assert!(partial.truncated_final_record);
    let n = normalized("partial", &generic_profile());
    assert_eq!(n.capture_state, "partial");
    assert!(n
        .limitations
        .iter()
        .any(|l| l.contains("final record is truncated")));
    // Equal bytes have equal identity.
    assert_eq!(jsonl::read(&raw).digest, s.digest);
}

// ---- S2: profile engine ----------------------------------------------------------------

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn normalization_is_deterministic_and_profile_bound() {
    let a = normalized("generic", &generic_profile());
    let b = normalized("generic", &generic_profile());
    assert_eq!(a.events, b.events);
    assert_eq!(a.coverage, b.coverage);
    assert_eq!(a.external_session_id.as_deref(), Some("acme-0001"));
    assert_eq!(a.harness_version.as_deref(), Some("0.9.2"));
    assert_eq!(a.started_at.as_deref(), Some("2026-10-12T10:04:00Z"));
    assert_eq!(a.ended_at.as_deref(), Some("2026-10-12T10:31:00Z"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn the_profile_language_stays_declarative() {
    let base: Value = serde_json::from_slice(&std::fs::read(generic_profile()).unwrap()).unwrap();
    let bad = |edit: &dyn Fn(&mut Value)| {
        let mut p = base.clone();
        edit(&mut p);
        profile::parse(p.to_string().as_bytes()).is_err()
    };
    assert!(profile::parse(base.to_string().as_bytes()).is_ok());
    assert!(
        bad(&|p| p["records"][0]["script"] = json!("return 1")),
        "no embedded code"
    );
    assert!(
        bad(&|p| p["pairing"] = json!("adjacent")),
        "adjacency is never assumed"
    );
    assert!(
        bad(&|p| p["records"][1]["when"][0]["not_equals"] = json!("x")),
        "one operator per condition"
    );
    assert!(
        bad(&|p| p["records"][1]["when"][0]["pointer"] = json!("type")),
        "JSON Pointer only"
    );
    assert!(
        bad(&|p| p["records"][3]["call_id"] = Value::Null),
        "tool calls need a call ID"
    );
    assert!(bad(
        &|p| p["tools"]["shell"]["operation"] = json!("guess_side_effects")
    ));
    assert!(
        bad(&|p| {
            p["guarantees"]
                .as_object_mut()
                .unwrap()
                .remove("shell_side_effects");
        }),
        "every guarantee family is declared"
    );
    for (name, _) in profile::BUILTIN {
        profile::load(name).unwrap();
    }
}

// ---- S4: pairing and deterministic actions ------------------------------------------------

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn edits_need_a_paired_successful_result() {
    let n = normalized("generic", &generic_profile());
    // write orders (ok), patch payments (ok), write Cargo.toml (failed), write refunds (no result).
    assert_eq!(
        paths(&n, "file_edit_attempt"),
        [
            "src/orders.rs",
            "src/payments.rs",
            "Cargo.toml",
            "src/refunds.rs"
        ]
    );
    assert_eq!(
        paths(&n, "file_edit_confirmed"),
        ["src/orders.rs", "src/payments.rs"]
    );
    let outcome = |p: &str| {
        kinds(&n, "file_edit_attempt")
            .iter()
            .find(|e| e["path"]["path"] == p)
            .unwrap()["outcome"]
            .clone()
    };
    assert_eq!(outcome("Cargo.toml"), "failed");
    assert_eq!(outcome("src/refunds.rs"), "unknown");
    // The assistant's "I updated src/lib.rs" is a message, never an edit.
    assert!(!paths(&n, "file_edit_attempt").contains(&"src/lib.rs".to_owned()));
    let c = &n.coverage;
    assert_eq!(c["tool_calls_without_result"], 1);
    assert_eq!(
        c["tool_results_unpaired"], 1,
        "the c99 result names no call"
    );
    assert_eq!(
        c["records_unknown"], 1,
        "the telemetry record is counted, not dropped"
    );
    assert_eq!(c["tools_without_semantics"], json!({"web_search": 1}));
    assert_eq!(n.capture_state, "partial");
    // Commands keep their exit code and outcome; the patch is parsed for its path.
    let finished = kinds(&n, "command_finished");
    assert_eq!(finished.len(), 1);
    assert_eq!(
        (
            finished[0]["command"].clone(),
            finished[0]["exit_code"].clone()
        ),
        (json!("cargo test"), json!(0))
    );
    let patch = kinds(&n, "file_edit_confirmed")
        .into_iter()
        .find(|e| e["path"]["path"] == "src/payments.rs")
        .unwrap();
    assert_eq!(patch["patch_operation"], "update");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn paths_resolve_lexically_and_outside_paths_stay_external() {
    let n = normalized("generic", &generic_profile());
    let reads: Vec<(String, String)> = kinds(&n, "file_read")
        .iter()
        .map(|e| (text(&e["path"], "location"), text(&e["path"], "path")))
        .collect();
    assert_eq!(
        reads,
        [
            ("repository".into(), "src/lib.rs".into()),
            ("external".into(), "/etc/hosts".into())
        ]
    );
    // An absolute path inside the root and a relative one resolve to the same identity.
    let orders = kinds(&n, "file_edit_attempt")[0];
    assert_eq!(orders["path"]["raw"], "/work/shop/src/orders.rs");
    assert_eq!(orders["path"]["path"], "src/orders.rs");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn duplicate_call_ids_never_pair() {
    let mut raw = String::from_utf8(fixture("generic")).unwrap();
    // A second call reusing c2 makes every c2 result ambiguous.
    raw.push_str("{\"type\":\"tool_call\",\"call_id\":\"c2\",\"tool\":{\"name\":\"write_file\",\"arguments\":{\"path\":\"src/x.rs\"}}}\n");
    let p = profile::load(&generic_profile()).unwrap();
    let n = normalize::normalize(&jsonl::read(raw.as_bytes()), &p.profile, ROOT);
    assert_eq!(n.coverage["duplicate_call_ids"], 2);
    assert_eq!(n.coverage["tool_results_ambiguous"], 1);
    assert!(!kinds(&n, "file_edit_confirmed")
        .iter()
        .any(|e| e["path"]["path"] == "src/orders.rs"));
}

// ---- S8: real harness profiles -------------------------------------------------------------

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn claude_code_profile_pairs_by_tool_use_id() {
    let n = normalized("claude", "claude-code-v1");
    assert_eq!(n.capture_state, "complete");
    assert_eq!(
        n.external_session_id.as_deref(),
        Some("5f0c0b5e-claude-fixture")
    );
    // The meta caveat is ignored; the real request is kept verbatim.
    let users: Vec<String> = kinds(&n, "user_message")
        .iter()
        .map(|e| text(e, "text"))
        .collect();
    assert_eq!(users, ["Fix the payments rounding bug."]);
    assert_eq!(paths(&n, "file_read"), ["src/payments.rs"]);
    assert_eq!(paths(&n, "file_edit_confirmed"), ["src/payments.rs"]);
    let failed = kinds(&n, "file_edit_attempt")
        .into_iter()
        .find(|e| e["path"]["path"] == "src/orders.rs")
        .unwrap();
    assert_eq!(
        failed["outcome"], "failed",
        "is_error marks the result failed"
    );
    let cmd = kinds(&n, "command_finished")[0];
    assert_eq!(
        (cmd["outcome"].clone(), cmd["exit_code"].clone()),
        (json!("failed"), Value::Null)
    );
    assert_eq!(n.coverage["tools_without_semantics"], json!({"Grep": 1}));
    // The final "fixed and tests pass" message establishes nothing.
    assert_eq!(n.coverage["file_edits_confirmed"], 1);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn codex_profile_reads_self_contained_operations() {
    let n = normalized("codex", "codex-v1");
    assert_eq!(n.capture_state, "complete");
    assert_eq!(n.working_directory.as_deref(), Some("/work/shop"));
    // Injected environment context in response_item messages is not a request.
    assert_eq!(kinds(&n, "user_message").len(), 1);
    let mut edited = paths(&n, "file_edit_confirmed");
    edited.sort();
    assert_eq!(edited, ["src/lib.rs", "src/refunds.rs"]);
    let commands: Vec<(Value, Value, Value)> = kinds(&n, "command_finished")
        .iter()
        .map(|e| {
            (
                e["command"].clone(),
                e["exit_code"].clone(),
                e["outcome"].clone(),
            )
        })
        .collect();
    assert_eq!(
        commands,
        [
            (json!("ls src"), json!(0), json!("succeeded")),
            (json!("cargo build"), json!(101), json!("failed"))
        ]
    );
    assert_eq!(
        kinds(&n, "command_finished")[0]["cwd"],
        "/work/shop",
        "file:// prefix stripped"
    );
    assert_eq!(n.coverage["tools_without_semantics"], json!({"exec": 1}));
}

// ---- S3/S5: substrate, publication and queries ---------------------------------------------

#[test]
#[ignore = "evaluation: requires evaluation fixtures absent from this checkout"]
fn import_publishes_evidence_without_a_source_scan_and_replays_idempotently() {
    let db = Db::new();
    let first = db.import("generic", &generic_profile(), "s1");
    assert_eq!(first["status"], "published");
    assert_eq!(first["context"]["kind"], "session");
    let again = db.cli(&[
        "sessions",
        "import",
        sources().join("generic/session.jsonl").to_str().unwrap(),
        "--profile",
        &generic_profile(),
        "--repository-root",
        ROOT,
    ]);
    assert_eq!(again["status"], "up-to-date");
    assert_eq!(again["view_id"], first["view_id"]);
    let listed = db.cli(&["sessions"]);
    assert_eq!(rows(&listed, "sessions").len(), 1);
    assert_eq!(listed["sessions"][0]["external_session_id"], "acme-0001");
    // Selectable by label, view, context ID and the harness's own ID.
    for selector in [
        "s1",
        first["view_id"].as_str().unwrap(),
        first["session_context_id"].as_str().unwrap(),
        "acme-0001",
        "latest",
    ] {
        let r = db.cli(&["session-files", "--session", selector]);
        assert_eq!(r["response"]["context"]["kind"], "session", "{selector}");
    }
}

#[test]
#[ignore = "evaluation: requires evaluation fixtures absent from this checkout"]
fn session_queries_cite_exact_transcript_lines() {
    let db = Db::new();
    db.import("generic", &generic_profile(), "s1");
    let files = db.cli(&["session-files", "--session", "s1"]);
    let by_path = |p: &str| {
        details(&files)
            .into_iter()
            .find(|(_, d)| d["path"] == p)
            .unwrap()
    };
    let (row, d) = by_path("src/payments.rs");
    assert_eq!(d["status"], "edited");
    assert_eq!(row["assertion_ids"].as_array().unwrap().len(), 1);
    let (row, d) = by_path("Cargo.toml");
    assert_eq!(d["status"], "edit_attempted");
    assert!(
        row["assertion_ids"].as_array().unwrap().is_empty(),
        "a failed attempt asserts nothing"
    );
    let (_, d) = by_path("/etc/hosts");
    assert_eq!(d["location"], "external");
    // The explanation's source text is the exact call and result lines.
    let (row, _) = by_path("src/payments.rs");
    let explained = db.cli(&["explain", row["explanation_handle"].as_str().unwrap()]);
    let texts: Vec<String> = rows(&explained, "support")
        .iter()
        .flat_map(|s| rows(s, "sources").iter().map(|x| text(x, "text")))
        .collect();
    assert_eq!(texts.len(), 2);
    let raw = String::from_utf8(fixture("generic")).unwrap();
    for t in &texts {
        assert!(raw.lines().any(|l| l == t), "{t}");
        serde_json::from_str::<Value>(t).unwrap();
    }
    assert!(
        texts.iter().any(|t| t.contains("apply_patch"))
            && texts.iter().any(|t| t.contains("\"ok\":true"))
    );
    // The session report keeps requests verbatim and lists gaps as unknown rows.
    let s = db.cli(&["session", "--session", "s1"]);
    let request = details(&s)
        .into_iter()
        .find(|(_, d)| d["section"] == "request")
        .unwrap();
    assert_eq!(
        request.1["text"],
        "Make payment charging idempotent and keep orders unchanged."
    );
    assert!(details(&s)
        .iter()
        .any(|(r, d)| d["section"] == "coverage" && r["conclusion"] == "unknown"));
    let commands = db.cli(&["session-commands", "--session", "s1"]);
    assert_eq!(details(&commands)[0].1["exit_code"], 0);
    let events = db.cli(&["session-events", "--session", "s1", "--kind", "unknown"]);
    assert_eq!(details(&events).len(), 1);
    assert_eq!(details(&events)[0].0["conclusion"], "unknown");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn a_profile_version_change_reinterprets_without_rewriting_the_transcript() {
    let db = Db::new();
    let first = db.import("generic", &generic_profile(), "v1");
    let dir = tempfile::tempdir().unwrap();
    let mut p: Value = serde_json::from_slice(&std::fs::read(generic_profile()).unwrap()).unwrap();
    p["version"] = json!("2");
    let v2 = dir.path().join("profile-v2.json");
    std::fs::write(&v2, p.to_string()).unwrap();
    let second = db.import("generic", v2.to_str().unwrap(), "v2");
    assert_eq!(second["status"], "published");
    assert_ne!(first["session_context_id"], second["session_context_id"]);
    assert_eq!(
        first["diagnostics"]["raw_artifact_id"],
        second["diagnostics"]["raw_artifact_id"]
    );
    assert_ne!(
        first["diagnostics"]["profile"]["digest"],
        second["diagnostics"]["profile"]["digest"]
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn session_knowledge_is_scoped_and_vocabulary_bound() {
    let raw = fixture("generic");
    let o = session::ImportOptions {
        profile: generic_profile(),
        repository_root: ROOT.into(),
        label: None,
        repository_id: Some("demo".into()),
        source_name: "session.jsonl".into(),
        retention: session::Retention::Full,
    };
    let dir = tempfile::tempdir().unwrap();
    let mut l = briefcase::architecture::ledger::Ledger::open(&dir.path().join("t.db")).unwrap();
    let out = session::import(&mut l, &raw, &o).unwrap();
    let v = l.load(out["view_id"].as_str().unwrap()).unwrap();
    let predicates: std::collections::BTreeSet<String> = rows(&v.bundle, "assertions")
        .iter()
        .map(|a| text(&a["proposition"], "predicate"))
        .collect();
    assert_eq!(
        predicates.into_iter().collect::<Vec<_>>(),
        [
            "EDITED_PATH",
            "RAN_COMMAND",
            "READ_PATH",
            "RECORDED_USER_MESSAGE"
        ]
    );
    assert!(rows(&v.bundle, "runs")
        .iter()
        .all(|r| r["producer_kind"] == "session"));
    // One session entity; no entity per message, call or command.
    let kinds: Vec<String> = rows(&v.bundle, "entity_versions")
        .iter()
        .map(|e| text(e, "kind"))
        .collect();
    assert_eq!(kinds, ["session"]);
    // A session producer cannot write architecture predicates.
    let mut forged = v.bundle.clone();
    let a = forged["assertions"][0].clone();
    let mut b = a.clone();
    b["proposition"]["predicate"] = json!("DEPENDS_ON");
    b["id"] = json!("assertion:forged");
    forged["assertions"].as_array_mut().unwrap().push(b);
    let payloads = l.payloads(&v.bundle).unwrap();
    assert!(briefcase::architecture::contracts::validate(&forged, &payloads).is_err());
    // Every evidence span cites the retained transcript.
    let raw_id = text(&rows(&v.bundle, "session_contexts")[0], "raw_artifact_id");
    for r in rows(&v.bundle, "evidence_refs") {
        assert_eq!(r["artifact_id"], raw_id);
    }
    assert!(strings(&v.diagnostics["limitations"])
        .iter()
        .any(|x| x.contains("not that statements in it are true")));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn pi_profile_preserves_tree_ids_and_pairs_actual_tool_shapes() {
    let n = normalized("pi", "pi-v1");
    assert_eq!(n.capture_state, "complete");
    assert_eq!(n.external_session_id.as_deref(), Some("pi-fixture"));
    assert_eq!(
        n.harness_version, None,
        "header version is a format version, not software version"
    );
    assert_eq!(kinds(&n, "user_message").len(), 1);
    assert_eq!(paths(&n, "file_edit_confirmed"), ["src/payments.rs"]);
    assert_eq!(paths(&n, "file_read"), ["src/payments.rs"]);
    let write = kinds(&n, "file_edit_attempt")
        .into_iter()
        .find(|e| e["path"]["path"] == "src/orders.rs")
        .unwrap();
    assert_eq!(write["outcome"], "failed");
    assert_eq!(write["external_message_id"], "a2");
    assert_eq!(write["external_parent_id"], "r1");
    let branch = kinds(&n, "assistant_message")
        .into_iter()
        .find(|e| e["external_message_id"] == "a4")
        .unwrap();
    assert_eq!(branch["external_parent_id"], "u1");
    assert!(
        branch.get("external_turn_id").is_none(),
        "turn identity is never inferred"
    );
    let cmd = kinds(&n, "command_finished")[0];
    assert_eq!(cmd["outcome"], "failed");
    assert!(
        cmd.get("exit_code").is_none(),
        "exit codes are not guessed from output"
    );
    let db = Db::new();
    db.import("pi", "pi-v1", "pi");
    let events = db.cli(&["session-events", "--session", "pi", "--limit", "1000"]);
    assert!(details(&events)
        .iter()
        .any(|(_, d)| d["external_message_id"] == "a4"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn reduced_retention_keeps_exact_cited_bytes_and_omits_private_text() {
    use briefcase::architecture::{ledger::Ledger, query::Options};
    let raw = fixture("pi");
    let dir = tempfile::tempdir().unwrap();
    let mut l = Ledger::open(&dir.path().join("briefcase.db")).unwrap();
    let mut digests = Vec::new();
    for retention in [
        session::Retention::Cited,
        session::Retention::Metadata,
        session::Retention::Full,
    ] {
        let o = session::ImportOptions {
            profile: "pi-v1".into(),
            repository_root: ROOT.into(),
            label: None,
            repository_id: Some("demo".into()),
            source_name: "session.jsonl".into(),
            retention,
        };
        let result = session::import(&mut l, &raw, &o).unwrap();
        assert_eq!(
            session::import(&mut l, &raw, &o).unwrap()["status"],
            "up-to-date"
        );
        let opts = Options {
            view: text(&result, "view_id"),
            ..Options::default()
        };
        let s = session::load(&l, &opts).unwrap();
        digests.push(s.context["raw_digest"].clone());
        if retention == session::Retention::Metadata {
            assert!(s.raw.is_empty());
            assert!(rows(&s.view.bundle, "assertions").is_empty());
            assert!(s
                .events()
                .iter()
                .all(|e| e.get("text").is_none() && e.get("command").is_none()));
            // Data-only publications still enforce exact replay ownership.
            use briefcase::architecture::{identity, ledger::Prepared};
            let mut payloads = l.payloads(&s.view.bundle).unwrap();
            let sealed = Prepared::seal(
                s.view.bundle.clone(),
                payloads.clone(),
                s.view.generation,
                s.view.diagnostics.clone(),
            )
            .unwrap();
            assert_eq!(l.commit(s.view.generation, &sealed).unwrap(), s.view.id);
            let mut changed = s.view.bundle.clone();
            let bytes = b"different retained bytes".to_vec();
            let digest = identity::bytes_digest(&bytes);
            let mut artifact =
                json!({"path":"sessions/extra.jsonl","digest":digest,"source_kind":"session"});
            let id = identity::id("artifact", &artifact).unwrap();
            artifact["id"] = json!(id);
            artifact["availability"] = json!("available");
            changed["artifacts"].as_array_mut().unwrap().push(artifact);
            payloads.insert(id, bytes);
            let changed = Prepared::seal(
                changed,
                payloads,
                s.view.generation,
                s.view.diagnostics.clone(),
            )
            .unwrap();
            assert!(l
                .commit(s.view.generation, &changed)
                .unwrap_err()
                .to_string()
                .contains("CONFLICTING_REPLAY"));
        } else {
            for e in s.events().iter().filter(|e| s.record(e).is_some()) {
                let original = std::str::from_utf8(
                    &raw[e["byte_start"].as_u64().unwrap() as usize
                        ..e["byte_end"].as_u64().unwrap() as usize],
                )
                .unwrap();
                assert_eq!(s.record(e).unwrap(), original);
            }
            if retention == session::Retention::Cited {
                assert!(s.raw.len() < raw.len());
                assert!(!String::from_utf8_lossy(&s.raw)
                    .contains("No operation supports this statement"));
                let assistant = s
                    .events()
                    .iter()
                    .find(|e| e["external_message_id"] == "a4")
                    .unwrap();
                assert!(assistant.get("text").is_none());
                assert!(s.record(assistant).is_none());
                let report = session::query::files(&l, &opts).unwrap();
                let row = rows(&report["response"], "results")
                    .iter()
                    .find(|r| !rows(r, "assertion_ids").is_empty())
                    .unwrap();
                let handle = row["explanation_handle"].as_str().unwrap();
                let explained =
                    briefcase::architecture::query::explain(&l, handle, 100, 0).unwrap();
                assert!(!rows(&explained, "support").is_empty());
                assert!(rows(&explained, "support")
                    .iter()
                    .flat_map(|a| rows(a, "sources"))
                    .all(|s| s["text"].is_string()));
            }
        }
        session::query::session(&l, &opts).unwrap();
    }
    assert!(
        digests.windows(2).all(|d| d[0] == d[1]),
        "original input identity survives retention choices"
    );
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn pi_history_discovery_uses_headers_and_dry_run_does_not_create_the_database() {
    let db = Db::new();
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("misleading-directory-name");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("ours.jsonl"), fixture("pi")).unwrap();
    std::fs::write(
        nested.join("other.jsonl"),
        String::from_utf8(fixture("pi"))
            .unwrap()
            .replace(ROOT, "/work/other"),
    )
    .unwrap();
    std::fs::write(nested.join("bad.jsonl"), "not JSON\n").unwrap();
    let args = [
        "sessions",
        "import-pi",
        "--sessions-dir",
        dir.path().to_str().unwrap(),
        "--repository-root",
        ROOT,
    ];
    let mut dry = args.to_vec();
    dry.push("--dry-run");
    let inspected = db.cli(&dry);
    assert_eq!(inspected["matched"], 1);
    assert_eq!(inspected["skipped_other_repositories"], 1);
    assert_eq!(inspected["malformed_headers"], 1);
    assert!(!db.db.exists());
    let imported = db.cli(&args);
    assert_eq!(imported["matched"], 1);
    assert_eq!(imported["sessions"][0]["result"]["status"], "published");
    assert_eq!(
        db.cli(&args)["sessions"][0]["result"]["status"],
        "up-to-date"
    );
    assert_eq!(rows(&db.cli(&["sessions"]), "sessions").len(), 1);
}

#[test]
#[ignore = "evaluation: requires evaluation fixtures absent from this checkout"]
fn codex_and_claude_code_histories_are_discovered_by_recorded_cwd() {
    for (harness, name, profile) in [
        ("codex", "codex", "codex-v1"),
        ("claude-code", "claude", "claude-code-v1"),
    ] {
        let db = Db::new();
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("2026/10/12");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("ours.jsonl"), fixture(name)).unwrap();
        std::fs::write(
            nested.join("other.jsonl"),
            String::from_utf8(fixture(name))
                .unwrap()
                .replace(ROOT, "/work/other"),
        )
        .unwrap();
        std::fs::write(nested.join("bad.jsonl"), "not JSON\n").unwrap();
        let args = [
            "sessions",
            "import-local",
            "--harness",
            harness,
            "--sessions-dir",
            dir.path().to_str().unwrap(),
            "--repository-root",
            ROOT,
        ];
        let mut dry = args.to_vec();
        dry.push("--dry-run");
        let inspected = db.cli(&dry);
        assert_eq!(inspected["operation"], format!("import-{harness}"));
        assert_eq!(inspected["matched"], 1, "{harness}: {inspected}");
        assert_eq!(inspected["skipped_other_repositories"], 1, "{harness}");
        assert_eq!(inspected["malformed_headers"], 1, "{harness}");
        assert!(!db.db.exists());
        let imported = db.cli(&args);
        assert_eq!(imported["sessions"][0]["result"]["status"], "published");
        let listed = db.cli(&["sessions"]);
        let session = &rows(&listed, "sessions")[0];
        assert_eq!(
            format!("{}-v1", session["profile"]["name"].as_str().unwrap()),
            profile,
            "{harness}: {session}"
        );
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn windows_recorded_roots_import_on_a_posix_host() {
    let db = Db::new();
    let dir = tempfile::tempdir().unwrap();
    let root = r"C:\work\shop";
    let raw = String::from_utf8(fixture("pi"))
        .unwrap()
        .replace(ROOT, r"C:\\work\\shop");
    let path = dir.path().join("windows.jsonl");
    std::fs::write(&path, raw).unwrap();
    db.cli(&[
        "sessions",
        "import",
        path.to_str().unwrap(),
        "--profile",
        "pi-v1",
        "--repository-root",
        root,
        "--label",
        "windows",
    ]);
    let files = db.cli(&["session-files", "--session", "windows"]);
    assert!(details(&files)
        .iter()
        .any(|(_, d)| d["location"] == "repository" && d["path"] == "src/payments.rs"));
}
