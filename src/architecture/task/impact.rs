//! Change-specific composition over retained source, task indexes and policy
//! reports. This is a report, not another assertion store or a validation runner.
use super::*;
use crate::architecture::{contracts, history, knowledge, query, reports};

const METHOD: &str = "architectural-impact-v1";
const SECTIONS: &[&str] = &[
    "changes", "resolutions", "affected_components", "relationship_changes",
    "correspondence", "constraints", "verification", "findings",
    "governing_decisions", "unknowns",
];
// TESTS is deliberately absent: a shared suite must not bridge implementations.
const DEPENDENCIES: &[&str] = &[
    "IMPORTS", "DEPENDS_ON", "REFERENCES", "SYMBOL_BINDING", "CALLS_STATIC",
    "READS", "WRITES", "TRANSFORMS", "MAPS_TO", "ROUTES_TO",
];
const BINDINGS: &[&str] = &[
    "IMPLEMENTS_FEATURE", "ENTRYPOINT_FOR", "PARTICIPATES_IN", "REALIZES_FEATURE",
    "SUBFEATURE_OF", "BELONGS_TO_DOMAIN",
];

pub(crate) fn validate_request(a: &Value) -> Result<()> {
    let op = crate::architecture::service::find("impact").ok_or("IMPACT: Missing operation schema")?;
    crate::architecture::service::validate(&op.schema, a)?;
    for field in ["view", "base", "case_id", "target"] {
        if let Some(value) = a[field].as_str() {
            require(!value.trim().is_empty() && value.len() <= 4096,
                "ARGUMENTS", &format!("{field} must contain 1..4096 bytes"))?;
        }
    }
    for target in contracts::strings(&a["targets"]) {
        require(!target.trim().is_empty() && target.len() <= 4096,
            "ARGUMENTS", "Each target must contain 1..4096 bytes")?;
    }
    if a["mode"] == "pre" {
        require(a["base"].is_null(), "ARGUMENTS", "pre mode does not accept base")?;
        require(a["task"].is_string() || a["case_id"].is_string(),
            "ARGUMENTS", "pre mode requires task or case_id")?;
        require(a["view"].is_string() || a["case_id"].is_string(),
            "ARGUMENTS", "pre mode requires view or case_id")?;
    } else {
        require(a["base"].is_string() && a["view"].is_string(),
            "ARGUMENTS", "post mode requires base and view source snapshots")?;
        require(a["target"].is_null() && contracts::rows(a, "targets").is_empty(),
            "ARGUMENTS", "post mode assesses the full captured delta; targets cannot filter it")?;
    }
    Ok(())
}

struct Side<'l> {
    name: &'static str,
    source: Snapshot<'l>,
    index: Index,
}
impl<'l> Side<'l> {
    fn load(l: &'l Ledger, name: &'static str, selector: &str) -> Result<Self> {
        // Source only. Runtime/deployment selection is an independent future
        // integration, and must never be silently selected by latest aliases.
        let source = Snapshot::source(l, selector)?;
        let mut index = index::load(l, &source)?;
        // Findings/convergence inspect `links`; provide the complete retained
        // adjacency in memory without altering the stored hot index or its ID.
        index.links = index::query_links(l, &index, "all")?;
        index.overflow_links.clear();
        Ok(Self { name, source, index })
    }
    fn snapshot(&self) -> &Value {
        let id = knowledge::context(&self.source.view.bundle)["snapshot_id"].clone();
        rows(&self.source.view.bundle, "snapshots").iter().find(|s| s["id"] == id)
            .expect("Snapshot::source validated the snapshot")
    }
    fn capture_complete(&self) -> bool {
        self.snapshot()["capture_state"] == "complete"
            && rows(&self.source.view.diagnostics, "exclusions").is_empty()
            && rows(&self.source.view.diagnostics["capture"]["inventory"], "unretained_tracked_paths").is_empty()
    }
    fn file_ref(&self, path: &str) -> Value {
        reference(&self.index.id, &format!("/files/{}", pointer(path)))
    }
    fn link_ref(&self, link: &Link) -> Value { reference(&self.index.id, &link.id) }
    fn pin(&self) -> Value {
        json!({"side":self.name,"view_id":self.source.view.id,"index_id":self.index.id,
            "snapshot_id":self.snapshot()["id"],"repository_id":self.snapshot()["repository_id"],
            "captured_files":self.source.files.len(),"capture_complete":self.capture_complete(),
            "dependencies_complete_for_producer_scope":query::complete(&self.source.view),
            "coverage_ref":reference(&self.index.id,"/coverage")})
    }
}
fn pointer(s: &str) -> String { s.replace('~', "~0").replace('/', "~1") }
fn reference(report: &str, target: &str) -> Value { json!({"report_id":report,"ref":target}) }
fn gap(area: &str, side: Option<&str>, reason: &str, refs: Vec<Value>) -> Value {
    json!({"area":area,"side":side,"status":"unknown","reason":reason,"evidence_refs":refs})
}
fn selected_paths(case: &Value) -> BTreeSet<String> {
    source::ARTIFACT_ROLES.iter().flat_map(|role| rows(&case["slice"], role))
        .map(|r| text(r, "path")).collect()
}

