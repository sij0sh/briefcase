//! SQLite is the only commit authority. Every accepted payload remains readable without projections.
use super::{
    contracts::{self, rows, strings, text, Capability, Payloads},
    identity, knowledge, require, workspace, Result,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    time::Duration,
};

pub struct Prepared {
    pub bundle: Value,
    pub payloads: Payloads,
    pub batches: Vec<(Value, Capability)>,
    pub diagnostics: Value,
}
impl Prepared {
    pub fn seal(
        mut bundle: Value,
        mut payloads: Payloads,
        generation: u64,
        diagnostics: Value,
    ) -> Result<Self> {
        require(
            generation > 0,
            "GENERATION",
            "Publication generations start at one",
        )?;
        let selected = rows(&bundle, "assertions")
            .iter()
            .map(|a| text(a, "id"))
            .collect();
        let batches = Self::batches(
            &mut bundle,
            &mut payloads,
            generation,
            &selected,
            &[
                Capability::Evidence,
                Capability::Derived,
                Capability::Declaration,
            ],
        )?;
        require(
            !batches.is_empty() || knowledge::context(&bundle)["kind"] == "session",
            "EMPTY",
            "No architecture evidence extracted",
        )?;
        contracts::validate(&bundle, &payloads)?;
        Ok(Self {
            bundle,
            payloads,
            batches,
            diagnostics,
        })
    }
    pub(crate) fn extension(
        mut bundle: Value,
        mut payloads: Payloads,
        generation: u64,
        selected: &BTreeSet<String>,
        capabilities: &[Capability],
        diagnostics: Value,
    ) -> Result<Self> {
        let batches = Self::batches(
            &mut bundle,
            &mut payloads,
            generation,
            selected,
            capabilities,
        )?;
        contracts::validate(&bundle, &payloads)?;
        Ok(Self {
            bundle,
            payloads,
            batches,
            diagnostics,
        })
    }
    fn batches(
        bundle: &mut Value,
        payloads: &mut Payloads,
        generation: u64,
        selected: &BTreeSet<String>,
        capabilities: &[Capability],
    ) -> Result<Vec<(Value, Capability)>> {
        require(
            generation > 0,
            "GENERATION",
            "Batch generations start at one",
        )?;
        let producer = identity::id("producer", &bundle["analysis_configs"])?;
        let mut batches = Vec::new();
        for &capability in capabilities {
            let class = capability.class().name();
            let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for a in rows(bundle, "assertions")
                .iter()
                .filter(|a| a["class"] == class && selected.contains(&text(a, "id")))
            {
                groups
                    .entry(String::from_utf8(identity::canonical(&a["context"])?)?)
                    .or_default()
                    .push(a.clone());
            }
            for (context, mut records) in groups {
                records.sort_by_key(|r| text(r, "id"));
                let context = identity::parse(context.as_bytes())?;
                let body = json!({"context":context,"class":class,"producer_id":producer,"records":records});
                let bytes = serde_json::to_vec(&body)?;
                let digest = identity::bytes_digest(&bytes);
                let aid = knowledge::artifact(bundle, payloads, "transport", bytes)?;
                batches.push((json!({"schema_version":"1.0",
                    "batch_id":identity::id("batch",&json!({"generation":generation,"digest":digest,"class":class}))?,
                    "class":class,"producer_id":producer,"context":context,
                    "record_ids":records.iter().map(|r|text(r,"id")).collect::<Vec<_>>(),
                    "expected_generation":generation-1,"payload_digest":digest,"payload_artifact_id":aid}),capability));
            }
        }
        require(
            batches
                .iter()
                .flat_map(|(b, _)| strings(&b["record_ids"]))
                .collect::<BTreeSet<_>>()
                == *selected,
            "CAPABILITY",
            "Producer may not select assertions outside its typed write grant",
        )?;
        Ok(batches)
    }
}
/// A ranked recall candidate's unit.
pub(crate) enum RecallHit {
    Source(i64),
    Commit(i64),
}
/// SQLite `application_id` for Briefcase databases (the original "TRCE" marker).
/// Keep this stable across product renames so existing ledgers remain readable.
pub const APPLICATION_ID: i32 = 0x5452_4345;
#[derive(PartialEq)]
enum Ownership {
    Marked,
    Empty,
}
/// A schema version is not an application identity. Briefcase accepts its own
/// marker or an empty file; everything else is foreign.
fn ownership(db: &Connection) -> Result<Ownership> {
    let application: i32 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: u64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tables: i64 = db.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))?;
    let foreign = |why: &str| -> Result<Ownership> {
        Err(
            format!("FOREIGN_DATABASE: Not a Briefcase database ({why}); it was left unchanged")
                .into(),
        )
    };
    if application == APPLICATION_ID {
        require(
            version == 1,
            "SCHEMA_VERSION",
            "Briefcase database schema version is not supported by this build",
        )?;
        return Ok(Ownership::Marked);
    }
    if application != 0 {
        return foreign("another application_id");
    }
    if tables == 0 && version == 0 {
        return Ok(Ownership::Empty);
    }
    foreign("unrecognized schema")
}
/// Long-lived processes (serve mode) keep extra parsed copies of immutable
/// reports and retained views. One-shot commands skip the copy cost.
static PROCESS_CACHES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn enable_process_caches() { PROCESS_CACHES.store(true, std::sync::atomic::Ordering::Relaxed); }
pub(crate) fn process_caches() -> bool { PROCESS_CACHES.load(std::sync::atomic::Ordering::Relaxed) }

