//! Operator-selected toolchains and bounded, isolated adapter processes.
//! Only embedded analyzer source is compiled. Repository code is never built.
use super::{identity, require, workspace, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const STDOUT_LIMIT: u64 = 16 * 1024 * 1024;
const STDERR_LIMIT: u64 = 65536;

fn temporary_builder() -> tempfile::Builder<'static, 'static> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("briefcase-native-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
}
pub(crate) fn scratch() -> Result<tempfile::TempDir> {
    Ok(temporary_builder().tempdir()?)
}

pub(crate) fn executable(path: &Path, language: &str) -> Result<PathBuf> {
    let selected = if path.components().count() == 1 && !path.is_absolute() {
        std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .filter(|dir| dir.is_absolute())
                    .map(|dir| dir.join(path))
                    .find(|p| p.is_file())
            })
            .ok_or_else(|| {
                format!(
                    "ANALYZER: {language} toolchain not found: {}",
                    path.display()
                )
            })?
    } else if path.is_file() {
        path.to_owned()
    } else {
        return Err(format!(
            "ANALYZER: Missing {language} executable: {}",
            path.display()
        )
        .into());
    };
    Ok(selected.canonicalize()?)
}

pub(crate) fn command(executable: &Path, dir: &Path) -> Command {
    let mut cmd = Command::new(executable);
    // Environment options can load agents, plugins, alternate toolchains or
    // arbitrary startup libraries. No repository or user startup settings pass.
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("TMPDIR", dir)
        .env("LANG", "C.UTF-8")
        .current_dir(dir);
    cmd
}

struct Process {
    child: std::process::Child,
    reaped: bool,
}
impl Drop for Process {
    fn drop(&mut self) {
        // Compilers may have subprocesses. Kill their isolated process group,
        // not just the parent, on failure or if children survive its exit.
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            // SAFETY: the child was spawned into a new process group whose id
            // is its positive pid. Signal 9 is SIGKILL on supported Unix hosts.
            unsafe { kill(-(self.child.id() as i32), 9) };
        }
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(crate) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn read_output(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    require(
        bytes.len() as u64 <= limit,
        "ANALYZER",
        "Native analyzer output exceeded byte limits",
    )?;
    Ok(bytes)
}

/// Process-runner seam for every language adapter. Adapters build a
/// `Command` and call [`run`]; tests can substitute a fake runner to verify
/// timeout, failure and quota handling without starting a real toolchain
/// or sleeping for a full timeout window.
pub(crate) trait ProcessRunner {
    fn run(&self, cmd: Command, input: &[u8], deadline: Instant) -> Result<Output>;
}

/// Runner backed by a real, process-group-isolated child process.
pub(crate) struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, cmd: Command, input: &[u8], deadline: Instant) -> Result<Output> {
        run_system(cmd, input, deadline, STDOUT_LIMIT)
    }
}

pub(crate) fn run(cmd: Command, input: &[u8], deadline: Instant) -> Result<Output> {
    SystemProcessRunner.run(cmd, input, deadline)
}

/// For an adapter whose own row bounds imply more output than the default
/// limit; the limit is still explicit and enforced while the process runs.
pub(crate) fn run_with_stdout_limit(
    cmd: Command,
    input: &[u8],
    deadline: Instant,
    stdout_limit: u64,
) -> Result<Output> {
    run_system(cmd, input, deadline, stdout_limit.max(STDOUT_LIMIT))
}

/// Preserve timeout identity across the boxed error boundary.
fn check_deadline(deadline: Instant, message: &str) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("ANALYZER: {message}"),
        )
        .into());
    }
    Ok(())
}

