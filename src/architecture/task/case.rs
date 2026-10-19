//! The retained authority for a task. Briefs are disposable projections of it.
use super::*;
use super::{
    completeness::{self, Completeness, Gap, Outcome},
    request::Request,
    trace::{Seed, Trace},
};

pub(super) struct LedgerState {
    pub watermark: u64,
    pub pending_source: bool,
    pub retraction_epoch: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Coverage {
    pub completeness: Completeness,
    #[serde(flatten)]
    pub detail: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Case {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) id: String,
    schema_version: String,
    kind: String,
    method: String,
    request: Request,
    task: String,
    duty: Duty,
    outcome: Outcome,
    outcome_scope: String,
    view_id: String,
    index_id: String,
    pinned_views: Vec<String>,
    anchors: Seed,
    slice: BTreeMap<String, Vec<Value>>,
    neighborhood: Vec<Value>,
    history: Vec<Value>,
    sessions: Vec<Value>,
    errors: Vec<Value>,
    joins: Vec<Value>,
    cochange: Vec<Value>,
    external_touchpoints: Vec<Value>,
    lexical_candidates: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    candidates: Vec<retrieval::Candidate>,
    /// Shared engineering findings (schema 1.1). Absent in 1.0 cases, where
    /// it means "not evaluated", never "none exist"; skipped on serialization
    /// when empty/not evaluated so retained 1.0 identities stay valid.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    findings: Vec<Value>,
    #[serde(
        default = "default_findings_status",
        skip_serializing_if = "is_not_evaluated"
    )]
    findings_status: String,
    not_covered: Vec<Gap>,
    coverage: Coverage,
    readiness: Value,
    provenance: Value,
}

fn default_findings_status() -> String {
    "not_evaluated".into()
}
fn is_not_evaluated(value: &str) -> bool {
    value == "not_evaluated"
}

