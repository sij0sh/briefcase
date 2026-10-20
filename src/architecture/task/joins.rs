use super::trace::Boundary;
use super::*;
use crate::architecture::session;
use regex::Regex;

#[derive(Default)]
pub(super) struct Joins {
    pub history: Vec<Value>,
    pub sessions: Vec<Value>,
    pub errors: Vec<Value>,
    pub links: Vec<Value>,
    pub cochange: Vec<Value>,
    pub boundaries: Vec<Boundary>,
    pub session_views: Vec<String>,
    pub history_tip: Option<String>,
    pub sessions_considered: usize,
    pub session_findings: Vec<Value>,
    pub session_findings_diag: Value,
}

fn add_link(out: &mut Joins, predicate: &str, from: &str, to: &str, basis: Value) -> Result<()> {
    let id = identity::id("task-join", &json!([METHOD, predicate, from, to, basis]))?;
    out.links.push(
        json!({"id":id,"predicate":predicate,"from":from,"to":to,"class":"derived","basis":basis}),
    );
    Ok(())
}

const SESSION_LIMIT: usize = 200;

/// Retrieval precedes final slice selection. Only retained changed paths and
/// normalized read/confirmed-edit events can introduce files, never transcript
/// prose or failed edit attempts. Foreign repository recordings cannot seed.
pub(super) fn discover(
    l: &Ledger,
    s: &Snapshot,
    r: &super::request::Request,
    paths: &BTreeSet<String>,
) -> Result<(Vec<Value>, Value)> {
    let terms = super::retrieval::terms(&r.task);
    let mut hints = vec![];
    let mut scores = BTreeMap::<String, (u64, Vec<String>)>::new();
    if r.enabled("history") {
        for commit in l.history_commits()? {
            let touched: BTreeSet<_> = commit
                .changes
                .iter()
                .flat_map(|c| std::iter::once(c.path.clone()).chain(c.old_path.clone()))
                .collect();
            let overlap = !paths.is_disjoint(&touched);
            let task_match = super::retrieval::matches(&commit.message, &terms)
                >= terms.len().clamp(1, 2)
                && !terms.is_empty();
            if !overlap && !task_match
                || commit.parents.len() > 1
                || commit.changes.len() != commit.files_total
            {
                continue;
            }
            for path in touched.iter().filter(|p| s.files.contains_key(*p)) {
                let entry = scores.entry(path.clone()).or_default();
                entry.0 = (entry.0
                    + (if task_match { 100 } else { 80 }) / commit.files_total.max(2) as u64)
                    .min(80);
                if entry.1.len() < 8 {
                    entry.1.push(commit.oid.clone());
                }
            }
        }
    }
    for (path, (strength, commits)) in scores {
        hints.push(json!({"path":path,"facet":"history","kind":"retained_cochange_or_message",
        "strength":strength,"confidence":"observational_candidate","provenance":{"commits":commits,
            "meaning":"complete non-merge changed paths; inverse commit-size weighting; co-change and message relevance are not dependencies"}}));
    }
    let publications = l.session_publications()?;
    let mut considered = 0;
    let mut omitted_paths = 0;
    let mut omitted_requests = 0;
    let mut remaining_work=vec![];
    // Foreign recordings are filtered before the bound, so other repositories'
    // sessions cannot exhaust this repository's discovery budget.
    let repository = rows(&s.view.bundle, "snapshots")
        .first()
        .map(|v| text(v, "repository_id"))
        .unwrap_or_default();
    let root = s.view.diagnostics["capture"]["repository_root"]
        .as_str()
        .and_then(session::paths::normalize);
    let local: Vec<_> = publications
        .iter()
        .filter(|publication| {
            let recorded_root = publication["context"]["repository_root"]
                .as_str()
                .and_then(session::paths::normalize);
            publication["context"]["repository_id"] == repository
                && root
                    .as_ref()
                    .zip(recorded_root.as_ref())
                    .is_some_and(|(a, b)| a == b)
        })
        .collect();
    let omitted_sessions: Vec<_> = if r.enabled("sessions") {
        local
            .iter()
            .skip(SESSION_LIMIT)
            .map(|p| text(p, "view_id"))
            .collect()
    } else {
        vec![]
    };
    if r.enabled("sessions") {
        for publication in local.iter().take(SESSION_LIMIT) {
            considered += 1;
            if publication["context"]["retention"] == "metadata" {
                continue;
            }
            let view = text(publication, "view_id");
            let summary = session::touch_summary(l, &view)?;
            if summary["invalidated"] == true {
                continue;
            }
            omitted_requests += summary["omitted_requests"].as_u64().unwrap_or(0)
                + u64::from(summary["omitted_request_bytes"].as_u64().unwrap_or(0) > 0);
            if summary["omitted_requests"].as_u64().unwrap_or(0)>0 || summary["omitted_request_bytes"].as_u64().unwrap_or(0)>0 {
                remaining_work.push(json!({"kind":"session_requests","target":view,"status":"available",
                    "reason":"session_summary_bound","action":{"operation":"session_events","arguments":{"session":view,"kind":"user_message"}}}));
            }
            let mut touches = BTreeMap::<String, &Value>::new();
            for event in rows(&summary, "activities") {
                let path = text(&event["path"], "path");
                if !s.files.contains_key(&path) {
                    continue;
                }
                if touches
                    .get(&path)
                    .is_none_or(|old| old["kind"] != "file_edit_confirmed")
                {
                    touches.insert(path, event);
                }
            }
            let overlap = touches.keys().any(|p| paths.contains(p));
            let request = rows(&summary, "user_requests").iter().find(|e| {
                !terms.is_empty()
                    && super::retrieval::matches(&text(e, "text"), &terms)
                        >= terms.len().clamp(1, 2)
            });
            if !overlap && request.is_none() {
                continue;
            }
            let fanout = 1 + touches.len() / 8;
            omitted_paths += touches.len().saturating_sub(64);
            // Seed overlap and confirmed edits outrank reads within the path
            // bound; alphabetical order only breaks ties.
            let mut touches: Vec<_> = touches.into_iter().collect();
            touches.sort_by_key(|(path, event)| {
                (
                    !paths.contains(path),
                    event["kind"] != "file_edit_confirmed",
                    path.clone(),
                )
            });
            for (path,event) in touches.iter().skip(64) {
                remaining_work.push(json!({"kind":"session_path","target":path,"session_view":view,"event_id":event["id"],
                    "status":"available","reason":"session_path_limit",
                    "action":{"operation":"session_files","arguments":{"session":view}}}));
            }
            for (path, event) in touches.into_iter().take(64) {
                hints.push(json!({"path":path,"facet":"sessions","kind":event["kind"],
                    "strength":(if event["kind"]=="file_edit_confirmed" {60} else {40})/fanout as u64,
                    "confidence":"recorded_activity_candidate","provenance":{"session_view":view,"event_id":event["id"],
                        "summary_report_id":summary["id"],
                        "request_event_id":request.map(|e|&e["id"]),"matched_by":if overlap {"seed_path_overlap"} else {"recorded_request_terms"},
                        "meaning":"recorded read or confirmed edit; source-change ownership and causality unestablished"}}));
            }
        }
    }
    Ok((
        hints,
        json!({"history_enabled":r.enabled("history"),"sessions_enabled":r.enabled("sessions"),
        "session_publications":publications.len(),"repository_session_publications":local.len(),
        "foreign_session_publications":publications.len()-local.len(),"session_discovery_limit":SESSION_LIMIT,"sessions_considered":considered,
        "omitted_session_publications":omitted_sessions.len(),
        "continue_with":omitted_sessions.iter().take(16).map(|view|json!({"operation":"session_files","arguments":{"session":view}})).collect::<Vec<_>>(),
        "remaining_work":remaining_work.into_iter().chain(omitted_sessions.iter().map(|view|json!({"kind":"session_publication",
            "target":view,"status":"available","reason":"session_publication_limit",
            "action":{"operation":"session_files","arguments":{"session":view}}}))).collect::<Vec<_>>(),
        "omitted_session_paths":omitted_paths,"omitted_or_truncated_requests":omitted_requests}),
    ))
}

