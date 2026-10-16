use super::*;
#[allow(clippy::too_many_arguments)] // One constructor owns the named response dimensions.
pub(crate) fn finish(
    l: &Ledger,
    operation: &str,
    view_name: &str,
    views: &[View],
    ctx: Option<Value>,
    mut findings: Vec<Finding>,
    o: &Options,
    resolution_complete: bool,
) -> Result<Value> {
    require(
        o.historical
            || views
                .iter()
                .all(|v| v.retraction_watermark == l.retraction_epoch().unwrap_or(u64::MAX)),
        "FRESHNESS",
        "Retraction occurred during query; retry with a newly pinned view",
    )?;
    let ctx = ctx.unwrap_or_else(|| context(&views[0]));
    let mut limitations = Vec::new();
    for v in views {
        limitations.extend(restrictions(l, v, o)?);
    }
    let watermark = views
        .iter()
        .flat_map(|v| {
            v.projections
                .iter()
                .filter_map(|p| p["ledger_watermark"].as_u64())
                .chain([v.overlay_watermark])
        })
        .max()
        .unwrap_or(0);
    let projections = if views.len() == 1 {
        views[0].projections.clone()
    } else {
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for v in views {
            ids.extend(v.eligible.clone());
        }
        let generation = views.iter().map(|v| v.generation).max().unwrap();
        ["graph","retrieval"].into_iter().map(|kind|{let mut p=json!({"kind":kind,"context":ctx,"ledger_watermark":watermark,"generation":generation,"input_assertion_ids":ids,"eligible_classes":["evidence","derived","declaration","claim"]});p["id"]=json!(identity::id("projection",&json!({"pinned_source_projections":views.iter().map(|v|v.projections.clone()).collect::<Vec<_>>(),"manifest":p})).unwrap());p}).collect()
    };
    let truncated = findings.len() > o.limit;
    // The response contract forbids a pass in a truncated or otherwise incomplete answer.
    let pending_latest = o.view == "latest" && !o.historical && l.pending_source()?;
    let effective_complete = resolution_complete && !truncated && !pending_latest;
    if !effective_complete {
        for f in &mut findings {
            if f.conclusion == "pass" {
                f.conclusion = "unknown";
                f.detail["limitation"] =
                    json!("Full coverage and an untruncated response are required for compliance.");
            }
        }
    }
    if truncated {
        limitations.push(
            "Result presentation is paginated; continuation remains pinned to this report.".into(),
        );
    }
    match operation {
        "session" | "session-events" | "session-files" | "session-commands" => limitations.push("File reads and edits cover the profile's supported tools only; files touched by other tools or by shell commands are not reconstructed.".into()),
        "session-changes" => limitations.push("Recorded edits and changes between the selected source views are distinct facts: a change between the views is not attributed to the session, and Briefcase does not check that the views bracket it. Joins use exact repository paths only.".into()),
        _ => {}
    }
    if ["callgraph", "reachability"].contains(&operation) {
        if !resolution_complete {
            limitations.push("CALLS_STATIC coverage or execution analysis is incomplete; missing static paths cannot establish absence.".into());
        }
        limitations.push("Producer-supported exact direct-call scope only; unresolved dynamic/interface dispatch, external targets and generated code remain coverage boundaries. CALLS_STATIC is not CALLS_OBSERVED.".into());
    }
    limitations.sort();
    limitations.dedup();
    let outcome = if findings.iter().any(|f| f.conclusion == "unknown") {
        "unknown"
    } else if effective_complete {
        "complete"
    } else {
        "partial"
    };
    let report_id = identity::id(
        "report",
        &json!({"operation":operation,"context":ctx,"projections":projections,"details":findings.iter().map(|f|&f.detail).collect::<Vec<_>>(),"ids":findings.iter().map(|f|&f.ids).collect::<Vec<_>>(),"outcome":outcome,"historical":o.historical,"limitations":limitations,"page_size":o.limit}),
    )?;
    let mut result_rows = Vec::new();
    let mut details = BTreeMap::new();
    let mut explanations = Vec::new();
    for (index, f) in findings.iter().enumerate() {
        let row_id = format!("{report_id}:row:{index}");
        let handle = format!("explain:{report_id}:{index}");
        result_rows.push(json!({"id":row_id,"assertion_ids":f.ids,"explanation_handle":handle,"conclusion":f.conclusion}));
        details.insert(row_id, f.detail.clone());
        explanations.push((handle,json!({"report_id":report_id,"operation":operation,"context":ctx,"projection_ids":projections.iter().map(|p|text(p,"id")).collect::<Vec<_>>(),"views":views.iter().map(|v|&v.id).collect::<Vec<_>>(),"assertion_ids":f.ids,"detail":f.detail,"conclusion":f.conclusion,"limitations":limitations,"selection_reasons":["named architecture query","requested captured scope"],"historical":o.historical})));
    }
    let response = json!({"schema_version":"1.0","id":report_id,"operation":operation,"context":ctx,"view":view_name,"projection_ids":projections.iter().map(|p|text(p,"id")).collect::<Vec<_>>(),"ledger_watermark":watermark,"outcome":outcome,"results":result_rows,"resolution_complete":effective_complete,"required_predicates":required_predicates(operation),"limitations":limitations,"truncated":false});
    let report = json!({"response":response,"details":details,"manifests":projections,"pinned_views":views.iter().map(|v|&v.id).collect::<Vec<_>>(),"historical":o.historical,"page_size":o.limit,"retraction_epoch":l.retraction_epoch()?});
    let bundle = merge_bundle(views, &projections)?;
    let mut payloads = contracts::Payloads::new();
    for v in views {
        payloads.extend(l.payloads(&v.bundle)?);
    }
    // Validation is deterministic over bundle and payload bytes, so a verdict
    // for identical bytes under the identical validator can be reused. Any
    // byte change misses the cache and revalidates fully; per-query taint and
    // response checks always run fresh.
    let mut keyed = serde_json::to_vec(&bundle)?;
    for (id, bytes) in &payloads {
        keyed.extend(id.as_bytes());
        keyed.extend(identity::bytes_digest(bytes).as_bytes());
    }
    let digest = identity::bytes_digest(&keyed);
    let validator = contracts::validator_version()?;
    let validation_cache_hit = l.validation_cached(&digest, &validator)?;
    if validation_cache_hit {
        let taint = contracts::taint(&bundle)?;
        contracts::validate_query_with_taint(&report["response"], &bundle, &taint)?;
    } else {
        contracts::validate_query(&report["response"], &bundle, &payloads)?;
        l.record_validation(&digest, &validator)?;
    }
    l.save_report(&report_id, &report, &explanations)?;
    let mut report = report;
    report["freshness"] = super::super::freshness::for_views(l, views)?;
    report["validation_cache_hit"] = json!(validation_cache_hit);
    page(l, report, 0, o.limit)
}
pub(crate) fn page(l: &Ledger, mut report: Value, offset: usize, limit: usize) -> Result<Value> {
    require(
        report["historical"] == true
            || report["retraction_epoch"].as_u64() == Some(l.retraction_epoch()?),
        "FRESHNESS",
        "Pinned current report predates a retraction; rerun the query",
    )?;
    let all = rows(&report["response"], "results").to_vec();
    require(
        offset <= all.len(),
        "PAGINATION",
        "Offset outside pinned report",
    )?;
    let end = (offset + limit).min(all.len());
    let ids: BTreeSet<_> = all[offset..end].iter().map(|r| text(r, "id")).collect();
    report["response"]["results"] = json!(all[offset..end]);
    report["details"]
        .as_object_mut()
        .unwrap()
        .retain(|k, _| ids.contains(k));
    if end < all.len() {
        report["response"]["truncated"] = json!(true);
        report["response"]["continuation_token"] = json!(format!(
            "{}@{end}",
            report["response"]["id"].as_str().unwrap()
        ));
    }
    let retracted = l.retracted()?;
    if report["historical"] != true {
        for row in rows(&report["response"], "results") {
            require(strings(&row["assertion_ids"]).iter().all(|id|!retracted.contains(id)),"FRESHNESS","Pinned current report has since been retracted; rerun the query or inspect its historical explanation")?;
        }
    }
    contracts::schema(&report["response"], "query", Some("Response"))?;
    for key in ["manifests", "page_size", "retraction_epoch"] {
        report.as_object_mut().unwrap().remove(key);
    }
    report["report_id"]=report["response"]["id"].clone();
    report["section"]=json!("/response/results");
    report["page"]=json!({"offset":offset,"returned":end-offset,"total":all.len(),"remaining":all.len()-end,"next_offset":(end<all.len()).then_some(end)});
    report["continue_with"]=if end<all.len(){super::super::reports::continuation(report["report_id"].as_str().unwrap(),"/response/results",end,limit)}else{Value::Null};
    super::super::response::summarize_limitations(&mut report);
    super::super::freshness::attach(l, &mut report, None)?;
    Ok(report)
}
pub fn continuation(l: &Ledger, token: &str) -> Result<Value> {
    let (id, offset) = token.rsplit_once('@').ok_or("PAGINATION: Invalid token")?;
    let offset = offset.parse::<usize>()?;
    let report = l.report(id)?;
    let limit = report["page_size"].as_u64().unwrap() as usize;
    page(l, report, offset, limit)
}
pub fn explain(l: &Ledger, handle: &str, limit: usize, offset: usize) -> Result<Value> {
    explain_page(l, handle, limit, offset, 0)
}
pub fn explain_page(
    l: &Ledger,
    handle: &str,
    limit: usize,
    offset: usize,
    source_offset: usize,
) -> Result<Value> {
    explain_cached(
        l,
        handle,
        limit,
        offset,
        source_offset,
        &mut ExplainInputs::default(),
    )
}

