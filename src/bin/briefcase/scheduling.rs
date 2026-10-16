//! Process boundary for background work; hooks never hold the source writer lock.
use super::*;
use std::{
    io::Read,
    path::Path,
    process::{Command as ProcessCommand, Stdio},
};

fn base(root: &Path, db: &Path) -> Result<ProcessCommand> {
    let mut cmd = ProcessCommand::new(std::env::current_exe()?);
    cmd.current_dir(root)
        .args(["--project-root"])
        .arg(root)
        .arg("--db")
        .arg(db);
    Ok(cmd)
}

pub(crate) fn wake(root: &Path, db: &Path) -> Result<()> {
    let mut cmd = base(root, db)?;
    cmd.arg("sync-worker");
    let mut child = detach(cmd)?;
    // Watchers are long-lived parents; reap completed workers instead of accumulating zombies.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub(crate) fn run_sync(
    root: &Path,
    db: &Path,
    timeout: u64,
    domains: &briefcase::architecture::reconcile::DirtySet,
) -> Result<Value> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut cmd = base(root, db)?;
    cmd.args([
        "--json",
        "sync",
        "--timeout",
        &timeout.to_string(),
        "--lock-wait",
        "0",
    ]);
    if domains.project {
        cmd.arg("--project");
    }
    if domains.git {
        cmd.arg("--git");
    }
    // Repeated flags represent the scheduler's union of dirty session domains.
    for harness in &domains.sessions {
        cmd.arg("--sessions").arg(harness.name());
    }
    cmd.stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(timeout + 15);
    let exit = loop {
        if let Some(exit) = child.try_wait()? {
            break exit;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(
                json!({"sync":"timeout","ready":false,"message":"Scheduler outer deadline expired"}),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    use std::io::{Seek, SeekFrom};
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    stdout.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() <= 65536 {
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            return Ok(value);
        }
    }
    let mut error = String::new();
    stderr.take(4096).read_to_string(&mut error)?;
    Ok(json!({"sync":"failed","ready":false,"error":format!("sync exited {exit}: {error}")}))
}

fn detach(mut cmd: ProcessCommand) -> Result<std::process::Child> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and accesses no Rust runtime state.
        unsafe {
            cmd.pre_exec(|| {
                unsafe extern "C" {
                    fn setsid() -> i32;
                }
                if setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000008 | 0x00000200);
    }
    Ok(cmd.spawn()?)
}
/// Serialize launchers separately from watcher ownership. Wait for child admission
/// before releasing bootstrap admission, so simultaneous harness starts spawn one owner.
pub(crate) fn ensure_monitor(root: &Path, db: &Path, timeout: u64) -> Result<Value> {
    use briefcase::architecture::scheduler;
    if !scheduler::enabled(root)? {
        return Ok(json!({"monitor":"disabled"}));
    }
    let db = scheduler::database(db)?;
    let _bootstrap = scheduler::bootstrap_lock(&db)?;
    sync::validate_database(root, &db)?;
    scheduler::bind_root(root, &db)?;
    if scheduler::status(&db)["watcher_active"] == true {
        return Ok(json!({"monitor":"already-running","database":db}));
    }
    let mut cmd = base(root, &db)?;
    cmd.args(["watch", "--timeout", &timeout.to_string()]);
    let mut child = detach(cmd)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(exit) = child.try_wait()? {
            return Err(format!("MONITOR: Watcher exited {exit}").into());
        }
        if scheduler::status(&db)["watcher_active"] == true {
            return Ok(json!({"monitor":"started","database":db,"pid":child.id()}));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("MONITOR: Startup timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