const MAX_SESSION_FINDINGS: usize = 16;

/// A foreign recording that the path join skips still has its drafts
/// counted as wrong-repository exclusions, so the diagnostic cannot read
/// as "no session findings existed".
fn count_wrong_repository(l: &Ledger, view: &str, diag: &mut Value) {
    let Ok(recording) = session::load_retained(l, view) else {
        return;
    };
    let (drafts, _) = session::episodes::analyze(
        recording.events(),
        view,
        &text(&recording.context, "id"),
        recording.context["capture_state"]
            .as_str()
            .unwrap_or("partial"),
    );
    diag["sessions_analyzed"] = json!(diag["sessions_analyzed"].as_u64().unwrap_or(0) + 1);
    diag["drafts"] = json!(diag["drafts"].as_u64().unwrap_or(0) + drafts.len() as u64);
    diag["excluded_wrong_repository"] =
        json!(diag["excluded_wrong_repository"].as_u64().unwrap_or(0) + drafts.len() as u64);
}

pub(super) fn build(
    l: &Ledger,
    s: &Snapshot,
    ix: &Index,
    paths: &BTreeSet<String>,
    sessions_enabled: bool,
) -> Result<Joins> {
    let mut out = Joins::default();
    let mut session_diag = json!({"sessions_analyzed":0,"drafts":0,"emitted":0,
        "excluded_unjoined":0,"excluded_wrong_repository":0,"excluded_sessions_disabled":0,"truncated":false});
    // Configuration identities the selected implementations read; a session
    // finding joins the task when it touches a selected path, invokes a
    // touchpoint of one, or sets a configuration key one reads. Task wording
    // is never required to name the historical terminology.
    let config_keys: BTreeSet<String> = ix
        .links
        .iter()
        .filter(|link| link.predicate == "READS_CONFIG" && !paths.is_disjoint(&link.from_files))
        .map(|link| link.to.clone())
        .collect();
    out.history_tip = l.history_tip()?.map(|(tip, _)| tip);
    let commits = l.history_commits()?;
    if out.history_tip.is_none() {
        out.boundaries.push(Boundary::NoHistory);
    }
    let features: Vec<_> = ix
        .entities
        .values()
        .filter(|e| e.kind == "feature")
        .collect();
    let mut coupling = BTreeMap::<String, BTreeSet<String>>::new();
    for commit in &commits {
        let touched: BTreeSet<_> = commit
            .changes
            .iter()
            .flat_map(|c| std::iter::once(c.path.clone()).chain(c.old_path.clone()))
            .collect();
        if paths.is_disjoint(&touched) {
            continue;
        }
        out.history.push(json!({"commit":commit.oid,"timestamp":commit.timestamp,"message":commit.message,
            "paths":touched.intersection(paths).collect::<Vec<_>>(),"retained_paths":touched.len(),"files_total":commit.files_total,
            "coverage":if commit.changes.len()<commit.files_total {"partial_changed_paths"} else {"retained_commit_metadata"}}));
        if commit.changes.len() < commit.files_total {
            out.boundaries
                .push(Boundary::PartialHistory(json!({"commit":commit.oid,
                "reason":"Retained changed paths for a joined commit are partial."})));
        }
        if commit.parents.len() <= 1
            && commit.files_total <= 100
            && commit.changes.len() == commit.files_total
        {
            for path in touched.difference(paths) {
                coupling
                    .entry(path.clone())
                    .or_default()
                    .insert(commit.oid.clone());
            }
        }
        for feature in &features {
            if !feature.files.is_disjoint(&touched) {
                let basis = json!({"method":"exact_changed_path_intersects_declared_feature_files","source_view":s.view.id,
                    "paths":feature.files.intersection(&touched).collect::<Vec<_>>(),"history_tip":out.history_tip});
                add_link(
                    &mut out,
                    "COMMIT_TOUCHED_FEATURE",
                    &commit.oid,
                    &feature.key,
                    basis,
                )?;
            }
        }
    }
    out.cochange=coupling.into_iter().map(|(path,oids)|json!({"path":path,"shared_commits":oids.len(),"commit_oids":oids,
        "class":"derived","basis":"non-merge retained commits with complete changed paths and at most 100 files; co-change is not a dependency"})).collect();
    let repository = rows(&s.view.bundle, "snapshots")
        .first()
        .map(|v| text(v, "repository_id"))
        .unwrap_or_default();
    let root = s.view.diagnostics["capture"]["repository_root"]
        .as_str()
        .and_then(session::paths::normalize);
    let publications = l.session_publications()?;
    if publications.is_empty() {
        out.boundaries.push(Boundary::NoSessions);
    }
    let receipt = Regex::new(r"(?m)^\[[^\]\r\n]+ ([0-9a-f]{7,64})\](?: [^\r\n]*)?\r?$")?;
    for publication in publications {
        let view = text(&publication, "view_id");
        // Imports share the ledger repository ID even when the recording came
        // from another root. Require the retained roots to agree as well.
        let recorded_root = publication["context"]["repository_root"]
            .as_str()
            .and_then(session::paths::normalize);
        let same_repository = publication["context"]["repository_id"] == repository
            && root
                .as_ref()
                .zip(recorded_root.as_ref())
                .is_some_and(|(a, b)| a == b);
        let summary = session::touch_summary(l, &view)?;
        let absolute = crate::architecture::contracts::strings(&summary["absolute_paths"]);
        let foreign_paths: BTreeSet<String> = root
            .as_ref()
            .into_iter()
            .flat_map(|root| {
                absolute.iter().filter_map(|p| {
                    p.strip_prefix(&format!("{}/", root.trim_end_matches('/')))
                        .filter(|p| s.files.contains_key(*p))
                        .map(str::to_owned)
                })
            })
            .collect();
        let mut routed: BTreeSet<String> = if same_repository {
            crate::architecture::contracts::strings(&summary["paths"])
                .into_iter()
                .collect()
        } else {
            foreign_paths.clone()
        };
        routed.extend(foreign_paths);
        let touchpoints: BTreeSet<_> = ix
            .links
            .iter()
            .filter(|e| !paths.is_disjoint(&e.from_files) || !paths.is_disjoint(&e.to_files))
            .flat_map(|e| [&e.from, &e.to])
            .filter_map(|key| {
                key.strip_prefix("executable:")
                    .or_else(|| key.strip_prefix("path:"))
            })
            .collect();
        let invocations: Vec<_> = crate::architecture::contracts::strings(&summary["executables"])
            .into_iter()
            .filter(|exe| touchpoints.contains(exe.as_str()))
            .collect();
        if !same_repository && paths.is_disjoint(&routed) && invocations.is_empty() {
            count_wrong_repository(l, &view, &mut session_diag);
            continue;
        }
        out.sessions_considered += 1;
        if summary["retention"] == "metadata" {
            out.boundaries.push(Boundary::MetadataSession(json!({"view_id":view,"reason":"Metadata retention has no path/activity evidence."})));
        }
        if paths.is_disjoint(&routed) && invocations.is_empty() {
            continue;
        }
        let recording = session::load_retained(l, &view)?;
        out.session_views.push(view.clone());
        if recording.view.invalidated {
            out.boundaries.push(Boundary::RetractedSession(json!({"view_id":view,"reason":"Session contains retracted facts; task joins from this recording are withheld."})));
            continue;
        }
        let events = recording.events();
        // Phase 2: deterministic episode analyses over the same recording,
        // joined to this task only through verified subjects. No separate
        // storage, ranking or delivery path is created for them.
        {
            let (drafts, _analysis) = session::episodes::analyze(
                events,
                &view,
                &text(&recording.context, "id"),
                recording.context["capture_state"]
                    .as_str()
                    .unwrap_or("partial"),
            );
            session_diag["sessions_analyzed"] =
                json!(session_diag["sessions_analyzed"].as_u64().unwrap_or(0) + 1);
            session_diag["drafts"] =
                json!(session_diag["drafts"].as_u64().unwrap_or(0) + drafts.len() as u64);
            for draft in drafts {
                if !sessions_enabled {
                    session_diag["excluded_sessions_disabled"] = json!(
                        session_diag["excluded_sessions_disabled"]
                            .as_u64()
                            .unwrap_or(0)
                            + 1
                    );
                    continue;
                }
                if !same_repository {
                    session_diag["excluded_wrong_repository"] = json!(
                        session_diag["excluded_wrong_repository"]
                            .as_u64()
                            .unwrap_or(0)
                            + 1
                    );
                    continue;
                }
                let joined: Vec<String> =
                    crate::architecture::contracts::strings(&draft["subjects"])
                        .into_iter()
                        .filter(|subject| {
                            paths.contains(subject)
                                || config_keys.contains(subject)
                                || subject.strip_prefix("command:").is_some_and(|core| {
                                    invocations.iter().any(|inv| core.contains(inv.as_str()))
                                })
                        })
                        .collect();
                if joined.is_empty() {
                    session_diag["excluded_unjoined"] =
                        json!(session_diag["excluded_unjoined"].as_u64().unwrap_or(0) + 1);
                    continue;
                }
                if out.session_findings.len() >= MAX_SESSION_FINDINGS {
                    session_diag["truncated"] = json!(true);
                    continue;
                }
                let complete = recording.context["capture_state"] == "complete";
                let task_local_requirement = draft["kind"] == "session_review_requirement"
                    && draft["payload"]["scope"] == "task_local";
                let (status, reason) = if complete && !task_local_requirement {
                    ("match", "Same repository and a verified subject join; the recorded environment itself was not re-verified.")
                } else if task_local_requirement {
                    ("unknown", "The recorded requirement was scoped to its own task; it is not current guidance for this task.")
                } else {
                    (
                        "unknown",
                        "Session capture is not complete; applicability requires verification.",
                    )
                };
                out.session_findings.push(super::findings::session_envelope(
                    &draft, &s.view.id, &joined, status, reason,
                ));
                session_diag["emitted"] = json!(session_diag["emitted"].as_u64().unwrap_or(0) + 1);
            }
        }
        let mut touches: BTreeSet<_> = events
            .iter()
            .filter(|e| {
                e["path"]["location"] == "repository"
                    && ["file_read", "file_edit_attempt", "file_edit_confirmed"]
                        .contains(&e["kind"].as_str().unwrap_or(""))
            })
            .map(|e| text(&e["path"], "path"))
            .collect();
        if !same_repository {
            touches = routed.clone();
        }
        if paths.is_disjoint(&touches) && invocations.is_empty() {
            continue;
        }
        for invocation in &invocations {
            add_link(
                &mut out,
                "SESSION_INVOKED_TOUCHPOINT",
                &view,
                invocation,
                json!({"method":"literal first executable token in a direct recorded command","same_repository":same_repository,
                    "identity":"candidate reference; installed bytes and executed process identity are unestablished"}),
            )?;
        }
        let requests: Vec<_> = events
            .iter()
            .filter(|e| e["kind"] == "user_message")
            .collect();
        let user_requests:Vec<_>=requests.iter().take(16).map(|e|json!({"event_id":e["id"],
            "text":e["text"].as_str().unwrap_or("").chars().take(800).collect::<String>(),
            "omitted_characters":e["text"].as_str().unwrap_or("").chars().count().saturating_sub(800)})).collect();
        out.sessions.push(json!({"view_id":view,"session_context_id":recording.context["id"],
            "started_at":recording.context["started_at"],"capture_state":recording.context["capture_state"],
            "paths":touches.intersection(paths).collect::<Vec<_>>(),"user_requests":user_requests,"omitted_user_requests":requests.len().saturating_sub(16),
            "invoked_touchpoints":invocations,"same_repository":same_repository,
            "basis":"recorded paths or exact executable-token reference; not source change attribution or process identity"}));
        if recording.context["capture_state"] != "complete"
            || recording.context["retention"] == "metadata"
        {
            out.boundaries.push(Boundary::PartialSession(json!({"view_id":view,"reason":"Recording or retained transcript coverage is partial."})));
        }
        let edited: BTreeSet<_> = events
            .iter()
            .filter(|_| same_repository)
            .filter(|e| e["kind"] == "file_edit_confirmed" && e["path"]["location"] == "repository")
            .map(|e| text(&e["path"], "path"))
            .collect();
        for feature in &features {
            if !edited.is_disjoint(&feature.files) {
                add_link(
                    &mut out,
                    "SESSION_TOUCHED_FEATURE",
                    &view,
                    &feature.key,
                    json!({"method":"confirmed_recorded_edit_intersects_declared_feature_files","source_view":s.view.id,
                        "paths":edited.intersection(&feature.files).collect::<Vec<_>>(),"meaning":"recording, not ownership of a net source change"}),
                )?;
            }
        }
        for event in events.iter().filter(|e| e["kind"] == "command_finished") {
            let error = event["outcome"] == "failed";
            if error {
                out.errors.push(json!({"session_view":view,"event_id":event["id"],"command":event["command"],"exit_code":event["exit_code"],
                    "basis":"recorded failed command; root cause and eventual fix unknown"}));
                add_link(
                    &mut out,
                    "ERROR_OCCURRED_IN_SESSION",
                    &text(event, "id"),
                    &view,
                    json!({"event_id":event["id"],"outcome":event["outcome"]}),
                )?;
            }
            if event["outcome"] != "succeeded" {
                continue;
            }
            if !same_repository {
                continue;
            }
            let command = text(event, "command");
            // Only a literal git commit invocation, never an assistant message
            // mentioning a hash. Alternate git -C roots remain unjoined.
            if !direct_commit_command(&command) {
                if command.contains("git commit") {
                    out.boundaries.push(Boundary::IndirectCommit(json!({"session_view":view,"event_id":event["id"],
                        "reason":"Compound/indirect git invocation or dry-run; no commit attribution was inferred."})));
                }
                continue;
            }
            let cwd = event["cwd"]
                .as_str()
                .or_else(|| recording.context["working_directory"].as_str());
            let root = recording.context["repository_root"].as_str().unwrap_or("");
            let root = session::paths::normalize(root);
            let cwd = cwd.and_then(session::paths::normalize);
            if !cwd.zip(root).is_some_and(|(cwd, root)| {
                cwd == root || cwd.starts_with(&format!("{}/", root.trim_end_matches('/')))
            }) {
                continue;
            }
            let call = event["call_event"]
                .as_str()
                .and_then(|id| recording.event(id));
            let result = call
                .and_then(|c| c["result_event"].as_str())
                .and_then(|id| recording.event(id));
            let Some(result) =
                result.filter(|r| r["paired"] == true && r["outcome"] == "succeeded")
            else {
                continue;
            };
            let Some(raw) = recording.record(result) else {
                continue;
            };
            let raw: Value = serde_json::from_str(raw)?;
            let pointer = result["pointer"].as_str().unwrap_or("");
            let payload = raw.pointer(pointer).unwrap_or(&raw);
            let output = result_output(payload);
            for capture in receipt.captures_iter(&output) {
                let short = capture.get(1).unwrap().as_str();
                let candidates: Vec<_> = commits
                    .iter()
                    .filter(|c| c.oid.starts_with(short))
                    .collect();
                if candidates.len() != 1 {
                    continue;
                }
                add_link(
                    &mut out,
                    "SESSION_PRODUCED_COMMIT",
                    &view,
                    &candidates[0].oid,
                    json!({"method":"unique_retained_commit_matches_paired_successful_git_commit_receipt", "event_id":event["id"],
                        "result_event":result["id"],"record_ordinal":result["ordinal"],"receipt":capture.get(0).unwrap().as_str(),
                        "meaning":"session recorded this commit receipt; no attribution from time or file overlap"}),
                )?;
            }
        }
        for failure in events
            .iter()
            .filter(|e| e["kind"] == "command_finished" && e["outcome"] == "failed")
        {
            let from = failure["ordinal"].as_u64().unwrap_or(0);
            let retry = events.iter().find(|e| {
                e["kind"] == "command_finished"
                    && e["outcome"] == "succeeded"
                    && e["command"] == failure["command"]
                    && e["ordinal"].as_u64().unwrap_or(0) > from
            });
            if let Some(retry) = retry {
                let until = retry["ordinal"].as_u64().unwrap_or(0);
                let edits: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        e["kind"] == "file_edit_confirmed"
                            && e["ordinal"].as_u64().is_some_and(|n| n > from && n < until)
                    })
                    .map(|e| json!({"event_id":e["id"],"path":e["path"]}))
                    .collect();
                if !edits.is_empty() {
                    add_link(
                        &mut out,
                        "POSSIBLE_FIX_FOR_ERROR",
                        &text(retry, "id"),
                        &text(failure, "id"),
                        json!({"session_view":view,"method":"same recorded command failed, edits were recorded, then retry succeeded",
                            "edit_events":edits,"causality":"unestablished; a temporal candidate only"}),
                    )?;
                }
            }
        }
    }
    out.session_findings
        .sort_by_key(|f| text(f, "comparison_key"));
    out.session_findings
        .dedup_by(|a, b| a["comparison_key"] == b["comparison_key"]);
    out.session_findings_diag = session_diag;
    out.links.sort_by_key(|v| text(v, "id"));
    out.links.dedup_by(|a, b| a["id"] == b["id"]);
    Ok(out)
}

