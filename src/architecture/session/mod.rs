//! Agent sessions as retained evidence (ADR-0027).
//!
//! A session transcript establishes what the harness *recorded*: the user
//! messages, the tool calls and their paired results, the files supported
//! tools read or changed, the commands issued, and where the recording is
//! incomplete. It never establishes why the model acted, what was decided, or
//! that a statement in the transcript is true.
//!
//! Layers keep their concerns apart:
//! - [`jsonl`] is the transport: exact bytes, ordinals, byte ranges, strict
//!   parsing, malformed and truncated records;
//! - [`profile`] is the declarative meaning of one harness's records;
//! - [`normalize`] applies a profile to produce a small event vocabulary,
//!   with exact call-ID pairing and attempt/confirmed separation;
//! - [`paths`] resolves recorded paths lexically against recorded metadata;
//! - this module retains the raw bytes, the normalized projection and a
//!   session context, and promotes four Evidence predicates;
//! - [`query`] reads pinned session
//!   publications and join them to pinned source views by exact path only;
//! - [`activity`] aggregates many sessions by exact repository path.
pub mod activity;
pub mod episodes;
pub mod harness;
pub mod jsonl;
pub mod normalize;
pub mod paths;
pub mod profile;
pub mod query;

use super::{
    contracts::{self, add, rows, text, Payloads},
    identity, knowledge,
    ledger::{Ledger, Prepared, View},
    query::Options,
    require, rust, Result,
};
pub use harness::Harness;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BYTES: usize = 256 * 1024 * 1024;
pub const NORMALIZATION: &str = "briefcase-session-normalize-v1";
pub const PRODUCER: &str = "briefcase-session-import-v1";
pub const PREDICATES: [&str; 4] = [
    "RECORDED_USER_MESSAGE",
    "READ_PATH",
    "EDITED_PATH",
    "RAN_COMMAND",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    #[default]
    Full,
    Cited,
    Metadata,
}

impl std::str::FromStr for Retention {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "full" => Ok(Self::Full),
            "cited" => Ok(Self::Cited),
            "metadata" => Ok(Self::Metadata),
            _ => Err("session-retention must be full, cited or metadata".into()),
        }
    }
}

pub struct ImportOptions {
    /// Built-in profile name or a profile file.
    pub profile: String,
    /// Absolute repository root used to make recorded paths repository-relative.
    pub repository_root: String,
    pub label: Option<String>,
    /// Repository namespace for a database with no capture yet; defaults to
    /// the identity `scan` derives from the repository root.
    pub repository_id: Option<String>,
    /// Original file name, kept in diagnostics only (never in an identity).
    pub source_name: String,
    pub retention: Retention,
}

fn implementation() -> String {
    let parts: &[&[u8]] = &[
        include_bytes!("mod.rs"),
        include_bytes!("harness/mod.rs"),
        include_bytes!("harness/pi.rs"),
        include_bytes!("harness/codex.rs"),
        include_bytes!("harness/claude_code.rs"),
        include_bytes!("jsonl.rs"),
        include_bytes!("profile.rs"),
        include_bytes!("normalize.rs"),
        include_bytes!("episodes.rs"),
        include_bytes!("paths.rs"),
    ];
    identity::bytes_digest(&parts.concat())
}

/// The prepared publication, before a generation is allocated.
struct Built {
    bundle: Value,
    payloads: Payloads,
    diagnostics: Value,
    context_id: String,
}

fn integers(coverage: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in [
        "records_total",
        "records_valid_json",
        "records_malformed",
        "records_recognized",
        "records_ignored",
        "records_unknown",
        "items_unknown",
        "timestamps_present",
        "user_messages",
        "assistant_messages",
        "tool_calls",
        "tool_results",
        "tool_results_paired",
        "tool_calls_without_result",
        "tool_calls_without_semantics",
        "file_reads",
        "file_edit_attempts",
        "file_edits_confirmed",
        "commands",
        "paths_repository",
        "paths_external",
        "paths_unresolved",
    ] {
        out.insert(k.into(), json!(coverage[k].as_u64().unwrap_or(0)));
    }
    out.insert(
        "truncated_final_record".into(),
        coverage["truncated_final_record"].clone(),
    );
    Value::Object(out)
}

