//! Named session queries over pinned session publications. They report what
//! was recorded and where the recording is incomplete. `session-changes`
//! joins recorded paths to pinned source views by exact repository path and
//! keeps reported edits and source changes as distinct facts.
use super::{load, Session, PREDICATES};
use crate::architecture::{
    contracts::{strings, text},
    identity,
    ledger::{Ledger, View},
    query::{self, Finding, Options},
    require,
    retained_source::SourceFiles,
    Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Session publications, newest first.
pub fn sessions(l: &Ledger) -> Result<Value> {
    sessions_page(l, 1000, 0)
}

pub(crate) fn sessions_page(l: &Ledger, limit: usize, offset: usize) -> Result<Value> {
    let publications = l.session_publications()?;
    let total = publications.len();
    require(
        offset <= total,
        "PAGINATION",
        "Offset outside retained session publications",
    )?;
    let mut out = Vec::new();
    for publication in publications.iter().skip(offset).take(limit) {
        let s = &publication["context"];
        out.push(json!({"view_id":publication["view_id"],"label":publication["label"],"session_context_id":s["id"],
            "harness":s["harness"],"profile":s["profile"],"external_session_id":s["external_session_id"],
            "started_at":s["started_at"],"ended_at":s["ended_at"],"capture_state":s["capture_state"],
            "retention":s["retention"],"user_messages":s["coverage"]["user_messages"],"file_edits_confirmed":s["coverage"]["file_edits_confirmed"],
            "commands":s["coverage"]["commands"]}));
    }
    let end = offset + out.len();
    Ok(
        json!({"operation":"sessions","sessions":out,"total_publications":total,"offset":offset,
        "omitted_publications":total-end,"next_offset":if end<total {Some(end)} else {None},
        "limitations":["A session transcript establishes what the harness recorded, not that statements in it are true or why anything was done.",
            "Metadata listing does not validate architecture projections. Pages use publication order; a new import can shift offsets, so retain returned view IDs."]}),
    )
}

/// Assertions of the pinned session view, indexed by the evidence lines they cite.
struct Support {
    by_ref: BTreeMap<String, Vec<(String, String, Value)>>,
    raw_id: String,
}
impl Support {
    fn new(l: &Ledger, s: &Session, o: &Options) -> Result<Self> {
        let removed = l.retracted()?;
        let mut by_ref: BTreeMap<String, Vec<(String, String, Value)>> = BTreeMap::new();
        for p in PREDICATES {
            for a in query::eligible(&s.view, &removed, o.historical, p) {
                for r in strings(&a["evidence_ref_ids"]) {
                    by_ref.entry(r).or_default().push((
                        text(a, "id"),
                        p.to_owned(),
                        a["proposition"]["object"]["value"].clone(),
                    ));
                }
            }
        }
        Ok(Self {
            by_ref,
            raw_id: text(&s.context, "raw_artifact_id"),
        })
    }
    fn line(&self, e: &Value) -> Result<String> {
        let locator = json!({"kind":"span","start":e.get("retained_byte_start").unwrap_or(&e["byte_start"]),
            "end":e.get("retained_byte_end").unwrap_or(&e["byte_end"])});
        identity::id(
            "evidence",
            &json!({"artifact_id":self.raw_id,"locator":locator}),
        )
    }
    /// Assertions an event's own line supports for the event's own fact.
    fn of(&self, e: &Value) -> Result<Vec<String>> {
        let (predicate, literal) = match e["kind"].as_str().unwrap_or("") {
            "user_message" => ("RECORDED_USER_MESSAGE", e["text"].clone()),
            "file_read" => ("READ_PATH", path_literal(e)),
            "file_edit_attempt" | "file_edit_confirmed" => ("EDITED_PATH", path_literal(e)),
            "command_started" | "command_finished" => ("RAN_COMMAND", e["command"].clone()),
            _ => return Ok(vec![]),
        };
        Ok(self
            .by_ref
            .get(&self.line(e)?)
            .into_iter()
            .flatten()
            .filter(|(_, p, v)| p == predicate && *v == literal)
            .map(|(id, _, _)| id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
}
/// Repository-relative, or absolute outside the repository.
fn path_literal(e: &Value) -> Value {
    e["path"]["path"].clone()
}

/// Short, display-ready description of one event.
pub fn summary(e: &Value) -> Value {
    let mut d =
        json!({"event":e["id"],"ordinal":e["ordinal"],"kind":e["kind"],"timestamp":e["timestamp"]});
    for k in [
        "text",
        "tool",
        "call_id",
        "outcome",
        "path",
        "command",
        "cwd",
        "exit_code",
        "reason",
        "patch_operation",
        "call_event",
        "result_event",
        "attempt_event",
        "started_event",
        "operation",
        "paired",
        "external_turn_id",
        "external_message_id",
        "external_parent_id",
    ] {
        if !e[k].is_null() {
            d[k] = e[k].clone();
        }
    }
    d
}

fn conclusion(e: &Value) -> &'static str {
    let operation = [
        "file_read",
        "file_edit_attempt",
        "command_started",
        "tool_call",
    ]
    .contains(&e["kind"].as_str().unwrap_or(""));
    if e["kind"] == "unknown" || (operation && e["outcome"] == "unknown") {
        "unknown"
    } else {
        "present"
    }
}

fn finish(l: &Ledger, op: &str, s: &Session, findings: Vec<Finding>, o: &Options) -> Result<Value> {
    query::finish(
        l,
        op,
        "evidence",
        std::slice::from_ref(&s.view),
        None,
        findings,
        o,
        false,
    )
}

/// Every normalized event in record order, optionally of selected kinds.
pub fn events(l: &Ledger, o: &Options, kinds: &[String]) -> Result<Value> {
    let s = load(l, o)?;
    for k in kinds {
        require(
            super::normalize::KINDS.contains(&k.as_str()),
            "QUERY",
            &format!("Unknown event kind {k}"),
        )?;
    }
    let support = Support::new(l, &s, o)?;
    let mut findings = Vec::new();
    for e in s
        .events()
        .iter()
        .filter(|e| kinds.is_empty() || kinds.contains(&text(e, "kind")))
    {
        findings.push(Finding {
            ids: support.of(e)?,
            conclusion: conclusion(e),
            detail: summary(e),
        });
    }
    finish(l, "session-events", &s, findings, o)
}

/// Per recorded path: reads, edit attempts and confirmed edits with outcomes.
pub(crate) struct FileRow {
    pub location: String,
    pub path: String,
    pub reads: BTreeMap<String, u64>,
    pub attempts: BTreeMap<String, u64>,
    pub confirmed: u64,
    pub tools: BTreeSet<String>,
    pub events: Vec<String>,
    pub first: u64,
    pub last: u64,
    pub ids: BTreeSet<String>,
}
impl FileRow {
    pub fn read(&self) -> bool {
        self.reads.get("succeeded").is_some_and(|n| *n > 0)
    }
    pub fn edited(&self) -> bool {
        self.confirmed > 0
    }
    pub fn attempted(&self) -> bool {
        !self.attempts.is_empty()
    }
    pub fn detail(&self) -> Value {
        json!({"path":self.path,"location":self.location,"reads":self.reads,"edit_attempts":self.attempts,
            "edits_confirmed":self.confirmed,"tools":self.tools,"first_ordinal":self.first,"last_ordinal":self.last,
            "events":self.events.iter().take(32).collect::<Vec<_>>(),"event_count":self.events.len(),
            "status":if self.edited() {"edited"} else if self.attempted() {"edit_attempted"} else if self.read() {"read"} else {"read_attempted"}})
    }
}
pub(crate) fn file_rows(l: &Ledger, s: &Session, o: &Options) -> Result<Vec<FileRow>> {
    let support = Support::new(l, s, o)?;
    let mut out: BTreeMap<(String, String), FileRow> = BTreeMap::new();
    for e in s.events().iter().filter(|e| {
        ["file_read", "file_edit_attempt", "file_edit_confirmed"]
            .contains(&e["kind"].as_str().unwrap_or(""))
    }) {
        let key = (text(&e["path"], "location"), text(&e["path"], "path"));
        let ordinal = e["ordinal"].as_u64().unwrap_or(0);
        let row = out.entry(key.clone()).or_insert_with(|| FileRow {
            location: key.0.clone(),
            path: key.1.clone(),
            reads: BTreeMap::new(),
            attempts: BTreeMap::new(),
            confirmed: 0,
            tools: BTreeSet::new(),
            events: Vec::new(),
            first: ordinal,
            last: ordinal,
            ids: BTreeSet::new(),
        });
        let outcome = text(e, "outcome");
        match e["kind"].as_str().unwrap_or("") {
            "file_read" => *row.reads.entry(outcome).or_default() += 1,
            "file_edit_attempt" => *row.attempts.entry(outcome).or_default() += 1,
            _ => row.confirmed += 1,
        }
        if let Some(t) = e["tool"].as_str() {
            row.tools.insert(t.to_owned());
        }
        row.events.push(text(e, "id"));
        row.first = row.first.min(ordinal);
        row.last = row.last.max(ordinal);
        row.ids.extend(support.of(e)?);
    }
    let mut rows: Vec<FileRow> = out.into_values().collect();
    rows.sort_by(|a, b| {
        (a.location != "repository", &a.path).cmp(&(b.location != "repository", &b.path))
    });
    Ok(rows)
}

fn file_findings(rows: &[FileRow]) -> Vec<Finding> {
    rows.iter()
        .map(|r| Finding {
            ids: r.ids.iter().cloned().collect(),
            conclusion: if r.edited()
                || r.read()
                || r.location != "unresolved"
                    && (r.reads.contains_key("failed") || r.attempts.contains_key("failed"))
            {
                "present"
            } else {
                "unknown"
            },
            detail: r.detail(),
        })
        .collect()
}

pub fn files(l: &Ledger, o: &Options) -> Result<Value> {
    let s = load(l, o)?;
    let rows = file_rows(l, &s, o)?;
    finish(l, "session-files", &s, file_findings(&rows), o)
}

fn command_findings(s: &Session, support: &Support) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    let finished_by_start: BTreeMap<&str, &Value> = s
        .events()
        .iter()
        .filter(|e| e["kind"] == "command_finished")
        .filter_map(|e| Some((e["started_event"].as_str()?, e)))
        .collect();
    for e in s.events().iter().filter(|e| e["kind"] == "command_started") {
        let finished = e["id"]
            .as_str()
            .and_then(|id| finished_by_start.get(id))
            .copied();
        let mut ids: BTreeSet<String> = support.of(e)?.into_iter().collect();
        if let Some(f) = finished {
            ids.extend(support.of(f)?);
        }
        let outcome = finished.map_or("unknown".to_owned(), |f| text(f, "outcome"));
        out.push(Finding {
            ids: ids.into_iter().collect(),
            conclusion: if finished.is_some() { "present" } else { "unknown" },
            detail: json!({"command":e["command"],"cwd":e["cwd"],"tool":e["tool"],"outcome":outcome,
                "exit_code":finished.map(|f| f["exit_code"].clone()),"timestamp":e["timestamp"],
                "started_event":e["id"],"finished_event":finished.map(|f| f["id"].clone()),"ordinal":e["ordinal"],
                "status":if finished.is_some() {"result_recorded"} else {"no_result_recorded"}}),
        });
    }
    Ok(out)
}

pub fn commands(l: &Ledger, o: &Options) -> Result<Value> {
    let s = load(l, o)?;
    let support = Support::new(l, &s, o)?;
    let findings = command_findings(&s, &support)?;
    finish(l, "session-commands", &s, findings, o)
}

pub(crate) fn duration(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let (s, e) = (start?, end?);
    Some(
        crate::architecture::runtime_observations::seconds(e).ok()?
            - crate::architecture::runtime_observations::seconds(s).ok()?,
    )
}

/// Headline counts of one session.
pub(crate) fn headline(s: &Session) -> Value {
    let c = &s.context["coverage"];
    let failed = s
        .events()
        .iter()
        .filter(|e| {
            [
                "file_read",
                "file_edit_attempt",
                "command_started",
                "command_finished",
            ]
            .contains(&e["kind"].as_str().unwrap_or(""))
                && e["outcome"] == "failed"
        })
        .filter(|e| e["kind"] != "command_started")
        .count();
    let files = |kind: &str| {
        s.events()
            .iter()
            .filter(|e| e["kind"] == kind && e["path"]["location"] != "unresolved")
            .map(|e| text(&e["path"], "path"))
            .collect::<BTreeSet<_>>()
            .len()
    };
    let read = s
        .events()
        .iter()
        .filter(|e| e["kind"] == "file_read" && e["outcome"] == "succeeded")
        .map(|e| text(&e["path"], "path"))
        .collect::<BTreeSet<_>>()
        .len();
    json!({"user_requests":c["user_messages"],"assistant_messages":c["assistant_messages"],
        "files_read":read,"files_edited":files("file_edit_confirmed"),"files_edit_attempted":files("file_edit_attempt"),
        "commands":c["commands"],"failed_actions":failed,"tool_calls":c["tool_calls"],
        "duration_seconds":duration(s.context["started_at"].as_str(), s.context["ended_at"].as_str())})
}

/// The whole session: metadata, requests, files, commands and coverage gaps.
pub fn session(l: &Ledger, o: &Options) -> Result<Value> {
    let s = load(l, o)?;
    let support = Support::new(l, &s, o)?;
    let mut findings = vec![Finding {
        ids: vec![],
        conclusion: "present",
        detail: json!({"section":"metadata","session_context_id":s.context["id"],"harness":s.context["harness"],
            "profile":s.context["profile"],"external_session_id":s.context["external_session_id"],
            "started_at":s.context["started_at"],"ended_at":s.context["ended_at"],
            "working_directory":s.context["working_directory"],"repository_root":s.context["repository_root"],
            "capture_state":s.context["capture_state"],"raw_digest":s.context["raw_digest"],
            "retention":s.context["retention"],"retained_digest":s.context["retained_digest"],
            "headline":headline(&s),"coverage":s.doc["coverage"]}),
    }];
    for e in s.events().iter().filter(|e| e["kind"] == "user_message") {
        let mut d = summary(e);
        d["section"] = json!("request");
        findings.push(Finding {
            ids: support.of(e)?,
            conclusion: "present",
            detail: d,
        });
    }
    for mut f in file_findings(&file_rows(l, &s, o)?) {
        f.detail["section"] = json!("file");
        findings.push(f);
    }
    for mut f in command_findings(&s, &support)? {
        f.detail["section"] = json!("command");
        findings.push(f);
    }
    for gap in strings(&s.doc["limitations"]) {
        findings.push(Finding {
            ids: vec![],
            conclusion: "unknown",
            detail: json!({"section":"coverage","gap":gap}),
        });
    }
    finish(l, "session", &s, findings, o)
}

// ---- Source joins --------------------------------------------------------------------

pub(crate) fn source_view(l: &Ledger, selector: &str, o: &Options) -> Result<View> {
    let v = query::loaded(
        l,
        &Options {
            view: selector.into(),
            ..o.clone()
        },
    )?;
    require(
        v.scope == "source",
        "SCOPE",
        &format!("{selector} is not a source view"),
    )?;
    Ok(v)
}

/// One row of the reported-edit × source-change matrix.
pub(crate) struct Change {
    pub path: String,
    /// `edited`, `edit_attempted`, `read` or `none`.
    pub session: &'static str,
    /// `changed`, `unchanged`, `added`, `removed`, `not_captured` or `unknown` (no base view).
    pub source: &'static str,
    pub interpretation: &'static str,
    pub ids: Vec<String>,
}

pub(crate) fn matrix(
    files: &[FileRow],
    after: &SourceFiles,
    before: Option<&SourceFiles>,
) -> Vec<Change> {
    let by_path: BTreeMap<&str, &FileRow> = files
        .iter()
        .filter(|r| r.location == "repository")
        .map(|r| (r.path.as_str(), r))
        .collect();
    let mut paths: BTreeSet<String> = by_path
        .values()
        .filter(|r| r.edited() || r.attempted())
        .map(|r| r.path.clone())
        .collect();
    if let Some(before) = before {
        for p in after.digests.keys().chain(before.digests.keys()) {
            if after.digests.get(p) != before.digests.get(p) {
                paths.insert(p.clone());
            }
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let row = by_path.get(path.as_str());
            let session = match row {
                Some(r) if r.edited() => "edited",
                Some(r) if r.attempted() => "edit_attempted",
                Some(r) if r.read() => "read",
                _ => "none",
            };
            let source = match before {
                None => "unknown",
                Some(before) => before.net_change(after, &path),
            };
            let changed = ["changed", "added", "removed"].contains(&source);
            let interpretation = match (session, source) {
                (_, "unknown") => "net_change_unknown",
                (_, "not_captured") => "outside_captured_views",
                ("edited", _) if changed => "edited_and_net_changed",
                ("edited", _) => "edit_recorded_no_net_change",
                ("edit_attempted", _) if changed => "changed_with_unconfirmed_edit",
                ("edit_attempted", _) => "edit_attempted_no_net_change",
                (_, _) if changed => "changed_without_recorded_edit",
                _ => "no_relevant_change",
            };
            Change {
                path,
                session,
                source,
                interpretation,
                ids: row
                    .map(|r| r.ids.iter().cloned().collect())
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// Reported session edits against the actual difference between two pinned
/// source views. A change during the session is never attributed to the agent.
pub fn changes(l: &Ledger, o: &Options, view: &str, base: Option<&str>) -> Result<Value> {
    let s = load(l, o)?;
    let after = SourceFiles::new(&source_view(l, view, o)?)?;
    let before = base
        .map(|b| source_view(l, b, o).and_then(|v| SourceFiles::new(&v)))
        .transpose()?;
    let files = file_rows(l, &s, o)?;
    let mut findings = Vec::new();
    for c in matrix(&files, &after, before.as_ref()) {
        findings.push(Finding {
            ids: c.ids.clone(),
            conclusion: if ["unknown", "not_captured"].contains(&c.source) { "unknown" } else { "present" },
            detail: json!({"path":c.path,"session_record":c.session,"source_diff":c.source,"interpretation":c.interpretation,
                "in_view":after.digests.contains_key(&c.path),"file_key":after.keys.get(&c.path),
                "modules":after.modules.get(&c.path),"view":after.view,"view_snapshot":after.snapshot,
                "view_digest":after.digests.get(&c.path),
                "base":before.as_ref().map(|b| &b.view),"base_snapshot":before.as_ref().map(|b| &b.snapshot),
                "base_digest":before.as_ref().and_then(|b| b.digests.get(&c.path))}),
        });
    }
    if before.is_none() {
        findings.push(Finding {
            ids: vec![],
            conclusion: "unknown",
            detail: json!({"interpretation":"net_change_unknown","reason":"No before-session source view was supplied, so net source changes cannot be established."}),
        });
    }
    for r in files.iter().filter(|r| r.location != "repository") {
        findings.push(Finding {
            ids: r.ids.iter().cloned().collect(),
            conclusion: "unknown",
            detail: json!({"path":r.path,"location":r.location,"interpretation":"not_joinable",
                "reason":if r.location == "external" {"Outside the repository root; never joined to source."} else {"Relative path without a recorded working directory."}}),
        });
    }
    finish(l, "session-changes", &s, findings, o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured(digest: Option<&str>) -> SourceFiles {
        SourceFiles {
            view: "view:test".into(),
            snapshot: "snapshot:test".into(),
            digests: digest
                .into_iter()
                .map(|d| ("src/lib.rs".into(), d.into()))
                .collect(),
            keys: BTreeMap::new(),
            modules: BTreeMap::new(),
        }
    }

    #[test]
    fn source_digest_change_has_five_cases() {
        for (before, after, expected) in [
            (Some("a"), Some("a"), "unchanged"),
            (Some("a"), Some("b"), "changed"),
            (None, Some("a"), "added"),
            (Some("a"), None, "removed"),
            (None, None, "not_captured"),
        ] {
            assert_eq!(
                captured(before).net_change(&captured(after), "src/lib.rs"),
                expected
            );
        }
    }

    #[test]
    fn matrix_keeps_an_absent_base_unknown() {
        let row = FileRow {
            location: "repository".into(),
            path: "src/lib.rs".into(),
            reads: BTreeMap::new(),
            attempts: BTreeMap::new(),
            confirmed: 1,
            tools: BTreeSet::new(),
            events: vec![],
            first: 1,
            last: 1,
            ids: BTreeSet::new(),
        };
        let changes = matrix(&[row], &captured(Some("a")), None);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].source, "unknown");
        assert_eq!(changes[0].interpretation, "net_change_unknown");
    }
}