/// Retained inputs of explanations over the same views (bundle, payloads and
/// assertions by ID), loaded once when many handles are explained together.
#[derive(Default)]
pub struct ExplainInputs(BTreeMap<Vec<String>, ExplainInput>);
pub(crate) struct ExplainInput {
    bundle: Value,
    payloads: contracts::Payloads,
    assertions: BTreeMap<String, Value>,
}
impl ExplainInputs {
    pub(crate) fn get(&mut self, l: &Ledger, views: Vec<String>) -> Result<&ExplainInput> {
        if !self.0.contains_key(&views) {
            let mut bundle = contracts::empty_bundle();
            let mut payloads = contracts::Payloads::new();
            for id in &views {
                let v = l.load(id)?;
                payloads.extend(l.payloads(&v.bundle)?);
                super::super::knowledge::merge(&mut bundle, &v.bundle)?;
            }
            for overlay in l.explanation_overlays(&views)? {
                payloads.extend(l.payloads(&overlay)?);
                super::super::knowledge::merge(&mut bundle, &overlay)?;
            }
            let assertions = rows(&bundle, "assertions")
                .iter()
                .map(|a| (text(a, "id"), a.clone()))
                .collect();
            self.0.insert(
                views.clone(),
                ExplainInput {
                    bundle,
                    payloads,
                    assertions,
                },
            );
        }
        Ok(&self.0[&views])
    }
}