/// Read and retain one immutable impact contract. `limit` and `max_bytes`
/// affect delivery only; source capture, test execution and policy approval
/// remain explicit external actions.
pub fn impact(l: &Ledger, a: &Value) -> Result<Value> {
    validate_request(a)?;
    let mode = text(a, "mode");
    let mut retained_case = a["case_id"].as_str().map(|id| case::load_doc(l, id)).transpose()?;
    let selector = a["view"].as_str().or_else(|| retained_case.as_ref().and_then(|c| c["view_id"].as_str()))
        .ok_or("ARGUMENTS: A captured source view is required")?;
    let candidate = Side::load(l, "candidate", selector)?;
    let base = a["base"].as_str().map(|v| Side::load(l, "base", v)).transpose()?;
    if let Some(before) = &base {
        require(before.snapshot()["repository_id"] == candidate.snapshot()["repository_id"],
            "IMPACT", "Both snapshots must belong to the same repository")?;
        require(before.snapshot()["id"] != candidate.snapshot()["id"],
            "IMPACT", "post mode requires two distinct captured source snapshots")?;
    }
    if let Some(case) = &retained_case {
        let expected = base.as_ref().unwrap_or(&candidate);
        require(case["view_id"] == expected.source.view.id,
            "IMPACT", "Case source pin differs from the change baseline")?;
    }
    let task = a["task"].as_str().or_else(|| retained_case.as_ref().and_then(|c| c["task"].as_str()))
        .unwrap_or("Assess captured source changes").to_owned();
    let mut targets: BTreeSet<String> = contracts::strings(&a["targets"]).into_iter().collect();
    targets.extend(a["target"].as_str().map(str::to_owned));
    if mode == "pre" && retained_case.is_none() {
        let mut args = json!({"task":task,"duty":"impact","view":candidate.source.view.id});
        if let Some(target) = targets.first() { args["target"] = json!(target); }
        let (case, _) = brief::build_case(l, &args)?;
        retained_case = Some(serde_json::to_value(case)?);
    }
    let initial = retained_case.as_ref().map(selected_paths);
    let mut changes = vec![];
    let mut resolutions = vec![];
    let mut unknowns = vec![];
    let mut roots = BTreeMap::new();
    if let Some(before) = &base {
        let paths: BTreeSet<_> = before.source.files.keys().chain(candidate.source.files.keys()).cloned().collect();
        for path in paths {
            let old = before.source.files.get(&path);
            let new = candidate.source.files.get(&path);
            if old.map(|f| &f.digest) == new.map(|f| &f.digest) { continue; }
            let state = match (old, new) {
                (Some(_), Some(_)) => "modified",
                (None, Some(_)) if before.capture_complete() => "added",
                (Some(_), None) if candidate.capture_complete() => "deleted",
                _ => "unavailable",
            };
            let refs: Vec<_> = old.map(|_| before.file_ref(&path)).into_iter()
                .chain(new.map(|_| candidate.file_ref(&path))).collect();
            changes.push(json!({"path":path,"state":state,"before_digest":old.map(|f|&f.digest),
                "after_digest":new.map(|f|&f.digest),"outside_initial_slice":initial.as_ref().map(|s|!s.contains(&path)),
                "evidence_refs":refs}));
            roots.insert(path.clone(), true);
            if state == "unavailable" {
                unknowns.push(gap("capture", None,
                    &format!("{path} is absent on one side of an incomplete inventory; addition/deletion is not established"), refs));
            }
        }
    } else {
        if targets.is_empty() {
            if let Some(case) = &retained_case {
                if case["anchors"]["status"] == "resolved" {
                    targets.extend(contracts::strings(&case["anchors"]["paths"]));
                } else {
                    // Retrieval candidates remain hypothetical. No candidate
                    // is silently promoted to an exact change target.
                    for row in rows(case, "candidates").iter().take(8) {
                        let path = text(row, "path");
                        if candidate.source.files.contains_key(&path) { roots.insert(path, false); }
                    }
                    unknowns.push(gap("targets", Some("candidate"),
                        "No exact planned target; up to eight ranked Case candidates seed prospective investigation only",
                        vec![reference(&text(case,"id"),"/anchors")]));
                }
            }
        }
        for target in &targets {
            let (status, keys, paths) = trace::resolve(&candidate.index, target, true);
            resolutions.push(json!({"target":target,"status":status,"candidates":keys,"paths":paths}));
            if status == "resolved" && !paths.is_empty() {
                for path in paths { roots.insert(path, true); }
            } else {
                unknowns.push(gap("targets", Some("candidate"),
                    &format!("Target {target} is {status} or has no localized captured source; candidates are not direct changes"),
                    vec![reference(&candidate.index.id,"/entities")]));
            }
        }
        for (path, exact) in &roots {
            if *exact {
                changes.push(json!({"path":path,"state":"planned","before_digest":candidate.source.files[path].digest,
                    "after_digest":null,"outside_initial_slice":initial.as_ref().map(|s|!s.contains(path)),
                    "evidence_refs":[candidate.file_ref(path)]}));
            }
        }
    }
    let depth = a["depth"].as_u64().unwrap_or(2) as usize;
    let max_nodes = a["max_nodes"].as_u64().unwrap_or(4096) as usize;
    let sides: Vec<_> = base.iter().chain(std::iter::once(&candidate)).collect();
    let mut affected = vec![];
    let mut constraints = vec![];
    let mut verification = vec![];
    let mut findings = vec![];
    for side in &sides {
        let present: BTreeMap<_,_> = roots.iter().filter(|(p,_)|side.source.files.contains_key(*p))
            .map(|(p,exact)|(p.clone(),*exact)).collect();
        let exact_scope = present.values().any(|exact|*exact);
        let mut nodes: BTreeSet<String> = present.keys().cloned().collect();
        for (path, exact) in &present {
            affected.push(component(side, path, if *exact {"direct_change"} else {"candidate_target"},
                0, vec![side.file_ref(path)], *exact));
        }
        for (incoming, relationship) in [(true,"possible_consumer"),(false,"dependency_context")] {
            let walked = walk(side, &present, incoming, depth, max_nodes);
            for (node, (distance, trail)) in &walked.nodes {
                nodes.insert(node.clone());
                if !present.contains_key(node) {
                    affected.push(component(side, node, relationship, *distance, trail.clone(), exact_scope));
                }
            }
            for (node, (distance, trail)) in &walked.reached {
                nodes.insert(node.clone());
                affected.push(component(side, node, relationship, *distance, trail.clone(), exact_scope));
            }
            if walked.node_limit || walked.depth_limit {
                unknowns.push(gap("traversal", Some(side.name),
                    &format!("{relationship} traversal bounded at depth {depth} and {max_nodes} nodes; node_limit={}, depth_limit={}",
                        walked.node_limit, walked.depth_limit), vec![reference(&side.index.id,"/coverage")]));
            }
        }
        declared_components(side, &mut nodes, &mut affected, &mut unknowns, exact_scope);
        if affected.iter().any(|r|r["side"]==side.name && r["kind"]=="unlocalized") {
            unknowns.push(gap("graph",Some(side.name),"Some reached endpoints have no retained entity or source localization",
                vec![reference(&side.index.id,"/coverage")]));
        }
        side_gaps(side, &mut unknowns);
        policy(l, side, &nodes, &mut constraints, &mut verification, &mut unknowns)?;
        test_suggestions(side, &nodes, &mut verification);
        interface_suggestions(side, &nodes, &mut verification)?;
        let subjects: Vec<_> = present.keys().cloned().collect();
        // Resolve fallible source reads before the pure finding builder; a
        // corrupt payload is an error, never a missing-source advisory.
        let contents: BTreeMap<_,_> = subjects.iter().map(|p| Ok((p.clone(),side.source.content(p)?.map(str::to_owned))))
            .collect::<Result<_>>()?;
        let (mut built, finding_coverage) = findings::build(&side.index, &side.source.view.id, &subjects,
            &|p|contents.get(p).cloned().flatten());
        let (conventions, convention_coverage) = convergence::build(&side.index, &side.source.view.id, &subjects,
            &super::case::unresolved_dependency_paths(&side.source));
        built.extend(conventions);
        if finding_coverage["truncated"]==true || convention_coverage["truncated"]==true
            || convention_coverage["subjects_considered"].as_u64().unwrap_or(0)<subjects.len() as u64 {
            unknowns.push(gap("findings",Some(side.name),
                "Behavior or convention findings reached their existing producer bounds; the impact graph and full source delta remain available",
                vec![reference(&side.index.id,"/coverage")]));
        }
        for finding in built {
            let mut link_ids = BTreeSet::new();
            finding_links(&finding,&mut link_ids);
            let refs: Vec<_> = subjects.iter().map(|p|side.file_ref(p))
                .chain(link_ids.iter().map(|id|reference(&side.index.id,id))).collect();
            findings.push(json!({"side":side.name,"view_id":side.source.view.id,
                "index_id":side.index.id,"finding":finding,"evidence_refs":refs}));
        }
    }
    let mut correspondence = vec![];
    let mut relationship_changes = vec![];
    if let Some(before) = &base {
        let remap = correspondence_rows(l, before, &candidate, &mut correspondence, &mut unknowns)?;
        relationship_changes = relationship_delta(before, &candidate, &remap);
        let declaration = crate::architecture::declarations::ARTIFACT_PATH;
        if before.source.files.get(declaration).map(|f|&f.digest) != candidate.source.files.get(declaration).map(|f|&f.digest) {
            unknowns.push(gap("declarations", None,
                "Architecture declarations changed between snapshots; changed intent requires external review, not automatic approval", vec![]));
        }
    }
    unknowns.push(gap("design_decisions", None,
        "No pinned external decision registry is connected; governing ADRs and approvals are not inferred from repository prose", vec![]));
    unknowns.push(gap("verification", None,
        "Tests and runtime workflows were not executed. Static associations are suggested checks, not verification results", vec![]));
    affected.sort_by_key(|row| (text(row,"side"),text(row,"id"),text(row,"relationship")));
    affected.dedup_by(|a,b|a["side"]==b["side"] && a["id"]==b["id"] && a["relationship"]==b["relationship"]);
    verification.sort_by_key(|row| (text(row,"side"),text(row,"id")));
    verification.dedup_by(|a,b|a["side"]==b["side"] && a["id"]==b["id"]);
    let mut pins: BTreeSet<String> = sides.iter().flat_map(|s| contracts::strings(&s.index.coverage["profile_views"])).collect();
    pins.extend(sides.iter().map(|s|s.source.view.id.clone()));
    let mut doc = json!({"schema_version":"1.0","kind":"architectural_impact_contract","method":METHOD,
        "mode":mode,"task":task,"case_id":retained_case.as_ref().map(|c|text(c,"id")),
        "view_id":candidate.source.view.id,"index_id":candidate.index.id,"pinned_views":pins,
        "source":{"repository_id":candidate.snapshot()["repository_id"],
            "base_view":base.as_ref().map(|s|&s.source.view.id),"candidate_view":candidate.source.view.id,
            "base_snapshot":base.as_ref().map(|s|s.snapshot()["id"].clone()),"candidate_snapshot":candidate.snapshot()["id"]},
        "request":{"targets":targets,"depth":depth,"max_nodes":max_nodes},
        "sides":sides.iter().map(|s|s.pin()).collect::<Vec<_>>(),
        "changes":changes,"resolutions":resolutions,"affected_components":affected,
        "relationship_changes":relationship_changes,"correspondence":correspondence,
        "constraints":constraints,"verification":verification,"findings":findings,
        "governing_decisions":[],"governing_decisions_status":"not_connected","unknowns":unknowns,
        "assessment":"partial","assessment_scope":"retained_evidence_for_captured_source_only",
        "coverage":{"granularity":"captured files and localized retained entities; not a symbol-level semantic diff",
            "source_delta":if mode=="post" {"all captured paths, independent of initial Case selection"}else{"prospective targets and explicitly labelled candidates"},
            "graph":"eligible source relationships, including retained overflow; bounded directional traversal",
            "comparison":"observed-only relationship changes do not establish absence on the other side",
            "verification":"not_executed","decision_registry":"not_connected"},
        "retraction_epoch":l.retraction_epoch()?,
        "provenance":{"implementation_digest":identity::bytes_digest(include_bytes!("impact.rs")),
            "schema_digest":identity::bytes_digest(include_bytes!("../../../contracts/v1/architectural-impact-contract.schema.json"))},
        "instruction":"Use evidence_refs with explain and page retained sections. Review unknowns before external validation; this report does not certify compliance or task completion."});
    let id = identity::id("impact-contract", &doc)?;
    doc["report_id"] = json!(id);
    contracts::schema(&doc, "architectural-impact-contract", None)?;
    l.save_report(&id, &doc, &[])?;
    preview(&mut doc, a["limit"].as_u64().unwrap_or(20) as usize);
    Ok(doc)
}

