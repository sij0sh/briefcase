//! Retained evidence → request → trace → case → brief. Each relationship
//! preserves its assertion or byte basis; presentation never becomes authority.
mod agent;
pub(crate) mod brief;
pub(crate) mod case;
mod completeness;
mod duty;
mod evidence;
mod request;
mod retrieval;
mod trace;
pub use duty::{CasePolicy, Duty, DutyPolicy, TracePolicy};
use evidence::Snapshot;
pub use evidence::{Artifact, Link, Witness};
mod convergence;
mod findings;
mod index;
mod joins;
mod producers;
mod queries;
mod worktree;
pub use worktree::worktree;

pub use agent::agent_context;
pub(crate) use brief::table_paths as report_sections;
pub use brief::{brief, page};
pub(crate) use index::precompute;
pub use findings::compare as compare_findings;
pub use queries::{explain, neighbors, occurrences};

use crate::architecture::{
    contracts::{rows, text, Payloads},
    identity,
    ledger::{Ledger, View},
    require, source, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub fn retrieval_facets() -> Vec<String> {
    retrieval::FACETS.iter().map(|s| (*s).into()).collect()
}
pub fn selection_variants() -> Vec<String> {
    request::SELECTION_VARIANTS
        .iter()
        .map(|s| (*s).into())
        .collect()
}

pub const METHOD: &str = "task-context-v1";

pub(super) fn inventory_summary(inventory: &Value) -> Value {
    if !inventory.is_object() {
        return json!({"state":"unknown"});
    }
    let mut summary = inventory.clone();
    for field in ["unretained_tracked_paths", "exclusions", "diagnostics"] {
        let count = rows(inventory, field).len();
        summary.as_object_mut().unwrap().remove(field);
        summary[format!("{field}_count")] = json!(count);
    }
    summary
}

/// Independent availability layers. A missing semantic or runtime layer never
/// suppresses retained text; availability is not a freshness guarantee.
pub fn readiness(l: &Ledger) -> Result<Value> {
    let source = l
        .resolve("latest")
        .ok()
        .map(|id| l.retained(&id))
        .transpose()?;
    let source_state=source.as_ref().map(|v|json!({"state":"retained","view_id":v.id,
        "capture":v.diagnostics["capture"],"file_count":rows(&v.bundle,"snapshots").iter().map(|s|rows(s,"files").len()).sum::<usize>()}))
        .unwrap_or(json!({"state":"unavailable"}));
    let semantic = |predicate: &str| {
        let runs: Vec<_> = source
            .as_ref()
            .into_iter()
            .flat_map(|v| rows(&v.bundle, "runs"))
            .filter(|r| {
                rows(&r["coverage"], "guarantees")
                    .iter()
                    .any(|g| g["predicate"] == predicate)
            })
            .collect();
        json!({"state":if runs.is_empty() {"not_run"} else if runs.iter().all(|r|r["coverage"]["resolution_complete"] == true) {"complete_for_producer_scope"} else {"partial"},
            "runs":runs.iter().map(|r|json!({"id":r["id"],"coverage":r["coverage"]})).collect::<Vec<_>>()})
    };
    let history = l.history_tip()?;
    let sessions = l.session_publications()?;
    Ok(
        json!({"source_text":source_state,"parsed_source":semantic("DECLARES"),"static_analysis":semantic("CALLS_STATIC"),
        "history":{"state":if history.is_some() {"retained"} else {"not_run"},"tip":history.map(|h|h.0)},
        "sessions":{"state":if sessions.is_empty() {"not_imported"} else {"retained"},"publications":sessions.len()},
        "deployment":{"publications":l.scoped_views("deployment")?.len()},"runtime":{"publications":l.scoped_views("runtime")?.len()},
        "degraded_operations":["brief","search","occurrences","neighbors","page"],
        "interpretation":"Each layer has independent coverage. Retained data can lag live files, HEAD, sessions or runtime."}),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entity {
    key: String,
    kind: String,
    names: BTreeSet<String>,
    files: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Index {
    id: String,
    source_view: String,
    retraction_epoch: u64,
    files: BTreeMap<String, Artifact>,
    entities: BTreeMap<String, Entity>,
    links: Vec<Link>,
    coverage: Value,
}

fn page_items(kind: &str, view: &str, items: &[Value], offset: usize, limit: usize) -> Value {
    let end = offset.saturating_add(limit).min(items.len());
    let returned = items.get(offset..end).unwrap_or(&[]);
    json!({"kind":kind,"view_id":view,"items":returned,"page":{"offset":offset,"returned":returned.len(),
        "total":items.len(),"remaining":items.len().saturating_sub(end),"next_offset":(end < items.len()).then_some(end)},
        "ordering":"deterministic, exhaustive within the stated retained scope; not ranked"})
}

/// Validate all brief inputs before a path-based gateway opens a ledger.
pub(crate) fn validate_request(a: &Value) -> Result<()> {
    request::parse(a).map(|_| ())
}
