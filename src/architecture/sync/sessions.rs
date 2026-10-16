//! Exact-cwd transcript discovery shared with explicit session imports.
use crate::architecture::{
    identity,
    ledger::Ledger,
    reconcile::Alignment,
    session::{self, Harness, Retention},
    Result,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The local history directory for one harness, via the registry.
pub fn directory(harness: Harness) -> Result<PathBuf> {
    harness.default_dir()
}
pub fn check(root: &Path, db: &Path, harness: Harness) -> Result<Alignment> {
    let dir = directory(harness)?;
    check_directory(root, db, harness, &dir)
}
pub fn check_directory(root: &Path, db: &Path, harness: Harness, dir: &Path) -> Result<Alignment> {
    let source = format!("sessions:{}", harness.name());
    if !dir.exists() {
        let mut a = Alignment::new(&source, None, None);
        a.detail = json!({"status":"not_detected","directory":dir});
        return Ok(a);
    }
    let discovered = session::discover_local(harness, dir, &root.to_string_lossy())?;
    let publications = if db.is_file() {
        Ledger::read_only(db)?.session_publications()?
    } else {
        Vec::new()
    };
    let profile = session::profile::load(harness.profile())?;
    let mut observed = Vec::new();
    let mut applied = Vec::new();
    let mut pending = Vec::new();
    for path in &discovered.matched {
        // Freshness compares the materialized bytes, the same bytes
        // `import_local` digests into `raw_digest`.
        let digest = identity::bytes_digest(&harness.materialize(path)?.bytes);
        observed.push(json!([path, digest, profile.digest, "cited"]));
        let imported = publications.iter().any(|p| {
            let c = &p["context"];
            c["raw_digest"] == digest
                && c["profile"]["digest"] == profile.digest
                && c["repository_root"] == root.to_string_lossy().as_ref()
                && c["retention"] == "cited"
        });
        if imported {
            applied.push(json!([path, digest, profile.digest, "cited"]));
        } else {
            pending.push(path);
        }
    }
    let mut a = Alignment::new(
        &source,
        Some(identity::digest(&json!(observed))?),
        Some(identity::digest(&json!(applied))?),
    );
    a.detail = json!({"directory":dir,"matched":discovered.matched.len(),"pending":pending.len(),
        "skipped_other_repositories":discovered.skipped,"malformed_headers":discovered.malformed_headers});
    Ok(a)
}
pub fn apply(ledger: &mut Ledger, root: &Path, harness: Harness) -> Result<Value> {
    let dir = directory(harness)?;
    if !dir.exists() {
        return Ok(json!({"status":"not_detected"}));
    }
    let report = session::import_local(
        ledger,
        harness,
        &dir,
        &root.to_string_lossy(),
        Retention::Cited,
        false,
    )?;
    let imported = report["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["result"]["status"] == "published")
        .count();
    // Background output stays bounded even for stores with thousands of revisions.
    Ok(
        json!({"status":if imported > 0 {"refreshed"} else {"up-to-date"},
        "harness":harness.name(),"matched":report["matched"],"imported":imported,
        "skipped_other_repositories":report["skipped_other_repositories"],
        "malformed_headers":report["malformed_headers"],"retention":"cited"}),
    )
}
