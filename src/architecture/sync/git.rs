//! Git history has an independent revision and never enters source analysis.
use crate::architecture::{git_history, identity, ledger::Ledger, reconcile::Alignment, Result};
use serde_json::json;
use std::path::Path;

pub fn check(root: &Path, db: &Path) -> Result<Alignment> {
    let head = git_history::head(root)?;
    let applied = if db.is_file() {
        Ledger::read_only(db)?.history_tip()?
    } else {
        None
    };
    let config = identity::digest(&git_history::configuration())?;
    let observed_revision = head
        .as_ref()
        .map(|h| identity::digest(&json!([h, config])))
        .transpose()?;
    let applied_revision = applied
        .as_ref()
        .map(|v| identity::digest(&json!([v.0, v.1])))
        .transpose()?;
    let mut alignment = Alignment::new("git", observed_revision, applied_revision);
    alignment.detail = json!({"head":head,"status":if !root.join(".git").exists() {"not_a_repository"}
        else if head.is_none() {"no_commits"} else {"available"}});
    // An empty/unborn repository is a valid local state, with no history to import.
    if head.is_none() && applied.is_none() {
        alignment.state = crate::architecture::reconcile::AlignmentState::Aligned;
    }
    Ok(alignment)
}
