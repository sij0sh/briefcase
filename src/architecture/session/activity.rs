//! Cross-session activity: what many imported sessions recorded about each
//! repository path in a time window. It aggregates the per-session facts of
//! `session-files` (reads, edit attempts, confirmed edits, failed
//! operations) by exact repository path and reports, separately, what the
//! source views and the retained Git history say about those paths.
//!
//! The aggregate keeps the session rule: a recorded edit is not a source
//! change caused by the session. `window_source_change` compares two pinned
//! source views selected by the caller, and `history_commits_in_window`
//! counts retained commits in the window; neither is attributed to any
//! session. No intent, rationale or difficulty is inferred.
use super::{
    load,
    query::{file_rows, source_view},
};
use crate::architecture::{
    evolution, identity, ledger::Ledger, query::Options, require, retained_source::SourceFiles,
    Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const METHOD: &str = "session-activity-v1";
/// Sessions and assertions cited per path entry and per explanation handle.
const MAX_CITED: usize = 32;

/// Time window and optional source views. `since` is inclusive and `until`
/// exclusive (Unix seconds, `YYYY-MM-DD` or RFC 3339) and select sessions by
/// their recorded start. `view` (after) and `base` (before) are source
/// views; with both, each path reports its net change between them.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub since: Option<String>,
    pub until: Option<String>,
    pub view: Option<String>,
    pub base: Option<String>,
}

/// One session that recorded something about a path.
#[derive(Clone, Debug)]
pub(crate) struct Touch {
    pub view: String,
    pub started_at: Option<String>,
    pub read: bool,
    pub attempted: bool,
    pub edited: bool,
    pub failed: bool,
    pub user_requests: u64,
    pub ids: BTreeSet<String>,
}