fn component(side: &Side<'_>, node: &str, relationship: &str, depth: usize, refs: Vec<Value>, exact: bool) -> Value {
    let file = side.source.files.get(node);
    let entity = side.index.entities.get(node);
    json!({"id":node,"path":file.map(|f|&f.path),
        "kind":if file.is_some() {"file"}else{entity.map(|e|e.kind.as_str()).unwrap_or("unlocalized")},
        "side":side.name,"view_id":side.source.view.id,"index_id":side.index.id,
        "relationship":relationship,"depth":depth,
        "status":if !exact {"candidate"} else if entity.is_none() && file.is_none() {"requires_investigation"}else{"supported_in_retained_scope"},
        "evidence_refs":refs})
}

struct Walk {
    nodes: BTreeMap<String, (usize, Vec<Value>)>,
    /// Roots reached through a relationship from another root. They keep their
    /// direct-change row and also show the consumer/dependency context.
    reached: BTreeMap<String, (usize, Vec<Value>)>,
    node_limit: bool,
    depth_limit: bool,
}
fn endpoints(link: &Link, from: bool) -> BTreeSet<String> {
    if from { std::iter::once(link.from.clone()).chain(link.from_files.iter().cloned()).collect() }
    else { std::iter::once(link.to.clone()).chain(link.to_files.iter().cloned()).collect() }
}
fn test_node(side: &Side<'_>, node: &str) -> bool {
    side.source.files.get(node).is_some_and(|f|f.role=="test")
        || side.index.entities.get(node).is_some_and(|e|!e.files.is_empty()
            && e.files.iter().all(|p|side.source.files.get(p).is_some_and(|f|f.role=="test")))
}
fn walk(side: &Side<'_>, roots: &BTreeMap<String,bool>, incoming: bool, depth: usize, max_nodes: usize) -> Walk {
    let mut out = Walk { nodes: roots.keys().map(|p|(p.clone(),(0,vec![side.file_ref(p)]))).collect(),
        reached: BTreeMap::new(), node_limit: roots.len() > max_nodes, depth_limit: false };
    let mut frontier = out.nodes.clone();
    let mut links: Vec<_> = side.index.traversal_links().filter(|e|DEPENDENCIES.contains(&e.predicate.as_str())).collect();
    links.sort_by(|a,b|a.id.cmp(&b.id));
    // One extra inspection detects a real omitted frontier, rather than
    // claiming a depth limit merely because a leaf is at maximum distance.
    for layer in 1..=depth+1 {
        let mut next = BTreeMap::new();
        for link in &links {
            let sources = endpoints(link, !incoming);
            let Some((_, (_, trail))) = frontier.iter().find(|(node,(distance,_))|sources.contains(*node)
                && (*distance==0 || !test_node(side,node))) else { continue; };
            let mut trail = trail.clone();
            trail.push(side.link_ref(link));
            for node in endpoints(link, incoming) {
                if let Some((distance,_)) = out.nodes.get(&node) {
                    if *distance == 0 && layer > 0 {
                        out.reached.entry(node).or_insert_with(|| (layer,trail.clone()));
                    }
                    continue;
                }
                if next.contains_key(&node) { continue; }
                if layer > depth { out.depth_limit = true; continue; }
                if out.nodes.len()+next.len() >= max_nodes { out.node_limit = true; continue; }
                next.insert(node, (layer,trail.clone()));
            }
        }
        if layer > depth || next.is_empty() { break; }
        out.nodes.extend(next.clone());
        frontier = next;
    }
    out
}

