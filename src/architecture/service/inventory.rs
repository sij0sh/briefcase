//! Scope-aware inventory. Zero retained facts does not mean analysis was skipped.
use crate::architecture::{
    contracts::{rows, text},
    ledger::{Ledger, View},
    Result,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn counts(view: &View) -> Value {
    let mut entities = BTreeMap::<String, usize>::new();
    let mut classes = BTreeMap::<String, usize>::new();
    let mut predicates = BTreeMap::<String, usize>::new();
    for entity in rows(&view.bundle, "entity_versions") {
        *entities.entry(text(entity, "kind")).or_default() += 1;
    }
    for assertion in rows(&view.bundle, "assertions") {
        *classes.entry(text(assertion, "class")).or_default() += 1;
        *predicates
            .entry(text(&assertion["proposition"], "predicate"))
            .or_default() += 1;
    }
    json!({"view_id":view.id,"scope":view.scope,"captured_files":rows(&view.bundle,"snapshots").iter().map(|s|rows(s,"files").len()).sum::<usize>(),
        "entities_by_kind":entities,"assertions_by_class":classes,"assertions_by_predicate":predicates,
        "count_scope":"retained records in this pinned view, not a claim that every fact remains eligible",
        "projection_stale":view.stale,"invalidated":view.invalidated,"coverage":view.diagnostics["coverage"]})
}

fn analysis(
    bundle: &Value,
    predicates: &[&str],
    pass: Option<&str>,
    invalidated: bool,
    requested: Option<bool>,
) -> Value {
    let runs: Vec<_> = rows(bundle, "runs")
        .iter()
        .filter(|run| {
            rows(&run["coverage"], "guarantees")
                .iter()
                .any(|g| predicates.contains(&g["predicate"].as_str().unwrap_or("")))
        })
        .collect();
    let manifests: Vec<_> = rows(bundle, "pass_manifests")
        .iter()
        .filter(|m| pass.is_some_and(|p| m["pass_name"] == p))
        .collect();
    let selected: Vec<_> = if pass.is_some() {
        manifests.clone()
    } else {
        runs.clone()
    };
    let facts = rows(bundle, "assertions")
        .iter()
        .filter(|a| predicates.contains(&a["proposition"]["predicate"].as_str().unwrap_or("")))
        .count();
    let state = if selected.is_empty() {
        if requested == Some(false) && facts == 0 {
            "not_requested"
        } else {
            "unknown"
        }
    } else if selected
        .iter()
        .any(|r| ["failed", "interrupted", "aborted"].contains(&r["status"].as_str().unwrap_or("")))
    {
        "failed"
    } else if invalidated
        || selected
            .iter()
            .any(|r| r["status"] != "complete" || r["coverage"]["resolution_complete"] == false)
    {
        "partial"
    } else if selected
        .iter()
        .all(|r| r["coverage"]["resolution_complete"] == true)
    {
        "complete"
    } else {
        "unknown"
    };
    let requested = if !selected.is_empty() {
        Some(true)
    } else {
        requested
    };
    json!({"request_state":requested.map(|r|if r {"requested"}else{"not_requested"}).unwrap_or("unknown"),
        "request_metadata_basis":"Advisory publication diagnostics; canonical run/pass coverage controls analysis completeness.","state":state,"retained_fact_count":facts,"run_ids":runs.iter().map(|r|text(r,"id")).collect::<Vec<_>>(),
        "pass_manifest_ids":manifests.iter().map(|r|text(r,"id")).collect::<Vec<_>>(),
        "coverage":selected.iter().map(|r|json!({"id":r["id"],"status":r["status"],"coverage":r["coverage"]})).collect::<Vec<_>>(),
        "interpretation":"Complete describes the recorded producer/pass scope, not universal semantic coverage or observed behavior."})
}

type Family = (&'static str, &'static str, &'static [&'static str], Option<&'static str>);
const FAMILIES: &[Family] = &[
    ("dependencies", "source", &["DEPENDS_ON"], Some("module-dependency-overlay-v1")),
    ("static_calls", "source", &["CALLS_STATIC"], None),
    ("data", "source", &["READS", "WRITES"], None),
    ("deployment", "deployment", &["BUILDS", "DEPLOYS_AS", "DECLARES_CONNECTION"], None),
    ("observed_runtime", "runtime", &["CALLS_OBSERVED", "CONNECTION_OBSERVED"], None),
];

/// Bundle-derived counts and analysis states for one pinned view. Views are
/// immutable, so the result is memoized under every other input that can
/// change it (overlays, retractions, projection rows); only a miss parses
/// and verifies the bundle.
fn view_inventory(l: &Ledger, pin: &str, scope: &str) -> Result<Value> {
    let key = l.view_summary_key(pin, true)?;
    if let Some(memo) = l.view_summary(pin, "inventory", &key)? {
        return Ok(memo);
    }
    let view = l.load(pin)?;
    let analyses: BTreeMap<_, _> = FAMILIES.iter().filter(|f| f.1 == scope).map(|&(name, _, predicates, pass)| {
        (name, analysis(&view.bundle, predicates, pass, view.invalidated,
            view.diagnostics["requested_analyses"][name].as_bool()))
    }).collect();
    let memo = json!({"view_id":view.id,"counts":counts(&view),"analysis":analyses});
    l.record_view_summary(pin, "inventory", &key, &memo);
    Ok(memo)
}

pub(super) fn summary(l: &Ledger) -> Result<Value> {
    let status = l.status()?;
    let source_sync=l.sync_state()?.map(|(key,view,profile)|json!({"input_key":key,"view_id":view,"profile":profile}));
    let mut scopes = BTreeMap::new();
    let mut views = BTreeMap::new();
    for scope in ["source", "deployment", "runtime"] {
        let publications = l.scoped_views(scope)?;
        let pin = if scope == "source" {
            status["current_view"].as_str().map(str::to_owned)
        } else {
            publications.first().cloned()
        };
        if let Some(pin) = pin {
            let view = view_inventory(l, &pin, scope)?;
            let mut inventory = view["counts"].clone();
            inventory["published_views"] = json!(publications.len());
            if let Some(scan) = rows(&status, "scans")
                .iter()
                .rev()
                .find(|s| s["scope"] == scope)
            {
                inventory["latest_attempt"] = json!({"generation":scan["generation"],"status":scan["status"],"label":scan["label"],"view_id":scan["view_id"]});
            }
            scopes.insert(scope.to_owned(), inventory);
            views.insert(scope, view);
        } else {
            let attempted: Vec<_> = rows(&status, "scans")
                .iter()
                .filter(|scan| scan["scope"] == scope)
                .collect();
            let state = if attempted.is_empty() {
                "not_requested"
            } else if attempted.iter().any(|s| s["status"] == "interrupted") {
                "failed"
            } else {
                "pending"
            };
            scopes.insert(
                scope.to_owned(),
                json!({"state":state,"published_views":publications.len(),"view_id":null}),
            );
        }
    }
    let mut analyses = BTreeMap::new();
    for &(name, scope, _, _) in FAMILIES {
        let mut entry = if let Some(view) = views.get(scope) {
            view["analysis"][name].clone()
        } else {
            json!({"state":scopes[scope]["state"],"retained_fact_count":null})
        };
        entry["scope"] = json!(scope);
        entry["view_id"] = views.get(scope).map(|v| v["view_id"].clone()).unwrap_or(Value::Null);
        analyses.insert(name.to_owned(), entry);
    }
    let mut inventory = scopes.get("source").cloned().unwrap_or(Value::Null);
    inventory["scopes"] = json!(scopes);
    inventory["analysis"] = json!(analyses);
    inventory["analysed"] = json!(analyses
        .iter()
        .map(|(name, entry)| (
            name.clone(),
            matches!(entry["state"].as_str(), Some("complete" | "partial"))
        ))
        .collect::<BTreeMap<_, _>>());
    inventory["not_analysed"] = json!(analyses
        .iter()
        .filter(|(_, entry)| entry["state"] == "not_requested")
        .map(|(name, _)| name)
        .collect::<Vec<_>>());
    inventory["unknown_analyses"] = json!(analyses
        .iter()
        .filter(|(_, entry)| entry["state"] == "unknown")
        .map(|(name, _)| name)
        .collect::<Vec<_>>());
    inventory["history_commits_reachable"] = json!(l.history_reachable_count()?);
    let publications = l.session_publications()?;
    let mut known = std::collections::BTreeSet::new();
    let mut unidentified = 0;
    for publication in &publications {
        let context = &publication["context"];
        if let Some(id) = context["external_session_id"].as_str() {
            known.insert((
                text(context, "repository_id"),
                text(&context["harness"], "name"),
                id.to_owned(),
            ));
        } else {
            unidentified += 1;
        }
    }
    inventory["sessions_imported"] = json!(known.len());
    inventory["session_publications"] = json!(publications.len());
    inventory["session_count_basis"]=json!("Distinct retained repository, harness name and external session ID. Retention/profile interpretations are separate publications.");
    inventory["session_publications_without_external_id"] = json!(unidentified);
    let pins: Vec<_> = views.values().map(|v| v["view_id"].clone()).collect();
    Ok(
        json!({"operation":"status","summary":true,"repository":status["repository"],"current_view":status["current_view"],
        "pending_or_interrupted":status["pending_or_interrupted"],"ledger_watermark":status["ledger_watermark"],"source_sync":source_sync,
        "retracted_count":status["retracted_count"],"inventory":inventory,"layers":crate::architecture::task::readiness(l)?,"pinned_views":pins}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_results_complete_partial_missing_and_failed_are_distinct() {
        let complete = json!({"runs":[{"id":"run:1","status":"complete","coverage":{"resolution_complete":true,"guarantees":[{"predicate":"CALLS_STATIC","kind":"exact_supported"}]}}]});
        assert_eq!(
            analysis(&complete, &["CALLS_STATIC"], None, false, None)["state"],
            "complete"
        );
        assert_eq!(
            analysis(&complete, &["CALLS_STATIC"], None, false, None)["retained_fact_count"],
            0
        );
        assert_eq!(
            analysis(&complete, &["READS"], None, false, Some(false))["state"],
            "not_requested"
        );
        assert_eq!(
            analysis(&complete, &["CALLS_STATIC"], None, true, None)["state"],
            "partial"
        );
        let mut failed = complete.clone();
        failed["runs"][0]["status"] = json!("failed");
        assert_eq!(
            analysis(&failed, &["CALLS_STATIC"], None, false, None)["state"],
            "failed"
        );
        let empty_graph = json!({"pass_manifests":[{"id":"pass:1","pass_name":"module-dependency-overlay-v1","status":"complete","coverage":{"resolution_complete":true}}]});
        assert_eq!(
            analysis(
                &empty_graph,
                &["DEPENDS_ON"],
                Some("module-dependency-overlay-v1"),
                false,
                None
            )["state"],
            "complete"
        );
        // Assertions without retained producer coverage cannot prove completion.
        let unknown = json!({"assertions":[{"proposition":{"predicate":"READS"}}]});
        assert_eq!(
            analysis(&unknown, &["READS"], None, false, None)["state"],
            "unknown"
        );
    }
}