/// Interpret without writing: the dry run used to author and check profiles.
pub fn inspect(raw: &[u8], o: &ImportOptions) -> Result<Value> {
    require(
        raw.len() <= MAX_BYTES,
        "SESSION",
        "Session transcript byte limit exceeded",
    )?;
    let loaded = profile::load(&o.profile)?;
    let stream = jsonl::read(raw);
    require(
        paths::normalize(&o.repository_root).is_some(),
        "SESSION",
        "The repository root must be an absolute path",
    )?;
    let n = normalize::normalize(&stream, &loaded.profile, &o.repository_root);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &n.events {
        *kinds.entry(text(e, "kind")).or_default() += 1;
    }
    Ok(
        json!({"status":"inspected","profile":{"name":loaded.profile.name,"version":loaded.profile.version,"digest":loaded.digest},
        "harness":loaded.profile.harness,"raw_digest":stream.digest,"bytes":stream.bytes,
        "external_session_id":n.external_session_id,"harness_version":n.harness_version,"working_directory":n.working_directory,
        "started_at":n.started_at,"ended_at":n.ended_at,"capture_state":n.capture_state,"event_kinds":kinds,
        "coverage":n.coverage,"limitations":n.limitations,"retention":o.retention,
        "unknown_examples":n.events.iter().filter(|e| e["kind"] == "unknown").take(10)
            .map(|e| json!({"ordinal":e["ordinal"],"pointer":e["pointer"],"reason":e["reason"]})).collect::<Vec<_>>()}),
    )
}