fn declared_components(side: &Side<'_>, nodes: &mut BTreeSet<String>, affected: &mut Vec<Value>, unknowns: &mut Vec<Value>, exact_scope: bool) {
    let mut known: BTreeMap<String,Vec<Value>> = nodes.iter().map(|n|(n.clone(),
        if side.source.files.contains_key(n) {vec![side.file_ref(n)]}else{vec![]})).collect();
    // Only technical owners seed intent. Semantic ownership propagated by
    // the index is a retrieval convenience, not a replacement for bindings.
    for (key,entity) in &side.index.entities {
        if !crate::architecture::declarations::SEMANTIC_KINDS.contains(&entity.kind.as_str()) {
            if let Some(path) = entity.files.iter().find(|p|nodes.contains(*p)) {
                known.entry(key.clone()).or_insert_with(||vec![side.file_ref(path)]);
            }
        }
    }
    let mut remaining = false;
    for _ in 0..64 {
        let before = known.clone();
        remaining = false;
        for link in side.index.traversal_links().filter(|e| e.class=="declaration" && BINDINGS.contains(&e.predicate.as_str())) {
            if known.contains_key(&link.to) { continue; }
            let from = before.get(&link.from);
            if let Some(trail) = from {
                let mut trail = trail.clone();trail.push(side.link_ref(link));
                known.insert(link.to.clone(),trail.clone());
                affected.push(component(side,&link.to,"declared_binding",0,trail,exact_scope));
                nodes.insert(link.to.clone());
                remaining = true;
            }
        }
        if !remaining { break; }
    }
    if remaining {
        unknowns.push(gap("declarations",Some(side.name),"Declared membership propagation reached its 64-iteration bound",
            vec![reference(&side.index.id,"/coverage/propagation")]));
    }
}

