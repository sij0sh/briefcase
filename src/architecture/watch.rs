//! OS events only hint at dirtiness. The shared checker decides whether to reconcile.
//! A low-frequency full check catches dropped notifications and newly created stores.
use super::{
    reconcile::{self, DirtySet},
    scheduler,
    session::Harness,
    sync, workspace, Result,
};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

fn ignored(path: &Path, root: &Path, db: &Path) -> bool {
    if workspace::database_files(db).iter().any(|p| p == path) {
        return true;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative.components().any(|p| {
        matches!(
            p.as_os_str().to_str(),
            Some(".briefcase-scheduler" | "target" | "node_modules" | "briefcase-backups")
        )
    }) {
        return true;
    }
    if path.starts_with(root.join(".briefcase")) {
        let policy = std::fs::read(root.join(".briefcase/sync.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["policy"].as_str().map(|p| root.join(p)));
        return path != root.join(".briefcase/sync.json")
            && path != root.join(".briefcase/schedule.json")
            && policy.as_deref() != Some(path);
    }
    // Atomic status writes use a temporary sibling too.
    path.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        let db = db.file_name().unwrap_or_default().to_string_lossy();
        n.starts_with(&format!(".{db}.")) && n.ends_with(".tmp")
    })
}
fn hint(
    event: &Event,
    root: &Path,
    db: &Path,
    gitdirs: &[PathBuf],
    stores: &[(Harness, PathBuf)],
) -> DirtySet {
    let mut dirty = DirtySet::default();
    if matches!(event.kind, EventKind::Access(_)) {
        return dirty;
    }
    for path in &event.paths {
        if ignored(path, root, db) {
            continue;
        }
        if *path == root.join(".briefcase/sync.json")
            || *path == root.join(".briefcase/schedule.json")
        {
            dirty.union(&DirtySet::all());
        }
        for (h, dir) in stores {
            if path.starts_with(dir) || dir.starts_with(path) {
                dirty.sessions.insert(*h);
            }
        }
        if gitdirs.iter().any(|dir| path.starts_with(dir)) {
            dirty.git = true;
        } else if path.starts_with(root) {
            dirty.project = true;
            dirty.git = true;
        }
    }
    dirty
}
fn gitdirs(root: &Path) -> Vec<PathBuf> {
    ["--absolute-git-dir", "--git-common-dir"]
        .into_iter()
        .filter_map(|arg| {
            Command::new("git")
                .current_dir(root)
                .args(["rev-parse", arg])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| root.join(String::from_utf8_lossy(&o.stdout).trim()))
        })
        .collect()
}