pub struct Ledger {
    db: Connection,
    /// Verified views reused within this connection. SQLite changes outside
    /// report storage invalidate the cache, including direct integrity tests.
    view_cache: RefCell<BTreeMap<String, ((u64,u64), View)>>,
    /// Unverified retained views, invalidated by the same stamp.
    retained_cache: RefCell<BTreeMap<String, ((u64,u64), View)>>,
    report_changes: Cell<u64>,
}
/// Retained Git commit metadata for evolution metrics: `files_total` is the
/// commit's full changed-file count; `changes` holds the retained ones.
#[derive(Debug, Clone)]
pub struct HistoricalCommit {
    pub oid: String,
    pub parents: Vec<String>,
    pub timestamp: i64,
    pub files_total: usize,
    /// Retained commit message (narrative text, never Evidence).
    pub message: String,
    pub changes: Vec<HistoricalChange>,
}
#[derive(Debug, Clone)]
pub struct HistoricalChange {
    pub path: String,
    /// Git's rename or copy source; not canonical lineage.
    pub old_path: Option<String>,
    pub status: String,
}
#[derive(Debug, Clone)]
pub struct View {
    pub id: String,
    pub generation: u64,
    pub bundle: Value,
    pub diagnostics: Value,
    pub projections: Vec<Value>,
    pub eligible: BTreeSet<String>,
    pub stale: bool,
    pub invalidated: bool,
    pub retraction_watermark: u64,
    pub(crate) overlay_watermark: u64,
    /// Publication scope: `source`, `deployment`, `runtime` or `session`.
    pub scope: String,
}
mod publish;
mod recall_history;
pub(crate) mod recovery;
mod reports;
mod sessions;
mod storage;
mod views;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::architecture::{
        capture::Capture,
        query,
        rust::{self, ScanOptions},
    };
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };
    fn prepared(generation: u64) -> Prepared {
        let fixture=tempfile::tempdir().unwrap();
        let root=fixture.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"crash_test\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").unwrap();
        std::fs::write(root.join("src/lib.rs"),"mod store;\nuse store::save;\npub fn entry(){save();}\n").unwrap();
        std::fs::write(root.join("src/store.rs"),"pub fn save(){}\n").unwrap();
        rust::produce(
            Capture::read(&root, None).unwrap(),
            "crash-test",
            generation,
            &ScanOptions {
                analyzer: None,
                ..Default::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn freshness_capture_state_is_memoized_without_reparsing_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(&dir.path().join("m.db")).unwrap();
        let g = l.begin("memo-test", None).unwrap();
        let view = l.commit(g, &prepared(g)).unwrap();
        let memo = |l: &Ledger| -> Option<Option<String>> {
            l.db.query_row("SELECT capture_state FROM view_capture_states WHERE view_id=?", [&view], |r| r.get(0)).optional().unwrap()
        };
        assert_eq!(memo(&l), Some(Some("complete".into())));
        assert_eq!(l.view_freshness(&view).unwrap()["capture_state"], "complete");
        // A view published before the memo existed is extracted once and then memoized.
        l.db.execute("DELETE FROM view_capture_states", []).unwrap();
        assert_eq!(l.view_freshness(&view).unwrap()["capture_state"], "complete");
        assert_eq!(memo(&l), Some(Some("complete".into())));
        // The memo, not the bundle, answers later reads.
        l.db.execute("UPDATE view_capture_states SET capture_state='partial'", []).unwrap();
        assert_eq!(l.view_freshness(&view).unwrap()["capture_state"], "partial");
        // A read-only reader of a database without the memo table still answers from the bundle.
        l.db.execute_batch("DROP TABLE view_capture_states").unwrap();
        drop(l);
        let reader = Ledger::read_only(&dir.path().join("m.db")).unwrap();
        assert_eq!(reader.view_freshness(&view).unwrap()["capture_state"], "complete");
    }
    #[test]
    fn crash_worker() {
        let Some(db) = std::env::var_os("BRIEFCASE_CRASH_DB") else {
            return;
        };
        let ready = std::env::var_os("BRIEFCASE_CRASH_READY").unwrap();
        let stage = std::env::var("BRIEFCASE_CRASH_STAGE").unwrap();
        let mut l = Ledger::open(Path::new(&db)).unwrap();
        let g = l.begin("crash-test", None).unwrap();
        let p = prepared(g);
        if stage == "uncommitted" {
            let tx =
                l.db.transaction_with_behavior(TransactionBehavior::Immediate)
                    .unwrap();
            for t in contracts::TABLES {
                for r in rows(&p.bundle, t) {
                    tx.execute(
                        "INSERT INTO records(id,family,body) VALUES(?,?,?)",
                        params![text(r, "id"), t, r.to_string()],
                    )
                    .unwrap();
                }
            }
            tx.execute("INSERT INTO events(kind,body) VALUES('commit','{}')", [])
                .unwrap();
            std::fs::write(ready, "ready").unwrap();
            std::thread::sleep(Duration::from_secs(60));
            tx.commit().unwrap();
        } else {
            l.commit(g, &p).unwrap();
            std::fs::write(ready, "ready").unwrap();
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    #[test]
    fn killed_process_rolls_back_half_commit_and_recovers_committed_inputs() {
        for stage in ["uncommitted", "committed"] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("k.db");
            let ready = dir.path().join("ready");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "architecture::ledger::tests::crash_worker",
                    "--nocapture",
                ])
                .env("BRIEFCASE_CRASH_DB", &db)
                .env("BRIEFCASE_CRASH_READY", &ready)
                .env("BRIEFCASE_CRASH_STAGE", stage)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready.exists() {
                assert!(
                    Instant::now() < deadline,
                    "crash worker failed to reach {stage}"
                );
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "crash worker exited prematurely"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            let mut l = Ledger::open(&db).unwrap();
            l.recover().unwrap();
            let count: u64 =
                l.db.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
                    .unwrap();
            if stage == "uncommitted" {
                assert_eq!(count, 0);
                assert_eq!(l.watermark().unwrap(), 0);
                assert!(l.resolve("latest").is_err());
            } else {
                assert!(count > 0);
                assert_eq!(l.resolve("latest").unwrap(), "view:1");
                assert!(!l.load("latest").unwrap().stale);
                assert!(query::dependencies(&l, &Default::default()).is_ok());
            }
        }
    }
    #[test]
    fn projection_fault_and_index_loss_never_erase_ledger_or_advertise_complete() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("k.db");
        let mut l = Ledger::open(&db).unwrap();
        let g = l.begin("crash-test", None).unwrap();
        let p = prepared(g);
        let id = l.commit(g, &p).unwrap();
        l.db.execute_batch("CREATE TRIGGER fail_projection BEFORE INSERT ON projections BEGIN SELECT RAISE(ABORT,'simulated disk write failure'); END;").unwrap();
        assert!(l.publish(&id, true).is_err());
        assert_eq!(l.load(&id).unwrap().projections.len(), 0);
        assert_eq!(l.watermark().unwrap(), 1);
        drop(l);
        let mut l = Ledger::open(&db).unwrap();
        l.db.execute_batch("DROP TRIGGER fail_projection;").unwrap();
        l.recover().unwrap();
        assert!(!l.load("latest").unwrap().stale);
        let sources = l.payloads(&p.bundle).unwrap();
        assert_eq!(sources, p.payloads);
        l.db.execute_batch("DELETE FROM graph; DELETE FROM search;")
            .unwrap();
        assert!(l.load("latest").unwrap().stale);
        assert!(query::dependencies(&l, &Default::default()).is_err());
        l.recover().unwrap();
        assert!(!l.load("latest").unwrap().stale);
        assert_eq!(l.payloads(&p.bundle).unwrap(), sources);
    }

    #[test]
    fn pending_recovery_resumes_commits_without_auditing_published_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(&dir.path().join("k.db")).unwrap();
        let g = l.begin("crash-test", None).unwrap();
        let old = l.commit(g, &prepared(g)).unwrap();
        l.publish(&old, true).unwrap();
        let g = l.begin("crash-test", None).unwrap();
        let current = l.commit(g, &prepared(g)).unwrap();
        l.db.execute(
            "DELETE FROM graph WHERE projection_id IN (SELECT id FROM projections WHERE view_id=?)",
            [&old],
        )
        .unwrap();
        let recovered = l
            .recover_pending(Instant::now() + Duration::from_secs(10))
            .unwrap();
        assert_eq!(recovered, vec![current.clone()]);
        assert_eq!(l.resolve("latest").unwrap(), current);
        assert!(!l.load("latest").unwrap().stale);
        assert!(l.load(&old).unwrap().stale);
        assert!(query::dependencies(
            &l,
            &query::Options {
                view: old.clone(),
                ..Default::default()
            }
        )
        .is_err());
        // Explicit recovery retains its full integrity/repair semantics.
        l.recover().unwrap();
        assert!(!l.load(&old).unwrap().stale);
        assert_eq!(l.resolve("latest").unwrap(), current);
    }

    #[test]
    fn expired_recovery_budget_leaves_interrupted_work_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(&dir.path().join("k.db")).unwrap();
        let g = l.begin("crash-test", None).unwrap();
        let id = l.commit(g, &prepared(g)).unwrap();
        let interrupted = l.begin("crash-test", None).unwrap();
        let error = l.recover_pending(Instant::now()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(l.status().unwrap()["scans"][1]["status"], "preparing");
        let recovered = l
            .recover_pending(Instant::now() + Duration::from_secs(10))
            .unwrap();
        assert_eq!(recovered, vec![id.clone()]);
        assert!(
            l.resolve("latest").is_err(),
            "an older commit must not replace newer intent"
        );
        assert!(!l.load(&id).unwrap().stale);
        assert_eq!(l.status().unwrap()["scans"][1]["generation"], interrupted);
        assert_eq!(l.status().unwrap()["scans"][1]["status"], "interrupted");
    }
}