/// Pure in-memory assembly. Ledger state is captured by the caller.
pub(super) fn assemble(
    r: &Request,
    s: &Snapshot,
    ix: &Index,
    t: &Trace,
    policy: &CasePolicy,
    ledger: LedgerState,
) -> Result<Case> {
    let (mut neighborhood, mut external) = (vec![], vec![]);
    let traced: BTreeSet<_> = t.links.iter().cloned().collect();
    for edge in ix.links.iter().filter(|e| traced.contains(&e.id)) {
        let row = serde_json::to_value(edge)?;
        if [
            "READS_CONFIG",
            "REFERENCES_TOUCHPOINT",
            "DECLARES_CONFIG_KEY",
            "INSTALLS_TO",
        ]
        .contains(&edge.predicate.as_str())
        {
            external.push(row);
        } else {
            neighborhood.push(row);
        }
    }
    let mut slice = serde_json::Map::new();
    for role in source::ARTIFACT_ROLES {
        slice.insert((*role).into(), json!([]));
    }
    let mut selected: Vec<_> = t.candidates.iter().filter(|c| c.selected).collect();
    selected.sort_by_key(|c| c.delivery_rank.unwrap_or(c.rank));
    let subject_paths: Vec<String> = selected.iter().map(|c| c.path.clone()).collect();
    for candidate in selected {
        let path = &candidate.path;
        let Some(file) = s.files.get(path) else {
            continue;
        };
        let mut row = serde_json::to_value(file)?;
        row["selection"] = json!(if t.seed.status == trace::AnchorStatus::Resolved
            && t.seed.paths.contains(path)
        {
            "resolved_anchor"
        } else {
            "ranked_candidate"
        });
        row["rank"] = json!(candidate.delivery_rank.unwrap_or(candidate.rank));
        row["score"] = json!(candidate.score);
        row["candidate_rank"] = json!(candidate.rank);
        row["candidate_ref"] = json!(format!("/candidates/{}", candidate.rank - 1));
        row["selection_reasons"] = json!(candidate
            .signals
            .iter()
            .map(|s| json!({"facet":s.facet,"kind":s.kind,
            "strength":s.strength,"confidence":s.confidence,"via_link":s.via_link}))
            .collect::<Vec<_>>());
        row["spans"] = json!(candidate.spans);
        row["reached_by"] = serde_json::to_value(
            t.reached_by
                .get(path)
                .ok_or("TASK: Missing reach provenance")?,
        )?;
        row["text_is_untrusted_repository_content"] = json!(true);
        slice
            .get_mut(&file.role)
            .ok_or("TASK: Unknown artifact role")?
            .as_array_mut()
            .unwrap()
            .push(row);
    }
    // Findings are assembled with the selected slice: each cites retained
    // link IDs and witnesses resolvable through the pinned index `explain`.
    // The `behavior` facet opts out like every other facet, so outcome
    // trials can compare the hybrid with and without findings on one slice.
    let (mut findings, mut findings_diag) = if r.enabled("behavior") {
        findings::build(ix, &s.view.id, &subject_paths, &|path| {
            s.content(path).ok().flatten().map(str::to_owned)
        })
    } else {
        (Vec::new(), json!({"status":"disabled_by_request"}))
    };
    // Session findings ride the same collection, selection accounting and
    // delivery as behavior findings; current evidence stays ahead of
    // historical advisories in the ordering.
    findings.extend(t.joins.session_findings.iter().cloned());
    findings_diag["session"] = t.joins.session_findings_diag.clone();
    // Convergence advisories come last so bounded delivery trims them first;
    // unrelated tasks are never burdened with cohort cleanup.
    if r.enabled("convergence") {
        // Only the task's own anchors are compared; slice neighbors pulled in
        // by ranking are context, not subjects to audit.
        let anchored = t.seed.status == trace::AnchorStatus::Resolved;
        let subjects: Vec<String> = if anchored {
            subject_paths.iter().filter(|p| t.seed.paths.contains(*p)).cloned().collect()
        } else {
            subject_paths.iter().take(CONVERGENCE_UNANCHORED_SUBJECTS).cloned().collect()
        };
        let (convergence, mut diag) = convergence::build(ix, &s.view.id, &subjects, &unresolved_dependency_paths(s));
        diag["subject_scope"] = json!(if anchored {"resolved_anchors"} else {"top_ranked_selected"});
        findings.extend(convergence);
        findings_diag["convergence"] = diag;
    } else {
        findings_diag["convergence"] = json!({"status":"disabled_by_request"});
    }
    findings_diag["findings_total"] = json!(findings.len());
    let assessment = completeness::assess(s, ix, t, policy, &slice, &external);
    let mut pinned: Vec<_> = s
        .profiles
        .iter()
        .chain(s.evidence_views.iter())
        .map(|v| v.id.clone())
        .collect();
    pinned.extend(t.joins.session_views.iter().cloned());
    pinned.sort();
    pinned.dedup();
    pinned.extend(
        t.candidates
            .iter()
            .flat_map(|c| &c.signals)
            .filter_map(|s| s.provenance["session_view"].as_str())
            .map(str::to_owned),
    );
    pinned.sort();
    pinned.dedup();
    let uncovered: BTreeSet<_> = assessment
        .gaps
        .iter()
        .map(|g| String::from(g.area.clone()))
        .collect();
    let coverage = json!({"capture":inventory_summary(&s.view.diagnostics["capture"]["inventory"]),
        "rust_syntax":{"operation":"explain","arguments":{"report_id":ix.id,"ref":"/coverage/rust_syntax"}},
        "profiles":ix.coverage["languages"],"analysis_profiles":ix.coverage["analysis_profiles"],"producer":s.view.diagnostics["coverage"],
        "artifact_roles":assessment.role_coverage,"required_roles":policy.required_roles,"neighborhood_depth":t.depth,
        "producer_boundary_scope":{"outside_selected_slice":assessment.outside_boundaries,"unlocalized":assessment.unlocalized_boundaries,
            "details":index::summary(ix)["details"]},"retrieval":t.retrieval,
        "not_covered_areas":uncovered,"history_tip":t.joins.history_tip,"session_publications_considered":t.joins.sessions_considered,
        "findings":findings_diag,
        "completeness_contract":{"C1_anchors":t.seed.status,"C2_slice":"roles explicitly enumerated",
            "C3_neighborhood":"retained forward/reverse relationships with basis","C4_history":"retained Git history; live capture/HEAD comparison is advisory freshness",
            "C5_sessions":"imported recordings, no causal attribution","C6_uncovered":"explicit not_covered inventory"}});
    Ok(Case {
        id: String::new(),
        schema_version: "1.1".into(),
        kind: "case".into(),
        method: METHOD.into(),
        request: r.clone(),
        task: r.task.clone(),
        duty: r.duty,
        outcome: assessment.completeness.outcome(),
        outcome_scope: "retained_evidence".into(),
        view_id: s.view.id.clone(),
        index_id: ix.id.clone(),
        pinned_views: pinned,
        anchors: t.seed.clone(),
        slice: serde_json::from_value(json!(slice))?,
        neighborhood,
        external_touchpoints: external,
        history: t.joins.history.clone(),
        sessions: t.joins.sessions.clone(),
        errors: t.joins.errors.clone(),
        joins: t.joins.links.clone(),
        cochange: t.joins.cochange.clone(),
        lexical_candidates: t.lexical_candidates.clone(),
        candidates: t.candidates.clone(),
        findings,
        findings_status: "evaluated".into(),
        not_covered: assessment.gaps,
        coverage: Coverage {
            completeness: assessment.completeness,
            detail: coverage
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        },
        readiness: json!({"source_text":"digest_verified","graph":"not_required; eligible durable assertions used",
            "pending_source_capture":ledger.pending_source,"retraction_epoch":ledger.retraction_epoch}),
        provenance: json!({"source_view":s.view.id,"ledger_watermark":ledger.watermark,"retraction_epoch":ledger.retraction_epoch}),
    })
}