fn build(raw: &[u8], o: &ImportOptions, repository: &str) -> Result<Built> {
    require(
        raw.len() <= MAX_BYTES,
        "SESSION",
        "Session transcript byte limit exceeded",
    )?;
    require(
        paths::normalize(&o.repository_root).is_some(),
        "SESSION",
        "The repository root must be an absolute path",
    )?;
    let loaded = profile::load(&o.profile)?;
    let p = &loaded.profile;
    let stream = jsonl::read(raw);
    require(
        !stream.records.is_empty(),
        "SESSION",
        "The transcript has no records",
    )?;
    let mut n = normalize::normalize(&stream, p, &o.repository_root);
    // Reduced retention keeps exact selected lines, with separate retained
    // offsets. Original ordinals/ranges always describe the input transcript.
    let mut cited = BTreeSet::new();
    let by_event: BTreeMap<_, _> = n.events.iter().map(|e| (text(e, "id"), e)).collect();
    for e in &n.events {
        if [
            "user_message",
            "file_read",
            "file_edit_attempt",
            "file_edit_confirmed",
            "command_started",
            "command_finished",
            "unknown",
        ]
        .contains(&text(e, "kind").as_str())
        {
            cited.insert(e["ordinal"].as_u64().unwrap_or(0) as usize);
            for link in [
                "call_event",
                "result_event",
                "attempt_event",
                "started_event",
            ] {
                if let Some(linked) = e[link].as_str().and_then(|id| by_event.get(id)) {
                    cited.insert(linked["ordinal"].as_u64().unwrap_or(0) as usize);
                }
            }
        }
    }
    let mut retained = Vec::new();
    let mut retained_ranges = BTreeMap::new();
    if o.retention == Retention::Full {
        retained.extend_from_slice(raw);
    } else if o.retention == Retention::Cited {
        for r in stream.records.iter().filter(|r| cited.contains(&r.ordinal)) {
            let start = retained.len();
            retained.extend_from_slice(&raw[r.byte_start..r.byte_end]);
            retained_ranges.insert(r.ordinal, (start, retained.len()));
            retained.push(b'\n');
        }
    }
    let retained_digest = identity::bytes_digest(&retained);
    for e in &mut n.events {
        if o.retention != Retention::Full {
            let ordinal = e["ordinal"].as_u64().unwrap_or(0) as usize;
            if let Some((start, end)) = retained_ranges.get(&ordinal) {
                e["retained_byte_start"] = json!(start);
                e["retained_byte_end"] = json!(end);
            }
            if o.retention == Retention::Metadata || !cited.contains(&ordinal) {
                e.as_object_mut().unwrap().remove("text");
            }
            if o.retention == Retention::Metadata {
                e.as_object_mut().unwrap().remove("command");
            }
        }
    }
    if o.retention != Retention::Full {
        n.limitations.push(format!("Session retention is {:?}: omitted transcript bytes are unavailable; full reprofiling requires the original input. This does not remove any earlier full import.", o.retention));
    }
    let mut b = contracts::empty_bundle();
    let mut payloads = Payloads::new();
    // The raw artifact's identity depends on its bytes only, never on the
    // profile or the file name, so re-interpreting a transcript reuses it.
    let raw_path = match o.retention {
        Retention::Full => format!("sessions/{}.jsonl", &stream.digest[7..]),
        Retention::Cited => format!("sessions/cited/{}.jsonl", &stream.digest[7..]),
        Retention::Metadata => format!("sessions/metadata/{}.jsonl", &stream.digest[7..]),
    };
    let raw_id = identity::id(
        "artifact",
        &json!({"path":raw_path,"digest":retained_digest,"source_kind":"session"}),
    )?;
    add(
        &mut b,
        "artifacts",
        json!({"id":raw_id,"path":raw_path,"digest":retained_digest,"source_kind":"session","availability":"available"}),
    );
    payloads.insert(raw_id.clone(), retained);
    let profile_pin = json!({"name":p.name,"version":p.version,"digest":loaded.digest});
    let metadata = json!({"external_session_id":n.external_session_id,"harness_version":n.harness_version,
        "working_directory":n.working_directory,"started_at":n.started_at,"ended_at":n.ended_at,
        "bytes":stream.bytes,"blank_lines":stream.blank_lines,"missing_final_newline":stream.missing_final_newline});
    let normalized = json!({"schema_version":"1.0","normalization":NORMALIZATION,"profile":profile_pin,"harness":p.harness,
        "raw_artifact_id":raw_id,"raw_digest":stream.digest,"retained_digest":retained_digest,"retention":o.retention,
        "repository_root":o.repository_root,"metadata":metadata,
        "capture_state":n.capture_state,"coverage":n.coverage,"limitations":n.limitations,"events":n.events});
    contracts::schema(&normalized, "session", None)?;
    let normalized_bytes = identity::canonical(&normalized)?;
    let normalized_digest = identity::bytes_digest(&normalized_bytes);
    let normalized_path = format!(".briefcase/sessions/{}.json", &normalized_digest[7..]);
    let normalized_id = identity::id(
        "artifact",
        &json!({"path":normalized_path,"digest":normalized_digest,"source_kind":"session"}),
    )?;
    add(
        &mut b,
        "artifacts",
        json!({"id":normalized_id,"path":normalized_path,"digest":normalized_digest,"source_kind":"session","availability":"available"}),
    );
    payloads.insert(normalized_id.clone(), normalized_bytes);
    let mut harness = json!({"name":p.harness});
    if let Some(v) = &n.harness_version {
        harness["reported_version"] = json!(v);
    }
    let mut session = json!({"repository_id":repository,"harness":harness,"profile":profile_pin,
        "raw_artifact_id":raw_id,"raw_digest":stream.digest,"retained_digest":retained_digest,"retention":o.retention,"normalized_artifact_id":normalized_id,
        "repository_root":o.repository_root,"capture_state":n.capture_state,"coverage":integers(&n.coverage)});
    for (k, v) in [
        ("external_session_id", &n.external_session_id),
        ("working_directory", &n.working_directory),
        ("started_at", &n.started_at),
        ("ended_at", &n.ended_at),
    ] {
        if let Some(v) = v {
            session[k] = json!(v);
        }
    }
    let sid = identity::id("session", &session)?;
    session["id"] = json!(sid);
    add(&mut b, "session_contexts", session);
    let ctx = json!({"kind":"session","session_context_id":sid});
    let implementation = implementation();
    let settings = json!({"profile":profile_pin,"repository_root":o.repository_root,"normalization":NORMALIZATION,"retention":o.retention});
    let mut config = json!({"producer_name":PRODUCER,"implementation_digest":implementation,"settings_digest":identity::digest(&settings)?});
    let config_id = identity::id("analysis", &config)?;
    config["id"] = json!(config_id);
    add(&mut b, "analysis_configs", config);
    let provider = identity::id(
        "identity-provider",
        &json!({"mapping":"session-transcript-identity-v1","implementation_digest":implementation}),
    )?;
    let key = format!(
        "session:{}/{}",
        p.harness,
        n.external_session_id
            .clone()
            .unwrap_or_else(|| stream.digest.clone())
    );
    let entity =
        rust::entity_for_language(&mut b, &ctx, "session", &key, &provider, "observed", "none")?;
    let c = &n.coverage;
    let count = |k: &str| c[k].as_u64().unwrap_or(0);
    let attempted = count("user_messages")
        + count("file_reads")
        + count("file_edit_attempts")
        + count("commands");
    let unknown_outcomes = n
        .events
        .iter()
        .filter(|e| {
            ["file_read", "file_edit_attempt", "command_started"]
                .contains(&e["kind"].as_str().unwrap_or(""))
                && e["outcome"] == "unknown"
        })
        .count() as u64;
    let unresolved = count("tool_calls_without_result")
        + count("tool_results_unpaired")
        + count("paths_unresolved")
        + count("records_unknown")
        + count("items_unknown")
        + count("records_malformed");
    let guarantee = |family: &str| match p.guarantees.get(family).map(String::as_str) {
        Some("exact") => "exact_supported",
        Some("unsupported") | None => "unknown",
        _ => "recorded_tools_only",
    };
    let coverage = json!({"attempted":attempted,"completed":attempted.saturating_sub(unknown_outcomes.min(attempted)),
        "unsupported_count":count("tool_calls_without_semantics"),"unresolved_count":unresolved,
        "resolution_complete":false,
        "guarantees":[{"predicate":"RECORDED_USER_MESSAGE","kind":guarantee("user_messages")},
            {"predicate":"READ_PATH","kind":guarantee("file_reads")},
            {"predicate":"EDITED_PATH","kind":guarantee("file_edits")},
            {"predicate":"RAN_COMMAND","kind":guarantee("commands")}]});
    let run_id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config_id,"coverage":coverage}),
    )?;
    add(
        &mut b,
        "runs",
        json!({"id":run_id,"producer_kind":"session","analysis_config_id":config_id,"context":ctx,
            "status":if n.capture_state == "complete" {"complete"} else {"partial"},"coverage":coverage}),
    );
    let by_id: BTreeMap<&str, &Value> = n
        .events
        .iter()
        .map(|e| (e["id"].as_str().unwrap_or(""), e))
        .collect();
    let mut refs: BTreeMap<usize, String> = BTreeMap::new();
    let mut span = |b: &mut Value, e: &Value| -> Result<String> {
        let ordinal = e["ordinal"].as_u64().unwrap_or(0) as usize;
        if let Some(r) = refs.get(&ordinal) {
            return Ok(r.clone());
        }
        let locator = json!({"kind":"span","start":e.get("retained_byte_start").unwrap_or(&e["byte_start"]),
            "end":e.get("retained_byte_end").unwrap_or(&e["byte_end"])});
        let id = identity::id("evidence", &json!({"artifact_id":raw_id,"locator":locator}))?;
        add(
            b,
            "evidence_refs",
            json!({"id":id,"artifact_id":raw_id,"locator":locator}),
        );
        refs.insert(ordinal, id.clone());
        Ok(id)
    };
    // One assertion per distinct literal; every recording line that supports it is cited.
    // A path literal is repository-relative, or absolute when outside the repository.
    let mut grouped: BTreeMap<(&str, String), (Value, BTreeSet<String>)> = BTreeMap::new();
    for e in &n.events {
        if o.retention == Retention::Metadata {
            continue;
        }
        let linked = |k: &str| e[k].as_str().and_then(|id| by_id.get(id)).copied();
        let (predicate, literal, support): (&str, Value, Vec<&Value>) = match e["kind"]
            .as_str()
            .unwrap_or("")
        {
            "user_message" => ("RECORDED_USER_MESSAGE", e["text"].clone(), vec![e]),
            "file_read" if e["outcome"] == "succeeded" && e["path"]["location"] != "unresolved" => {
                (
                    "READ_PATH",
                    e["path"]["path"].clone(),
                    std::iter::once(e).chain(linked("result_event")).collect(),
                )
            }
            "file_edit_confirmed" if e["path"]["location"] != "unresolved" => (
                "EDITED_PATH",
                e["path"]["path"].clone(),
                std::iter::once(e).chain(linked("attempt_event")).collect(),
            ),
            "command_finished" => (
                "RAN_COMMAND",
                e["command"].clone(),
                std::iter::once(e).chain(linked("started_event")).collect(),
            ),
            _ => continue,
        };
        let slot = grouped
            .entry((predicate, identity::digest(&literal)?))
            .or_insert_with(|| (literal, BTreeSet::new()));
        for s in support {
            slot.1.insert(span(&mut b, s)?);
        }
    }
    for ((predicate, _), (literal, support)) in grouped {
        if o.retention == Retention::Metadata {
            continue;
        }
        rust::assertion(
            &mut b,
            &ctx,
            "evidence",
            &entity,
            predicate,
            json!({"kind":"literal","value":literal}),
            json!({"run_id":run_id}),
            support.into_iter().collect(),
            None,
        )?;
    }
    let diagnostics = json!({"session_context_id":sid,"raw_artifact_id":raw_id,"normalized_artifact_id":normalized_id,
        "harness":p.harness,"profile":profile_pin,"external_session_id":n.external_session_id,"source_name":o.source_name,
        "capture_state":n.capture_state,"coverage":n.coverage,"retention":o.retention,"retained_bytes":payloads[&raw_id].len(),
        "limitations":std::iter::once("A session transcript establishes what the harness recorded, not that statements in it are true or why anything was done.".to_owned())
            .chain(n.limitations.iter().cloned()).collect::<Vec<_>>()});
    Ok(Built {
        bundle: b,
        payloads,
        diagnostics,
        context_id: sid,
    })
}

