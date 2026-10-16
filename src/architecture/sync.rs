//! Reconcile deterministic local sources. This module never invokes Claim providers.
pub mod git;
pub mod project;
pub mod sessions;
use super::{
    ledger::Ledger,
    reconcile::{self, AlignmentState, DirtySet},
    runtime_observations, workspace, Result,
};
pub use project::{Language, SyncOptions};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const ERROR_LIMIT: usize = 4096;
pub fn status_path(db: &Path) -> PathBuf {
    workspace::sibling(db, "sync.json")
}

fn now() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    runtime_observations::timestamp(nanos)
}

fn bounded(message: &str) -> String {
    let mut end = message.len().min(ERROR_LIMIT);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

fn write_status(db: &Path, body: &Value) {
    // Diagnostics are advisory; a failed write never changes the sync outcome.
    let _ = workspace::write_atomic(
        &status_path(db),
        serde_json::to_string_pretty(body)
            .unwrap_or_default()
            .as_bytes(),
    );
}

/// Publication readiness for adapters without creating or writing the database.
pub fn readiness(db: &Path) -> Value {
    let sync = std::fs::read(status_path(db))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or(Value::Null);
    let writing = workspace::writer_active(db);
    let initialized = db.is_file();
    let ledger = initialized
        .then(|| Ledger::read_only(db).and_then(|l| l.status()))
        .transpose();
    let (state, error) = match &ledger {
        _ if writing => ("syncing", None),
        Err(e) => ("unavailable", Some(e.to_string())),
        Ok(None) => ("not_initialized", None),
        Ok(Some(s)) if s["current_view"].is_null() => ("not_ready", None),
        Ok(Some(s)) if s["pending_or_interrupted"] == true => ("stale_intent", None),
        Ok(Some(s)) => {
            let view = s["current_view"].as_str().unwrap_or("");
            match Ledger::read_only(db).and_then(|l| l.load(view)) {
                Ok(v) if v.stale || v.projections.len() != 2 => ("needs_rebuild", None),
                Ok(_) => ("ready", None),
                Err(e) => ("unavailable", Some(e.to_string())),
            }
        }
    };
    let layers=if initialized {Ledger::read_only(db).and_then(|l|super::task::readiness(&l)).unwrap_or_else(|e|json!({"state":"unavailable","error":e.to_string()}))} else {json!({"source_text":{"state":"unavailable"}})};
    json!({"database":db,"state":state,"layers":layers,"writer_active":writing,"error":error,"last_sync":sync,
        "scheduler":super::scheduler::status(db)})
}

impl SyncOptions {
    pub fn local(root: PathBuf, db: PathBuf, timeout: Duration) -> Self {
        Self {
            root,
            db,
            timeout,
            lock_wait: Duration::ZERO,
            language: None,
            project: None,
            policy: None,
            calls: None,
            analyzer: None,
            node: "node".into(),
            compiler: None,
            python: None,
            python_source_roots: vec![],
            go: None,
            go_source_roots: vec![],
            go_tags: vec![],
            goos: None,
            goarch: None,
            go_tests: false,
            javac: None,
            java_source_roots: vec![],
            java_release: None,
            dotnet: None,
            csharp_source_roots: vec![],
            csharp_defines: vec![],
            csharp_language_version: None,
            clang: None,
            llvm_config: None,
            cc_source_roots: vec![],
            cc_include_roots: vec![],
            cc_defines: vec![],
            cc_standard: None,
            cc_target: None,
        }
    }
}
/// Reject a selected database belonging to a different repository before any local writes.
pub fn validate_database(root: &Path, db: &Path) -> Result<()> {
    if let Some(bound) = super::scheduler::status(db)["root"]
        .as_str()
        .filter(|r| !r.is_empty())
    {
        super::require(
            Path::new(bound) == root,
            "SCHEDULE_ROOT",
            "This database already belongs to another repository root",
        )?;
    }
    if db.is_file() {
        let ledger = Ledger::read_only(db)?;
        if ledger.sync_state()?.is_some() {
            let repository = ledger.repository()?;
            let expected =
                super::identity::id("repository", &json!({"root":root.to_string_lossy()}))?;
            super::require(
                repository == expected,
                "NAMESPACE",
                "Database belongs to another repository",
            )?;
        }
    }
    Ok(())
}
/// Read-only: no locks, schema creation, status files, or generations.
pub fn check(o: &SyncOptions, domains: &DirtySet) -> Result<Value> {
    let alignment = reconcile::check(o, domains)?;
    let ready = alignment.iter().all(|a| a.state == AlignmentState::Aligned);
    Ok(json!({"sync":"checked","ready":ready,"alignment":alignment,
        "dirty":reconcile::dirty(&alignment),"claims":claims_status(&o.db)?}))
}
pub fn sync(o: &SyncOptions) -> Result<Value> {
    sync_domains(
        o,
        &reconcile::configured_domains(&o.root).unwrap_or_else(|_| DirtySet::all()),
    )
}

/// All local adapters share writer admission. A failing domain does not suppress others.
pub fn sync_domains(o: &SyncOptions, domains: &DirtySet) -> Result<Value> {
    let started = Instant::now();
    let Some(lock) = workspace::try_writer_lock(&o.db, o.lock_wait)? else {
        return Ok(json!({"sync":"locked","database":o.db,"ready":false,
            "message":"Another Briefcase writer holds this database"}));
    };
    let base = json!({"database":o.db,"root":o.root,"pid":std::process::id(),"started_at":now()});
    let mut running = base.clone();
    running["sync"] = json!("syncing");
    write_status(&o.db, &running);
    let outcome = run(o, domains, &lock, started);
    let mut result = match outcome {
        Ok(v) => v,
        Err(e) => failure(e),
    };
    for (k, v) in base.as_object().unwrap() {
        result[k] = v.clone();
    }
    result["finished_at"] = json!(now());
    result["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
    write_status(&o.db, &result);
    Ok(result)
}
fn failure(e: Box<dyn std::error::Error + Send + Sync>) -> Value {
    json!({"sync":if project::is_timeout(e.as_ref()) {"timeout"} else {"failed"},"status":"failed",
        "ready":false,"error":bounded(&e.to_string())})
}
fn run(
    o: &SyncOptions,
    domains: &DirtySet,
    lock: &workspace::WriterLock,
    started: Instant,
) -> Result<Value> {
    let alignment = reconcile::check(o, domains)?;
    let mut result = json!({"sync":"up-to-date","ready":true,"alignment":alignment,"sessions":{}});
    let mut ledger = None;
    let mut changed = false;
    for a in &alignment {
        let value = if a.state == AlignmentState::Unavailable {
            a.detail.clone()
        } else if a.source == "project" {
            if a.dirty() {
                project::apply(o, lock, started).unwrap_or_else(failure)
            } else {
                let mut v = a.detail.clone();
                v["sync"] = json!("up-to-date");
                v
            }
        } else if a.source == "git" {
            if a.dirty() {
                let l = ledger.insert(Ledger::open(&o.db)?);
                crate::architecture::git_history::update(l, &o.root, started + o.timeout)
                    .unwrap_or_else(failure)
            } else {
                json!({"status":a.detail["status"].as_str().filter(|s| *s != "available").unwrap_or("up-to-date"),"tip":a.detail["head"]})
            }
        } else if a.dirty() {
            crate::architecture::ledger::recovery::check_deadline(started + o.timeout)?;
            let h = a.source.trim_start_matches("sessions:").parse()?;
            let l = ledger.insert(Ledger::open(&o.db)?);
            sessions::apply(l, &o.root, h).unwrap_or_else(failure)
        } else {
            json!({"status":a.detail["status"].as_str().unwrap_or("up-to-date")})
        };
        let state = value["sync"]
            .as_str()
            .or(value["status"].as_str())
            .unwrap_or("failed");
        let good = matches!(
            state,
            "up-to-date"
                | "refreshed"
                | "rebuilt"
                | "published"
                | "not_detected"
                | "no_commits"
                | "not_a_repository"
        ) || value.get("operation").is_some();
        changed |= matches!(state, "refreshed" | "rebuilt" | "published")
            || value["sessions"]
                .as_array()
                .is_some_and(|ss| ss.iter().any(|s| s["result"]["status"] == "published"));
        if !good {
            result["ready"] = json!(false);
        }
        if a.source == "project" {
            // Preserve the established source result fields for CLI consumers.
            for (k, v) in value.as_object().into_iter().flatten() {
                result[k] = v.clone();
            }
            result["project"] = value;
            if let Some(view) = result["view_id"].as_str() {
                Ledger::open_existing(&o.db)?.prune_recall(view)?;
            }
        } else if a.source == "git" {
            result["history"] = value.clone();
            result["git"] = value;
        } else {
            result["sessions"][a.source.trim_start_matches("sessions:")] = value;
        }
    }
    // Source readiness keeps its established result name. Other domain failures stay visible.
    let failed = result["ready"] == false;
    if failed
        && matches!(
            result["sync"].as_str(),
            Some("up-to-date" | "refreshed" | "rebuilt")
        )
    {
        result["sync"] = json!("failed");
    }
    result["local_sync"] = json!(if failed {
        "incomplete"
    } else if changed {
        "updated"
    } else {
        "up-to-date"
    });
    result["claims"] = claims_status(&o.db)?;
    Ok(result)
}

pub fn claims_status(db: &Path) -> Result<Value> {
    if !db.is_file() {
        return Ok(
            json!({"state":"manual","domains":{"state":"not_run_for_current_view"},
        "lineage":{"state":"not_run_for_current_view"}}),
        );
    }
    Ledger::read_only(db)?.claims_coverage()
}