pub fn run(
    root: &Path,
    db: &Path,
    interval: u64,
    timeout: u64,
    max_polls: Option<u64>,
    wake: impl Fn() -> Result<()>,
) -> Result<Value> {
    let Some(_owner) = scheduler::watcher_lock(db)? else {
        return Ok(json!({"watch":"already-running","database":db}));
    };
    sync::validate_database(root, db)?;
    scheduler::bind_root(root, db)?;
    let (tx, rx) = mpsc::sync_channel(256);
    // A full fallback check recovers events dropped when the bounded channel is full.
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.try_send(event);
    })
    .ok();
    let gitdirs = gitdirs(root);
    let stores: Vec<_> = Harness::all()
        .into_iter()
        .filter_map(|h| sync::sessions::directory(h).ok().map(|p| (h, p)))
        .collect();
    let mut watched = BTreeSet::new();
    let mut last_requested = None;
    let mut polls = 0;
    let mut next_poll = Instant::now();
    let mut hints = DirtySet::default();
    let mut due = None;
    loop {
        if !root.is_dir() {
            return Ok(json!({"watch":"stopped","reason":"repository_removed"}));
        }
        // Watch the nearest existing parent of missing session stores, then move
        // down as directories appear. No transcript activity needs a harness event.
        if let Some(w) = watcher.as_mut() {
            for (_, store) in &stores {
                let path = store
                    .ancestors()
                    .find(|p| p.is_dir())
                    .unwrap_or(root)
                    .to_path_buf();
                let recursive = path == *store;
                let key = (path.clone(), recursive);
                if !watched.contains(&key)
                    && w.watch(
                        &path,
                        if recursive {
                            RecursiveMode::Recursive
                        } else {
                            RecursiveMode::NonRecursive
                        },
                    )
                    .is_ok()
                {
                    watched.insert(key);
                }
            }
        }
        // Retry unavailable paths on fallback: stores may be created after startup.
        if Instant::now() >= next_poll {
            if let Some(w) = watcher.as_mut() {
                for path in std::iter::once(root.to_path_buf())
                    .chain(gitdirs.iter().cloned())
                    .chain(stores.iter().map(|(_, p)| p.clone()))
                {
                    if path.exists()
                        && !watched.contains(&(path.clone(), true))
                        && w.watch(&path, RecursiveMode::Recursive).is_ok()
                    {
                        watched.insert((path, true));
                    }
                }
                // Policy files can live outside the captured tree.
                if let Ok(bytes) = std::fs::read(root.join(".briefcase/sync.json")) {
                    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                        if let Some(p) = v["policy"].as_str() {
                            let path = root.join(p);
                            let parent = path.parent().unwrap_or(root).to_path_buf();
                            if !watched.contains(&(parent.clone(), false))
                                && w.watch(&parent, RecursiveMode::NonRecursive).is_ok()
                            {
                                watched.insert((parent, false));
                            }
                        }
                    }
                }
            }
            hints.union(&DirtySet::all());
            due = Some(Instant::now());
            polls += 1;
            next_poll = Instant::now() + Duration::from_secs(interval);
        }
        if due.is_some_and(|d| Instant::now() >= d) {
            if scheduler::enabled(root)? {
                let options =
                    sync::SyncOptions::local(root.into(), db.into(), Duration::from_secs(timeout));
                let configured =
                    reconcile::configured_domains(root).unwrap_or_else(|_| DirtySet::all());
                let alignments = reconcile::check(&options, &hints.intersection(&configured))?;
                let dirty = reconcile::dirty(&alignments);
                let signature = super::identity::digest(&json!(alignments))?;
                if !dirty.is_empty() && last_requested.as_ref() != Some(&signature) {
                    last_requested = Some(signature);
                    scheduler::request_domains(
                        root,
                        db,
                        "watch",
                        "local-state",
                        timeout,
                        &dirty,
                        &wake,
                    )?;
                } else {
                    if dirty.is_empty() {
                        last_requested = None;
                    }
                    let status = scheduler::status(db);
                    if status["pending"] == true
                        && status["worker_active"] == false
                        && status["blocked"] != true
                        && status["attempts"].as_u64().unwrap_or(10) < 10
                    {
                        wake()?;
                    }
                }
            }
            hints = DirtySet::default();
            due = None;
            if max_polls.is_some_and(|limit| polls >= limit) {
                return Ok(
                    json!({"watch":"stopped","polls":polls,"database":db,"events":watcher.is_some()}),
                );
            }
        }
        let wait = due
            .unwrap_or(next_poll)
            .min(next_poll)
            .saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(Ok(event)) => {
                let next = hint(&event, root, db, &gitdirs, &stores);
                if !next.is_empty() {
                    hints.union(&next);
                    due.get_or_insert(Instant::now() + Duration::from_millis(250));
                }
            }
            Ok(Err(_)) => {
                hints.union(&DirtySet::all());
                due.get_or_insert(Instant::now() + Duration::from_millis(250));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(wait),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_hints_ignore_runtime_writes_and_include_session_directory_creation() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        let db = root.join("custom.db");
        let git = root.join(".git");
        let sessions = d.path().join("codex/sessions");
        let mut paths = workspace::database_files(&db);
        paths.extend([
            root.join(".custom.db.sync.json.123.tmp"),
            root.join(".briefcase-scheduler/x/state.json"),
        ]);
        let event = Event::new(EventKind::Any).add_path(paths[0].clone());
        let mut event = event;
        event.paths = paths;
        assert!(hint(
            &event,
            &root,
            &db,
            std::slice::from_ref(&git),
            &[(Harness::Codex, sessions.clone())]
        )
        .is_empty());
        let event = Event::new(EventKind::Any).add_path(git.join("HEAD"));
        let h = hint(&event, &root, &db, std::slice::from_ref(&git), &[]);
        assert!(h.git && !h.project && h.sessions.is_empty());
        let event = Event::new(EventKind::Any).add_path(d.path().join("codex"));
        let h = hint(&event, &root, &db, &[git], &[(Harness::Codex, sessions)]);
        assert!(!h.git && !h.project && h.sessions.contains(&Harness::Codex));
    }
}