/// Retain one transcript under one profile and publish a session view.
/// Replaying identical bytes, profile and root returns the existing view.
pub fn import(l: &mut Ledger, raw: &[u8], o: &ImportOptions) -> Result<Value> {
    let repository = match (l.repository(), &o.repository_id) {
        (Ok(r), _) => r,
        (Err(_), Some(id)) => id.clone(),
        (Err(_), None) => identity::id("repository", &json!({"root":o.repository_root}))?,
    };
    let built = build(raw, o, &repository)?;
    for publication in l.session_publications()? {
        if publication["context"]["id"] == built.context_id {
            touch_summary(l, publication["view_id"].as_str().unwrap_or(""))?;
            return Ok(
                json!({"status":"up-to-date","view_id":publication["view_id"],"scope":"session",
                "session_context_id":built.context_id,"diagnostics":built.diagnostics}),
            );
        }
    }
    let generation = l.begin_scope(&repository, o.label.as_deref(), "session")?;
    let result = (|| {
        let prepared = Prepared::seal(built.bundle, built.payloads, generation, built.diagnostics)?;
        let view = l.commit(generation, &prepared)?;
        let projections = l.publish(&view, false)?;
        touch_summary(l, &view)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
            json!({"status":"published","view_id":view,"scope":"session",
            "session_context_id":built.context_id,"context":knowledge::context(&prepared.bundle),
            "assertion_count":rows(&prepared.bundle,"assertions").len(),"projections":projections,
            "diagnostics":prepared.diagnostics}),
        )
    })();
    if let Err(e) = &result {
        l.fail(generation, &e.to_string())?;
    }
    result
}