fn finding_links(value: &Value, ids: &mut BTreeSet<String>) {
    if let Some(object) = value.as_object() {
        if let Some(id) = object.get("link_id").and_then(Value::as_str) {ids.insert(id.into());}
        for child in object.values() {finding_links(child,ids);}
    } else if let Some(items) = value.as_array() {for child in items {finding_links(child,ids);}}
}

fn side_gaps(side: &Side<'_>, unknowns: &mut Vec<Value>) {
    let evidence = || vec![reference(&side.index.id,"/coverage")];
    if !side.capture_complete() { unknowns.push(gap("capture",Some(side.name),"Source inventory is partial or has omitted paths",evidence())); }
    if !query::complete(&side.source.view) { unknowns.push(gap("dependencies",Some(side.name),"Dependency analysis lacks a complete producer guarantee; missing consumers cannot establish absence",evidence())); }
    if !query::complete_for(&side.source.view,"CALLS_STATIC") { unknowns.push(gap("static_calls",Some(side.name),"Static call analysis is missing or partial; dynamic and generated calls remain outside scope",evidence())); }
    if side.source.view.invalidated { unknowns.push(gap("retractions",Some(side.name),"Retractions invalidated part of this source view; withdrawn assertions are excluded",evidence())); }
    let summary = index::summary(&side.index);
    if summary["unresolved_occurrences"].as_u64().unwrap_or(0)>0 || summary["unresolved_bindings"].as_u64().unwrap_or(0)>0 {
        unknowns.push(gap("resolution",Some(side.name),"Producer occurrences or declaration bindings remain unresolved",evidence()));
    }
    if !rows(&side.index.coverage,"profile_boundaries").is_empty() {
        unknowns.push(gap("profiles",Some(side.name),"Some semantic profiles describe a different snapshot and were not joined",evidence()));
    }
    if side.index.coverage["propagation"]["feature_membership_complete"]==false
        || side.index.coverage["propagation"]["file_associations_complete"]==false {
        unknowns.push(gap("declarations",Some(side.name),"Index ownership or feature propagation is incomplete",evidence()));
    }
    if summary["omitted_relationship_occurrences"].as_u64().unwrap_or(0)>0
        && side.index.coverage["adjacency"]["overflow_report_id"].is_null() {
        unknowns.push(gap("graph",Some(side.name),"Legacy index omitted relationships without retaining an overflow inventory",evidence()));
    }
}