pub(super) fn retain(l: &Ledger, mut case: Case) -> Result<Case> {
    case.id.clear();
    case.id = identity::id("task-case", &serde_json::to_value(&case)?)?;
    let doc = serde_json::to_value(&case)?;
    crate::architecture::contracts::schema(&doc, "task-case", None)?;
    l.save_report(&case.id, &doc, &[])?;
    Ok(case)
}
pub(crate) fn load(l: &Ledger, id: &str) -> Result<Case> {
    require(
        id.starts_with("task-case:"),
        "TASK",
        "A retained task-case ID is required",
    )?;
    let doc = l.report(id)?;
    crate::architecture::contracts::schema(&doc, "task-case", None)?;
    let mut case: Case = serde_json::from_value(doc)?;
    require(case.id == id, "TASK", "Retained case identity mismatch")?;
    case.id.clear();
    require(
        identity::id("task-case", &serde_json::to_value(&case)?)? == id,
        "DIGEST",
        "Retained case is corrupted",
    )?;
    require(
        case.not_covered
            .iter()
            .all(|g| g.dimension == g.area.dimension()),
        "TASK",
        "Gap dimension differs from its area",
    )?;
    require(
        case.outcome == case.coverage.completeness.outcome(),
        "TASK",
        "Case outcome differs from its completeness",
    )?;
    case.id = id.into();
    Ok(case)
}

pub(crate) fn sections(doc: &Value) -> Vec<String> {
    let mut tables = source::ARTIFACT_ROLES
        .iter()
        .map(|r| format!("/slice/{r}"))
        .collect::<Vec<_>>();
    for field in [
        "neighborhood",
        "history",
        "sessions",
        "errors",
        "joins",
        "cochange",
        "external_touchpoints",
        "lexical_candidates",
        "candidates",
        "findings",
        "not_covered",
    ] {
        if doc[field].is_array() {
            tables.push(format!("/{field}"));
        }
    }
    tables
}