/// Read-only discovery and idempotent import of one harness's local
/// history for one repository, selected by the exact recorded cwd.
pub fn import_local(
    l: &mut Ledger,
    harness: Harness,
    dir: &std::path::Path,
    root: &str,
    retention: Retention,
    dry_run: bool,
) -> Result<Value> {
    let LocalFiles {
        matched: files,
        skipped,
        malformed_headers,
    } = discover_local(harness, dir, root)?;
    let root = paths::normalize(root).ok_or("SESSION: Absolute repository root required")?;
    let mut matched = Vec::new();
    for path in files {
        // Every harness converts its history candidate to canonical JSONL
        // first, bounded by MAX_BYTES; single-JSONL stores use the identity
        // materializer, so the retained bytes are exactly the file bytes.
        let materialized = harness.materialize(&path)?;
        let raw = &materialized.bytes;
        let options = ImportOptions {
            profile: harness.profile().into(),
            repository_root: root.clone(),
            label: None,
            repository_id: None,
            source_name: path.file_name().unwrap().to_string_lossy().into_owned(),
            retention,
        };
        let mut result = if dry_run {
            inspect(raw, &options)?
        } else {
            import(l, raw, &options)?
        };
        result["materializer"] = json!(materialized.materializer);
        matched.push(json!({"path":path,"materializer":materialized.materializer,"result":result}));
    }
    Ok(
        json!({"operation":format!("import-{}", harness.name()),"harness":harness.name(),"dry_run":dry_run,"repository_root":root,"sessions_dir":dir,
        "retention":retention,"matched":matched.len(),"skipped_other_repositories":skipped,
        "malformed_headers":malformed_headers,"sessions":matched}),
    )
}