fn policy(l: &Ledger, side: &Side<'_>, nodes: &BTreeSet<String>, constraints: &mut Vec<Value>, verification: &mut Vec<Value>, unknowns: &mut Vec<Value>) -> Result<()> {
    if rows(&side.source.view.bundle,"declaration_records").is_empty() {
        unknowns.push(gap("policy",Some(side.name),"No registered architecture declaration is retained for this source view",vec![]));
        return Ok(());
    }
    let options = query::Options {view:side.source.view.id.clone(),allow_partial:true,limit:1000,..Default::default()};
    let report = match query::policy(l,&options) {
        Ok(report)=>report,
        Err(error) if error.to_string().starts_with("FRESHNESS:") => {
            unknowns.push(gap("policy",Some(side.name),&error.to_string(),vec![reference(&side.index.id,"/coverage")]));
            return Ok(());
        },
        Err(error)=>return Err(error),
    };
    for row in query::all_rows(l,&report)? {
        if !row.detail["rule_id"].is_string() {
            unknowns.push(gap("policy",Some(side.name),row.detail["reason"].as_str().unwrap_or("Policy could not be resolved"),
                vec![reference(&text(&report["response"],"id"),&row.handle)]));
            continue;
        }
        let touches = |key:&str| nodes.contains(key) || side.index.entities.get(key).is_some_and(|e|!e.files.is_disjoint(nodes));
        if !nodes.contains(crate::architecture::declarations::ARTIFACT_PATH)
            && !touches(&text(&row.detail,"from")) && !touches(&text(&row.detail,"to")) { continue; }
        let refs = vec![reference(&text(&report["response"],"id"),&row.handle)];
        constraints.push(json!({"side":side.name,"rule_id":row.detail["rule_id"],"status":row.conclusion,
            "mode":"report-only","detail":row.detail,"evidence_refs":refs}));
        verification.push(json!({"id":format!("review-policy:{}",text(&row.detail,"rule_id")),"side":side.name,"path":null,
            "basis":"declared_constraint","requirement":"suggested","execution_status":"not_executed",
            "description":"Review the applicable report-only dependency constraint with an external validation system",
            "evidence_refs":refs}));
        if row.conclusion=="unknown" {
            unknowns.push(gap("policy",Some(side.name),"An applicable declared rule has unknown evidence coverage",refs));
        }
    }
    Ok(())
}

fn test_suggestions(side: &Side<'_>, nodes: &BTreeSet<String>, verification: &mut Vec<Value>) {
    let mut tests: BTreeMap<String,Vec<Value>> = BTreeMap::new();
    for link in side.index.traversal_links().filter(|e|e.predicate=="TESTS"
        && (nodes.contains(&e.to) || !nodes.is_disjoint(&e.to_files))) {
        for path in link.from_files.iter().filter(|p|side.source.files.get(*p).is_some_and(|f|f.role=="test")) {
            tests.entry(path.clone()).or_default().push(side.link_ref(link));
        }
    }
    for path in nodes.iter().filter(|p|side.source.files.get(*p).is_some_and(|f|f.role=="test")) {
        tests.entry(path.clone()).or_default().push(side.file_ref(path));
    }
    for (path, refs) in tests {
        verification.push(json!({"id":format!("test:{path}"),"side":side.name,"path":path,
            "basis":"static_test_association","requirement":"suggested","execution_status":"not_executed",
            "description":"Inspect and run this associated test externally; relevance and assertions have not been verified",
            "evidence_refs":refs}));
    }
}

