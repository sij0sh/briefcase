//! Shared M6 acceptance helpers. Every suite starts from retained reference
//! inputs: real podman build/OCI manifests, Kubernetes YAML, and the OTLP
//! export recorded by scripts/m6-reference.sh.
#![allow(dead_code)]
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("evaluation/sources/m6")
}
pub fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::create_dir_all(to).unwrap();
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
pub fn collection() -> Value {
    serde_json::from_slice(&std::fs::read(root().join("runtime/staging/collection.json")).unwrap())
        .unwrap()
}
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub db: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("knowledge.db");
        Self { dir, db }
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    /// A private copy of a retained M6 input directory that a test may edit.
    pub fn copy(&self, relative: &str, name: &str) -> PathBuf {
        let target = self.path(name);
        std::fs::create_dir_all(&target).unwrap();
        copy_dir(&root().join(relative), &target);
        target
    }
    pub fn try_cli(&self, args: &[&str]) -> Result<Value, String> {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_briefcase"))
            .arg("--db")
            .arg(&self.db)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        if output.status.success() {
            Ok(serde_json::from_slice(&output.stdout).unwrap())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }
    pub fn cli(&self, args: &[&str]) -> Value {
        self.try_cli(args)
            .unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }
    /// Capture the reference source with its declared policy, as the build did.
    pub fn scan(&self, source: &Path, label: &str) -> Value {
        let policy = root().join("policy/architecture.json");
        self.cli(&[
            "scan",
            source.to_str().unwrap(),
            "--language",
            "typescript",
            "--calls",
            "--label",
            label,
            "--repository-id",
            "m6-reference",
            "--policy",
            policy.to_str().unwrap(),
        ])
    }
    pub fn source(&self, label: &str) -> Value {
        let scanned = self.scan(&root().join("reference"), label);
        self.cli(&["analyze-data", "--view", label]);
        scanned
    }
    pub fn deployment(&self, inputs: &Path, source_view: &str, label: &str) -> Value {
        self.cli(&[
            "capture-deployment",
            inputs.to_str().unwrap(),
            "--environment",
            "staging",
            "--source-view",
            source_view,
            "--label",
            label,
        ])
    }
    pub fn runtime(
        &self,
        otlp: &Path,
        label: &str,
        start: &str,
        end: &str,
    ) -> Result<Value, String> {
        let c = collection();
        self.try_cli(&[
            "ingest-runtime",
            "--otlp",
            otlp.to_str().unwrap(),
            "--environment",
            "staging",
            "--interval-start",
            start,
            "--interval-end",
            end,
            "--collector",
            c["collector"]["id"].as_str().unwrap(),
            "--sampling",
            "complete",
            "--label",
            label,
        ])
    }
    pub fn real_runtime(&self, label: &str) -> Value {
        let c = collection();
        self.runtime(
            &root().join("runtime/staging/traces.jsonl"),
            label,
            c["interval_start"].as_str().unwrap(),
            c["interval_end"].as_str().unwrap(),
        )
        .unwrap()
    }
    fn build_complete() -> Self {
        let f = Self::new();
        f.source("app");
        f.deployment(&root().join("deployment/staging"), "app", "staging-desired");
        f.real_runtime("run1");
        f
    }
    /// Source + data overlay, desired staging topology, and the real interval.
    ///
    /// The canonical fixture is built once per test process; each caller
    /// gets a private copy of its database (plus any WAL sidecar) in a
    /// fresh temporary directory, so map assertions do not each pay for
    /// another TypeScript/compiler ingestion.
    pub fn complete() -> Self {
        static BASELINE: std::sync::OnceLock<Fixture> = std::sync::OnceLock::new();
        let baseline = BASELINE.get_or_init(Self::build_complete);
        let f = Self::new();
        std::fs::copy(&baseline.db, &f.db).unwrap();
        let wal = PathBuf::from(format!("{}-wal", baseline.db.display()));
        if wal.is_file() && std::fs::metadata(&wal).unwrap().len() > 0 {
            std::fs::copy(&wal, format!("{}-wal", f.db.display())).unwrap();
        }
        f
    }
}
/// (conclusion, detail, assertion_ids, explanation_handle) for each result row.
pub fn results(report: &Value) -> Vec<(String, Value, Vec<String>, String)> {
    report["response"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["conclusion"].as_str().unwrap().to_owned(),
                report["details"][r["id"].as_str().unwrap()].clone(),
                r["assertion_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_str().unwrap().to_owned())
                    .collect(),
                r["explanation_handle"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}
pub fn present(report: &Value) -> Vec<Value> {
    results(report)
        .into_iter()
        .filter(|r| r.0 == "present")
        .map(|r| r.1)
        .collect()
}
pub fn unknown(report: &Value) -> Vec<Value> {
    results(report)
        .into_iter()
        .filter(|r| r.0 == "unknown")
        .map(|r| r.1)
        .collect()
}
pub fn short(key: &Value) -> String {
    key.as_str()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap()
        .to_owned()
}
