//! Named queries share one pinned contract across CLI and MCP. Reports preserve explanations.
use super::{
    contracts::{self, rows, strings, text},
    identity,
    ledger::{Ledger, View},
    require, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone)]
pub struct Options {
    pub view: String,
    pub component: Option<String>,
    pub limit: usize,
    pub allow_partial: bool,
    pub historical: bool,
    pub include_structure: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            view: "latest".into(),
            component: None,
            limit: 100,
            allow_partial: false,
            historical: false,
            include_structure: false,
        }
    }
}
#[derive(Clone)]
pub(crate) struct Finding {
    pub(crate) ids: Vec<String>,
    pub(crate) conclusion: &'static str,
    pub(crate) detail: Value,
}
/// One result row of a pinned report, across all of its pages.
pub(crate) struct Row {
    pub(crate) conclusion: String,
    pub(crate) detail: Value,
    pub(crate) ids: Vec<String>,
    pub(crate) handle: String,
}
pub(crate) fn all_rows(l: &Ledger, report: &Value) -> Result<Vec<Row>> {
    let mut out = Vec::new();
    if report.is_null() {
        return Ok(out);
    }
    let retained = l.report(&text(&report["response"], "id"))?;
    let size = retained["page_size"].as_u64().unwrap_or(100) as usize;
    let mut page = finish_explain::page(l, retained, 0, size)?;
    loop {
        for r in rows(&page["response"], "results") {
            out.push(Row {
                conclusion: text(r, "conclusion"),
                detail: page["details"][text(r, "id")].clone(),
                ids: strings(&r["assertion_ids"]),
                handle: text(r, "explanation_handle"),
            });
        }
        match page["response"]["continuation_token"].as_str() {
            Some(token) => {
                let (id, offset) = token.rsplit_once('@').ok_or("PAGINATION: Invalid token")?;
                let retained = l.report(id)?;
                let size = retained["page_size"].as_u64().unwrap_or(100) as usize;
                page = finish_explain::page(l, retained, offset.parse()?, size)?;
            }
            None => return Ok(out),
        }
    }
}