fn result_output(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.into();
    }
    let mut parts = vec![];
    if let Some(message) = value
        .get("message")
        .filter(|m| m["role"] == "toolResult" || m["role"] == "tool")
    {
        parts.push(result_output(message));
    }
    if let Some(payload) = value.get("payload").filter(|p| {
        [
            "function_call_output",
            "custom_tool_call_output",
            "tool_result",
        ]
        .contains(&p["type"].as_str().unwrap_or(""))
    }) {
        parts.push(result_output(payload));
    }
    // Explicit result fields only. Input command strings and model messages
    // are never scanned as commit receipts.
    for field in ["output", "stdout", "content", "result", "response"] {
        if let Some(v) = value.get(field) {
            if let Some(text) = v.as_str() {
                parts.push(text.to_owned());
            } else if let Some(items) = v.as_array() {
                parts.extend(
                    items
                        .iter()
                        .filter_map(|i| i["text"].as_str().map(str::to_owned)),
                );
            } else if v.is_object() {
                parts.push(result_output(v));
            }
        }
    }
    parts.join("\n")
}

/// A conservative direct invocation: quoting is respected, compound commands,
/// substitutions, redirections and alternate Git roots are not interpreted.
fn direct_commit_command(command: &str) -> bool {
    let Some(words) = session::paths::direct_command(command) else {
        return false;
    };
    words.len() >= 2
        && words[0] == "git"
        && words[1] == "commit"
        && !words.iter().skip(2).any(|w| {
            [
                "--dry-run",
                "--help",
                "-h",
                "--git-dir",
                "--work-tree",
                "-C",
            ]
            .contains(&w.as_str())
                || w.starts_with("--git-dir=")
                || w.starts_with("--work-tree=")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn result_receipts_exclude_inputs_and_assistant_prose() {
        assert_eq!(
            result_output(&json!({"arguments":"[main abcdef1] fake","text":"[main abcdef1] fake"})),
            ""
        );
        assert_eq!(
            result_output(&json!({"content":[{"type":"text","text":"[main abcdef1] fix"}]})),
            "[main abcdef1] fix"
        );
        assert_eq!(
            result_output(
                &json!({"message":{"role":"toolResult","content":[{"type":"text","text":"[main abcdef1] fix"}]}})
            ),
            "[main abcdef1] fix"
        );
        assert_eq!(
            result_output(
                &json!({"message":{"role":"assistant","content":[{"type":"text","text":"[main abcdef1] fake"}]}})
            ),
            ""
        );
    }
    #[test]
    fn commit_invocation_requires_a_direct_command_and_rejects_shell_prose() {
        assert!(direct_commit_command("git commit -m 'fix import'"));
        assert!(direct_commit_command("git commit --amend --no-edit"));
        for command in [
            "echo 'git commit -m fake'",
            "echo 'x; git commit -m fake'",
            "git add . && git commit -m fix",
            "git commit --dry-run",
            "git -C /other commit -m fix",
            "git commit -m $(printf fake)",
        ] {
            assert!(!direct_commit_command(command), "{command}");
        }
    }
}