/// `explain_page` reusing `inputs` across calls.
pub fn explain_cached(
    l: &Ledger,
    handle: &str,
    limit: usize,
    offset: usize,
    source_offset: usize,
    inputs: &mut ExplainInputs,
) -> Result<Value> {
    require(
        (1..=1000).contains(&limit),
        "EXPLAIN",
        "Limit must be 1..1000",
    )?;
    let mut e = l.explanation(handle)?;
    let removed = l.retracted()?;
    let ExplainInput {
        bundle,
        payloads,
        assertions,
    } = inputs.get(l, strings(&e["views"]))?;
    let mut todo: VecDeque<_> = strings(&e["assertion_ids"]).into();
    let mut seen = BTreeSet::new();
    let mut all = Vec::new();
    while let Some(id) = todo.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let a = assertions
            .get(&id)
            .ok_or("EXPLAIN: Missing retained assertion")?;
        let mut detail = json!({"assertion":a,"currently_retracted":removed.contains(&id)});
        let mut evidence_ids = strings(&a["evidence_ref_ids"]);
        if a["class"] == "derived" {
            let d = rows(bundle, "derivations")
                .iter()
                .find(|d| d["id"] == a["origin"]["derivation_id"])
                .unwrap();
            detail["derivation"] = d.clone();
            if let Some(mid) = d["manifest_id"].as_str() {
                detail["pass_manifest"] =
                    super::super::knowledge::row(bundle, "pass_manifests", mid)?.clone();
            }
            detail["input_artifacts"] = json!(strings(&d["input_artifact_ids"])
                .iter()
                .filter_map(|id| rows(bundle, "artifacts").iter().find(|a| a["id"] == *id))
                .collect::<Vec<_>>());
            todo.extend(strings(&d["input_assertion_ids"]));
        }
        if a["class"] == "evidence" {
            detail["run"] = rows(bundle, "runs")
                .iter()
                .find(|r| r["id"] == a["origin"]["run_id"])
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(config) = rows(bundle, "analysis_configs")
                .iter()
                .find(|c| c["id"] == detail["run"]["analysis_config_id"])
            {
                detail["analysis_configuration"] = config.clone();
            }
            detail["analyzer_artifacts"] = json!(rows(bundle, "artifacts")
                .iter()
                .filter(|a| a["path"]
                    .as_str()
                    .is_some_and(|p| p.starts_with(".briefcase/analyzers/")))
                .collect::<Vec<_>>());
        }
        if a["class"] == "claim" {
            let inference = rows(bundle, "inferences")
                .iter()
                .find(|r| r["id"] == a["origin"]["inference_id"])
                .ok_or("EXPLAIN: Missing inference origin")?;
            let packet = rows(bundle, "input_packets")
                .iter()
                .find(|r| r["id"] == inference["packet_id"])
                .ok_or("EXPLAIN: Missing input packet")?;
            detail["inference"] = inference.clone();
            detail["input_packet"] = packet.clone();
            let mut artifacts = Vec::new();
            for aid in [
                text(packet, "artifact_id"),
                text(inference, "prompt_artifact_id"),
                text(inference, "configuration_artifact_id"),
                text(inference, "request_artifact_id"),
                text(inference, "output_artifact_id"),
                text(inference, "taxonomy_artifact_id"),
            ] {
                if let Some(raw) = payloads.get(&aid) {
                    let artifact = super::super::knowledge::row(bundle, "artifacts", &aid)?;
                    require(
                        identity::bytes_digest(raw) == artifact["digest"],
                        "DIGEST",
                        "Retained inference payload is corrupted",
                    )?;
                    artifacts.push(json!({"artifact":artifact,"payload":source_page(raw,0,raw.len(),source_offset)?}));
                }
            }
            detail["inference_artifacts"] = json!(artifacts);
            todo.extend(strings(&packet["assertion_ids"]));
        }
        if a["class"] == "declaration" {
            let d = rows(bundle, "declaration_records")
                .iter()
                .find(|d| d["id"] == a["origin"]["declaration_record_id"])
                .unwrap();
            detail["declaration"] = d.clone();
            let aid = text(d, "artifact_id");
            let artifact = rows(bundle, "artifacts")
                .iter()
                .find(|a| a["id"] == aid)
                .ok_or("EXPLAIN: Missing policy artifact")?;
            let bytes = &payloads[&aid];
            require(
                identity::bytes_digest(bytes) == artifact["digest"],
                "DIGEST",
                "Retained policy payload is corrupted",
            )?;
            let source = source_page(bytes, 0, bytes.len(), source_offset)?;
            detail["policy_source"] = source["text"].clone();
            detail["policy_source_page"] = source;
            detail["policy_artifact"] = artifact.clone();
        }
        todo.extend(strings(&a["supports"]));
        let mut refs = Vec::new();
        for eid in evidence_ids.drain(..) {
            refs.push(source_ref(bundle, payloads, &eid, source_offset)?);
        }
        detail["sources"] = json!(refs);
        all.push(detail);
    }
    if let Some(eid) = e["detail"]["evidence_ref_id"].as_str() {
        all.push(json!({"unresolved_source":source_ref(bundle,payloads,eid,source_offset)?}));
    }
    if let Some(did) = e["detail"]["derivation_id"].as_str() {
        let derivation = super::super::knowledge::row(bundle, "derivations", did)?;
        let mut artifacts = Vec::new();
        for aid in strings(&derivation["input_artifact_ids"]) {
            let artifact = super::super::knowledge::row(bundle, "artifacts", &aid)?;
            let raw = payloads
                .get(&aid)
                .ok_or("EXPLAIN: Missing retained correspondence payload")?;
            require(
                identity::bytes_digest(raw) == artifact["digest"],
                "DIGEST",
                "Correspondence payload is corrupted",
            )?;
            artifacts.push(
                json!({"artifact":artifact,"payload":source_page(raw,0,raw.len(),source_offset)?}),
            );
        }
        e["correspondence_derivation"] = derivation.clone();
        e["correspondence_artifacts"] = json!(artifacts);
    }
    e["explanation_handle"] = json!(handle);
    // Search retained named policy prohibitions against known import paths. This
    // is a scoped disagreement search, not a guarantee of universal agreement.
    let edges: Vec<_> = assertions
        .values()
        .filter(|a| {
            a["proposition"]["predicate"] == "DEPENDS_ON"
                && a["class"] == "derived"
                && a["support_class"] == "evidence_only"
        })
        .map(|a| {
            (
                text(&a["proposition"], "subject_id"),
                text(&a["proposition"]["object"], "entity_version_id"),
                text(a, "id"),
            )
        })
        .collect();
    let mut conflicts = Vec::new();
    for a in assertions
        .values()
        .filter(|a| a["proposition"]["predicate"] == "FORBIDS_DEPENDENCY")
    {
        let from = text(&a["proposition"], "subject_id");
        let to = text(&a["proposition"]["object"], "entity_version_id");
        let scoped: Vec<_> = edges
            .iter()
            .filter(|(_, _, id)| assertions[id]["context"] == a["context"])
            .cloned()
            .collect();
        if let Some(witness) = path(&scoped, &from, &to) {
            let id = text(a, "id");
            if seen.contains(&id) || witness.iter().any(|id| seen.contains(id)) {
                conflicts.push(json!({"kind":"declared_prohibition_vs_known_import_path",
                    "declaration_assertion_id":id,"witness_assertion_ids":witness,
                    "currently_retracted":removed.contains(&id) || witness.iter().any(|id|removed.contains(id))}));
            }
        }
    }
    require(
        offset <= all.len().max(conflicts.len()),
        "EXPLAIN",
        "Offset outside support and conflict pages",
    )?;
    let start = offset.min(all.len());
    let end = (start + limit).min(all.len());
    e["support"] = json!(all[start..end]);
    e["support_count"] = json!(all.len());
    e["omitted_support_count"] = json!(all.len() - end);
    e["next_offset"] = if end < all.len() {
        json!(end)
    } else {
        Value::Null
    };
    let conflict_start = offset.min(conflicts.len());
    let conflict_end = (conflict_start + limit).min(conflicts.len());
    e["known_conflicts"] = json!(conflicts[conflict_start..conflict_end]);
    e["conflict_count"] = json!(conflicts.len());
    e["omitted_conflict_count"] = json!(conflicts.len() - conflict_end);
    e["next_conflict_offset"] = if conflict_end < conflicts.len() {
        json!(conflict_end)
    } else {
        Value::Null
    };
    e["conflict_search_scope"] = json!({"views":e["views"],
        "predicates":["FORBIDS_DEPENDENCY","DEPENDS_ON"],
        "assertions_examined":assertions.len(),
        "limitation":"Retained inputs for this explanation only; missing conflicts do not establish universal agreement. Current retractions are annotated."});
    super::super::response::summarize_limitations(&mut e);
    super::super::freshness::attach(l, &mut e, None)?;
    Ok(e)
}
pub(crate) fn source_page(
    bytes: &[u8],
    start: usize,
    end: usize,
    source_offset: usize,
) -> Result<Value> {
    require(
        end <= bytes.len() && start <= end,
        "EXPLAIN",
        "Retained evidence span invalid",
    )?;
    let raw = std::str::from_utf8(&bytes[start..end])?;
    let mut from = source_offset.min(raw.len());
    while from < raw.len() && !raw.is_char_boundary(from) {
        from += 1;
    }
    let mut to = (from + 4096).min(raw.len());
    while to > from && !raw.is_char_boundary(to) {
        to -= 1;
    }
    Ok(json!({"text":&raw[from..to],"source_byte_count":raw.len(),
        "returned_byte_range":{"start":start+from,"end":start+to},
        "omitted_source_bytes":raw.len()-to,
        "next_source_offset":if to < raw.len() {Some(to)} else {None}}))
}
pub(crate) fn source_ref(
    bundle: &Value,
    payloads: &contracts::Payloads,
    eid: &str,
    source_offset: usize,
) -> Result<Value> {
    let er = rows(bundle, "evidence_refs")
        .iter()
        .find(|r| r["id"] == eid)
        .ok_or("EXPLAIN: Unknown source reference")?;
    let a = rows(bundle, "artifacts")
        .iter()
        .find(|a| a["id"] == er["artifact_id"])
        .unwrap();
    let mut result = json!({"evidence_ref":er,"artifact":a});
    if er["locator"]["kind"] == "event" {
        // A runtime witness is one retained normalized observation, not a byte page.
        if let Some(bytes) = payloads.get(&text(a, "id")) {
            require(
                identity::bytes_digest(bytes) == a["digest"],
                "DIGEST",
                "Retained runtime observation is corrupted",
            )?;
            let doc = identity::parse(bytes)?;
            result["event"] = rows(&doc, "events")
                .iter()
                .find(|e| e["id"] == er["locator"]["event_id"])
                .cloned()
                .unwrap_or(Value::Null);
            result["observation"] = json!({"environment":doc["environment"],"interval":doc["interval"],
                "collector_id":doc["collector_id"],"sampling":doc["sampling"],"source_artifact_id":doc["source_artifact_id"]});
        } else {
            result["availability"] = json!("unavailable");
        }
        return Ok(result);
    }
    if let Some(bytes) = payloads.get(&text(a, "id")) {
        require(
            identity::bytes_digest(bytes) == a["digest"],
            "DIGEST",
            "Retained source payload is corrupted",
        )?;
        let start = er["locator"]["start"].as_u64().unwrap_or(0) as usize;
        let end = er["locator"]["end"].as_u64().unwrap_or(bytes.len() as u64) as usize;
        result.as_object_mut().unwrap().extend(
            source_page(bytes, start, end, source_offset)?
                .as_object()
                .unwrap()
                .clone(),
        );
    } else {
        result["availability"] = json!("unavailable");
    }
    Ok(result)
}