// ---- Reading pinned session publications ------------------------------------------

/// One pinned session publication with its retained projection and transcript.
pub struct Session {
    pub view: View,
    pub context: Value,
    pub doc: Value,
    pub raw: Vec<u8>,
}
impl Session {
    pub fn events(&self) -> &[Value] {
        rows(&self.doc, "events")
    }
    pub fn event(&self, id: &str) -> Option<&Value> {
        self.events().iter().find(|e| e["id"] == id)
    }
    /// The exact transcript line an event came from.
    pub fn record(&self, e: &Value) -> Option<&str> {
        if self.context["retention"] == "metadata" {
            return None;
        }
        let (s, t) = if self.context["retention"] == "cited" {
            (
                e["retained_byte_start"].as_u64()? as usize,
                e["retained_byte_end"].as_u64()? as usize,
            )
        } else {
            (
                e["byte_start"].as_u64()? as usize,
                e["byte_end"].as_u64()? as usize,
            )
        };
        std::str::from_utf8(self.raw.get(s..t)?).ok()
    }
}

/// A session view: its ID or label, `latest`, its session context ID, or
/// the harness's external session ID (newest publication first).
pub fn resolve(l: &Ledger, selector: &str) -> Result<String> {
    if let Ok(id) = l.resolve_scoped("session", selector) {
        return Ok(id);
    }
    for publication in l.session_publications()? {
        let s = &publication["context"];
        if s["id"] == selector || s["external_session_id"] == selector {
            return Ok(text(&publication, "view_id"));
        }
    }
    Err(format!("VIEW: No session publication matches {selector}").into())
}

pub fn load(l: &Ledger, o: &Options) -> Result<Session> {
    let mut scoped = o.clone();
    scoped.view = resolve(l, &o.view)?;
    let view = super::query::loaded(l, &scoped)?;
    from_view(l, view)
}

/// Task composition reads the durable recording independently of graph/search
/// readiness. Retractions and byte integrity still apply.
pub(crate) fn load_retained(l: &Ledger, selector: &str) -> Result<Session> {
    let id = resolve(l, selector)?;
    from_view(l, l.retained(&id)?)
}