/// Per-path aggregate over the sessions in the window.
#[derive(Clone, Debug, Default)]
pub(crate) struct PathActivity {
    pub path: String,
    pub touches: Vec<Touch>,
}
impl PathActivity {
    fn count(&self, f: impl Fn(&Touch) -> bool) -> usize {
        self.touches.iter().filter(|t| f(t)).count()
    }
    pub fn sessions(&self) -> usize {
        self.touches.len()
    }
    pub fn read_sessions(&self) -> usize {
        self.count(|t| t.read)
    }
    pub fn attempt_sessions(&self) -> usize {
        self.count(|t| t.attempted)
    }
    pub fn confirmed_sessions(&self) -> usize {
        self.count(|t| t.edited)
    }
    /// Sessions that attempted an edit but recorded no confirmed edit.
    pub fn unconfirmed_sessions(&self) -> usize {
        self.count(|t| t.attempted && !t.edited)
    }
    pub fn failed_sessions(&self) -> usize {
        self.count(|t| t.failed)
    }
    pub fn assertion_ids(&self) -> Vec<String> {
        self.touches
            .iter()
            .flat_map(|t| t.ids.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// Sessions selected by the window, and every path they recorded.
pub(crate) struct Aggregate {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub sessions: Vec<Value>,
    pub sessions_total: usize,
    pub undated_excluded: usize,
    pub partial_recordings: usize,
    /// Repository paths, keyed by exact repository-relative path.
    pub paths: BTreeMap<String, PathActivity>,
    /// Recorded paths outside the repository or without a working directory.
    pub not_joinable: usize,
}

pub(crate) fn aggregate(l: &Ledger, o: &Options, r: &Request) -> Result<Aggregate> {
    let since = r.since.as_deref().map(evolution::instant).transpose()?;
    let until = r.until.as_deref().map(evolution::instant).transpose()?;
    require(
        since.zip(until).is_none_or(|(s, u)| s < u),
        "SESSION_ACTIVITY",
        "since must be earlier than until",
    )?;
    let windowed = since.is_some() || until.is_some();
    let mut out = Aggregate {
        since,
        until,
        sessions: vec![],
        sessions_total: 0,
        undated_excluded: 0,
        partial_recordings: 0,
        paths: BTreeMap::new(),
        not_joinable: 0,
    };
    for view in l.scoped_views("session")? {
        out.sessions_total += 1;
        let s = load(
            l,
            &Options {
                view: view.clone(),
                component: None,
                limit: 1000,
                ..o.clone()
            },
        )?;
        let started_at = s.context["started_at"].as_str().map(str::to_owned);
        let start = started_at
            .as_deref()
            .and_then(|t| evolution::instant(t).ok());
        if windowed {
            match start {
                None => {
                    out.undated_excluded += 1;
                    continue;
                }
                Some(t) if since.is_some_and(|s| t < s) || until.is_some_and(|u| t >= u) => {
                    continue
                }
                Some(_) => {}
            }
        }
        if s.context["capture_state"] != "complete" {
            out.partial_recordings += 1;
        }
        let user_requests = s.context["coverage"]["user_messages"].as_u64().unwrap_or(0);
        out.sessions
            .push(json!({"view_id":view,"session_context_id":s.context["id"],
            "harness":s.context["harness"],"external_session_id":s.context["external_session_id"],
            "started_at":s.context["started_at"],"ended_at":s.context["ended_at"],
            "capture_state":s.context["capture_state"],"user_requests":user_requests}));
        for row in file_rows(l, &s, o)? {
            if row.location != "repository" {
                out.not_joinable += 1;
                continue;
            }
            let failed = row.reads.contains_key("failed") || row.attempts.contains_key("failed");
            let entry = out
                .paths
                .entry(row.path.clone())
                .or_insert_with(|| PathActivity {
                    path: row.path.clone(),
                    touches: vec![],
                });
            entry.touches.push(Touch {
                view: view.clone(),
                started_at: started_at.clone(),
                read: row.read(),
                attempted: row.attempted(),
                edited: row.edited(),
                failed,
                user_requests,
                ids: row.ids.clone(),
            });
        }
    }
    Ok(out)
}

/// Non-merge retained commits in the window touching each path (merges are
/// excluded as in change-activity-v1; noise paths are skipped).
pub(crate) fn commits_in_window(
    l: &Ledger,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<Option<BTreeMap<String, usize>>> {
    let commits = l.history_commits()?;
    if commits.is_empty() {
        return Ok(None);
    }
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for c in commits.iter().filter(|c| {
        c.parents.len() <= 1
            && since.is_none_or(|s| c.timestamp >= s)
            && until.is_none_or(|u| c.timestamp < u)
    }) {
        let paths: BTreeSet<&str> = c
            .changes
            .iter()
            .filter(|ch| evolution::noise(&ch.path).is_none())
            .map(|ch| ch.path.as_str())
            .collect();
        for p in paths {
            *out.entry(p.to_owned()).or_default() += 1;
        }
    }
    Ok(Some(out))
}

/// Net change of one path between two pinned source views.
pub(crate) fn net_change(before: &SourceFiles, after: &SourceFiles, path: &str) -> &'static str {
    before.net_change(after, path)
}

fn inside(path: &str, component: &str) -> bool {
    let c = component.trim_end_matches('/');
    path == c || path.starts_with(&format!("{c}/"))
}

/// `session-activity`: per repository path, how many sessions in the window
/// read it, attempted to edit it, recorded a confirmed edit, recorded only
/// unconfirmed attempts or a failed operation, and what the selected source
/// views and the retained history say about it.
pub fn activity(l: &Ledger, o: &Options, r: &Request) -> Result<Value> {
    Ok(report(l, o, r)?.0)
}

/// The retained report and the aggregate it was built from.
pub(crate) fn report(l: &Ledger, o: &Options, r: &Request) -> Result<(Value, Aggregate)> {
    require(
        (1..=1000).contains(&o.limit),
        "QUERY",
        "Limit must be 1..1000",
    )?;
    require(
        r.base.is_none() || r.view.is_some(),
        "SESSION_ACTIVITY",
        "base needs view: net change compares base (before) with view (after)",
    )?;
    let a = aggregate(l, o, r)?;
    let mut limitations: Vec<String> = vec![
        "A recorded edit is not a source change caused by a session; window_source_change and history_commits_in_window are never attributed to any session.".into(),
        "File reads and edits cover each profile's supported tools only; shell side effects are not reconstructed.".into(),
        "Joins use exact repository-relative paths; paths outside the repository or without a recorded working directory are counted, not joined.".into(),
    ];
    let after = match &r.view {
        Some(v) => Some(SourceFiles::new(&source_view(l, v, o)?)?),
        None => source_view(l, "latest", o)
            .ok()
            .and_then(|v| SourceFiles::new(&v).ok()),
    };
    let before = r
        .base
        .as_deref()
        .map(|b| source_view(l, b, o).and_then(|v| SourceFiles::new(&v)))
        .transpose()?;
    if after.is_none() {
        limitations.push("No source view is available; module rollup is not evaluated.".into());
    }
    if before.is_none() {
        limitations.push(
            "No base view was selected; net source change per path is unknown, not absent.".into(),
        );
    }
    let history = commits_in_window(l, a.since, a.until)?;
    if history.is_none() {
        limitations.push(
            "No Git history is retained; history_commits_in_window is unknown, not zero.".into(),
        );
    }
    if a.undated_excluded > 0 {
        limitations.push(format!(
            "{} session(s) without a recorded start were excluded from the window.",
            a.undated_excluded
        ));
    }
    if a.partial_recordings > 0 {
        limitations.push(format!(
            "{} session(s) in the window have a partial recording; their counts are lower bounds.",
            a.partial_recordings
        ));
    }
    let mut selected: Vec<&PathActivity> = a
        .paths
        .values()
        .filter(|p| o.component.as_deref().is_none_or(|c| inside(&p.path, c)))
        .collect();
    selected.sort_by(|x, y| {
        (y.confirmed_sessions(), y.sessions(), y.attempt_sessions())
            .cmp(&(x.confirmed_sessions(), x.sessions(), x.attempt_sessions()))
            .then_with(|| x.path.cmp(&y.path))
    });
    let total = selected.len();
    let truncated = total > o.limit;
    let mut entries: Vec<Value> = selected
        .into_iter()
        .take(o.limit)
        .map(|p| {
            let started: Vec<&str> = p
                .touches
                .iter()
                .filter_map(|t| t.started_at.as_deref())
                .collect();
            let net = match (&before, &after) {
                (Some(b), Some(v)) => net_change(b, v, &p.path),
                _ => "unknown",
            };
            json!({"path":p.path,
                "modules":after.as_ref().and_then(|v| v.modules.get(&p.path)),
                "in_view":after.as_ref().map(|v| v.digests.contains_key(&p.path)),
                "session_count":p.sessions(),"read_session_count":p.read_sessions(),
                "edit_attempt_session_count":p.attempt_sessions(),
                "confirmed_edit_session_count":p.confirmed_sessions(),
                "unconfirmed_edit_session_count":p.unconfirmed_sessions(),
                "failed_operation_session_count":p.failed_sessions(),
                "user_requests_in_touching_sessions":p.touches.iter().map(|t| t.user_requests).sum::<u64>(),
                "first_session_started_at":started.iter().min(),"last_session_started_at":started.iter().max(),
                "window_source_change":net,
                "history_commits_in_window":history.as_ref().map(|h| h.get(&p.path).copied().unwrap_or(0)),
                "sessions":p.touches.iter().take(MAX_CITED).map(|t| &t.view).collect::<Vec<_>>(),
                "assertion_ids":p.assertion_ids().into_iter().take(MAX_CITED).collect::<Vec<_>>(),
                "class":"session_aggregate","method":METHOD})
        })
        .collect();
    limitations.sort();
    limitations.dedup();
    let window = json!({"since":a.since,"until":a.until,"since_at":a.since.map(evolution::civil),
        "until_at":a.until.map(evolution::civil)});
    let coverage = json!({"sessions_total":a.sessions_total,"sessions_in_window":a.sessions.len(),
        "sessions_undated_excluded":a.undated_excluded,"sessions_partial_recording":a.partial_recordings,
        "recorded_paths_not_joinable":a.not_joinable,
        "net_change":if before.is_some() {"evaluated"} else {"not_requested"},
        "history":if history.is_some() {"retained"} else {"unavailable"},
        "module_rollup":if after.is_some() {"evaluated"} else {"unavailable"}});
    let outcome = if a.sessions.is_empty() {
        "unknown"
    } else if truncated || a.partial_recordings > 0 || a.undated_excluded > 0 {
        "partial"
    } else {
        "complete"
    };
    let request = json!({"since":r.since,"until":r.until,"view":r.view,"base":r.base,
        "component":o.component,"limit":o.limit,"historical":o.historical});
    let id = identity::id(
        "session-activity",
        &json!({"request":request,"window":window,"views":{"after":after.as_ref().map(|v| &v.view),
            "before":before.as_ref().map(|v| &v.view)},"sessions":a.sessions,"entries":entries,
            "coverage":coverage,"limitations":limitations,"outcome":outcome,
            "implementation":identity::bytes_digest(include_bytes!("activity.rs")),
            "source_implementation":identity::bytes_digest(include_bytes!("../retained_source.rs"))}),
    )?;
    let mut explanations = Vec::new();
    for (i, e) in entries.iter_mut().enumerate() {
        let handle = format!("explain:{id}:path:{i}");
        explanations.push((handle.clone(), json!({"report_id":id,"operation":"session_activity",
            "views":e["sessions"],"assertion_ids":e["assertion_ids"],"detail":e.clone(),
            "conclusion":"session_aggregate","historical":o.historical,"limitations":limitations,
            "selection_reasons":["imported session publications in the window","exact repository path"]})));
        e["explanation_handle"] = json!(handle);
    }
    let report = json!({"schema_version":"1.0","id":id,"operation":"session_activity","request":request,
        "window":window,"configuration":{"method":METHOD,"join":"exact repository-relative path",
            "window_selects":"session recorded start (since inclusive, until exclusive)",
            "max_cited_per_entry":MAX_CITED},
        "view":after.as_ref().map(|v| &v.view),"base_view":before.as_ref().map(|v| &v.view),
        "sessions":a.sessions,"entries":entries,"total_entries":total,"truncated":truncated,
        "coverage":coverage,"limitations":limitations,"outcome":outcome});
    crate::architecture::contracts::schema(&report, "session-activity", None)?;
    l.save_report(&id, &report, &explanations)?;
    Ok((report, a))
}

/// Compact text: one line per path, every count named.
pub fn render_text(report: &Value) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let c = &report["coverage"];
    let _ = writeln!(
        out,
        "SESSION ACTIVITY {} outcome={} sessions={}/{} window={}..{}",
        report["id"].as_str().unwrap_or(""),
        report["outcome"].as_str().unwrap_or(""),
        c["sessions_in_window"],
        c["sessions_total"],
        report["window"]["since_at"].as_str().unwrap_or("start"),
        report["window"]["until_at"].as_str().unwrap_or("now"),
    );
    for e in report["entries"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "- {} sessions={} read={} edit_attempted={} edit_confirmed={} unconfirmed_only={} failed={} net_change={} commits_in_window={}",
            e["path"].as_str().unwrap_or(""),
            e["session_count"],
            e["read_session_count"],
            e["edit_attempt_session_count"],
            e["confirmed_edit_session_count"],
            e["unconfirmed_edit_session_count"],
            e["failed_operation_session_count"],
            e["window_source_change"].as_str().unwrap_or("unknown"),
            e["history_commits_in_window"],
        );
    }
    if report["truncated"] == true {
        let _ = writeln!(
            out,
            "({} paths in total; raise --limit)",
            report["total_entries"]
        );
    }
    for l in report["limitations"].as_array().into_iter().flatten() {
        let _ = writeln!(out, "limitation: {}", l.as_str().unwrap_or(""));
    }
    out
}