// Deterministic SCCs for the bounded architecture graph, independent of any inference.
pub fn components(edges: &[(String, String)]) -> Vec<Vec<String>> {
    let nodes: BTreeSet<_> = edges
        .iter()
        .flat_map(|(a, b)| [a.clone(), b.clone()])
        .collect();
    let mut remaining = nodes.clone();
    let mut result = Vec::new();
    pub(crate) fn reach(
        start: &str,
        edges: &[(String, String)],
        reverse: bool,
    ) -> BTreeSet<String> {
        let mut seen = BTreeSet::from([start.to_owned()]);
        let mut stack = vec![start.to_owned()];
        while let Some(n) = stack.pop() {
            for (a, b) in edges {
                let (from, to) = if reverse { (b, a) } else { (a, b) };
                if from == &n && seen.insert(to.clone()) {
                    stack.push(to.clone());
                }
            }
        }
        seen
    }
    while let Some(n) = remaining.first().cloned() {
        let f = reach(&n, edges, false);
        let r = reach(&n, edges, true);
        let group: Vec<_> = f.intersection(&r).cloned().collect();
        for n in &group {
            remaining.remove(n);
        }
        if group.len() > 1 || edges.contains(&(n.clone(), n)) {
            result.push(group);
        }
    }
    result
}

/// Shared report protocol entry for pinned query rows.
pub fn report_page(l:&Ledger,report:Value,offset:usize,limit:usize)->Result<Value>{page(l,report,offset,limit)}