/// A compact immutable path index avoids hydrating unrelated transcripts for
/// each coding duty. It is a routing aid, never promoted architecture Evidence.
pub(crate) fn touch_summary(l: &Ledger, selector: &str) -> Result<Value> {
    let view = resolve(l, selector)?;
    let id = identity::id(
        "session-touch-index",
        &json!([
            "session-touch-v3",
            view,
            l.retraction_epoch()?,
            identity::bytes_digest(include_bytes!("mod.rs"))
        ]),
    )?;
    if let Ok(summary) = l.report(&id) {
        return Ok(summary);
    }
    let session = load_retained(l, &view)?;
    let paths: BTreeSet<_> = session
        .events()
        .iter()
        .filter(|e| {
            e["path"]["location"] == "repository"
                && ["file_read", "file_edit_attempt", "file_edit_confirmed"]
                    .contains(&e["kind"].as_str().unwrap_or(""))
        })
        .map(|e| text(&e["path"], "path"))
        .collect();
    let root = session.context["repository_root"].as_str().unwrap_or("");
    let absolute_paths: BTreeSet<_> = session
        .events()
        .iter()
        .filter_map(|event| {
            let path = event["path"]["path"].as_str()?;
            match event["path"]["location"].as_str()? {
                "external" => paths::normalize(path),
                "repository" => paths::normalize(&format!("{}/{path}", root.trim_end_matches('/'))),
                _ => None,
            }
        })
        .collect();
    let executables: BTreeSet<_> = session
        .events()
        .iter()
        .filter(|event| event["kind"] == "command_started")
        .filter_map(|event| paths::direct_command(event["command"].as_str()?))
        .filter_map(|words| words.into_iter().next())
        .collect();
    let mut activities = BTreeMap::<String, Value>::new();
    for event in session.events().iter().filter(|e| {
        e["path"]["location"] == "repository"
            && e["outcome"] == "succeeded"
            && ["file_read", "file_edit_confirmed"].contains(&e["kind"].as_str().unwrap_or(""))
    }) {
        let path = text(&event["path"], "path");
        if activities
            .get(&path)
            .is_none_or(|old| old["kind"] != "file_edit_confirmed")
        {
            activities.insert(path,json!({"id":event["id"],"kind":event["kind"],"path":event["path"],"outcome":"succeeded"}));
        }
    }
    let requests: Vec<_> = session
        .events()
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .collect();
    let mut omitted_request_bytes = 0;
    let user_requests: Vec<_> = requests
        .iter()
        .take(64)
        .map(|e| {
            let content = e["text"].as_str().unwrap_or("");
            let mut end = content.len().min(4096);
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            omitted_request_bytes += content.len() - end;
            json!({"id":e["id"],"text":&content[..end]})
        })
        .collect();
    let summary = json!({"id":id,"view_id":view,"paths":paths,"absolute_paths":absolute_paths,"executables":executables,
        "activities":activities.into_values().collect::<Vec<_>>(),"user_requests":user_requests,"invalidated":session.view.invalidated,
        "omitted_requests":requests.len().saturating_sub(64),"omitted_request_bytes":omitted_request_bytes,
        "capture_state":session.context["capture_state"],"retention":session.context["retention"]});
    l.save_report(&id, &summary, &[])?;
    Ok(summary)
}

fn from_view(l: &Ledger, view: View) -> Result<Session> {
    require(
        view.scope == "session",
        "SCOPE",
        "Not a session publication",
    )?;
    let ctx = knowledge::context(&view.bundle);
    let context = knowledge::row(
        &view.bundle,
        "session_contexts",
        &text(&ctx, "session_context_id"),
    )?
    .clone();
    let ids = [
        text(&context, "raw_artifact_id"),
        text(&context, "normalized_artifact_id"),
    ];
    let payloads = l.payloads_for(&ids)?;
    let normalized = knowledge::row(&view.bundle, "artifacts", &ids[1])?;
    require(
        identity::bytes_digest(&payloads[&ids[1]]) == normalized["digest"],
        "DIGEST",
        "Retained normalized session is corrupted",
    )?;
    let doc = identity::parse(&payloads[&ids[1]])?;
    let raw = payloads[&ids[0]].clone();
    require(
        identity::bytes_digest(&raw)
            == *context
                .get("retained_digest")
                .unwrap_or(&context["raw_digest"]),
        "DIGEST",
        "Retained transcript is corrupted",
    )?;
    Ok(Session {
        view,
        context,
        doc,
        raw,
    })
}

/// Shared discovery for alignment and imports. Directory names never establish ownership.
pub struct LocalFiles {
    pub matched: Vec<std::path::PathBuf>,
    pub skipped: usize,
    pub malformed_headers: usize,
}
pub fn discover_local(harness: Harness, dir: &std::path::Path, root: &str) -> Result<LocalFiles> {
    let root = paths::normalize(root).ok_or("SESSION: Absolute repository root required")?;
    let mut matched = Vec::new();
    let mut skipped = 0;
    let mut malformed_headers = 0;
    // Candidates and their recorded cwd come from the harness, so stores
    // that are not single JSONL files are discovered without materializing.
    for path in harness.candidates(dir)? {
        let Some(cwd) = harness.recorded_cwd(&path)? else {
            malformed_headers += 1;
            continue;
        };
        let cwd = cwd.as_deref().and_then(paths::normalize);
        let matches = cwd.as_deref().is_some_and(|cwd| {
            if paths::PathDialect::of(&root) == paths::PathDialect::Windows {
                cwd.eq_ignore_ascii_case(&root)
            } else {
                cwd == root
            }
        });
        if !matches {
            skipped += 1;
            continue;
        }
        matched.push(path);
    }
    Ok(LocalFiles {
        matched,
        skipped,
        malformed_headers,
    })
}
