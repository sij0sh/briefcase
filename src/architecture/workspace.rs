//! One resolver for every adapter. The CLI, the Pi extension and the MCP
//! server select the same repository root, database and writer admission.
use super::{require, Result};
use std::{
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const DB_ENV: &str = "BRIEFCASE_DB";
pub const DEFAULT_DB: &str = ".briefcase/briefcase.db";

/// The nearest ancestor holding `.git`, as a directory or as the file that
/// linked worktrees use. Distinct worktrees therefore stay distinct.
pub fn git_root(start: &Path) -> Result<Option<PathBuf>> {
    let start = start.canonicalize()?;
    Ok(start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf))
}

/// An explicit project root wins; otherwise the Git/worktree root. Briefcase
/// does not guess which project owns a directory outside version control.
pub fn repository_root(cwd: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = explicit {
        let root = absolute(cwd, root).canonicalize()?;
        require(
            root.is_dir(),
            "NEEDS_CONFIGURATION",
            "Project root must be a directory",
        )?;
        return Ok(root);
    }
    git_root(cwd)?.ok_or_else(|| {
        "NEEDS_CONFIGURATION: No Git repository contains this directory; pass --project-root, --db or BRIEFCASE_DB".into()
    })
}

/// Selection order: `--db`, then `BRIEFCASE_DB`, then
/// `<repository-root>/.briefcase/briefcase.db`. Relative overrides resolve once,
/// against the invocation directory, and the result is always absolute.
pub fn resolve_db(
    explicit: Option<&Path>,
    env: Option<OsString>,
    cwd: &Path,
    project_root: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(absolute(cwd, path));
    }
    if let Some(path) = env.filter(|p| !p.is_empty()) {
        return Ok(absolute(cwd, Path::new(&path)));
    }
    Ok(repository_root(cwd, project_root)?.join(DEFAULT_DB))
}

fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Create a state directory that only its owner can read when Briefcase creates it.
/// Existing directories keep their permissions; changing shared state is the owner's call.
pub fn private_dir(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)?;
    Ok(())
}

/// Create a new file with owner-only permissions. Fails if it exists.
pub fn private_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    Ok(options.open(path)?)
}

/// Write bytes by replacing the destination atomically. New files are private.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().ok_or("Path has no parent directory")?;
    private_dir(dir)?;
    let name = path
        .file_name()
        .ok_or("Path has no file name")?
        .to_string_lossy();
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let mut file = private_file(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// Process-shared writer admission for one database. The operating system
/// releases the lock when its holder exits, so a crashed writer never strands it.
pub struct WriterLock {
    _file: File,
}
pub fn lock_path(db: &Path) -> PathBuf {
    sibling(db, "lock")
}
/// `briefcase.db` → `briefcase.db.<suffix>`, beside the database.
pub fn sibling(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    db.with_file_name(name)
}
/// Runtime files are excluded even when an operator selects a DB inside captured directories.
pub(crate) fn database_files(db: &Path) -> Vec<PathBuf> {
    let resolved = db
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.join(db.file_name().unwrap_or_default()))
        .unwrap_or_else(|| db.to_owned());
    let mut paths = vec![
        resolved.clone(),
        sibling(&resolved, "sync.json"),
        sibling(&resolved, "lock"),
    ];
    for suffix in ["-wal", "-shm"] {
        let mut name = resolved.as_os_str().to_os_string();
        name.push(suffix);
        paths.push(name.into());
    }
    paths
}
fn open_lock(db: &Path) -> Result<File> {
    let path = lock_path(db);
    match private_file(&path) {
        Ok(file) => Ok(file),
        Err(_) => Ok(std::fs::OpenOptions::new().write(true).open(path)?),
    }
}
/// Wait for writer admission.
pub fn writer_lock(db: &Path) -> Result<WriterLock> {
    let file = open_lock(db)?;
    file.lock()?;
    Ok(WriterLock { _file: file })
}
/// Try for writer admission until `wait` elapses; `None` means another writer holds it.
pub fn try_writer_lock(db: &Path, wait: Duration) -> Result<Option<WriterLock>> {
    let file = open_lock(db)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(WriterLock { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < wait => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// Whether another process holds writer admission. Probing never creates
/// files: writers create the lock file, so its absence means none is active.
pub fn writer_active(db: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(lock_path(db)) else {
        return false;
    };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_subdirectory_and_symlink_select_one_database() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        let root = resolve_db(None, None, &repo, None).unwrap();
        let sub = resolve_db(None, None, &repo.join("src/deep"), None).unwrap();
        assert_eq!(root, sub);
        assert_eq!(root, repo.canonicalize().unwrap().join(DEFAULT_DB));
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&repo, &link).unwrap();
            assert_eq!(
                resolve_db(None, None, &link.join("src"), None).unwrap(),
                root
            );
        }
    }

    #[test]
    fn worktrees_stay_separate_and_overrides_win_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(main.join(".git")).unwrap();
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join(".git"), "gitdir: ../main/.git/worktrees/tree\n").unwrap();
        let a = resolve_db(None, None, &main, None).unwrap();
        let b = resolve_db(None, None, &tree, None).unwrap();
        assert_ne!(a, b);
        let env = Some(OsString::from("state/x.db"));
        assert_eq!(
            resolve_db(None, env.clone(), &tree, None).unwrap(),
            tree.join("state/x.db")
        );
        assert_eq!(
            resolve_db(Some(Path::new("/abs/y.db")), env, &tree, None).unwrap(),
            PathBuf::from("/abs/y.db")
        );
        let outside = tempfile::tempdir().unwrap();
        let error = resolve_db(None, None, outside.path(), None).unwrap_err();
        assert!(error.to_string().starts_with("NEEDS_CONFIGURATION"));
        assert_eq!(
            resolve_db(None, None, outside.path(), Some(outside.path())).unwrap(),
            outside.path().canonicalize().unwrap().join(DEFAULT_DB)
        );
    }

    #[test]
    fn writer_admission_is_exclusive_across_handles() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("briefcase.db");
        assert!(!writer_active(&db) && !lock_path(&db).exists());
        let held = writer_lock(&db).unwrap();
        assert!(writer_active(&db));
        assert!(try_writer_lock(&db, Duration::ZERO).unwrap().is_none());
        drop(held);
        // A sibling test's fork can share the descriptor until its exec closes it.
        assert!(try_writer_lock(&db, Duration::from_secs(5))
            .unwrap()
            .is_some());
    }
}
