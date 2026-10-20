//! Case policy, typed gaps and completeness for the retained evidence scope.
use super::trace::{AnchorStatus, Boundary, Trace};
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Dimension {
    Anchors,
    Slice,
    Neighborhood,
    History,
    Sessions,
    GapInventory,
    Verification,
}
impl Dimension {
    fn name(self) -> &'static str {
        match self {
            Self::Anchors => "anchors",
            Self::Slice => "slice",
            Self::Neighborhood => "neighborhood",
            Self::History => "history",
            Self::Sessions => "sessions",
            Self::GapInventory => "gap_inventory",
            Self::Verification => "verification",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(super) enum GapArea {
    Anchors,
    ArtifactRole(String),
    CandidateSelection,
    Capture,
    CapturePath,
    CaptureExclusion,
    CaptureDiagnostic,
    RelationshipIndex,
    RelationshipPropagation,
    SemanticAnalysis,
    SemanticBinding,
    StaticCalls,
    LanguageProfile,
    ProducerBoundary,
    ProfileSnapshot,
    DeploymentSource,
    DeploymentPublications,
    RuntimeIdentity,
    RuntimeSelection,
    RuntimePublications,
    RuntimeObservationScope,
    History,
    Sessions,
    SessionCommitJoin,
    ProducerBoundaryScope,
    ExecutedTestCoverage,
    PlanOutcomes,
    ExternalInstallations,
}
impl GapArea {
    pub fn dimension(&self) -> Dimension {
        match self {
            Self::Anchors => Dimension::Anchors,
            Self::ArtifactRole(_)
            | Self::CandidateSelection
            | Self::Capture
            | Self::CapturePath
            | Self::CaptureExclusion
            | Self::CaptureDiagnostic => Dimension::Slice,
            Self::RelationshipIndex
            | Self::RelationshipPropagation
            | Self::SemanticAnalysis
            | Self::SemanticBinding
            | Self::StaticCalls
            | Self::LanguageProfile
            | Self::ProducerBoundary
            | Self::ProfileSnapshot
            | Self::DeploymentSource
            | Self::DeploymentPublications
            | Self::RuntimeIdentity
            | Self::RuntimeSelection
            | Self::RuntimePublications
            | Self::RuntimeObservationScope => Dimension::Neighborhood,
            Self::History => Dimension::History,
            Self::Sessions | Self::SessionCommitJoin => Dimension::Sessions,
            Self::ProducerBoundaryScope => Dimension::GapInventory,
            Self::ExecutedTestCoverage | Self::PlanOutcomes | Self::ExternalInstallations => {
                Dimension::Verification
            }
        }
    }
}
impl From<GapArea> for String {
    fn from(area: GapArea) -> String {
        match area {
            GapArea::Anchors => "anchors",
            GapArea::ArtifactRole(role) => return format!("artifact_role:{role}"),
            GapArea::CandidateSelection => "candidate_selection",
            GapArea::Capture => "capture",
            GapArea::CapturePath => "capture_path",
            GapArea::CaptureExclusion => "capture_exclusion",
            GapArea::CaptureDiagnostic => "capture_diagnostic",
            GapArea::RelationshipIndex => "relationship_index",
            GapArea::RelationshipPropagation => "relationship_propagation",
            GapArea::SemanticAnalysis => "semantic_analysis",
            GapArea::SemanticBinding => "semantic_binding",
            GapArea::StaticCalls => "static_calls",
            GapArea::LanguageProfile => "language_profile",
            GapArea::ProducerBoundary => "producer_boundary",
            GapArea::ProfileSnapshot => "profile_snapshot",
            GapArea::DeploymentSource => "deployment_source",
            GapArea::DeploymentPublications => "deployment_publications",
            GapArea::RuntimeIdentity => "runtime_identity",
            GapArea::RuntimeSelection => "runtime_selection",
            GapArea::RuntimePublications => "runtime_publications",
            GapArea::RuntimeObservationScope => "runtime_observation_scope",
            GapArea::History => "history",
            GapArea::Sessions => "sessions",
            GapArea::SessionCommitJoin => "session_commit_join",
            GapArea::ProducerBoundaryScope => "producer_boundary_scope",
            GapArea::ExecutedTestCoverage => "executed_test_coverage",
            GapArea::PlanOutcomes => "plan_outcomes",
            GapArea::ExternalInstallations => "external_installations",
        }
        .into()
    }
}
impl TryFrom<String> for GapArea {
    type Error = String;
    fn try_from(area: String) -> std::result::Result<Self, String> {
        Ok(match area.as_str() {
            "anchors" => Self::Anchors,
            "candidate_selection" => Self::CandidateSelection,
            "capture" => Self::Capture,
            "capture_path" => Self::CapturePath,
            "capture_exclusion" => Self::CaptureExclusion,
            "capture_diagnostic" => Self::CaptureDiagnostic,
            "relationship_index" => Self::RelationshipIndex,
            "relationship_propagation" => Self::RelationshipPropagation,
            "semantic_analysis" => Self::SemanticAnalysis,
            "semantic_binding" => Self::SemanticBinding,
            "static_calls" => Self::StaticCalls,
            "language_profile" => Self::LanguageProfile,
            "producer_boundary" => Self::ProducerBoundary,
            "profile_snapshot" => Self::ProfileSnapshot,
            "deployment_source" => Self::DeploymentSource,
            "deployment_publications" => Self::DeploymentPublications,
            "runtime_identity" => Self::RuntimeIdentity,
            "runtime_selection" => Self::RuntimeSelection,
            "runtime_publications" => Self::RuntimePublications,
            "runtime_observation_scope" => Self::RuntimeObservationScope,
            "history" => Self::History,
            "sessions" => Self::Sessions,
            "session_commit_join" => Self::SessionCommitJoin,
            "producer_boundary_scope" => Self::ProducerBoundaryScope,
            "executed_test_coverage" => Self::ExecutedTestCoverage,
            "plan_outcomes" => Self::PlanOutcomes,
            "external_installations" => Self::ExternalInstallations,
            _ => {
                if let Some(role) = area
                    .strip_prefix("artifact_role:")
                    .filter(|r| source::ARTIFACT_ROLES.contains(r))
                {
                    Self::ArtifactRole(role.into())
                } else {
                    return Err(format!("TASK: Unknown gap area {area}"));
                }
            }
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Gap {
    pub area: GapArea,
    pub dimension: Dimension,
    pub reason: Value,
    #[serde(flatten)]
    pub detail: BTreeMap<String, Value>,
}
impl Gap {
    fn new(area: GapArea, detail: Value) -> Self {
        let mut detail: BTreeMap<String, Value> = detail
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        detail.remove("area");
        detail.remove("dimension");
        let reason = detail.remove("reason").unwrap_or(Value::Null);
        Self {
            dimension: area.dimension(),
            area,
            reason,
            detail,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State {
    Established,
    Partial,
    Unknown,
    NotRequired,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Completeness {
    pub anchors: State,
    pub slice: State,
    pub neighborhood: State,
    pub history: State,
    pub sessions: State,
    pub gap_inventory: State,
    pub verification: State,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Outcome {
    Unknown,
    Complete,
    Partial,
}
impl Completeness {
    pub fn derive(
        status: AnchorStatus,
        gaps: &[Gap],
        policy: &CasePolicy,
        history_available: bool,
        sessions_available: bool,
    ) -> Self {
        use State::*;
        let has = |d| gaps.iter().any(|g| g.dimension == d);
        let evidence = |d| if has(d) { Partial } else { Established };
        Self {
            anchors: match status {
                AnchorStatus::Resolved => Established,
                AnchorStatus::Ambiguous => Partial,
                AnchorStatus::Unresolved => Unknown,
            },
            slice: if status == AnchorStatus::Unresolved {
                Unknown
            } else {
                evidence(Dimension::Slice)
            },
            neighborhood: evidence(Dimension::Neighborhood),
            history: if !policy.history_required {
                NotRequired
            } else if !history_available {
                Unknown
            } else {
                evidence(Dimension::History)
            },
            sessions: if !policy.sessions_required {
                NotRequired
            } else if !sessions_available {
                Unknown
            } else {
                evidence(Dimension::Sessions)
            },
            gap_inventory: evidence(Dimension::GapInventory),
            verification: if has(Dimension::Verification) {
                Unknown
            } else {
                NotRequired
            },
        }
    }
    pub fn outcome(&self) -> Outcome {
        if self.anchors == State::Unknown {
            return Outcome::Unknown;
        }
        if [
            self.anchors,
            self.slice,
            self.neighborhood,
            self.history,
            self.sessions,
            self.gap_inventory,
            self.verification,
        ]
        .iter()
        .all(|s| matches!(s, State::Established | State::NotRequired))
        {
            Outcome::Complete
        } else {
            Outcome::Partial
        }
    }
}

fn boundary_gap(boundary: &Boundary, p: &CasePolicy) -> Option<Gap> {
    let (area, detail) = match boundary {
        Boundary::NoHistory if p.history_required => (
            GapArea::History,
            json!({"reason":"No retained Git history tip; sync Git history."}),
        ),
        Boundary::PartialHistory(v) if p.history_required => (GapArea::History, v.clone()),
        Boundary::NoHistory | Boundary::PartialHistory(_) => return None,
        Boundary::NoSessions if p.sessions_required => (
            GapArea::Sessions,
            json!({"reason":"No imported session recordings."}),
        ),
        Boundary::MetadataSession(v)
        | Boundary::RetractedSession(v)
        | Boundary::PartialSession(v)
            if p.sessions_required =>
        {
            (GapArea::Sessions, v.clone())
        }
        Boundary::IndirectCommit(v) if p.sessions_required => {
            (GapArea::SessionCommitJoin, v.clone())
        }
        Boundary::NoSessions
        | Boundary::MetadataSession(_)
        | Boundary::RetractedSession(_)
        | Boundary::PartialSession(_)
        | Boundary::IndirectCommit(_) => return None,
        Boundary::ProfileSnapshot(v) => (GapArea::ProfileSnapshot, v.clone()),
        Boundary::DeploymentSource(v) => (GapArea::DeploymentSource, v.clone()),
        Boundary::DeploymentPublications(v) => (GapArea::DeploymentPublications, v.clone()),
        Boundary::RuntimeIdentity(v) => (GapArea::RuntimeIdentity, v.clone()),
        Boundary::RuntimeSelection(v) => (GapArea::RuntimeSelection, v.clone()),
        Boundary::RuntimePublications(v) => (GapArea::RuntimePublications, v.clone()),
        Boundary::RuntimeObservationScope(v) => (GapArea::RuntimeObservationScope, v.clone()),
    };
    Some(Gap::new(area, detail))
}

pub(super) struct Assessment {
    pub gaps: Vec<Gap>,
    pub completeness: Completeness,
    pub role_coverage: BTreeMap<&'static str, Value>,
    pub outside_boundaries: usize,
    pub unlocalized_boundaries: usize,
}

/// Pure assembly of policy gaps from facts captured by Trace and the substrate.
pub(super) fn assess(
    s: &Snapshot,
    ix: &Index,
    t: &Trace,
    policy: &CasePolicy,
    slice: &serde_json::Map<String, Value>,
    external: &[Value],
) -> Assessment {
    let mut not_covered: Vec<Gap> = t
        .boundaries
        .iter()
        .filter_map(|b| boundary_gap(b, policy))
        .collect();
    let candidate_only =
        t.candidates
            .iter()
            .filter(|c| {
                c.selected
                    && !c.signals.iter().any(|s| {
                        s.kind == "exact_anchor" || s.confidence == "retained_relationship"
                    })
            })
            .count();
    if t.candidates.iter().any(|c| !c.selected)
        || candidate_only > 0
        || !rows(&t.retrieval, "excluded_facets").is_empty()
        || t.retrieval["join_discovery"]["omitted_session_publications"]
            .as_u64()
            .unwrap_or(0)
            > 0
        || t.retrieval["join_discovery"]["omitted_session_paths"]
            .as_u64()
            .unwrap_or(0)
            > 0
        || t.retrieval["join_discovery"]["omitted_or_truncated_requests"]
            .as_u64()
            .unwrap_or(0)
            > 0
        || t.retrieval["lexical_discovery"]["candidate_limit_reached"] == true
        || t.retrieval["lexical_discovery"]["omitted_candidates"]
            .as_u64()
            .unwrap_or(0)
            > 0
        || !rows(&t.retrieval["lexical_discovery"], "omitted_terms").is_empty()
        || t.retrieval["graph_expansion"]["omitted_starters"]
            .as_u64()
            .unwrap_or(0)
            > 0
        || t.retrieval["graph_expansion"]["unexpanded_frontier_files"]
            .as_u64()
            .unwrap_or(0)
            > 0
    {
        not_covered.push(Gap::new(GapArea::CandidateSelection,json!({"reason":"Task retrieval is bounded and ranked. Omitted candidates and disabled facets are not absence; page candidates to inspect alternatives.",
            "retrieval":t.retrieval,"candidate_only_files":candidate_only,"section":"/candidates"})));
    }
    let views: Vec<_> = std::iter::once(&s.view)
        .chain(s.evidence_views.iter())
        .collect();
    let artifact_paths: BTreeMap<_, _> = views
        .iter()
        .flat_map(|v| rows(&v.bundle, "artifacts"))
        .map(|a| (text(a, "id"), text(a, "path")))
        .collect();
    let reference_paths: BTreeMap<_, _> = views
        .iter()
        .flat_map(|v| rows(&v.bundle, "evidence_refs"))
        .filter_map(|r| {
            artifact_paths
                .get(&text(r, "artifact_id"))
                .map(|p| (text(r, "id"), p.clone()))
        })
        .collect();
    let entity_keys: BTreeMap<_, _> = views
        .iter()
        .flat_map(|v| rows(&v.bundle, "entity_versions"))
        .map(|e| (text(e, "id"), text(e, "canonical_key")))
        .collect();
    let (mut outside_boundaries, mut unlocalized_boundaries) = (0usize, 0usize);
    for producer in s.profiles.iter().chain(s.evidence_views.iter()) {
        let mut groups = BTreeMap::<String, Value>::new();
        for boundary in rows(&producer.bundle, "unresolved_references") {
            match boundary_location(
                ix,
                &t.paths,
                &t.keys,
                &reference_paths,
                &entity_keys,
                boundary,
            ) {
                BoundaryLocation::InSlice => {}
                BoundaryLocation::Outside => {
                    outside_boundaries += 1;
                    continue;
                }
                BoundaryLocation::Unlocalized => {
                    unlocalized_boundaries += 1;
                    continue;
                }
            }
            let run = boundary["run_id"].as_str().unwrap_or("unattributed");
            let group=groups.entry(run.into()).or_insert_with(||json!({"view_id":producer.id,"scope":producer.scope,
                "run_id":run,"count":0,"by_predicate_reason":{},"examples":[],"actionable":true,"locality":"task_local",
                "reason":"Task-local producer occurrences remain unresolved or unsupported; missing links do not establish absence."}));
            group["count"] = json!(group["count"].as_u64().unwrap_or(0) + 1);
            let key = format!(
                "{}:{}",
                text(boundary, "predicate"),
                text(boundary, "reason")
            );
            group["by_predicate_reason"][&key] =
                json!(group["by_predicate_reason"][&key].as_u64().unwrap_or(0) + 1);
            let examples = group["examples"].as_array_mut().unwrap();
            if examples.len() < 3 {
                examples.push(
                    json!({"id":boundary["id"],"predicate":boundary["predicate"],
                "reason":boundary["reason"],"evidence_ref_id":boundary["evidence_ref_id"]}),
                );
            }
        }
        let detail = rows(&ix.coverage, "producer_boundaries")
            .iter()
            .find(|p| p["view_id"] == producer.id);
        for mut group in groups.into_values() {
            if let Some(id) = detail.and_then(|p| p["detail_report_id"].as_str()) {
                group["details"] = crate::architecture::reports::continuation(id, "/items", 0, 100);
            }
            not_covered.push(Gap::new(GapArea::ProducerBoundary, group));
        }
    }
    if unlocalized_boundaries > 0 {
        not_covered.push(Gap::new(GapArea::ProducerBoundaryScope,json!({"count":unlocalized_boundaries,"actionable":false,"locality":"global_health",
            "reason":"Unlocalized producer health: relevance to this task is unknown. Details remain in the pinned producer reports."})));
    }
    for binding in rows(&ix.coverage, "unresolved_bindings") {
        not_covered.push(Gap::new(GapArea::SemanticBinding,json!({"binding":binding,
            "reason":"Declaration binding is unresolved, ambiguous, deferred or withdrawn within this profile scope; another compatible profile may bind it."})));
    }
    if ix.coverage["omitted_relationship_occurrences"]
        .as_u64()
        .unwrap_or(0)
        > 0 && ix.coverage["assembly_relationship_scope"]!="all"
    {
        not_covered.push(Gap::new(GapArea::RelationshipIndex,json!({"reason":"Initial task assembly uses bounded hot adjacency. Query all retained relationships with neighbors; legacy overflow may require a new index.",
            "adjacency":super::index::adjacency_coverage(ix),
            "omitted_occurrences":ix.coverage["omitted_relationship_occurrences"]})));
    }
    if ix.coverage["propagation"]["feature_membership_complete"] == false
        || ix.coverage["propagation"]["file_associations_complete"] == false
    {
        not_covered.push(Gap::new(GapArea::RelationshipPropagation,json!({"reason":"File association propagation reached its iteration bound; associations can be incomplete.","coverage":ix.coverage["propagation"]})));
    }
    let required_roles = policy.required_roles;
    let mut role_coverage = BTreeMap::new();
    for role in source::ARTIFACT_ROLES {
        let captured = s.files.values().filter(|f| f.role == *role).count();
        let selected = slice
            .get(*role)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let established = slice
            .get(*role)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|row| {
                row["selection"] == "resolved_anchor"
                    || rows(row, "selection_reasons")
                        .iter()
                        .any(|s| s["confidence"] == "retained_relationship")
            })
            .count();
        role_coverage.insert(*role,json!({"captured_files":captured,"selected_files":selected,"established_files":established,
            "candidate_only_files":selected-established,"state":if established>0 {"associations_established"} else if selected>0 {"candidates_only"} else {"no_association_established"}}));
        if required_roles.contains(role) && established == 0 {
            not_covered.push(Gap::new(GapArea::ArtifactRole((*role).into()),json!({"reason":"This duty requires considering this role; retrieval hints alone do not establish a retained association.","captured_files":captured,"selected_candidates":selected,
                // No captured file can fill a role absent from the inventory.
                "actionable":captured>0,"locality":if captured>0 {"task_local"} else {"global_health"}})));
        }
    }
    if policy.external_required && !external.is_empty() {
        not_covered.push(Gap::new(GapArea::ExternalInstallations,json!({"reason":"Referenced installations/configuration are desired state. Use explicit external inspection to verify selected installed files; active runtime selection remains unknown."})));
    }
    if policy.test_execution_required {
        not_covered.push(Gap::new(
            GapArea::ExecutedTestCoverage,
            json!({"reason":"Static test associations do not establish executed test results."}),
        ));
    }
    if policy.plan_outcomes_required
        && t.paths.iter().any(|p| {
            s.files
                .get(p)
                .is_some_and(|f| f.role == "plan" || f.role == "adr")
        })
    {
        not_covered.push(Gap::new(GapArea::PlanOutcomes,json!({"reason":"Plan references establish declared intent; outcomes require implementing evidence."})));
    }
    if !rows(&ix.coverage, "analysis_profiles")
        .iter()
        .any(|p| !rows(p, "static_call_runs").is_empty())
    {
        not_covered.push(Gap::new(GapArea::StaticCalls,json!({"reason":"No static-call producer guarantees are present in the selected profiles."})));
    }
    if t.seed.status != AnchorStatus::Resolved {
        not_covered.push(Gap::new(GapArea::Anchors,json!({"reason":"No unique retained identity resolved. Located candidates may be selected as retrieval hints, without establishing implementation membership; supply an exact path or canonical key to disambiguate."})));
    }
    let inventory = &s.view.diagnostics["capture"]["inventory"];
    if inventory["tracked_text_complete"] != true {
        not_covered.push(Gap::new(GapArea::Capture,json!({"reason":"Full tracked-file coverage is unestablished; inspect the inventory omissions.","inventory":inventory_summary(inventory)})));
    }
    // Inventory health is aggregated per area: one row per area with category
    // counts, never one row per excluded file. Full lists stay in the view.
    for (area, field, reason) in [
        (
            GapArea::CapturePath,
            "unretained_tracked_paths",
            "Tracked paths were not retained; see capture exclusions.",
        ),
        (
            GapArea::CaptureExclusion,
            "exclusions",
            "Capture excluded inputs; excluded files are not evidence of absence.",
        ),
        (
            GapArea::CaptureDiagnostic,
            "diagnostics",
            "Capture reported diagnostics for the inventory.",
        ),
    ] {
        if let Some(gap) = inventory_gap(area, field, rows(inventory, field), reason) {
            not_covered.push(gap);
        }
    }
    for language in rows(&ix.coverage, "languages")
        .iter()
        .filter(|v| v["semantic_profile"] == "not_run")
    {
        not_covered.push(Gap::new(GapArea::LanguageProfile,json!({"language":language["language"],"reason":"Text is retained but this language's semantic profile is not present in the selected view."})));
    }
    if s.view.diagnostics["coverage"]["resolution_complete"] != true {
        not_covered.push(Gap::new(GapArea::SemanticAnalysis,json!({"reason":"Producer coverage does not establish complete resolution; missing edges do not establish absence.","coverage":s.view.diagnostics["coverage"]})));
    }
    // Stable wire order: actionable before global health, then dimension name,
    // then area; equal keys preserve fact order. Bounded previews therefore
    // keep task-local gaps ahead of inventory health.
    not_covered.sort_by_key(|g| {
        (
            g.detail.get("actionable") == Some(&Value::Bool(false)),
            g.dimension.name(),
            String::from(g.area.clone()),
        )
    });
    let completeness = Completeness::derive(
        t.seed.status,
        &not_covered,
        policy,
        t.joins.history_tip.is_some(),
        !t.boundaries
            .iter()
            .any(|b| matches!(b, Boundary::NoSessions)),
    );
    Assessment {
        gaps: not_covered,
        completeness,
        role_coverage,
        outside_boundaries,
        unlocalized_boundaries,
    }
}

/// One aggregated global-health gap for an inventory list of
/// `category: label` strings (paths group by their first segment).
fn inventory_gap(area: GapArea, field: &str, items: &[Value], reason: &str) -> Option<Gap> {
    if items.is_empty() {
        return None;
    }
    let mut by_category = BTreeMap::<String, u64>::new();
    for item in items {
        let item = item
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| item.to_string());
        let category = if field == "unretained_tracked_paths" {
            item.split('/').next().unwrap_or("").to_owned()
        } else {
            item.split_once(": ").map_or("other", |(c, _)| c).to_owned()
        };
        *by_category.entry(category).or_default() += 1;
    }
    Some(Gap::new(
        area,
        json!({"count":items.len(),"by_category":by_category,"examples":items.iter().take(3).collect::<Vec<_>>(),
        "actionable":false,"locality":"global_health","reason":reason,"source":format!("/diagnostics/capture/inventory/{field}")}),
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum BoundaryLocation {
    InSlice,
    Outside,
    Unlocalized,
}
fn boundary_location(
    ix: &Index,
    paths: &BTreeSet<String>,
    keys: &BTreeSet<String>,
    references: &BTreeMap<String, String>,
    entities: &BTreeMap<String, String>,
    boundary: &Value,
) -> BoundaryLocation {
    let path = references.get(&text(boundary, "evidence_ref_id"));
    let key = entities.get(&text(boundary, "source_entity_id"));
    // A precise occurrence path takes precedence over a broad containing entity.
    if let Some(path) = path {
        return if paths.contains(path) {
            BoundaryLocation::InSlice
        } else {
            BoundaryLocation::Outside
        };
    }
    if key.is_some_and(|k| {
        keys.contains(k)
            || ix
                .entities
                .get(k)
                .is_some_and(|e| !e.files.is_disjoint(paths))
    }) {
        BoundaryLocation::InSlice
    } else if path.is_some()
        || key.is_some_and(|k| ix.entities.get(k).is_some_and(|e| !e.files.is_empty()))
    {
        BoundaryLocation::Outside
    } else {
        BoundaryLocation::Unlocalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> CasePolicy {
        CasePolicy {
            required_roles: &[],
            history_required: true,
            sessions_required: true,
            external_required: false,
            test_execution_required: false,
            plan_outcomes_required: false,
        }
    }
    fn gap(area: GapArea) -> Gap {
        Gap::new(area, json!({"reason":"fixture"}))
    }
    fn states(
        status: AnchorStatus,
        areas: Vec<GapArea>,
        history: bool,
        sessions: bool,
    ) -> Completeness {
        Completeness::derive(
            status,
            &areas.into_iter().map(gap).collect::<Vec<_>>(),
            &policy(),
            history,
            sessions,
        )
    }
    #[test]
    fn every_gap_area_has_a_wire_name_and_one_dimension() {
        use Dimension::*;
        let groups = [
            (Anchors, vec!["anchors"]),
            (
                Slice,
                vec![
                    "candidate_selection",
                    "capture",
                    "capture_path",
                    "capture_exclusion",
                    "capture_diagnostic",
                ],
            ),
            (
                Neighborhood,
                vec![
                    "relationship_index",
                    "relationship_propagation",
                    "semantic_analysis",
                    "semantic_binding",
                    "static_calls",
                    "language_profile",
                    "producer_boundary",
                    "profile_snapshot",
                    "deployment_source",
                    "deployment_publications",
                    "runtime_identity",
                    "runtime_selection",
                    "runtime_publications",
                    "runtime_observation_scope",
                ],
            ),
            (History, vec!["history"]),
            (Sessions, vec!["sessions", "session_commit_join"]),
            (GapInventory, vec!["producer_boundary_scope"]),
            (
                Verification,
                vec![
                    "executed_test_coverage",
                    "plan_outcomes",
                    "external_installations",
                ],
            ),
        ];
        for (dimension, areas) in groups {
            for name in areas {
                let area = GapArea::try_from(name.to_owned()).unwrap();
                assert_eq!(area.dimension(), dimension, "{name}");
                assert_eq!(String::from(area.clone()), name);
                let row = serde_json::to_value(gap(area)).unwrap();
                assert_eq!(row["dimension"], dimension.name());
            }
        }
        for role in source::ARTIFACT_ROLES {
            let area = GapArea::try_from(format!("artifact_role:{role}")).unwrap();
            assert_eq!(area.dimension(), Slice);
        }
        assert!(GapArea::try_from("unassigned_future_area".to_string()).is_err());
    }
    #[test]
    fn anchors_determine_unknown_outcome_and_unknown_slice() {
        for (status, anchor_state, slice_state, outcome) in [
            (
                AnchorStatus::Resolved,
                State::Established,
                State::Established,
                Outcome::Complete,
            ),
            (
                AnchorStatus::Ambiguous,
                State::Partial,
                State::Established,
                Outcome::Partial,
            ),
            (
                AnchorStatus::Unresolved,
                State::Unknown,
                State::Unknown,
                Outcome::Unknown,
            ),
        ] {
            let c = states(status, vec![], true, true);
            assert_eq!(c.anchors, anchor_state);
            assert_eq!(c.slice, slice_state);
            assert_eq!(c.outcome(), outcome);
        }
    }
    #[test]
    fn capture_and_missing_roles_make_the_slice_partial() {
        for area in [
            GapArea::Capture,
            GapArea::CapturePath,
            GapArea::CaptureExclusion,
            GapArea::CaptureDiagnostic,
            GapArea::ArtifactRole("test".into()),
        ] {
            let c = states(AnchorStatus::Resolved, vec![area], true, true);
            assert_eq!(c.slice, State::Partial);
            assert_eq!(c.outcome(), Outcome::Partial);
        }
    }
    #[test]
    fn relationship_gaps_make_the_neighborhood_partial() {
        let c = states(
            AnchorStatus::Resolved,
            vec![GapArea::RuntimeIdentity],
            true,
            true,
        );
        assert_eq!(c.neighborhood, State::Partial);
        assert_eq!(c.outcome(), Outcome::Partial);
    }
    #[test]
    fn history_distinguishes_unavailable_partial_and_optional() {
        assert_eq!(
            states(AnchorStatus::Resolved, vec![], false, true).history,
            State::Unknown
        );
        assert_eq!(
            states(AnchorStatus::Resolved, vec![GapArea::History], true, true).history,
            State::Partial
        );
        let mut p = policy();
        p.history_required = false;
        assert_eq!(
            Completeness::derive(
                AnchorStatus::Resolved,
                &[gap(GapArea::History)],
                &p,
                false,
                true
            )
            .history,
            State::NotRequired
        );
    }
    #[test]
    fn sessions_distinguish_unavailable_partial_and_optional() {
        assert_eq!(
            states(AnchorStatus::Resolved, vec![], true, false).sessions,
            State::Unknown
        );
        for area in [GapArea::Sessions, GapArea::SessionCommitJoin] {
            assert_eq!(
                states(AnchorStatus::Resolved, vec![area], true, true).sessions,
                State::Partial
            );
        }
        let mut p = policy();
        p.sessions_required = false;
        assert_eq!(
            Completeness::derive(
                AnchorStatus::Resolved,
                &[gap(GapArea::Sessions)],
                &p,
                true,
                false
            )
            .sessions,
            State::NotRequired
        );
    }
    #[test]
    fn unlocalized_boundaries_make_the_gap_inventory_partial() {
        let c = states(
            AnchorStatus::Resolved,
            vec![GapArea::ProducerBoundaryScope],
            true,
            true,
        );
        assert_eq!(c.gap_inventory, State::Partial);
        assert_eq!(c.outcome(), Outcome::Partial);
    }
    #[test]
    fn verification_is_never_established_by_static_evidence() {
        assert_eq!(
            states(AnchorStatus::Resolved, vec![], true, true).verification,
            State::NotRequired
        );
        for area in [
            GapArea::ExecutedTestCoverage,
            GapArea::PlanOutcomes,
            GapArea::ExternalInstallations,
        ] {
            let c = states(AnchorStatus::Resolved, vec![area], true, true);
            assert_eq!(c.verification, State::Unknown);
            assert_eq!(c.outcome(), Outcome::Partial);
        }
    }
    #[test]
    fn boundary_facts_are_interpreted_by_duty_policy() {
        let mut p = policy();
        for b in [
            Boundary::NoHistory,
            Boundary::PartialHistory(json!({"reason":"partial"})),
            Boundary::NoSessions,
            Boundary::MetadataSession(json!({"reason":"metadata"})),
            Boundary::RetractedSession(json!({"reason":"retracted"})),
            Boundary::PartialSession(json!({"reason":"partial"})),
            Boundary::IndirectCommit(json!({"reason":"indirect"})),
        ] {
            assert!(boundary_gap(&b, &p).is_some());
            p.history_required = false;
            p.sessions_required = false;
            assert!(boundary_gap(&b, &p).is_none());
            p = policy();
        }
    }
    #[test]
    fn inventory_lists_aggregate_into_one_global_health_gap_per_area() {
        let items: Vec<Value> = [
            "binary input: a.png",
            "binary input: b.png",
            "per-file byte limit: c.a",
        ]
        .map(|s| json!(s))
        .into();
        let gap = inventory_gap(GapArea::CaptureExclusion, "exclusions", &items, "r").unwrap();
        assert_eq!(gap.detail["count"], 3);
        assert_eq!(gap.detail["by_category"]["binary input"], 2);
        assert_eq!(gap.detail["by_category"]["per-file byte limit"], 1);
        assert_eq!(gap.detail["actionable"], false);
        let paths: Vec<Value> = ["vendor/x", "vendor/y", "docs/z"].map(|s| json!(s)).into();
        assert_eq!(
            inventory_gap(
                GapArea::CapturePath,
                "unretained_tracked_paths",
                &paths,
                "r"
            )
            .unwrap()
            .detail["by_category"]["vendor"],
            2
        );
        assert!(inventory_gap(GapArea::CaptureDiagnostic, "diagnostics", &[], "r").is_none());
        // Aggregation keeps the conservative completeness effect.
        let c = Completeness::derive(AnchorStatus::Resolved, &[gap], &policy(), true, true);
        assert_eq!(c.slice, State::Partial);
    }
    #[test]
    fn producer_boundaries_are_local_outside_or_unlocalized() {
        let ix = Index {
            id: "ix".into(),
            source_view: "v".into(),
            retraction_epoch: 0,
            files: BTreeMap::new(),
            entities: BTreeMap::from([(
                "entity".into(),
                Entity {
                    key: "entity".into(),
                    kind: "module".into(),
                    names: BTreeSet::new(),
                    files: BTreeSet::from(["src/a.rs".into()]),
                },
            )]),
            links: vec![],
            overflow_links: vec![],
            coverage: json!({}),
        };
        let paths = BTreeSet::from(["src/a.rs".into()]);
        let keys = BTreeSet::new();
        let references = BTreeMap::from([
            ("local".into(), "src/a.rs".into()),
            ("other".into(), "src/b.rs".into()),
        ]);
        let entities = BTreeMap::from([("ev".into(), "entity".into())]);
        for (boundary, expected) in [
            (
                json!({"evidence_ref_id":"local"}),
                BoundaryLocation::InSlice,
            ),
            (json!({"source_entity_id":"ev"}), BoundaryLocation::InSlice),
            (
                json!({"evidence_ref_id":"other"}),
                BoundaryLocation::Outside,
            ),
            (
                json!({"evidence_ref_id":"missing"}),
                BoundaryLocation::Unlocalized,
            ),
        ] {
            assert_eq!(
                boundary_location(&ix, &paths, &keys, &references, &entities, &boundary),
                expected
            );
        }
    }
}