pub(crate) fn context(v: &View) -> Value {
    rows(&v.bundle, "runs")[0]["context"].clone()
}
pub(crate) fn complete(v: &View) -> bool {
    complete_for(v, "IMPORTS")
}
pub(crate) fn complete_for(v: &View, predicate: &str) -> bool {
    !v.invalidated
        && rows(&v.bundle, "snapshots")
            .iter()
            .filter(|s| s["id"] == context(v)["snapshot_id"])
            .all(|s| s["capture_state"] == "complete")
        && rows(&v.bundle, "runs").iter().any(|r| {
            r["context"] == context(v)
                && r["coverage"]["resolution_complete"] == true
                && rows(&r["coverage"], "guarantees")
                    .iter()
                    .any(|g| g["predicate"] == predicate && g["kind"] == "exact_supported")
        })
}
pub(crate) fn entities(v: &View) -> BTreeMap<String, String> {
    rows(&v.bundle, "entity_versions")
        .iter()
        .map(|r| (text(r, "id"), text(r, "canonical_key")))
        .collect()
}
pub(crate) fn eligible<'a>(
    v: &'a View,
    removed: &BTreeSet<String>,
    historical: bool,
    predicate: &str,
) -> Vec<&'a Value> {
    rows(&v.bundle, "assertions")
        .iter()
        .filter(|a| {
            a["proposition"]["predicate"] == predicate
                && (a["context"] == context(v) || a["context"]["kind"] == "comparison")
                && v.eligible.contains(&text(a, "id"))
                && (historical || !removed.contains(&text(a, "id")))
                && a["class"] != "claim"
                && a["support_class"] != "includes_inference"
        })
        .collect()
}
pub(crate) fn loaded(l: &Ledger, o: &Options) -> Result<View> {
    require(
        (1..=1000).contains(&o.limit),
        "QUERY",
        "Limit must be 1..1000",
    )?;
    let v = if o.historical {
        l.load_historical(&o.view)?
    } else {
        l.load(&o.view)?
    };
    require(
        v.projections.len() == 2,
        "FRESHNESS",
        "View has no complete projection pair; recover/rebuild first",
    )?;
    require(
        o.historical || o.allow_partial || !v.stale,
        "FRESHNESS",
        "Retraction or projection lag requires rebuild, historical mode, or allow-partial",
    )?;
    if o.view == "latest" && !o.historical {
        require(
            o.allow_partial || !l.pending_source()?,
            "FRESHNESS",
            "A newer scan is pending/interrupted; answer from the last published view with `briefcase call --fallback-published <operation>` (Pi does this automatically), select a prior view explicitly, or allow-partial",
        )?;
    }
    Ok(v)
}
pub(crate) fn restrictions(l: &Ledger, v: &View, o: &Options) -> Result<Vec<String>> {
    let mut xs = strings(&v.diagnostics["limitations"]);
    if v.scope == "source" && !complete(v) {
        xs.push("Import resolution is incomplete; missing relationships cannot establish absence or policy compliance.".into());
    }
    if v.stale && !o.historical {
        xs.push(
            "Projection pair lags durable retractions; current ineligible facts were filtered."
                .into(),
        );
    }
    if v.invalidated {
        xs.push(
            "Retraction invalidated this view’s current negative/compliance coverage guarantee."
                .into(),
        );
    }
    if o.historical {
        xs.push("Historical view: original inputs are retained; current retractions are annotated in explanations.".into());
    }
    if o.view == "latest" && l.pending_source()? {
        xs.push(
            "Latest aliases the last published view while a newer scan is pending/interrupted."
                .into(),
        );
    }
    Ok(xs)
}
pub(crate) fn selected(v: &View, key: &str) -> Vec<String> {
    let exact: Vec<_> = rows(&v.bundle, "entity_versions")
        .iter()
        .filter(|e| e["id"] == key || e["canonical_key"] == key)
        .map(|e| text(e, "id"))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    // A canonical key without its `#path:offset` disambiguator, as agents
    // copy it from candidate and suggestion lists.
    let undisambiguated: Vec<_> = rows(&v.bundle, "entity_versions")
        .iter()
        .filter(|e| {
            e["canonical_key"]
                .as_str()
                .and_then(|s| s.split_once('#'))
                .is_some_and(|(stem, _)| stem == key)
        })
        .map(|e| text(e, "id"))
        .collect();
    if !undisambiguated.is_empty() {
        return undisambiguated;
    }
    rows(&v.bundle, "entity_versions")
        .iter()
        .filter(|e| {
            e["canonical_key"].as_str().is_some_and(|s| {
                s.rsplit("::").next().and_then(|s| s.split('#').next()) == Some(key)
            })
        })
        .map(|e| text(e, "id"))
        .collect()
}
/// Near matches for an unresolved component key, best first. Suffix and
/// containment matches outrank a shared final segment; the result is a
/// suggestion list for the caller to choose from, never a resolution.
pub(crate) fn suggested_keys(key: &str, canonical_keys: &[String]) -> Vec<String> {
    fn segment(text: &str) -> &str {
        text.rsplit([':', '/', '#', '.']).next().unwrap_or(text)
    }
    let path_form = key.replace("::", "/");
    let mut scored: Vec<(u8, &String)> = canonical_keys
        .iter()
        .filter_map(|candidate| {
            let score = if candidate.ends_with(&format!("::{key}"))
                || candidate.ends_with(&format!("/{key}"))
            {
                3
            } else if candidate.contains(key) || candidate.contains(&path_form) {
                2
            } else if segment(candidate) == segment(key) {
                1
            } else {
                return None;
            };
            Some((score, candidate))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then(a.cmp(b)));
    scored.dedup_by(|a, b| a.1 == b.1);
    scored
        .into_iter()
        .take(5)
        .map(|(_, candidate)| candidate.clone())
        .collect()
}

pub(crate) fn ambiguity(v: &View, key: &str) -> Option<Finding> {
    let ids = selected(v, key);
    if ids.len() == 1 {
        None
    } else {
        let es = entities(v);
        let mut detail = json!({"component":key,"resolution":if ids.is_empty(){"unresolved"}else{"ambiguous"},"candidates":ids.iter().map(|id|json!({"id":id,"canonical_key":es[id]})).collect::<Vec<_>>()});
        if ids.is_empty() {
            let keys: Vec<String> = es.values().cloned().collect();
            let suggestions = suggested_keys(key, &keys);
            if !suggestions.is_empty() {
                detail["suggestions"] = json!(suggestions
                    .iter()
                    .map(|k| json!({"canonical_key": k}))
                    .collect::<Vec<_>>());
            }
        }
        Some(Finding {
            ids: vec![],
            conclusion: "unknown",
            detail,
        })
    }
}
mod graph;
pub use graph::*;
mod history_policy;
pub use history_policy::*;
mod finish_explain;
pub use finish_explain::*;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_rank_suffix_matches_and_stay_empty_when_unrelated() {
        let keys: Vec<String> = [
            "briefcase::architecture::native_source",
            "briefcase::architecture::ledger",
            "src/architecture/native_source.rs",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let found = suggested_keys("architecture::native_source", &keys);
        assert_eq!(found[0], "briefcase::architecture::native_source");
        assert!(found.contains(&"src/architecture/native_source.rs".to_string()));
        assert!(suggested_keys("no-such-thing", &keys).is_empty());
    }

    #[test]
    fn pinned_report_reader_keeps_rows_across_pages_and_retractions() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::open(&dir.path().join("briefcase.db")).unwrap();
        assert!(all_rows(&l, &Value::Null).unwrap().is_empty());
        let mut response=json!({"schema_version":"1.0","id":"report:paging","operation":"dependencies",
            "context":{"kind":"source","snapshot_id":"snapshot:test","build_context_id":"build-context:test"},
            "view":"evidence","projection_ids":["projection:test"],"ledger_watermark":0,"outcome":"partial",
            "results":[],"resolution_complete":false,"required_predicates":["DEPENDS_ON"],"limitations":[],"truncated":false});
        response["id"] = json!("report:paging");
        response["outcome"] = json!("partial");
        response["resolution_complete"] = json!(false);
        let result: Vec<Value> = (0..5).map(|n| json!({"id":format!("row:{n}"),
            "assertion_ids":[format!("assertion:{n}")],"explanation_handle":format!("explain:row:{n}"),
            "conclusion":if n == 2 {"unknown"} else {"present"}})).collect();
        let details: BTreeMap<String, Value> = (0..5)
            .map(|n| (format!("row:{n}"), json!({"n":n})))
            .collect();
        response["results"] = json!(result);
        let report = json!({"response":response,"details":details,"page_size":2,
            "historical":true,"retraction_epoch":0});
        l.save_report("report:paging", &report, &[]).unwrap();
        let first = continuation(&l, "report:paging@0").unwrap();
        assert_eq!(rows(&first["response"], "results").len(), 2);
        let read = all_rows(&l, &first).unwrap();
        assert_eq!(read.len(), 5);
        for (n, row) in read.iter().enumerate() {
            assert_eq!(row.detail["n"], n);
            assert_eq!(row.ids, [format!("assertion:{n}")]);
            assert_eq!(row.handle, format!("explain:row:{n}"));
            assert_eq!(row.conclusion, if n == 2 { "unknown" } else { "present" });
        }
        let mut stale = report;
        stale["historical"] = json!(false);
        stale["retraction_epoch"] = json!(1);
        l.save_report("report:stale", &stale, &[]).unwrap();
        assert!(continuation(&l, "report:stale@0")
            .unwrap_err()
            .to_string()
            .contains("predates a retraction"));
    }
}