fn interface_suggestions(side: &Side<'_>, nodes: &BTreeSet<String>, verification: &mut Vec<Value>) -> Result<()> {
    // Only explicit technical domain assignments support this candidate.
    // Shared feature membership does not establish an interface boundary.
    let mut domains: BTreeMap<String,BTreeSet<String>> = BTreeMap::new();
    let mut domain_refs: BTreeMap<String,Vec<Value>> = BTreeMap::new();
    for link in side.index.traversal_links().filter(|e|e.class=="declaration" && e.predicate=="BELONGS_TO_DOMAIN"
        && side.index.entities.get(&e.from).is_some_and(|entity|
            !crate::architecture::declarations::SEMANTIC_KINDS.contains(&entity.kind.as_str()))) {
        for path in &link.from_files {
            domains.entry(path.clone()).or_default().insert(link.to.clone());
            domain_refs.entry(path.clone()).or_default().push(side.link_ref(link));
        }
    }
    let mut checks: BTreeMap<(String,String,String),Vec<Value>> = BTreeMap::new();
    for link in side.index.traversal_links().filter(|e|DEPENDENCIES.contains(&e.predicate.as_str())) {
        if !nodes.contains(&link.from) && !nodes.contains(&link.to)
            && nodes.is_disjoint(&link.from_files) && nodes.is_disjoint(&link.to_files) {continue;}
        let cross_domain = link.from_files.iter().any(|from|link.to_files.iter().any(|to|
            domains.get(from).zip(domains.get(to)).is_some_and(|(a,b)|a.is_disjoint(b))));
        if link.predicate!="ROUTES_TO" && !cross_domain {continue;}
        let refs = checks.entry((link.predicate.clone(),link.from.clone(),link.to.clone())).or_default();
        refs.push(side.link_ref(link));
        if cross_domain {
            for path in link.from_files.iter().chain(&link.to_files) {
                if let Some(declarations) = domain_refs.get(path) {refs.extend(declarations.iter().cloned());}
            }
        }
    }
    for (key,refs) in checks {
        let id = identity::id("interface-check",&json!([key.0,key.1,key.2]))?;
        verification.push(json!({"id":id,"side":side.name,"path":null,"basis":"interface_relationship",
            "requirement":"suggested","execution_status":"not_executed",
            "description":"Inspect the contract at this endpoint or declared domain boundary; static relationships do not establish compatibility",
            "evidence_refs":refs}));
    }
    Ok(())
}

fn correspondence_rows(l: &Ledger, before: &Side<'_>, after: &Side<'_>, out: &mut Vec<Value>, unknowns: &mut Vec<Value>) -> Result<BTreeMap<String,String>> {
    let mut remap = BTreeMap::new();
    if !history::prepared(l,&before.source.view,&after.source.view)? {
        unknowns.push(gap("correspondence",None,"No retained correspondence analysis for this snapshot pair; added/deleted paths are not automatically renamed",vec![]));
        return Ok(remap);
    }
    let options = query::Options {view:after.source.view.id.clone(),allow_partial:true,limit:1000,..Default::default()};
    let report = match query::history_lineage(l,&before.source.view.id,&options,"combined") {
        Ok(report)=>report,
        Err(error) if error.to_string().starts_with("FRESHNESS:") => {
            unknowns.push(gap("correspondence",None,&error.to_string(),vec![]));return Ok(remap);
        },
        Err(error)=>return Err(error),
    };
    for row in query::all_rows(l,&report)? {
        let refs = vec![reference(&text(&report["response"],"id"),&row.handle)];
        if row.detail["certainty_class"]=="exact_content_move" {
            remap.insert(text(&row.detail,"to"),text(&row.detail,"from"));
        } else {
            unknowns.push(gap("correspondence",None,"Historical correspondence is ambiguous or inferred; it was not used to merge identities",refs.clone()));
        }
        out.push(json!({"status":row.conclusion,"detail":row.detail,"evidence_refs":refs}));
    }
    Ok(remap)
}

type EdgeKey = (String,String,String,String);
fn relationship_keys(side: &Side<'_>, remap: &BTreeMap<String,String>) -> BTreeMap<EdgeKey,Vec<Value>> {
    let mut out: BTreeMap<_,Vec<Value>> = BTreeMap::new();
    for link in side.index.traversal_links().filter(|e|DEPENDENCIES.contains(&e.predicate.as_str())
        || BINDINGS.contains(&e.predicate.as_str()) || e.predicate=="FORBIDS_DEPENDENCY") {
        let from = remap.get(&link.from).unwrap_or(&link.from).clone();
        let to = remap.get(&link.to).unwrap_or(&link.to).clone();
        out.entry((link.predicate.clone(),from,to,link.class.clone())).or_default().push(side.link_ref(link));
    }
    out
}
fn relationship_delta(before: &Side<'_>, after: &Side<'_>, remap: &BTreeMap<String,String>) -> Vec<Value> {
    let old = relationship_keys(before,&BTreeMap::new());
    let new = relationship_keys(after,remap);
    let keys: BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
    keys.into_iter().filter_map(|key| {
        if old.contains_key(&key) && new.contains_key(&key) {return None;}
        let (state,refs) = if let Some(refs)=new.get(&key) {("observed_only_in_candidate",refs)}else{("observed_only_in_base",&old[&key])};
        Some(json!({"state":state,"predicate":key.0,"from":key.1,"to":key.2,"class":key.3,"evidence_refs":refs}))
    }).collect()
}