/// Compare complete retained documents, including legacy brief reports.
/// Dirty paths are advisory live observations supplied by the caller.
pub(crate) fn compare(initial: &Value, review: &Value, dirty: &BTreeSet<String>) -> Value {
    let paths = |doc: &Value| {
        source::ARTIFACT_ROLES
            .iter()
            .flat_map(|role| rows(&doc["slice"], role))
            .map(|row| text(row, "path"))
            .collect::<BTreeSet<_>>()
    };
    let gaps = |doc: &Value| {
        rows(doc, "not_covered")
            .iter()
            .map(|g| (g.to_string(), g.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let (before, after) = (paths(initial), paths(review));
    let (old_gaps, new_gaps) = (gaps(initial), gaps(review));
    let mut dimensions = BTreeMap::new();
    for dimension in [
        "anchors",
        "slice",
        "neighborhood",
        "history",
        "sessions",
        "gap_inventory",
        "verification",
    ] {
        let a = &initial["coverage"]["completeness"][dimension];
        let b = &review["coverage"]["completeness"][dimension];
        if a != b {
            dimensions.insert(dimension, json!({"before":a,"after":b}));
        }
    }
    json!({"initial_case_id":initial.get("case_id").unwrap_or(&initial["id"]),"case_id":review.get("case_id").unwrap_or(&review["id"]),
        "slice_added":after.difference(&before).collect::<Vec<_>>(),"slice_removed":before.difference(&after).collect::<Vec<_>>(),
        "gaps_opened":new_gaps.iter().filter(|(k,_)|!old_gaps.contains_key(*k)).map(|(_,g)|g).collect::<Vec<_>>(),
        "gaps_closed":old_gaps.iter().filter(|(k,_)|!new_gaps.contains_key(*k)).map(|(_,g)|g).collect::<Vec<_>>(),
        "dimension_deltas":dimensions,"dirty_paths_outside_initial_slice":dirty.difference(&before).collect::<Vec<_>>(),
        "findings":findings::compare(rows(initial,"findings"),rows(review,"findings")),
        "interpretation":"Advisory comparison of retained cases. Live dirty paths may predate this task; this does not establish task completion."})
}

/// Without a resolved anchor, only this many top-ranked files are compared.
const CONVERGENCE_UNANCHORED_SUBJECTS: usize = 2;

/// Source paths carrying producer-unresolved dependency references; their
/// files cannot be counted as conforming or deviating in a cohort.
fn unresolved_dependency_paths(s: &Snapshot) -> BTreeSet<String> {
    let b = &s.view.bundle;
    let artifacts: BTreeMap<String, String> = rows(b, "artifacts").iter().map(|a| (text(a, "id"), text(a, "path"))).collect();
    let refs: BTreeMap<String, String> = rows(b, "evidence_refs").iter().map(|e| (text(e, "id"), text(e, "artifact_id"))).collect();
    rows(b, "unresolved_references").iter()
        .filter(|u| u["predicate"].as_str().is_none_or(|p| p == "IMPORTS"))
        .filter_map(|u| artifacts.get(refs.get(&text(u, "evidence_ref_id"))?).cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_uses_full_cases_and_reports_opened_closed_and_changed_evidence() {
        let initial = json!({"id":"task-case:before","slice":{"impl":[{"path":"src/a.rs"},{"path":"src/b.rs"}]},
            "not_covered":[{"area":"capture","dimension":"slice","reason":"missing"}],
            "coverage":{"completeness":{"slice":"partial","history":"unknown"}}});
        let review = json!({"id":"task-case:after","slice":{"impl":[{"path":"src/b.rs"},{"path":"src/c.rs"}]},
            "not_covered":[{"area":"executed_test_coverage","dimension":"verification","reason":"not executed"}],
            "coverage":{"completeness":{"slice":"established","history":"unknown"}}});
        let dirty = BTreeSet::from(["src/b.rs".into(), "src/new.rs".into()]);
        let delta = compare(&initial, &review, &dirty);
        assert_eq!(delta["initial_case_id"], "task-case:before");
        assert_eq!(delta["case_id"], "task-case:after");
        assert_eq!(delta["slice_added"], json!(["src/c.rs"]));
        assert_eq!(delta["slice_removed"], json!(["src/a.rs"]));
        assert_eq!(delta["gaps_opened"][0]["area"], "executed_test_coverage");
        assert_eq!(delta["gaps_closed"][0]["area"], "capture");
        assert_eq!(
            delta["dimension_deltas"],
            json!({"slice":{"before":"partial","after":"established"}})
        );
        assert_eq!(
            delta["dirty_paths_outside_initial_slice"],
            json!(["src/new.rs"])
        );
    }
    #[test]
    fn unchanged_cases_have_empty_deltas() {
        let case = json!({"id":"task-case:same","slice":{"impl":[{"path":"a"}]},"not_covered":[],
            "coverage":{"completeness":{"anchors":"established"}}});
        let delta = compare(&case, &case, &BTreeSet::new());
        for field in [
            "slice_added",
            "slice_removed",
            "gaps_opened",
            "gaps_closed",
            "dirty_paths_outside_initial_slice",
        ] {
            assert_eq!(delta[field], json!([]));
        }
        assert_eq!(delta["dimension_deltas"], json!({}));
    }
}
