//! Database selection, ownership, migration, bounded sync, shared read
//! operations and retained-source recall through the shipped CLI.
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use briefcase::architecture::{ledger::Ledger, mcp, service, workspace};

const TOKEN: &str = "zanzibarquux";

/// The vendored compiler pin: `npm ci --ignore-scripts --prefix tools/typescript`.
fn typescript_compiler() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tools/typescript/node_modules/typescript/lib/typescript.js")
}

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    empty_path: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let empty_path = dir.path().join("empty-path");
        std::fs::create_dir_all(&root).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::create_dir_all(&empty_path).unwrap();
        // Git stays available; rust-analyzer does not.
        #[cfg(unix)]
        if let Some(git) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|d| d.join("git"))
            .find(|p| p.is_file())
        {
            std::os::unix::fs::symlink(git, empty_path.join("git")).unwrap();
        }
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "mod store;\npub fn entry() { store::save(); }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/store.rs"), "pub fn save() {}\n").unwrap();
        std::fs::write(
            root.join("README.md"),
            format!("# Demo\n\nThe {TOKEN} credential rotates nightly.\n"),
        )
        .unwrap();
        Self {
            root: root.canonicalize().unwrap(),
            _dir: dir,
            empty_path,
        }
    }
    fn db(&self) -> PathBuf {
        self.root.join(workspace::DEFAULT_DB)
    }
    /// Run from `cwd` without BRIEFCASE_DB and without rust-analyzer on PATH, so
    /// the automatic Rust profile is the deterministic syntax-only capture.
    fn run(&self, cwd: &Path, args: &[&str]) -> (bool, Value, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_briefcase"))
            .current_dir(cwd)
            .env_remove("BRIEFCASE_DB")
            .env_remove("BRIEFCASE_SYNC")
            .env("PI_CODING_AGENT_DIR", self._dir.path().join("pi"))
            .env("CODEX_HOME", self._dir.path().join("codex"))
            .env("CLAUDE_CONFIG_DIR", self._dir.path().join("claude"))
            .env("MUSE_HOME", self._dir.path().join("muse"))
            .env("PATH", &self.empty_path)
            .env("BRIEFCASE_TYPESCRIPT_COMPILER", typescript_compiler())
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        (
            output.status.success(),
            serde_json::from_str(&stdout).unwrap_or(Value::Null),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
    fn ok(&self, args: &[&str]) -> Value {
        let (ok, value, stderr) = self.run(&self.root, args);
        assert!(ok, "{args:?}: stderr={stderr}; result={value}");
        value
    }
    fn sync(&self) -> Value {
        self.ok(&["sync", "--timeout", "60"])
    }
    fn scans(&self) -> usize {
        self.ok(&["status"])["scans"].as_array().unwrap().len()
    }
    fn typescript() -> Self {
        let repo = Self::new();
        std::fs::remove_file(repo.root.join("Cargo.toml")).unwrap();
        std::fs::write(repo.root.join("package.json"), "{}").unwrap();
        std::fs::write(
            repo.root.join("tsconfig.base.json"),
            r#"{"compilerOptions":{"strict":true}}"#,
        )
        .unwrap();
        for app in ["server", "dashboard"] {
            std::fs::create_dir_all(repo.root.join(format!("apps/{app}/src"))).unwrap();
            std::fs::write(repo.root.join(format!("apps/{app}/tsconfig.json")),
                "{ // retained JSONC and extends\n\"extends\":\"../../tsconfig.base.json\",\"include\":[\"src/**/*.ts\"],\n}").unwrap();
            std::fs::write(
                repo.root.join(format!("apps/{app}/src/lib.ts")),
                "export function entry() {}\n",
            )
            .unwrap();
        }
        repo
    }
    fn ts_sync(&self, args: &[&str]) -> (bool, Value, String) {
        let wrapper = self._dir.path().join("node-wrapper");
        let node = if wrapper.is_file() {
            wrapper
        } else {
            Self::node()
        };
        let mut argv = vec!["sync", "--require-ready", "--node", node.to_str().unwrap()];
        argv.extend_from_slice(args);
        self.run(&self.root, &argv)
    }
    fn node() -> PathBuf {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join(format!("node{}", std::env::consts::EXE_SUFFIX)))
            .find(|p| p.is_file())
            .expect("TypeScript regression tests require Node")
    }
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn cli_branding_and_storage_names_are_stable() {
    let binary = env!("CARGO_BIN_EXE_briefcase");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("Usage: briefcase "));
    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("briefcase {}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(workspace::DEFAULT_DB, ".briefcase/briefcase.db");
    assert_eq!(workspace::DB_ENV, "BRIEFCASE_DB");
}

#[test]
fn root_and_subdirectory_share_one_private_database_and_status_creates_nothing() {
    let repo = Repo::new();
    let status = repo.ok(&["status"]);
    assert_eq!(status["readiness"]["state"], "not_initialized");
    assert_eq!(status["readiness"]["database"], json!(repo.db()));
    assert!(
        !repo.root.join(".briefcase").exists(),
        "status must not create state"
    );

    let (ok, first, stderr) = repo.run(&repo.root.join("src/nested"), &["sync"]);
    assert!(ok, "{stderr}");
    assert_eq!(first["sync"], "refreshed");
    assert_eq!(first["database"], json!(repo.db()));
    assert_eq!(first["profile"]["semantic"], false);
    let again = repo.sync();
    assert_eq!(again["sync"], "up-to-date");
    assert_eq!(again["database"], first["database"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(repo.db()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "new database must be private");
    }
    let status = repo.ok(&["status"]);
    assert_eq!(status["readiness"]["state"], "ready");
    assert_eq!(status["readiness"]["last_sync"]["sync"], "up-to-date");

    // Explicit --db wins and is resolved against the invocation directory.
    let (ok, other, _) = repo.run(&repo.root.join("src"), &["--db", "x.db", "status"]);
    assert!(ok);
    assert_eq!(
        other["readiness"]["database"],
        json!(repo.root.join("src/x.db"))
    );
    assert!(!repo.root.join("src/x.db").exists());
}

#[test]
fn foreign_and_snoop_databases_are_rejected_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::new();
    for (name, setup) in [
        (
            "foreign-v1.db",
            "PRAGMA user_version=1; CREATE TABLE notes(body TEXT);",
        ),
        (
            "snoop.db",
            "PRAGMA user_version=4; CREATE TABLE chunks(id INTEGER);",
        ),
        ("other-app.db", "PRAGMA application_id=42;"),
    ] {
        let path = dir.path().join(name);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(setup)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let path_text = path.to_str().unwrap();
        for args in [
            vec!["--db", path_text, "scan", "."],
            vec!["--db", path_text, "sync"],
            vec!["--db", path_text, "call", "status"],
        ] {
            let (ok, _, stderr) = repo.run(&repo.root, &args);
            assert!(!ok, "{name} {args:?} was accepted");
            let (_, value, _) = repo.run(&repo.root, &args);
            assert!(
                stderr.contains("FOREIGN_DATABASE")
                    || value["error"]
                        .as_str()
                        .is_some_and(|e| e.contains("FOREIGN_DATABASE")),
                "{stderr} {value}"
            );
        }
        assert!(Ledger::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before, "{name} changed");
    }
}

#[test]
fn sync_is_idempotent_rebuilds_projections_and_never_undoes_retractions() {
    let repo = Repo::new();
    assert_eq!(repo.sync()["sync"], "refreshed");
    assert_eq!(repo.scans(), 1);
    for _ in 0..2 {
        assert_eq!(repo.sync()["sync"], "up-to-date");
    }
    assert_eq!(repo.scans(), 1, "a no-op sync allocated a generation");

    // An equal-size edit changes the digest even without a size change.
    std::fs::write(repo.root.join("src/store.rs"), "pub fn load() {}\n").unwrap();
    assert_eq!(repo.sync()["sync"], "refreshed");
    assert_eq!(repo.scans(), 2);

    // A published current view still receives full integrity verification,
    // even though sync no longer audits every historical publication.
    let connection = rusqlite::Connection::open(repo.db()).unwrap();
    connection
        .execute_batch("DELETE FROM graph; UPDATE retrieval_fts SET evidence_text='damaged';")
        .unwrap();
    assert_eq!(repo.sync()["sync"], "rebuilt");
    assert_eq!(repo.sync()["sync"], "up-to-date");

    // A retraction makes projections stale; sync rebuilds them and the
    // retracted assertion stays retracted.
    let deps = repo.ok(&["--json","call","neighbors","--args",
        "{\"target\":\".\",\"predicate\":\"DECLARES\"}"]);
    let assertion=deps["items"][0]["assertion_ids"][0].as_str().expect("a structural assertion to retract").to_owned();
    repo.ok(&["--json", "retract", &assertion, "--reason", "test"]);
    assert_eq!(repo.sync()["sync"], "rebuilt");
    assert_eq!(repo.ok(&["status"])["retracted_count"], 1);
    assert_eq!(repo.sync()["sync"], "up-to-date");
    assert_eq!(repo.ok(&["status"])["retracted_count"], 1);

    // Mixed roots need explicit configuration rather than a guess.
    std::fs::write(repo.root.join("package.json"), "{}").unwrap();
    let mixed = repo.run(&repo.root, &["sync", "--require-ready"]);
    assert!(!mixed.0);
    assert_eq!(mixed.1["sync"], "needs_configuration");
    std::fs::write(
        repo.root.join(".briefcase/sync.json"),
        "{\"language\":\"rust\"}",
    )
    .unwrap();
    assert_eq!(repo.sync()["sync"], "refreshed");
}

#[test]
fn scheduled_cli_waits_for_its_revision_and_does_not_capture_its_own_state() {
    let repo = Repo::new();
    let queued = repo.ok(&[
        "sync-request",
        "--source",
        "openrouter",
        "--reason",
        "turn-end",
        "--timeout",
        "10",
        "--wait",
    ]);
    assert_eq!(queued["ready"], true, "{queued}");
    let first = queued["view_id"].clone();
    let again = repo.ok(&["sync-request", "--timeout", "10", "--wait"]);
    assert_eq!(again["sync"], "up-to-date");
    assert_eq!(again["view_id"], first);
    let status = repo.ok(&["status"]);
    assert_eq!(status["readiness"]["scheduler"]["pending"], false);
    // Runtime files outside the default state directory must not cause a capture loop.
    let args = [
        "--db",
        "custom.db",
        "sync",
        "--timeout",
        "10",
        "--require-ready",
    ];
    let custom = repo.ok(&args);
    let custom_again = repo.ok(&args);
    assert_eq!(custom_again["sync"], "up-to-date");
    assert_eq!(custom_again["view_id"], custom["view_id"]);
}

#[test]
fn hooks_use_event_cwd_keep_stdout_empty_and_watchers_do_not_self_trigger() {
    use std::io::Write;
    let repo = Repo::new();
    for agent in ["codex", "claude-code"] {
        let report = repo.ok(&["install", "--agent", agent]);
        assert_eq!(report["status"], "wired");
        assert_eq!(
            repo.ok(&["install", "--agent", agent])["status"],
            "already-configured"
        );
    }
    // Launch elsewhere; the event cwd must select this checkout.
    let mut child = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(repo.empty_path.parent().unwrap())
        .env_remove("BRIEFCASE_DB")
        .env_remove("BRIEFCASE_SYNC")
        .env("PATH", &repo.empty_path)
        .args([
            "sync-request",
            "--source",
            "codex",
            "--hook",
            "--quiet",
            "--wait",
            "--timeout",
            "10",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({"cwd":repo.root.join("src/nested")})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(repo.db().exists());
    let watched = repo.ok(&[
        "watch",
        "--max-polls",
        "2",
        "--interval",
        "1",
        "--timeout",
        "10",
    ]);
    assert_eq!(watched["watch"], "stopped");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let s = briefcase::architecture::scheduler::status(&repo.db());
        if s["pending"] == false && s["worker_active"] == false {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{s}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = briefcase::architecture::scheduler::status(&repo.db());
    assert_eq!(
        s["requested_revision"], 1,
        "database/status writes must not enqueue another revision"
    );
    std::fs::write(
        repo.root.join(".briefcase/schedule.json"),
        r#"{"enabled":false}"#,
    )
    .unwrap();
    assert_eq!(repo.ok(&["sync-request"])["schedule"], "disabled");
    assert_eq!(repo.sync()["ready"], true, "manual sync remains available");
}

#[test]
fn watcher_captures_external_edits_without_a_harness_or_manual_sync() {
    let repo = Repo::new();
    let mut watcher = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .current_dir(&repo.root)
        .env_remove("BRIEFCASE_DB")
        .env_remove("BRIEFCASE_SYNC")
        .env("PATH", &repo.empty_path)
        .args([
            "watch",
            "--interval",
            "1",
            "--max-polls",
            "5",
            "--timeout",
            "10",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while briefcase::architecture::scheduler::status(&repo.db())["requested_revision"]
        .as_u64()
        .unwrap_or(0)
        == 0
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    // Simulate an editor outside any coding session, including an equal-size code edit.
    std::fs::write(repo.root.join("src/store.rs"), "pub fn load() {}\n").unwrap();
    std::fs::rename(repo.root.join("README.md"), repo.root.join("GUIDE.md")).unwrap();
    let exit = loop {
        if let Some(exit) = watcher.try_wait().unwrap() {
            break exit;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(exit.success());
    loop {
        let s = briefcase::architecture::scheduler::status(&repo.db());
        if s["pending"] == false && s["worker_active"] == false {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{s}");
        std::thread::sleep(Duration::from_millis(25));
    }
    let found = repo.ok(&["--json", "recall", TOKEN]);
    assert!(found.to_string().contains("GUIDE.md"), "{found}");
    assert!(!found.to_string().contains("README.md"), "{found}");
}

#[test]
fn typescript_sync_requires_explicit_retained_configuration_without_generations() {
    let repo = Repo::typescript();
    for _ in 0..2 {
        let (ok, result, stderr) = repo.ts_sync(&[]);
        assert!(!ok, "{stderr}");
        assert_eq!(result["sync"], "needs_configuration", "{result}");
        assert_eq!(result["ready"], false);
        assert_eq!(result["configuration"]["project"], "tsconfig.json");
        assert_eq!(result["configuration"]["reason"], "missing_project");
        assert_eq!(
            result["configuration"]["candidates"],
            json!([
                "apps/dashboard/tsconfig.json",
                "apps/server/tsconfig.json",
                "tsconfig.base.json"
            ])
        );
        assert!(result["message"]
            .as_str()
            .unwrap()
            .contains("--require-ready"));
        assert!(
            !repo.db().exists(),
            "missing config must not initialize the ledger"
        );
        assert_eq!(
            repo.ok(&["status"])["readiness"]["last_sync"]["sync"],
            "needs_configuration"
        );
    }
    // A single nested candidate is still an explicit coverage decision.
    std::fs::remove_file(repo.root.join("apps/dashboard/tsconfig.json")).unwrap();
    std::fs::remove_file(repo.root.join("tsconfig.base.json")).unwrap();
    assert_eq!(repo.ts_sync(&[]).1["sync"], "needs_configuration");
    assert!(!repo.db().exists());

    for path in ["../tsconfig.json", ".", ""] {
        let result = repo.ts_sync(&["--project", path]).1;
        assert_eq!(
            result["configuration"]["reason"], "invalid_project",
            "{result}"
        );
    }
    let absolute = repo.root.join("apps/server/tsconfig.json");
    assert_eq!(
        repo.ts_sync(&["--project", absolute.to_str().unwrap()]).1["configuration"]["reason"],
        "invalid_project"
    );

    std::fs::write(repo.root.join(".gitignore"), "ignored/\n").unwrap();
    for directory in ["ignored", "node_modules"] {
        std::fs::create_dir_all(repo.root.join(directory)).unwrap();
        let path = format!("{directory}/tsconfig.json");
        std::fs::write(repo.root.join(&path), "{}").unwrap();
        let result = repo.ts_sync(&["--project", &path]).1;
        assert_eq!(
            result["configuration"]["reason"], "unavailable_project",
            "{result}"
        );
        assert!(!result["configuration"]["candidates"]
            .as_array()
            .unwrap()
            .contains(&json!(path)));
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("apps/server/tsconfig.json", repo.root.join("tsconfig.json"))
            .unwrap();
        assert_eq!(
            repo.ts_sync(&[]).1["configuration"]["reason"],
            "unavailable_project"
        );
    }
    assert!(!repo.db().exists());
    for index in 0..25 {
        std::fs::write(repo.root.join(format!("tsconfig.{index:02}.json")), "{}").unwrap();
    }
    let result = repo.ts_sync(&[]).1;
    assert_eq!(
        result["configuration"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    assert_eq!(result["configuration"]["candidates_truncated"], true);

    // An unreadable profile must not quietly use defaults.
    std::fs::create_dir(repo.root.join(".briefcase/sync.json")).unwrap();
    let result = repo.ts_sync(&[]).1;
    assert_eq!(result["sync"], "needs_configuration");
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains(".briefcase/sync.json"));
    #[cfg(unix)]
    {
        std::fs::remove_dir(repo.root.join(".briefcase/sync.json")).unwrap();
        std::os::unix::fs::symlink("missing-profile", repo.root.join(".briefcase/sync.json"))
            .unwrap();
        let result = repo.ts_sync(&[]).1;
        assert_eq!(result["sync"], "needs_configuration");
        assert!(result["message"]
            .as_str()
            .unwrap()
            .contains(".briefcase/sync.json"));
    }
}

#[test]
fn typescript_sync_preflight_preserves_publications_and_explicit_selection() {
    let repo = Repo::typescript();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let wrapper = repo._dir.path().join("node-wrapper");
        let log = repo._dir.path().join("node-launches");
        let quote = |p: &Path| format!("'{}'", p.to_str().unwrap().replace('\'', "'\\''"));
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf 'launch\\n' >> {}\nexec {} \"$@\"\n",
                quote(&log),
                quote(&Repo::node())
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let server = "apps/server/tsconfig.json";
    let (ok, first, stderr) = repo.ts_sync(&["--project", "./apps/server/../server/tsconfig.json"]);
    assert!(ok, "{first} {stderr}");
    assert_eq!(first["sync"], "refreshed");
    assert_eq!(first["profile"]["project"], server);
    let view = first["view_id"].clone();
    assert_eq!(repo.ts_sync(&["--project", server]).1["sync"], "up-to-date");
    assert_eq!(repo.scans(), 1);
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_to_string(repo._dir.path().join("node-launches"))
            .unwrap()
            .lines()
            .count(),
        2,
        "unchanged sync must launch neither preflight nor analysis"
    );
    std::fs::write(
        repo.root.join(".briefcase/sync.json"),
        r#"{"language":"typescript","project":"apps/server/tsconfig.json"}"#,
    )
    .unwrap();
    assert_eq!(repo.ts_sync(&[]).1["sync"], "up-to-date");
    for (config, reason) in [
        ("{", "invalid_config"),
        (
            r#"{"extends":"./missing.json","include":["src/**/*.ts"]}"#,
            "invalid_config",
        ),
        (
            r#"{"files":[],"references":[{"path":"../dashboard"}]}"#,
            "unsupported_references",
        ),
        (r#"{"files":[]}"#, "no_sources"),
    ] {
        std::fs::write(repo.root.join(server), config).unwrap();
        for _ in 0..2 {
            let (ok, result, stderr) = repo.ts_sync(&[]);
            assert!(!ok, "{stderr}");
            assert_eq!(result["sync"], "needs_configuration", "{result}");
            assert_eq!(result["configuration"]["reason"], reason, "{result}");
            let status = repo.ok(&["status"]);
            assert_eq!(status["current_view"], view);
            assert_eq!(status["scans"].as_array().unwrap().len(), 1);
            assert_eq!(status["pending_or_interrupted"], false);
            assert_eq!(status["readiness"]["state"], "ready");
        }
    }
    // CLI selection overrides the now-invalid repository setting.
    let result = repo
        .ts_sync(&["--project", "apps/dashboard/tsconfig.json"])
        .1;
    assert_eq!(result["sync"], "refreshed", "{result}");
    assert_eq!(result["profile"]["project"], "apps/dashboard/tsconfig.json");

    std::fs::write(
        repo.root.join("jsconfig.json"),
        r#"{"compilerOptions":{"allowJs":true},"include":["client.js"]}"#,
    )
    .unwrap();
    std::fs::write(repo.root.join("client.js"), "export function client() {}\n").unwrap();
    assert_eq!(
        repo.ts_sync(&["--project", "jsconfig.json"]).1["sync"],
        "refreshed"
    );
    assert_eq!(
        repo.ts_sync(&["--project", "jsconfig.json"]).1["sync"],
        "up-to-date"
    );

    // Failed preflight must not recover or otherwise mutate an older attempt.
    let current = repo.ok(&["status"])["current_view"].clone();
    let generation = Ledger::open(&repo.db())
        .unwrap()
        .begin(repo.ok(&["status"])["repository"].as_str().unwrap(), None)
        .unwrap();
    assert_eq!(repo.ts_sync(&[]).1["sync"], "needs_configuration");
    let blocked = repo.ok(&["status"]);
    assert_eq!(blocked["current_view"], current);
    assert_eq!(
        blocked["scans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["generation"] == generation)
            .unwrap()["status"],
        "preparing"
    );
    // Correction permits normal recovery of a genuinely interrupted attempt.
    std::fs::write(repo.root.join(server), r#"{"include":["src/**/*.ts"]}"#).unwrap();
    assert_eq!(repo.ts_sync(&[]).1["sync"], "refreshed");
    let status = repo.ok(&["status"]);
    assert_eq!(status["pending_or_interrupted"], false);
    assert_eq!(
        status["scans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["generation"] == generation)
            .unwrap()["status"],
        "interrupted"
    );
}

#[test]
fn typescript_sync_preflight_process_failures_do_not_allocate_generations() {
    let repo = Repo::typescript();
    let (ok, result, _) = repo.run(
        &repo.root,
        &[
            "sync",
            "--require-ready",
            "--project",
            "apps/server/tsconfig.json",
            "--node",
            repo.root.join("absent-node").to_str().unwrap(),
        ],
    );
    assert!(!ok);
    assert_eq!(result["sync"], "failed", "{result}");
    assert_eq!(repo.scans(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let fake = repo.root.join("fake-node");
        std::fs::write(&fake, "#!/bin/sh\nexec /bin/sleep 5\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = repo
            .run(
                &repo.root,
                &[
                    "sync",
                    "--require-ready",
                    "--project",
                    "apps/server/tsconfig.json",
                    "--node",
                    fake.to_str().unwrap(),
                    "--timeout",
                    "1",
                ],
            )
            .1;
        assert_eq!(result["sync"], "timeout", "{result}");
        assert_eq!(repo.scans(), 0);
        std::fs::write(&fake, "#!/bin/sh\nprintf 'invalid-json'\n").unwrap();
        let result = repo
            .run(
                &repo.root,
                &[
                    "sync",
                    "--project",
                    "apps/server/tsconfig.json",
                    "--node",
                    fake.to_str().unwrap(),
                ],
            )
            .1;
        assert_eq!(result["sync"], "failed", "{result}");
        assert_eq!(repo.scans(), 0);
    }
}

#[test]
fn concurrent_writers_are_admitted_one_at_a_time_and_dead_attempts_recover() {
    let repo = Repo::new();
    assert_eq!(repo.sync()["sync"], "refreshed");
    let view = repo.ok(&["status"])["current_view"].clone();
    {
        let _held = workspace::writer_lock(&repo.db()).unwrap();
        let locked = repo.run(&repo.root, &["sync", "--require-ready"]);
        assert!(!locked.0, "locked must not claim readiness");
        assert_eq!(locked.1["sync"], "locked");
        assert_eq!(repo.ok(&["status"])["readiness"]["state"], "syncing");
    }
    // A process that died mid-preparation leaves a newer source intent.
    let generation = Ledger::open(&repo.db())
        .unwrap()
        .begin(repo.ok(&["status"])["repository"].as_str().unwrap(), None)
        .unwrap();
    let retained = repo.ok(&["--json","call","dependencies"]);
    assert_eq!(retained["kind"],"neighbors");
    assert_eq!(retained["view_id"],view);
    assert_eq!(retained["freshness"]["pending_or_interrupted"],true);
    assert_eq!(repo.ok(&["status"])["current_view"], view);
    let result = repo.sync();
    assert_eq!(result["sync"], "refreshed");
    let status = repo.ok(&["status"]);
    let dead = status["scans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["generation"] == generation)
        .unwrap();
    assert_eq!(dead["status"], "interrupted");
    assert_eq!(status["current_view"], result["view_id"]);
    assert_eq!(status["pending_or_interrupted"], false);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn git_history_is_retained_incrementally_and_follows_reachability() {
    let repo = Repo::new();
    std::fs::remove_dir_all(repo.root.join(".git")).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&repo.root)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "Initial layout"]);
    std::fs::write(
        repo.root.join("src/store.rs"),
        "pub fn save() { flush_journal(); }\n",
    )
    .unwrap();
    git(&[
        "commit",
        "-qam",
        "Flush the journal before saving\n\nPrevents torn writes after a crash.",
    ]);
    let flush = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.root.join(".gitignore"), ".briefcase/\n").unwrap();
    git(&["add", ".gitignore"]);
    git(&["commit", "-qm", "Ignore Briefcase state"]);

    let first = repo.sync();
    assert_eq!(first["history"]["status"], "refreshed");
    assert_eq!(first["history"]["added"], 3);
    assert_eq!(repo.sync()["history"]["status"], "up-to-date");

    let found = repo.ok(&[
        "--json",
        "call",
        "recall",
        "--args",
        &json!({"text":"torn writes crash"}).to_string(),
    ]);
    let item = &found["items"][0];
    assert_eq!(item["path"], format!("git:{flush}"));
    assert_eq!(item["source_kind"], "commit");
    assert_eq!(item["file"], "src/store.rs");
    assert!(item["text"]
        .as_str()
        .unwrap()
        .contains("+pub fn save() { flush_journal(); }"));
    assert_eq!(found["coverage"]["history"]["reachable_commits"], 3);
    let without = repo.ok(&[
        "--json",
        "call",
        "recall",
        "--args",
        &json!({"text":"torn writes crash","history":false}).to_string(),
    ]);
    assert!(without["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["source_kind"] != "commit"));

    // A new commit on top is added incrementally.
    std::fs::write(
        repo.root.join("README.md"),
        "# Demo\n\nJournal compaction runs weekly.\n",
    )
    .unwrap();
    git(&["commit", "-qam", "Describe journal compaction"]);
    let next = repo.sync();
    assert_eq!(next["history"]["added"], 1);

    // Moving HEAD to an older commit hides newer commits without deleting them.
    git(&["checkout", "-q", "HEAD~3"]);
    let moved = repo.sync();
    assert_eq!(moved["history"]["added"], 0);
    let hidden = repo.ok(&[
        "--json",
        "call",
        "recall",
        "--args",
        &json!({"text":"torn writes crash"}).to_string(),
    ]);
    assert!(hidden["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["source_kind"] != "commit"));
    git(&["checkout", "-q", "main"]);
    assert_eq!(repo.sync()["history"]["added"], 0);
    let back = repo.ok(&[
        "--json",
        "call",
        "recall",
        "--args",
        &json!({"text":"torn writes crash"}).to_string(),
    ]);
    assert_eq!(back["items"][0]["path"], format!("git:{flush}"));
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn operation_discovery_needs_no_database_and_agrees_across_transports() {
    let repo = Repo::new();
    let direct = repo.ok(&["operations", "reachability", "--json"]);
    let via_call = repo.ok(&[
        "call",
        "describe",
        "--args",
        "{\"operation\":\"reachability\"}",
    ]);
    assert_eq!(direct, via_call);
    assert_eq!(direct["arguments"]["required"], json!(["from", "to"]));
    let search_args = json!({"query":"what calls this function","limit":2});
    let search = repo.ok(&[
        "--json",
        "operations",
        "--search",
        "what calls this function",
        "--limit",
        "2",
    ]);
    assert_eq!(search, service::discover(&search_args).unwrap());
    let names = repo.ok(&["operations", "--names", "--json"]);
    assert!(names.as_array().unwrap().iter().any(|n| n == "brief"));
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"briefcase_describe","arguments":search_args}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"briefcase_query","arguments":{"operation":"describe","arguments":{"operation":"reachability"}}}})
    );
    let mut output = Vec::new();
    mcp::serve_path(&repo.db(), std::io::Cursor::new(input), &mut output).unwrap();
    let lines: Vec<Value> = output
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    assert_eq!(lines[0]["result"]["structuredContent"], search);
    assert_eq!(lines[1]["result"]["structuredContent"], direct);
    assert!(!repo.root.join(".briefcase").exists());
    assert!(
        !repo
            .run(&repo.root, &["operations", "map", "--search", "calls"])
            .0
    );
    assert!(!repo.run(&repo.root, &["operations", "--limit", "0"]).0);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn mcp_gateway_search_aliases_and_validation_share_the_service() {
    let repo = Repo::new();
    repo.sync();
    let ledger = Ledger::open_existing(&repo.db()).unwrap();
    let call = |name: &str, arguments: Value| {
        mcp::dispatch(&ledger,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}))["result"].clone()
    };
    let args = json!({"text":TOKEN,"max_bytes":16000,"history":false});
    let search = call(
        "briefcase_search",
        json!({"query":TOKEN,"max_bytes":16000,"history":false}),
    );
    let canonical = service::call(&ledger, "recall", &args).unwrap();
    let mut transport = search["structuredContent"].clone();
    transport["budget"]["returned_response_bytes"] =
        canonical["budget"]["returned_response_bytes"].clone();
    assert_eq!(transport, canonical);
    let status = call("briefcase_status", json!({}));
    assert_eq!(
        status["structuredContent"],
        call("architecture_status", json!({}))["structuredContent"]
    );
    assert_eq!(
        status["structuredContent"],
        call("briefcase_query", json!({"operation":"status"}))["structuredContent"]
    );
    for (tool, args) in [
        (
            "briefcase_query",
            json!({"operation":"reachability","arguments":{"from":"a"}}),
        ),
        (
            "briefcase_query",
            json!({"operation":"dependencies","arguments":{"sql":"DROP"}}),
        ),
        (
            "briefcase_query",
            json!({"operation":"dependencies","arguments":[]}),
        ),
        ("briefcase_query", json!({"operation":42})),
        ("briefcase_query", json!({"operation":"sync"})),
        ("briefcase_search", json!({"query":TOKEN,"max_bytes":0})),
        ("briefcase_search", json!({"text":TOKEN})),
        ("briefcase_describe", json!({"operation":"scan"})),
        ("briefcase_status", json!({"view":"latest"})),
        ("sync", json!({})),
    ] {
        assert_eq!(call(tool, args)["isError"], true, "{tool}");
    }
    assert!(mcp::tools().to_string().len() < 5000);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn mcp_compact_surface_mirrors_the_pi_pair() {
    let repo = Repo::new();
    repo.sync();
    let ledger = Ledger::open_existing(&repo.db()).unwrap();
    let compact = |name: &str, arguments: Value| {
        mcp::dispatch_for(
            mcp::Surface::Compact,
            &ledger,
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        )["result"]
            .clone()
    };
    let listed = mcp::dispatch_for(
        mcp::Surface::Compact,
        &ledger,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    );
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["context", "briefcase"]);
    assert!(tools
        .iter()
        .all(|t| t["annotations"]["readOnlyHint"] == true));
    assert!(mcp::tools_for(mcp::Surface::Compact).to_string().len() < 5000);
    // `context` is the Pi search over the same recall operation.
    let found = compact(
        "context",
        json!({"query":TOKEN,"max_bytes":16000,"history":false}),
    );
    let canonical = service::call(
        &ledger,
        "recall",
        &json!({"text":TOKEN,"max_bytes":16000,"history":false}),
    )
    .unwrap();
    let mut transport = found["structuredContent"].clone();
    transport["budget"]["returned_response_bytes"] =
        canonical["budget"]["returned_response_bytes"].clone();
    assert_eq!(transport, canonical);
    // `briefcase` multiplexes the catalog, including database-free discovery.
    let discovered = compact(
        "briefcase",
        json!({"operation":"describe","arguments":{"operation":"reachability"}}),
    );
    assert_eq!(
        discovered["structuredContent"],
        service::discover(&json!({"operation":"reachability"})).unwrap()
    );
    let via_trace = compact(
        "briefcase",
        json!({"operation":"dependencies","arguments":{}}),
    );
    let via_gateway = mcp::dispatch(
        &ledger,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"briefcase_query","arguments":{"operation":"dependencies","arguments":{}}}}),
    )["result"]
        .clone();
    // Advisory timing and validation-cache diagnostics differ per call.
    let mut trace_content = via_trace["structuredContent"].clone();
    let mut gateway_content = via_gateway["structuredContent"].clone();
    for value in [&mut trace_content, &mut gateway_content] {
        if let Some(object) = value.as_object_mut() {
            object.remove("profile");
            object.remove("validation_cache_hit");
            object.remove("freshness");
            object.remove("budget");
        }
    }
    assert_eq!(trace_content, gateway_content);
    for (tool, args) in [
        ("context", json!({})),
        ("context", json!({"text":TOKEN})),
        ("context", json!({"query":TOKEN,"max_bytes":0})),
        (
            "briefcase",
            json!({"operation":"reachability","arguments":{"from":"a"}}),
        ),
        ("briefcase", json!({"operation":"sync","arguments":{}})),
        (
            "briefcase_query",
            json!({"operation":"status","arguments":{}}),
        ),
        ("briefcase_search", json!({"query":TOKEN})),
    ] {
        assert_eq!(compact(tool, args)["isError"], true, "{tool}");
    }
    // The gateway surface likewise rejects the compact names.
    let gateway_context = mcp::dispatch(
        &ledger,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"context","arguments":{"query":TOKEN}}}),
    );
    assert_eq!(gateway_context["result"]["isError"], true);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn mcp_compact_server_answers_discovery_without_a_database() {
    let repo = Repo::new();
    let input = format!(
        "{}\n{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"briefcase","arguments":{"operation":"describe","arguments":{"operation":"brief"}}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"context","arguments":{"query":TOKEN}}})
    );
    let mut output = Vec::new();
    mcp::serve_path_for(
        mcp::Surface::Compact,
        &repo.db(),
        std::io::Cursor::new(input),
        &mut output,
    )
    .unwrap();
    let lines: Vec<Value> = output
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    let names: Vec<_> = lines[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["context", "briefcase"]);
    assert_eq!(lines[1]["result"]["isError"], false);
    assert_eq!(lines[2]["result"]["isError"], true);
    assert!(lines[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("NOT_INITIALIZED"));
    assert!(!repo.db().exists());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn muse_install_wires_the_compact_surface_and_sync_hooks() {
    let repo = Repo::new();
    let project = repo.root.to_str().unwrap().to_owned();
    let (ok, report, stderr) = repo.run(
        &repo.root,
        &["install", "--agent", "muse", "--project", &project],
    );
    assert!(ok, "{stderr}");
    assert_eq!(report["status"], "wired");
    assert_eq!(report["surface"], "compact");
    let mcp: Value =
        serde_json::from_str(&std::fs::read_to_string(repo.root.join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(
        mcp["mcpServers"]["briefcase"]["args"],
        json!(["mcp", "--surface", "compact"])
    );
    assert_eq!(
        mcp["mcpServers"]["briefcase"]["framing"],
        "line_delimited_json"
    );
    let hooks: Value =
        serde_json::from_str(&std::fs::read_to_string(repo.root.join(".muse/hooks.json")).unwrap())
            .unwrap();
    assert!(hooks["hooks"].get("Stop").is_none());
    {
        let event = "SessionStart";
        let handler = &hooks["hooks"][event][0]["hooks"][0];
        assert_eq!(handler["async"], true);
        assert!(handler["command"]
            .as_str()
            .unwrap()
            .contains("monitor ensure"));
    }
    let (ok, again, stderr) = repo.run(
        &repo.root,
        &["install", "--agent", "muse", "--project", &project],
    );
    assert!(ok, "{stderr}");
    assert_eq!(again["status"], "already-configured");
    let help = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .args(["mcp", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8(help.stdout).unwrap().contains("compact"));
}

#[test]
fn cli_call_and_mcp_share_one_catalog_and_reject_writes() {
    let repo = Repo::new();
    repo.sync();
    let ledger = Ledger::open_existing(&repo.db()).unwrap();
    let names: Vec<String> = repo
        .ok(&["operations"])
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_owned())
        .collect();
    let tools = mcp::dispatch(
        &ledger,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    );
    assert!(names.len() > 20);
    let tool_names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tool_names,
        [
            "briefcase_search",
            "briefcase_query",
            "briefcase_describe",
            "briefcase_status"
        ]
    );
    for (operation, args) in [
        ("dependencies", json!({})),
        ("callgraph", json!({"limit":5})),
        ("recall", json!({"text":TOKEN})),
        ("status", json!({})),
    ] {
        let cli = repo.ok(&["--json", "call", operation, "--args", &args.to_string()]);
        let via_mcp = mcp::dispatch(
            &ledger,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"briefcase_query","arguments":{"operation":operation,"arguments":args}}}),
        );
        let mut structured = via_mcp["result"]["structuredContent"].clone();
        let mut cli = cli;
        for v in [&mut structured, &mut cli] {
            // Advisory timing and validation-cache diagnostics differ per call.
            if let Some(o) = v.as_object_mut() {
                o.remove("profile");
                o.remove("validation_cache_hit");
                o.remove("freshness");
                o.remove("budget");
            }
        }
        if operation == "status" {
            assert_eq!(cli["current_view"], structured["current_view"]);
        } else {
            assert_eq!(cli, structured, "{operation}");
        }
    }
    // Ordinary named CLI reads share dispatch while passing CLI defaults.
    for (command, operation) in [
        ("dependencies", "dependencies"),
        ("callgraph", "callgraph"),
        ("domains", "domains"),
        ("drift", "drift"),
        ("policy", "policy"),
        ("unresolved", "unresolved"),
    ] {
        for extra in [
            json!({}),
            json!({"component":"missing","limit":1,"allow_partial":true,"include_structure":true}),
        ] {
            let mut named_args = vec!["--json", command];
            if extra.get("limit").is_some() {
                named_args.extend([
                    "--component",
                    "missing",
                    "--limit",
                    "1",
                    "--allow-partial",
                    "--include-structure",
                ]);
            }
            let mut named = repo.ok(&named_args);
            let mut called = repo.ok(&["--json", "call", operation, "--args", &extra.to_string()]);
            for value in [&mut named, &mut called] {
                if let Some(o) = value.as_object_mut() {
                    o.remove("profile");
                    o.remove("validation_cache_hit");
                    o.remove("freshness");
                    o.remove("budget");
                }
            }
            assert_eq!(named, called, "{command}: {extra}");
        }
    }
    for (command, operation) in [
        ("deployment", "deployment"),
        ("desired-connections", "desired_connections"),
    ] {
        let named = repo.run(&repo.root, &["--json", command]);
        let called = repo.run(&repo.root, &["--json", "call", operation]);
        assert!(!named.0 && !called.0);
        assert!(
            named.2.contains("unrecognized subcommand"),
            "{}",
            named.2
        );
        assert!(
            called.2.contains("UNKNOWN_OPERATION"),
            "{}",
            called.2
        );
    }
    let invalid_limit = repo.run(&repo.root, &["dependencies", "--limit", "0"]);
    assert!(!invalid_limit.0, "Clap bounds must remain active");
    for write in ["scan", "retract", "rebuild", "sync", "install"] {
        let (ok, _, stderr) = repo.run(&repo.root, &["call", write]);
        assert!(
            !ok && stderr.contains("UNKNOWN_OPERATION"),
            "{write}: {stderr}"
        );
        let denied = mcp::dispatch(
            &ledger,
            &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"briefcase_query","arguments":{"operation":write,"arguments":{}}}}),
        );
        assert_eq!(denied["result"]["isError"], true);
    }
    let bad = repo.run(
        &repo.root,
        &["call", "reachability", "--args", "{\"from\":\"a\"}"],
    );
    assert!(!bad.0 && bad.2.contains("to is required"), "{}", bad.2);
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn mcp_server_reports_not_initialized_without_creating_a_database() {
    let repo = Repo::new();
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"briefcase_status","arguments":{}}})
    );
    let mut output = Vec::new();
    mcp::serve_path(&repo.db(), std::io::Cursor::new(input), &mut output).unwrap();
    let lines: Vec<Value> = output
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    assert_eq!(lines[0]["result"]["tools"].as_array().unwrap().len(), 4);
    assert_eq!(lines[1]["result"]["isError"], true);
    assert!(lines[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("NOT_INITIALIZED"));
    assert!(!repo.db().exists());
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn recall_finds_documentation_only_text_and_rebuilds_from_retained_bytes() {
    let repo = Repo::new();
    repo.sync();
    let recalled = repo.ok(&[
        "--json",
        "call",
        "recall",
        "--args",
        &json!({"text":TOKEN}).to_string(),
    ]);
    assert_eq!(recalled["authority"], "retained_source");
    let item = &recalled["items"][0];
    assert_eq!(item["path"], "README.md");
    assert_eq!(item["source_kind"], "documentation");
    assert!(item["text"].as_str().unwrap().contains(TOKEN));
    assert_eq!(item["line_range"]["start"], 1);
    assert!(recalled["coverage"]["not_included"]
        .as_array()
        .unwrap()
        .contains(&json!("Git commit history")));

    // Compatibility spellings use the same ranked source index.
    let retrieved = repo.ok(&["--json", "retrieve", TOKEN]);
    assert_eq!(retrieved["items"],recalled["items"]);

    // Remove the projection; recall rebuilds it from retained payloads even
    // after the live file disappears, and returns the same result.
    std::fs::remove_file(repo.root.join("README.md")).unwrap();
    let conn = rusqlite::Connection::open(repo.db()).unwrap();
    conn.execute_batch(
        "DELETE FROM recall_indexes; DELETE FROM recall_units; DELETE FROM recall_fts;",
    )
    .unwrap();
    drop(conn);
    let rebuilt = repo.ok(&[
        "--json",
        "call",
        "recall",
        "--args",
        &json!({"text":TOKEN}).to_string(),
    ]);
    assert_eq!(rebuilt["items"], recalled["items"]);

    // Tiny evidence-only budgets can no longer fit the pinned JSON envelope.
    let small = repo.run(
        &repo.root,
        &[
            "--json",
            "call",
            "recall",
            "--args",
            &json!({"text":"demo rotates","max_bytes":10}).to_string(),
        ],
    );
    assert!(!small.0 && small.2.contains("max_bytes"));

    // Corrupted retained bytes are a hard failure, never a silent answer.
    let conn = rusqlite::Connection::open(repo.db()).unwrap();
    conn.execute_batch("DELETE FROM recall_indexes; DELETE FROM recall_units; DELETE FROM recall_fts;
        UPDATE payloads SET bytes=CAST('tampered' AS BLOB) WHERE artifact_id IN (SELECT id FROM records WHERE body LIKE '%README.md%');")
        .unwrap();
    drop(conn);
    let (ok, _, stderr) = repo.run(
        &repo.root,
        &[
            "call",
            "recall",
            "--args",
            &json!({"text":TOKEN}).to_string(),
        ],
    );
    assert!(!ok && stderr.contains("DIGEST"), "{stderr}");
}

#[test]
#[ignore = "conformance: not in the required PR suite; run via the conformance lane"]
fn call_waits_briefly_for_an_active_writer() {
    let repo = Repo::new();
    repo.sync();
    let db = repo.db();
    let held = workspace::writer_lock(&db).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        drop(held);
    });
    let started = std::time::Instant::now();
    repo.ok(&["--json", "call", "status", "--wait-ready", "10"]);
    assert!(started.elapsed() >= Duration::from_millis(500));
    release.join().unwrap();
}

#[test]
fn alignment_checks_are_read_only_and_git_never_enters_project_analysis() {
    let repo = Repo::new();
    let check = repo.ok(&["sync", "--check"]);
    assert_eq!(check["sync"], "checked");
    assert_eq!(check["alignment"][0]["state"], "uninitialized");
    assert!(
        !repo.root.join(".briefcase").exists(),
        "check created runtime state"
    );
    let first = repo.sync();
    let bytes = std::fs::read(repo.db()).unwrap();
    let status = std::fs::read(repo.db().with_extension("db.sync.json")).unwrap();
    let aligned=repo.ok(&["sync", "--check"]);
    assert_eq!(aligned["ready"], true,"{aligned}");
    assert_eq!(std::fs::read(repo.db()).unwrap(), bytes);
    assert_eq!(
        std::fs::read(repo.db().with_extension("db.sync.json")).unwrap(),
        status
    );
    let commit = Command::new("git")
        .current_dir(&repo.root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "history only",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success(), "{:?}", commit);
    let check = repo.ok(&["sync", "--check"]);
    assert_eq!(check["alignment"][0]["state"], "aligned");
    assert_eq!(check["alignment"][1]["state"], "uninitialized");
    // An invalid source profile proves Git-only reconciliation does not enter that engine.
    std::fs::write(repo.root.join(".briefcase/sync.json"), "invalid").unwrap();
    let git = repo.ok(&["sync", "--git", "--require-ready"]);
    assert_eq!(git["git"]["status"], "refreshed");
    assert_eq!(git["local_sync"], "updated");
    assert!(git.get("project").is_none());
    std::fs::remove_file(repo.root.join(".briefcase/sync.json")).unwrap();
    assert_eq!(repo.sync()["view_id"], first["view_id"]);
    assert_eq!(repo.scans(), 1);
}

#[test]
fn matching_transcript_revisions_sync_independently_for_all_supported_harnesses() {
    let repo = Repo::new();
    let first = repo.sync();
    let root = repo.root.to_string_lossy();
    for (harness, directory, header) in [
        (
            "pi",
            repo._dir.path().join("pi/sessions"),
            json!({"type":"session","version":3,"id":"test","cwd":root}),
        ),
        (
            "codex",
            repo._dir.path().join("codex/sessions"),
            json!({"type":"session_meta","payload":{"id":"test","cwd":root}}),
        ),
        (
            "claude-code",
            repo._dir.path().join("claude/projects"),
            json!({"type":"user","uuid":"test","sessionId":"test","cwd":root,"message":{"role":"user","content":"hello"}}),
        ),
    ] {
        std::fs::create_dir_all(&directory).unwrap();
        let transcript = directory.join("test.jsonl");
        std::fs::write(&transcript, format!("{header}\n")).unwrap();
        let check = repo.ok(&["sync", "--check", "--sessions", harness]);
        assert_eq!(check["alignment"][0]["state"], "dirty", "{check}");
        let synced = repo.ok(&["sync", "--sessions", harness, "--require-ready"]);
        assert_eq!(synced["sessions"][harness]["matched"], 1, "{synced}");
        assert!(synced.get("project").is_none());
        assert_eq!(
            repo.ok(&["sync", "--check", "--sessions", harness])["ready"],
            true
        );
        let before = repo.scans();
        assert_eq!(
            repo.ok(&["sync", "--sessions", harness])["local_sync"],
            "up-to-date"
        );
        assert_eq!(repo.scans(), before);
        // Appending an unknown complete record is still a new retained transcript revision.
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap()
            .write_all(b"{\"type\":\"unknown\"}\n")
            .unwrap();
        assert_eq!(
            repo.ok(&["sync", "--check", "--sessions", harness])["ready"],
            false
        );
        repo.ok(&["sync", "--sessions", harness]);
        assert_eq!(repo.scans(), before + 1);
        // Exact cwd selection excludes otherwise valid records from other repositories.
        let mut other = header;
        if harness == "codex" {
            other["payload"]["cwd"] = json!("/another/repository");
        } else {
            other["cwd"] = json!("/another/repository");
        }
        std::fs::write(directory.join("other.jsonl"), format!("{other}\n")).unwrap();
        assert_eq!(
            repo.ok(&["sync", "--check", "--sessions", harness])["ready"],
            true
        );
    }
    assert_eq!(repo.ok(&["status"])["current_view"], first["view_id"]);
    let status = repo.ok(&["status"]);
    assert_eq!(
        status["scans"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["scope"] == "source")
            .count(),
        1
    );
}

struct TestMonitor(u64);
impl Drop for TestMonitor {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &self.0.to_string()])
                .status();
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.0.to_string(), "/F"])
                .status();
        }
    }
}
fn drain_scheduler(db: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let status = briefcase::architecture::scheduler::status(db);
        if status["pending"] != true && status["worker_active"] != true {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn init_previews_installs_and_repeats_without_new_generations() {
    let repo = Repo::new();
    let preview = repo.ok(&["init", "--agents", "all", "--dry-run"]);
    assert_eq!(preview["init"], "planned");
    assert!(!repo.root.join(".briefcase").exists());
    for dir in [".pi", ".codex", ".claude", ".muse"] {
        assert!(!repo.root.join(dir).exists());
    }
    let initialized = repo.ok(&["init", "--agents", "all"]);
    let _monitor = TestMonitor(initialized["monitor"]["pid"].as_u64().unwrap());
    assert_eq!(initialized["init"], "complete", "{initialized}");
    assert_eq!(initialized["integrations"].as_array().unwrap().len(), 4);
    drain_scheduler(&repo.db());
    let before = repo.scans();
    let again = repo.ok(&["init"]);
    assert_eq!(again["monitor"]["monitor"], "already-running");
    assert_eq!(again["local"]["local_sync"], "up-to-date");
    assert_eq!(repo.scans(), before);
    for integration in repo.ok(&["status"])["integrations"].as_array().unwrap() {
        assert_eq!(integration["state"], "configured", "{integration}");
    }
    // Claude supports prompt/stop advisory hooks; other hosts keep bootstrap hooks.
    for path in [
        ".codex/hooks.json",
        ".claude/settings.json",
        ".muse/hooks.json",
    ] {
        let doc: Value =
            serde_json::from_slice(&std::fs::read(repo.root.join(path)).unwrap()).unwrap();
        if path==".claude/settings.json" {
            assert!(doc["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"].as_str().unwrap().contains("context-hook --phase prompt"));
            assert!(doc["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().contains("context-hook --phase complete"));
        } else {assert!(doc["hooks"].get("Stop").is_none());}
        assert!(doc["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("monitor ensure --hook --quiet"));
    }
}

#[test]
fn simultaneous_monitor_bootstraps_share_one_owner_and_startup_catches_missed_edits() {
    let repo = Repo::new();
    let first = repo.sync();
    std::fs::write(repo.root.join("src/store.rs"), "pub fn load() {}\n").unwrap();
    let reports = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| repo.ok(&["monitor", "ensure"])))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        reports.iter().filter(|r| r["monitor"] == "started").count(),
        1,
        "{reports:?}"
    );
    let _monitor = TestMonitor(reports.iter().find_map(|r| r["pid"].as_u64()).unwrap());
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let status = repo.ok(&["status"]);
        if status["current_view"] != first["view_id"] {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(50));
    }
    drain_scheduler(&repo.db());
    assert_eq!(repo.scans(), 2);
    // Both edits below happen after startup, well before the 300-second fallback.
    std::fs::write(repo.root.join("src/store.rs"), "pub fn save() {}\n").unwrap();
    std::fs::rename(repo.root.join("README.md"), repo.root.join("GUIDE.md")).unwrap();
    let directory = repo._dir.path().join("codex/sessions");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("test.jsonl"),
        format!(
            "{}\n",
            json!({"type":"session_meta","payload":{"cwd":repo.root,"id":"new"}})
        ),
    )
    .unwrap();
    loop {
        let status = repo.ok(&["status"]);
        let scans = status["scans"].as_array().unwrap();
        if scans.iter().filter(|s| s["scope"] == "source").count() == 3
            && scans.iter().filter(|s| s["scope"] == "session").count() == 1
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(50));
    }
    drain_scheduler(&repo.db());
    let before = repo.scans();
    let synced = repo.ok(&["sync", "--require-ready"]);
    assert_eq!(synced["sync"], "up-to-date", "{synced}");
    assert_eq!(repo.scans(), before);
}