fn preview(doc: &mut Value, limit: usize) {
    let mut delivery = serde_json::Map::new();
    for section in SECTIONS {
        let rows = doc[*section].as_array_mut().expect("Impact section is an array");
        let total = rows.len();rows.truncate(limit);
        delivery.insert(format!("/{section}"),json!({"total":total,"returned":rows.len(),"omitted":total-rows.len()}));
    }
    doc["delivery"] = json!(delivery);
    continuation(doc);
}
fn continuation(doc: &mut Value) {
    doc["continue_with"] = doc["delivery"].as_object().unwrap().iter()
        .find(|(_,d)|d["omitted"].as_u64().unwrap_or(0)>0)
        .map(|(section,d)|reports::continuation(doc["report_id"].as_str().unwrap(),section,
            d["returned"].as_u64().unwrap_or(0) as usize,20)).unwrap_or(Value::Null);
}
/// Presentation-only trimming for the shared UTF-8 budget gateway.
pub(crate) fn trim(doc: &mut Value) -> bool {
    if doc["kind"]!="architectural_impact_contract" {return false;}
    let Some(section) = SECTIONS.iter().filter(|s|**s!="unknowns")
        .filter_map(|s|doc[*s].as_array().map(|a|(*s,a.len())))
        .filter(|(_,n)|*n>0).max_by_key(|(_,n)|*n).map(|(s,_)|s) else {return false;};
    let items = doc[section].as_array_mut().unwrap();items.pop();let returned = items.len();
    let delivery = &mut doc["delivery"][format!("/{section}")];
    let total = delivery["total"].as_u64().unwrap_or(0);
    *delivery = json!({"total":total,"returned":returned,"omitted":total.saturating_sub(returned as u64)});
    continuation(doc);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link(predicate: &str, from: &str, to: &str) -> Link {
        Link::new(predicate,from,to,"derived","fixture; static relationship",vec![],vec![],
            BTreeSet::from([from.into()]),BTreeSet::from([to.into()])).unwrap()
    }
    fn with_side(test: impl FnOnce(&mut Side<'_>)) {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");
        for path in ["src/a.rs","src/b.rs","src/c.rs","src/d.rs","tests/suite.rs"] {
            let file=root.join(path);std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file,"// static fixture\n").unwrap();
        }
        let ledger=crate::architecture::service::open_context(&temp.path().join("context.db"),&root).unwrap();
        let view=ledger.resolve("latest").unwrap();
        let mut side=Side::load(&ledger,"candidate",&view).unwrap();
        side.index.links.clear();side.index.overflow_links.clear();test(&mut side);
    }
    #[test]
    fn reverse_walk_includes_overflow_and_complete_shortest_witnesses() {
        with_side(|side| {
            side.index.links=vec![link("IMPORTS","src/b.rs","src/a.rs")];
            side.index.overflow_links=vec![link("IMPORTS","src/c.rs","src/b.rs")];
            let result=walk(side,&BTreeMap::from([("src/a.rs".into(),true)]),true,3,100);
            assert_eq!(result.nodes["src/c.rs"].0,2);
            assert_eq!(result.nodes["src/c.rs"].1.len(),3);
            assert_eq!(result.nodes["src/c.rs"].1[2],side.link_ref(&side.index.overflow_links[0]));
            assert!(!result.depth_limit);assert!(!result.node_limit);
        });
    }
    #[test]
    fn forward_and_reverse_scopes_do_not_merge_shared_dependencies() {
        with_side(|side| {
            side.index.links=vec![link("IMPORTS","src/a.rs","src/b.rs"),link("IMPORTS","src/c.rs","src/b.rs")];
            let roots=BTreeMap::from([("src/a.rs".into(),true)]);
            let forward=walk(side,&roots,false,3,100);let reverse=walk(side,&roots,true,3,100);
            assert!(forward.nodes.contains_key("src/b.rs"));
            assert!(!forward.nodes.contains_key("src/c.rs"));assert!(!reverse.nodes.contains_key("src/c.rs"));
        });
    }
    #[test]
    fn test_associations_and_test_symbols_cannot_bridge_to_other_implementations() {
        with_side(|side| {
            side.index.links=vec![link("TESTS","tests/suite.rs","src/a.rs"),link("TESTS","tests/suite.rs","src/b.rs"),
                link("IMPORTS","src/a.rs","tests/suite.rs"),link("IMPORTS","tests/suite.rs","src/b.rs")];
            let result=walk(side,&BTreeMap::from([("src/a.rs".into(),true)]),false,3,100);
            assert!(result.nodes.contains_key("tests/suite.rs"));assert!(!result.nodes.contains_key("src/b.rs"));
            let reverse=walk(side,&BTreeMap::from([("src/a.rs".into(),true)]),true,3,100);
            assert_eq!(reverse.nodes.len(),1);
        });
    }
    #[test]
    fn depth_and_node_bounds_are_detected_only_for_actual_unvisited_frontiers() {
        with_side(|side| {
            side.index.links=vec![link("IMPORTS","src/b.rs","src/a.rs"),link("IMPORTS","src/c.rs","src/b.rs")];
            let roots=BTreeMap::from([("src/a.rs".into(),true)]);
            assert!(walk(side,&roots,true,1,100).depth_limit);
            assert!(walk(side,&roots,true,3,1).node_limit);
            assert!(!walk(side,&roots,true,2,100).depth_limit);
        });
    }
    #[test]
    fn observed_only_relationship_deltas_never_claim_negative_coverage() {
        with_side(|before|with_side(|after| {
            before.index.links=vec![link("IMPORTS","src/a.rs","src/b.rs")];
            let delta=relationship_delta(before,after,&BTreeMap::new());
            assert_eq!(delta[0]["state"],"observed_only_in_base");
            assert_eq!(rows(&delta[0],"evidence_refs").len(),1);
        }));
    }
}
