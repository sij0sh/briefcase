//! Durable lifecycle requests. The short state lock never waits for source analysis.
//! Worker ownership and acknowledgement are changed under that same state lock,
//! so a request racing worker exit or publication cannot disappear.
use super::{identity, reconcile::DirtySet, require, workspace, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct State {
    root: PathBuf,
    domains: DirtySet,
    requested: u64,
    completed: u64,
    first_pending_ms: u64,
    requested_at_ms: u64,
    checked_at_ms: u64,
    attempted_at_ms: u64,
    retry_at_ms: u64,
    source: String,
    reason: String,
    timeout: u64,
    attempts: u32,
    blocked: bool,
    last_result: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    enabled: bool,
}

pub fn enabled(root: &Path) -> Result<bool> {
    if !super::reconcile::watch_enabled(root).unwrap_or(true)
        || std::env::var("BRIEFCASE_SYNC").as_deref() == Ok("0")
    {
        return Ok(false);
    }
    match std::fs::read(root.join(".briefcase/schedule.json")) {
        Ok(bytes) => Ok(serde_json::from_slice::<Policy>(&bytes)?.enabled),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e.into()),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Resolve symlinked parents before deriving the process-shared namespace.
pub fn database(db: &Path) -> Result<PathBuf> {
    let parent = db.parent().ok_or("SCHEDULE: Database has no parent")?;
    workspace::private_dir(parent)?;
    if db.exists() {
        return Ok(db.canonicalize()?);
    }
    Ok(parent
        .canonicalize()?
        .join(db.file_name().ok_or("SCHEDULE: Database has no name")?))
}

fn namespace(db: &Path) -> Result<PathBuf> {
    let digest = identity::bytes_digest(db.as_os_str().as_encoded_bytes());
    Ok(db
        .parent()
        .ok_or("SCHEDULE: Database has no parent")?
        .join(".briefcase-scheduler")
        .join(digest.replace(':', "-")))
}
fn read(dir: &Path) -> Result<State> {
    match std::fs::read(dir.join("state.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}
fn save(dir: &Path, s: &State) -> Result<()> {
    workspace::write_atomic(&dir.join("state.json"), &serde_json::to_vec_pretty(s)?)
}
fn aligned(s: &State, root: &Path) -> Result<()> {
    require(
        s.root.as_os_str().is_empty() || s.root == root,
        "SCHEDULE_ROOT",
        "This database already belongs to another repository root",
    )
}
fn snapshot(dir: &Path, s: &State) -> Value {
    json!({"requested_revision":s.requested,"completed_revision":s.completed,
        "pending":s.requested > s.completed,"worker_active":workspace::writer_active(&dir.join("worker")),
        "watcher_active":workspace::writer_active(&dir.join("watcher")),
        "domains":s.domains,"root":s.root,"source":s.source,"reason":s.reason,"requested_at_ms":s.requested_at_ms,
        "last_checked_at_ms":s.checked_at_ms,"last_attempt_at_ms":s.attempted_at_ms,"retry_at_ms":s.retry_at_ms,
        "blocked":s.blocked,"attempts":s.attempts,"last_result":s.last_result,
        "freshness":"A completed request checks its capture; later live changes require another request."})
}
/// Read-only diagnostic: never create scheduler files.
pub fn status(db: &Path) -> Value {
    let resolved = if db.exists() {
        db.canonicalize()
    } else {
        db.parent()
            .unwrap_or(Path::new("."))
            .canonicalize()
            .map(|p| p.join(db.file_name().unwrap_or_default()))
    };
    let result = resolved
        .map_err(Into::into)
        .and_then(|db| namespace(&db))
        .and_then(|dir| read(&dir).map(|s| snapshot(&dir, &s)));
    result.unwrap_or_else(|e: Box<dyn std::error::Error + Send + Sync>| json!({"state":"unavailable","error":e.to_string()}))
}

/// Persist before waking a worker. `wake` runs under state admission, not writer admission.
pub fn request(
    root: &Path,
    db: &Path,
    source: &str,
    reason: &str,
    timeout: u64,
    wake: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    request_domains(
        root,
        db,
        source,
        reason,
        timeout,
        &super::reconcile::configured_domains(root).unwrap_or_else(|_| DirtySet::all()),
        wake,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn request_domains(
    root: &Path,
    db: &Path,
    source: &str,
    reason: &str,
    timeout: u64,
    domains: &DirtySet,
    wake: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    if !enabled(root)? {
        return Ok(json!({"schedule":"disabled"}));
    }
    if domains.is_empty() {
        return Ok(json!({"schedule":"up-to-date","sync":"up-to-date","ready":true}));
    }
    require(
        !source.is_empty() && source.len() <= 64 && !reason.is_empty() && reason.len() <= 64,
        "SCHEDULE",
        "Source and reason must contain 1..64 bytes",
    )?;
    let dir = namespace(db)?;
    let _guard = workspace::writer_lock(&dir.join("state"))?;
    let mut s = read(&dir)?;
    aligned(&s, root)?;
    let now = now_ms();
    if s.requested == s.completed {
        s.first_pending_ms = now;
    }
    s.domains.union(domains);
    s.root = root.to_owned();
    s.requested = s
        .requested
        .checked_add(1)
        .ok_or("SCHEDULE: Revision overflow")?;
    s.requested_at_ms = now;
    s.source = source.into();
    s.reason = reason.into();
    s.timeout = timeout;
    s.blocked = false;
    s.attempts = 0;
    s.retry_at_ms = 0;
    save(&dir, &s)?;
    wake()?;
    Ok(json!({"schedule":"queued","revision":s.requested,"database":db}))
}

/// One owner drains requests. Duplicate launchers exit without waiting.
/// Transient retries are rate/budget bounded; pending state survives exhaustion.
pub fn work(root: &Path, db: &Path, mut sync: impl FnMut(u64) -> Result<Value>) -> Result<()> {
    work_domains(root, db, |timeout, _| sync(timeout))
}
pub fn work_domains(
    root: &Path,
    db: &Path,
    mut sync: impl FnMut(u64, &DirtySet) -> Result<Value>,
) -> Result<()> {
    let dir = namespace(db)?;
    let guard = workspace::writer_lock(&dir.join("state"))?;
    let Some(owner) = workspace::try_writer_lock(&dir.join("worker"), Duration::ZERO)? else {
        return Ok(());
    };
    drop(guard);
    loop {
        let guard = workspace::writer_lock(&dir.join("state"))?;
        let s = read(&dir)?;
        aligned(&s, root)?;
        if s.requested == s.completed || s.blocked || s.attempts >= 10 || !enabled(root)? {
            // Release ownership before state admission: a new request can now safely wake us.
            drop(owner);
            drop(guard);
            return Ok(());
        }
        let due = s
            .requested_at_ms
            .saturating_add(1000)
            .min(s.first_pending_ms.saturating_add(10_000))
            .max(s.retry_at_ms);
        if now_ms() < due {
            drop(guard);
            std::thread::sleep(Duration::from_millis((due - now_ms().min(due)).min(250)));
            continue;
        }
        let revision = s.requested;
        let timeout = s.timeout.clamp(1, 3600);
        drop(guard);
        let selected = if s.domains.is_empty() {
            DirtySet::all()
        } else {
            s.domains.clone()
        };
        let configured =
            super::reconcile::configured_domains(root).unwrap_or_else(|_| DirtySet::all());
        let selected = selected.intersection(&configured);
        let result = if selected.is_empty() {
            Ok(json!({"sync":"up-to-date","ready":true}))
        } else {
            sync(timeout, &selected)
        }
        .unwrap_or_else(|e| json!({"sync":"failed","error":e.to_string()}));
        let guard = workspace::writer_lock(&dir.join("state"))?;
        let mut current = read(&dir)?;
        aligned(&current, root)?;
        acknowledge(&mut current, revision, result, now_ms());
        save(&dir, &current)?;
        drop(guard);
    }
}

fn acknowledge(s: &mut State, revision: u64, result: Value, now: u64) {
    let ready = result["ready"] == true;
    let configuration = result["sync"] == "needs_configuration";
    s.last_result = result;
    s.attempted_at_ms = now;
    if ready {
        s.checked_at_ms = now;
        s.completed = s.completed.max(revision);
        if s.requested == revision {
            s.domains = DirtySet::default();
        }
        s.attempts = 0;
        s.retry_at_ms = 0;
    } else if s.requested == revision {
        s.blocked = configuration;
        s.attempts += 1;
        s.retry_at_ms = now + (1000_u64 << s.attempts.min(5)) + u64::from(std::process::id() % 251);
    }
    if s.requested > revision {
        s.first_pending_ms = now;
    }
}

/// Bind infrastructure to its root even before the first worker publishes anything.
pub fn bind_root(root: &Path, db: &Path) -> Result<()> {
    let dir = namespace(db)?;
    let _guard = workspace::writer_lock(&dir.join("state"))?;
    let mut s = read(&dir)?;
    aligned(&s, root)?;
    if s.root.as_os_str().is_empty() {
        s.root = root.to_owned();
        save(&dir, &s)?;
    }
    Ok(())
}
pub fn bootstrap_lock(db: &Path) -> Result<workspace::WriterLock> {
    workspace::writer_lock(&namespace(db)?.join("bootstrap"))
}
pub fn watcher_lock(db: &Path) -> Result<Option<workspace::WriterLock>> {
    workspace::try_writer_lock(&namespace(db)?.join("watcher"), Duration::ZERO)
}

pub fn wait(db: &Path, revision: u64, budget: Duration) -> Value {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let s = status(db);
        if s["completed_revision"].as_u64().unwrap_or(0) >= revision
            || s["blocked"] == true
            || s["attempts"].as_u64().unwrap_or(0) >= 10
        {
            return s["last_result"].clone();
        }
        if std::time::Instant::now() >= deadline {
            return json!({"sync":"queued","ready":false,"message":"Request remains pending; inspect status for progress"});
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn immediate(dir: &Path) {
        let mut s = read(dir).unwrap();
        s.requested_at_ms = 0;
        s.first_pending_ms = 0;
        s.retry_at_ms = 0;
        save(dir, &s).unwrap();
    }
    #[test]
    fn worker_drains_an_edit_received_during_analysis_and_serializes_owners() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let db = database(&root.join("x.db")).unwrap();
        request(root, &db, "pi", "startup", 1, || Ok(())).unwrap();
        let dir = namespace(&db).unwrap();
        immediate(&dir);
        let mut passes = 0;
        work(root, &db, |_| {
            passes += 1;
            // Another worker cannot enter while this owner is analyzing.
            work(root, &db, |_| panic!("duplicate worker entered")).unwrap();
            if passes == 1 {
                // A manual writer can hold the source lock while a hook persists a request.
                let _writer = workspace::writer_lock(&db).unwrap();
                request(root, &db, "muse", "turn-end", 1, || Ok(())).unwrap();
                immediate(&dir);
            }
            Ok(json!({"ready":true,"sync":"refreshed"}))
        })
        .unwrap();
        assert_eq!(passes, 2);
        assert_eq!(read(&dir).unwrap().completed, 2);
        assert!(!workspace::writer_active(&dir.join("worker")));
    }
    #[test]
    fn an_interrupted_worker_leaves_requests_for_the_next_owner() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let db = database(&root.join("x.db")).unwrap();
        request(root, &db, "host", "files", 1, || Ok(())).unwrap();
        let dir = namespace(&db).unwrap();
        immediate(&dir);
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            work(root, &db, |_| panic!("simulated worker interruption")).unwrap();
        }));
        assert!(stopped.is_err());
        assert_eq!(read(&dir).unwrap().completed, 0);
        work(root, &db, |_| Ok(json!({"ready":true,"sync":"up-to-date"}))).unwrap();
        assert_eq!(read(&dir).unwrap().completed, 1);
    }
    #[test]
    fn late_request_survives_ack_and_other_roots_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let db = database(&root.join("x.db")).unwrap();
        request(root, &db, "pi", "startup", 1, || Ok(())).unwrap();
        request(root, &db, "claude-code", "turn-end", 1, || Ok(())).unwrap();
        let dir = namespace(&db).unwrap();
        let mut s = read(&dir).unwrap();
        acknowledge(&mut s, 1, json!({"ready":true,"view_id":"view:1"}), 100);
        save(&dir, &s).unwrap();
        assert_eq!(s.completed, 1);
        assert_eq!(s.requested, 2);
        assert!(request(&root.join("other"), &db, "host", "files", 1, || Ok(())).is_err());
        // Reloading the durable state after a worker crash retains the late revision.
        assert_eq!(read(&dir).unwrap().requested, 2);
    }
    #[test]
    fn disabling_all_domains_cannot_turn_an_empty_hint_into_a_full_source_sync() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let db = database(&root.join("x.db")).unwrap();
        request(root, &db, "watch", "initial", 1, || Ok(())).unwrap();
        let dir = namespace(&db).unwrap();
        immediate(&dir);
        workspace::write_atomic(&root.join(".briefcase/sync.json"), br#"{"sync":{"project":false,"git":false,"sessions":{"pi":false,"codex":false,"claude-code":false}}}"#).unwrap();
        work_domains(root, &db, |_, _| panic!("disabled domains were applied")).unwrap();
        assert_eq!(read(&dir).unwrap().completed, 1);
        assert_eq!(
            request(root, &db, "watch", "again", 1, || panic!(
                "empty request woke worker"
            ))
            .unwrap()["schedule"],
            "up-to-date"
        );
    }
    #[test]
    fn domain_union_survives_a_late_request_and_clears_only_after_its_acknowledgement() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let db = database(&root.join("x.db")).unwrap();
        let project = DirtySet {
            project: true,
            ..Default::default()
        };
        request_domains(root, &db, "watch", "project", 1, &project, || Ok(())).unwrap();
        let late = DirtySet {
            git: true,
            sessions: [super::super::session::Harness::Codex].into(),
            ..Default::default()
        };
        request_domains(root, &db, "watch", "git-and-sessions", 1, &late, || Ok(())).unwrap();
        let dir = namespace(&db).unwrap();
        let mut s = read(&dir).unwrap();
        acknowledge(&mut s, 1, json!({"ready":true}), 100);
        assert!(s.domains.project && s.domains.git && !s.domains.sessions.is_empty());
        assert_eq!(s.completed, 1);
        assert_eq!(s.requested, 2);
        acknowledge(&mut s, 2, json!({"ready":true}), 200);
        assert!(s.domains.is_empty());
        assert_eq!(s.completed, 2);
    }
    #[test]
    fn failed_wake_and_configuration_keep_pending_work() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let db = database(&root.join("x.db")).unwrap();
        assert!(request(root, &db, "host", "files", 1, || Err("spawn failed".into())).is_err());
        let dir = namespace(&db).unwrap();
        let mut s = read(&dir).unwrap();
        acknowledge(
            &mut s,
            1,
            json!({"sync":"needs_configuration","ready":false}),
            100,
        );
        assert!(s.blocked);
        assert_eq!(s.completed, 0);
        save(&dir, &s).unwrap();
        request(root, &db, "host", "files", 1, || Ok(())).unwrap();
        assert!(!read(&dir).unwrap().blocked);
    }
}