#[test]
fn claims_sync_is_explicit_and_old_claims_remain_pinned_after_source_changes() {
    let repo = Repo::new();
    let script = repo._dir.path().join("provider.py");
    let marker = repo._dir.path().join("provider-invocations");
    std::fs::write(&script, r#"import json, sys
from pathlib import Path
r = json.load(sys.stdin)
with Path(sys.argv[1]).open('a') as f: f.write('invoked\n')
print(json.dumps({'proposals':[{'target_id':r['target_id'],'predicate':'BELONGS_TO_DOMAIN','domain':'Storage','supports':r['packet']['assertion_ids'][:1],'limitations':['controlled test']}], 'abstained':False,'limitations':[]}))
"#).unwrap();
    let provider = repo._dir.path().join("provider.json");
    std::fs::write(&provider, json!({"schema_version":"1.0","name":"controlled-local","model_name":"test","model_version":"1",
        "settings":{},"command":{"program":"/usr/bin/python3","args":[script,marker]},"timeout_seconds":5,"opaque":false,"deterministic":true}).to_string()).unwrap();
    let taxonomy = repo._dir.path().join("taxonomy.json");
    std::fs::write(
        &taxonomy,
        json!({"schema_version":"1.0","version":"test","domains":["Storage"]}).to_string(),
    )
    .unwrap();
    let first = repo.sync();
    assert!(!marker.exists());
    let claims = repo.ok(&[
        "claims",
        "sync",
        "domains",
        "--component",
        "store",
        "--provider",
        provider.to_str().unwrap(),
        "--taxonomy",
        taxonomy.to_str().unwrap(),
    ]);
    assert!(
        !claims["claim_ids"].as_array().unwrap().is_empty(),
        "{claims}"
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "invoked\n");
    assert_eq!(
        repo.ok(&["status"])["claims"]["domains"]["state"],
        "run_for_current_view"
    );
    let before = Ledger::read_only(&repo.db())
        .unwrap()
        .load(first["view_id"].as_str().unwrap())
        .unwrap()
        .bundle;
    std::fs::write(repo.root.join("src/store.rs"), "pub fn load() {}\n").unwrap();
    let initialized = repo.ok(&["init", "--agents", "codex"]);
    let _monitor = TestMonitor(initialized["monitor"]["pid"].as_u64().unwrap());
    drain_scheduler(&repo.db());
    assert_eq!(initialized["local"]["ready"], true);
    let coverage = repo.ok(&["status"])["claims"]["domains"].clone();
    assert_eq!(coverage["state"], "not_run_for_current_view");
    assert_eq!(coverage["latest_claim_view"], first["view_id"]);
    assert_ne!(coverage["latest_source_view"], first["view_id"]);
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "invoked\n",
        "automatic work invoked a provider"
    );
    let after = Ledger::read_only(&repo.db())
        .unwrap()
        .load(first["view_id"].as_str().unwrap())
        .unwrap()
        .bundle;
    assert_eq!(before, after, "historical Claims were changed");
    assert!(
        !repo
            .run(
                &repo.root,
                &["claims", "sync", "lineage", "--base", "view:1"]
            )
            .0,
        "provider must be explicitly selected"
    );
}