fn run_system(mut cmd: Command, input: &[u8], deadline: Instant, stdout_limit: u64) -> Result<Output> {
    // All callers, including language-specific launchers, get the same
    // process-group cleanup without changing their environment policy.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let dir = scratch()?;
    let stdin = dir.path().join("stdin");
    let stdout = dir.path().join("stdout");
    let stderr = dir.path().join("stderr");
    std::fs::write(&stdin, input)?;
    check_deadline(deadline, "Native analyzer timed out")?;
    let mut process = Process {
        child: cmd
            .stdin(Stdio::from(std::fs::File::open(stdin)?))
            .stdout(Stdio::from(std::fs::File::create(&stdout)?))
            .stderr(Stdio::from(std::fs::File::create(&stderr)?))
            .spawn()?,
        reaped: false,
    };
    let status = loop {
        require(
            std::fs::metadata(&stdout)?.len() <= stdout_limit
                && std::fs::metadata(&stderr)?.len() <= STDERR_LIMIT,
            "ANALYZER",
            "Native analyzer output exceeded byte limits",
        )?;
        if let Some(status) = process.child.try_wait()? {
            process.reaped = true;
            break status;
        }
        check_deadline(deadline, "Native analyzer timed out")?;
        std::thread::sleep(Duration::from_millis(10));
    };
    // Stop surviving subprocesses before checking and reading final output.
    drop(process);
    require(
        std::fs::metadata(&stdout)?.len() <= stdout_limit
            && std::fs::metadata(&stderr)?.len() <= STDERR_LIMIT,
        "ANALYZER",
        "Native analyzer output exceeded byte limits",
    )?;
    let output = Output {
        stdout: read_output(&stdout, stdout_limit)?,
        stderr: read_output(&stderr, STDERR_LIMIT)?,
    };
    require(
        status.success(),
        "ANALYZER",
        &format!(
            "Native analyzer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    Ok(output)
}

pub(crate) fn deadline(timeout: Duration) -> Result<Instant> {
    require(
        (Duration::from_secs(1)..=Duration::from_secs(600)).contains(&timeout),
        "ANALYZER",
        "Native analyzer timeout must be 1..600 seconds",
    )?;
    Ok(Instant::now() + timeout)
}

fn file_digest(path: &Path, deadline: Instant) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        check_deadline(deadline, "Compiler fingerprint timed out")?;
        let count = file.read(&mut buf)?;
        if count == 0 {
            break;
        }
        hasher.update(&buf[..count]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Pin the trusted compiler and every selected compiler/library input by bytes.
/// Symlink directories are never followed. Toolchain selection is not a sandbox.
pub(crate) fn fingerprint(root: &Path, inputs: &[PathBuf], deadline: Instant) -> Result<String> {
    fingerprint_inputs(root, inputs, deadline, false)
}
/// LLVM development packages can install header aliases outside their prefix.
/// Resolve aliases only in this operator-trusted toolchain, never source inputs.
pub(crate) fn fingerprint_aliases(
    root: &Path,
    inputs: &[PathBuf],
    deadline: Instant,
) -> Result<String> {
    fingerprint_inputs(root, inputs, deadline, true)
}
fn fingerprint_inputs(
    root: &Path,
    inputs: &[PathBuf],
    deadline: Instant,
    aliases: bool,
) -> Result<String> {
    let mut pending = inputs.to_vec();
    let mut files = BTreeMap::new();
    let mut directories = std::collections::BTreeSet::new();
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        check_deadline(deadline, "Compiler fingerprint timed out")?;
        let metadata = std::fs::symlink_metadata(&path)?;
        require(
            aliases || !metadata.file_type().is_symlink(),
            "ANALYZER",
            "Symlink in compiler fingerprint inputs",
        )?;
        let metadata = if metadata.file_type().is_symlink() {
            std::fs::metadata(&path)?
        } else {
            metadata
        };
        if metadata.is_dir() {
            require(
                directories.len() < 16384 && directories.insert(path.canonicalize()?),
                "ANALYZER",
                "Compiler directory quota/cycle exceeded",
            )?;
            for item in std::fs::read_dir(path)? {
                require(
                    pending.len() + files.len() <= 32768,
                    "ANALYZER",
                    "Compiler traversal quota exceeded",
                )?;
                pending.push(item?.path());
            }
        } else if metadata.is_file() {
            bytes += metadata.len();
            require(
                files.len() < 16384 && bytes <= 1024 * 1024 * 1024,
                "ANALYZER",
                "Compiler identity byte/file quota exceeded",
            )?;
            files.insert(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
                file_digest(&path, deadline)?,
            );
        } else {
            return Err("ANALYZER: Non-regular compiler fingerprint input".into());
        }
    }
    identity::digest(&json!(files))
}

pub(crate) struct Compiled {
    pub directory: PathBuf,
    pub digest: String,
}
fn compiled_digest(directory: &Path, deadline: Instant) -> Result<String> {
    fingerprint(directory, &[directory.join("output")], deadline)
}

/// A private host cache contains only builds of the embedded adapter. The key
/// pins compiler bytes, adapter bytes and launcher implementation. No cache
/// timestamp or temporary path enters canonical source identities.
pub(crate) fn compiled(
    language: &str,
    toolchain: &Value,
    adapter: &[u8],
    deadline: Instant,
    build: impl FnOnce(&Path, &Path) -> Result<()>,
) -> Result<Compiled> {
    let base = std::env::var_os("BRIEFCASE_ANALYZER_CACHE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CACHE_HOME")
                .map(|p| PathBuf::from(p).join("briefcase/analyzers"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache/briefcase/analyzers"))
        })
        .ok_or(
            "ANALYZER: Set BRIEFCASE_ANALYZER_CACHE when no host cache directory is available",
        )?;
    workspace::private_dir(&base)?;
    let key = identity::digest(&json!({"language":language,"toolchain":toolchain,
        "adapter":identity::bytes_digest(adapter),"launcher":identity::bytes_digest(include_bytes!("native_host.rs"))}))?;
    let key = key.trim_start_matches("sha256:");
    let directory = base.join(format!("{language}-{key}"));
    let lock_path = base.join(format!("{language}-{key}.lock"));
    let lock = workspace::private_file(&lock_path)
        .or_else(|_| std::fs::OpenOptions::new().write(true).open(&lock_path))?;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                check_deadline(deadline, "Native compiler cache lock timed out")?;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
    let manifest = std::fs::read(directory.join("manifest.json"))
        .ok()
        .and_then(|bytes| identity::parse(&bytes).ok());
    if let Some(manifest) = manifest {
        if let Ok(digest) = compiled_digest(&directory, deadline) {
            if manifest["digest"] == digest && manifest["key"] == key {
                return Ok(Compiled {
                    directory: directory.join("output"),
                    digest,
                });
            }
        }
    }
    let temp = temporary_builder().tempdir_in(&base)?;
    let output = temp.path().join("output");
    std::fs::create_dir(&output)?;
    // Build scratch (including Go's compiler cache) is disposable. Only
    // compiled adapter bytes and their manifest belong in the host cache.
    let build_dir = scratch()?;
    build(build_dir.path(), &output)?;
    check_deadline(deadline, "Native compiler timed out")?;
    let digest = compiled_digest(temp.path(), deadline)?;
    std::fs::write(
        temp.path().join("manifest.json"),
        serde_json::to_vec(&json!({"key":key,"digest":digest}))?,
    )?;
    if directory.exists() {
        std::fs::remove_dir_all(&directory)?;
    }
    std::fs::rename(temp.path(), &directory)?;
    Ok(Compiled {
        directory: directory.join("output"),
        digest,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn runner_preserves_input_stdout_and_stderr() {
        let output = run(
            shell("cat; printf diagnostic >&2"),
            b"retained input",
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.stdout, b"retained input");
        assert_eq!(output.stderr, b"diagnostic");
    }

    #[test]
    fn nonzero_exit_and_non_utf8_stderr_are_reported() {
        let error = run(
            shell("printf '\\377' >&2; exit 7"),
            &[],
            Instant::now() + Duration::from_secs(5),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(
            error.contains("ANALYZER: Native analyzer failed:"),
            "{error}"
        );
        assert!(error.contains('\u{fffd}'));
    }

    #[test]
    fn output_quotas_include_bytes_written_immediately_before_exit() {
        for script in [
            "dd if=/dev/zero bs=1048576 count=17 2>/dev/null",
            "dd if=/dev/zero bs=65537 count=1 1>&2 2>/dev/null",
        ] {
            let error = run(shell(script), &[], Instant::now() + Duration::from_secs(5))
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("output exceeded byte limits"), "{error}");
        }
    }

    struct FakeRunner;

    impl ProcessRunner for FakeRunner {
        fn run(&self, _cmd: Command, _input: &[u8], _deadline: Instant) -> Result<Output> {
            Err("ANALYZER: Native analyzer timed out".into())
        }
    }

    #[test]
    fn fake_runner_reports_timeout_without_spawning_or_sleeping() {
        let started = Instant::now();
        let error = FakeRunner
            .run(
                shell("exec sleep 5"),
                &[],
                started + Duration::from_secs(60),
            )
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn sleeper_is_stopped_at_a_short_deadline() {
        let started = Instant::now();
        let error = SystemProcessRunner
            .run(
                shell("exec sleep 5"),
                &[],
                started + Duration::from_millis(100),
            )
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn command_strips_caller_environment() {
        std::env::set_var("BRIEFCASE_TEST_SECRET", "must-not-leak");
        let dir = scratch().unwrap();
        let output = SystemProcessRunner
            .run(
                command(Path::new("/bin/sh"), dir.path()),
                b"printf '%s' \"$BRIEFCASE_TEST_SECRET\"; printf '|%s' \"$PATH\"",
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(output.stdout, b"|/usr/bin:/bin");
    }

    #[test]
    fn deadline_bounds_are_checked_without_sleeping() {
        assert!(deadline(Duration::ZERO).is_err());
        assert!(deadline(Duration::from_secs(601)).is_err());
        assert!(deadline(Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn timeout_and_parent_exit_stop_surviving_children() {
        let dir = scratch().unwrap();
        for (i, exit) in ["wait", "exit 0", "exit 7"].into_iter().enumerate() {
            let marker = dir.path().join(format!("survived-{i}"));
            let script = format!("(sleep 0.3; printf survived > \"$1\") & {exit}");
            let mut command = Command::new("/bin/sh");
            command.args(["-c", &script, "sh"]).arg(&marker);
            let started = Instant::now();
            let output = run(command, &[], started + Duration::from_millis(100));
            match exit {
                "wait" => assert!(output.err().unwrap().to_string().contains("timed out")),
                "exit 0" => assert!(output.is_ok()),
                _ => assert!(output.err().unwrap().to_string().contains("failed")),
            }
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(400));
            assert!(!marker.exists(), "child survived {exit}");
        }
    }
}