#[test]
fn configured_domains_and_legacy_hook_repairs_preserve_operator_control() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.root.join(".briefcase")).unwrap();
    std::fs::write(repo.root.join(".briefcase/sync.json"),
        r#"{"sync":{"project":false,"git":true,"sessions":{"pi":false,"codex":false,"claude-code":false}},"watch":{"enabled":false}}"#).unwrap();
    let check = repo.ok(&["sync", "--check"]);
    assert_eq!(check["alignment"].as_array().unwrap().len(), 1);
    assert_eq!(check["alignment"][0]["source"], "git");
    assert_eq!(repo.ok(&["monitor", "ensure"])["monitor"], "disabled");
    assert_eq!(
        repo.ok(&["sync", "--project"])["sync"],
        "refreshed",
        "an explicit domain overrides defaults"
    );
    let path = repo.root.join(".codex/hooks.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = json!({"theme":"keep","hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"briefcase sync-request --source codex --reason startup"}]}],
        "Stop":[{"matcher":"keep","hooks":[{"type":"command","command":"echo unrelated"},{"type":"command","command":"briefcase sync-request --source codex --reason turn-end"}]}]}});
    std::fs::write(&path, old.to_string()).unwrap();
    let (ok, report, _) = repo.run(&repo.root, &["init", "--agents", "codex"]);
    assert!(!ok);
    assert_eq!(report["init"], "blocked");
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        old
    );
    assert_eq!(repo.ok(&["status"])["integrations"][1]["state"], "outdated");
    let repaired = repo.ok(&["init", "--agents", "codex", "--repair"]);
    assert_eq!(repaired["monitor"]["monitor"], "disabled");
    assert!(Path::new(
        repaired["integrations"][0]["backups"][0]["backup"]
            .as_str()
            .unwrap()
    )
    .is_file());
    let new: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(new["theme"], "keep");
    assert_eq!(new["hooks"]["Stop"][0]["matcher"], "keep");
    assert_eq!(
        new["hooks"]["Stop"][0]["hooks"],
        json!([{"type":"command","command":"echo unrelated"}])
    );
    assert!(new["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("monitor ensure"));
    std::fs::write(&path, "invalid JSON").unwrap();
    assert!(
        !repo
            .run(&repo.root, &["init", "--agents", "codex", "--repair"])
            .0
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid JSON");
    std::fs::write(
        repo.root.join(".briefcase/sync.json"),
        r#"{"sync":{"sessions":{"muse":true}}}"#,
    )
    .unwrap();
    let unsupported = repo.run(&repo.root, &["sync", "--require-ready"]);
    assert!(!unsupported.0);
    assert_eq!(unsupported.1["sync"], "needs_configuration");
    assert!(unsupported.1["message"].as_str().unwrap().contains("Muse"));
}

#[test]
fn monitor_hooks_resolve_event_cwd_and_quiet_bootstraps_emit_no_stdout() {
    use std::io::Write;
    let repo = Repo::new();
    let launch = |quiet: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_briefcase"));
        cmd.current_dir(repo._dir.path())
            .env_remove("BRIEFCASE_DB")
            .env_remove("BRIEFCASE_SYNC")
            .env("PI_CODING_AGENT_DIR", repo._dir.path().join("pi"))
            .env("CODEX_HOME", repo._dir.path().join("codex"))
            .env("CLAUDE_CONFIG_DIR", repo._dir.path().join("claude"))
            .env("PATH", &repo.empty_path)
            .args(["monitor", "ensure", "--hook"]);
        if quiet {
            cmd.arg("--quiet");
        }
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                json!({"cwd":repo.root.join("src/nested")})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    let output = launch(false);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let _monitor = TestMonitor(report["pid"].as_u64().unwrap());
    assert_eq!(report["monitor"], "started");
    assert_eq!(report["database"], json!(repo.db()));
    let output = launch(true);
    assert!(output.stdout.is_empty());
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !repo.db().exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    drain_scheduler(&repo.db());
    assert!(!repo.root.join("src/nested/.briefcase").exists());
    let other = Repo::new();
    let (ok, _, stderr) = other.run(
        &other.root,
        &["--db", repo.db().to_str().unwrap(), "monitor", "ensure"],
    );
    assert!(!ok);
    assert!(stderr.contains("another repository"), "{stderr}");
}
