//! Selective reads and explicit analytical expansion over immutable evidence.
use super::*;

fn pinned_case(l: &Ledger, id: &str) -> Result<(Value, Index)> {
    let doc = case::load_doc(l, id)?;
    let ix = index::retained(
        l,
        doc["index_id"].as_str().ok_or("TASK: Case has no index")?,
    )?
    .ok_or("TASK: Case index is unavailable; rebuild from its retained source view")?;
    require(
        doc["view_id"] == ix.source_view,
        "TASK",
        "Case source differs from its index",
    )?;
    Ok((doc, ix))
}
fn lexical_report(l: &Ledger, doc: &Value) -> Result<Option<Value>> {
    let Some(id) = doc["coverage"]["retrieval"]["lexical_discovery"]["report_id"].as_str() else {
        return Ok(None);
    };
    let report = l.report(id)?;
    require(
        report["view_id"] == doc["view_id"] && report["kind"] == "lexical-discovery",
        "TASK",
        "Lexical report differs from its case pin",
    )?;
    Ok(Some(report))
}
fn pointer(path: &str) -> String {
    path.replace('~', "~0").replace('/', "~1")
}
fn offset(row: &Value) -> u64 {
    rows(row, "spans")
        .first()
        .and_then(|s| s["byte_range"]["start"].as_u64())
        .unwrap_or(0)
}
fn compact(row: &Value, index: usize, ix: &Index) -> Value {
    let facets: BTreeSet<_> = rows(row, "signals")
        .iter()
        .filter_map(|s| s["facet"].as_str())
        .collect();
    json!({"path":row["path"],"rank":row["rank"],"score":row["score"],"selected":row["selected"],
        "role":row["role"].as_str().or_else(||ix.files.get(&text(row,"path")).map(|f|f.role.as_str())),"facets":facets,"source_offset":offset(row),
        "source_ref":row["source_ref"].as_str().filter(|r|!r.is_empty()).map(str::to_owned).unwrap_or_else(||format!("/candidates/{index}"))})
}
fn path_matches(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_start_matches("./").trim_end_matches('/');
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}
fn matches(row: &Value, a: &Value) -> bool {
    if let Some(prefix) = a["path"].as_str() {
        if !path_matches(
            row["path"]
                .as_str()
                .or(row["target"].as_str())
                .unwrap_or(""),
            prefix,
        ) {
            return false;
        }
    }
    if let Some(role) = a["role"].as_str() {
        if row["role"] != role {
            return false;
        }
    }
    if let Some(selected) = a["selected"].as_bool() {
        if row["selected"].as_bool() != Some(selected) {
            return false;
        }
    }
    if let Some(kind) = a["work_kind"].as_str() {
        if row["kind"] != kind {
            return false;
        }
    }
    if a["min_score"]
        .as_u64()
        .is_some_and(|min| row["score"].as_u64().is_none_or(|score| score < min))
    {
        return false;
    }
    if a["max_score"]
        .as_u64()
        .is_some_and(|max| row["score"].as_u64().is_none_or(|score| score > max))
    {
        return false;
    }
    if a["graph_only"] == true
        && (rows(row, "facets")
            .iter()
            .any(|f| f == "lexical" || f == "anchors")
            || rows(row, "facets").is_empty())
    {
        return false;
    }
    true
}

pub fn inventory(l: &Ledger, a: &Value) -> Result<Value> {
    let id = a["case_id"]
        .as_str()
        .ok_or("ARGUMENTS: case_id is required")?;
    let (doc, ix) = pinned_case(l, id)?;
    let kind = a["kind"].as_str().unwrap_or("candidates");
    require(
        a["min_score"]
            .as_u64()
            .zip(a["max_score"].as_u64())
            .is_none_or(|(min, max)| min <= max),
        "ARGUMENTS",
        "min_score must not exceed max_score",
    )?;
    require(
        a["role"]
            .as_str()
            .is_none_or(|role| source::ARTIFACT_ROLES.contains(&role)),
        "ARGUMENTS",
        "Unknown artifact role",
    )?;
    let mut items = vec![];
    let mut coverage = json!({"state":"retained","filters_before_paging":true});
    let mut source_report = json!(id);
    match kind {
        "candidates" => {
            if !doc["candidates"].is_array()
                && source::ARTIFACT_ROLES
                    .iter()
                    .any(|role| !rows(&doc["slice"], role).is_empty())
            {
                coverage = json!({"state":"legacy_inventory_unavailable",
                    "remediation":"Read the historical selected /slice sections, or build a new case from the retained source view for a complete candidate inventory. An absent legacy candidate array is not an empty universe."});
            }
            items = rows(&doc, "candidates")
                .iter()
                .enumerate()
                .map(|(i, r)| compact(r, i, &ix))
                .collect()
        }
        "lexical" => {
            source_report = json!(ix.id);
            if let Some(report) = lexical_report(l, &doc)? {
                let admitted = doc["coverage"]["retrieval"]["lexical_discovery"]["candidate_limit"]
                    .as_u64()
                    .unwrap_or(1000);
                let promoted: BTreeSet<_> = rows(&doc, "lexical_candidates")
                    .iter()
                    .filter_map(|r| r["path"].as_str())
                    .collect();
                for row in rows(&report, "items") {
                    let path = text(row, "path");
                    let tail = row["rank"].as_u64().unwrap_or(0) > admitted
                        && !promoted.contains(path.as_str());
                    if a["tail_only"] == true && !tail {
                        continue;
                    }
                    items.push(json!({"path":path,"role":row["role"],"lexical_rank":row["rank"],"tail":tail,
                        "source_ref":format!("/files/{}",pointer(&path)),"source_offset":row["byte_range"]["start"]}));
                }
            } else {
                coverage = json!({"state":if doc["coverage"]["retrieval"]["lexical_discovery"]["enabled"]==false {"disabled_by_request"} else {"legacy_inventory_unavailable"},
                "remediation":"Build a new case from the retained source view with lexical retrieval enabled. The old case has no complete lexical inventory."});
            }
        }
        "files" => {
            source_report = json!(ix.id);
            let selected: BTreeSet<_> = source::ARTIFACT_ROLES
                .iter()
                .flat_map(|role| rows(&doc["slice"], role))
                .map(|r| text(r, "path"))
                .collect();
            items=ix.files.values().map(|file|json!({"path":file.path,"role":file.role,"bytes":file.bytes,
                "selected":selected.contains(&file.path),"source_ref":format!("/files/{}",pointer(&file.path)),"source_offset":0})).collect();
        }
        "work" => items = remaining_work(l, &doc, &ix)?,
        _ => return Err("ARGUMENTS: kind must be candidates, lexical, files or work".into()),
    }
    items.retain(|row| matches(row, a));
    let mut filters = a.clone();
    for field in ["offset", "limit", "max_bytes"] {
        filters.as_object_mut().unwrap().remove(field);
    }
    let retained = crate::architecture::reports::retain_items(
        l,
        json!({"kind":"task_inventory","inventory_kind":kind,
        "case_id":id,"view_id":doc["view_id"],"index_id":ix.id,"pinned_views":doc["pinned_views"],
        "retraction_epoch":doc["provenance"]["retraction_epoch"],"filters":filters,"items":items,"coverage":coverage,
        "fetch_source":{"operation":"explain","arguments":{"report_id":source_report}},
        "interpretation":"Compact inventory; source inspection, full-file digest verification, and case assembly are independent."}),
        0,
    )?;
    crate::architecture::reports::page(
        l,
        &json!({"report_id":retained["report_id"],"section":"/items",
        "offset":a["offset"].as_u64().unwrap_or(0),"limit":a["limit"].as_u64().unwrap_or(100)}),
    )
}

fn remaining_work(l: &Ledger, doc: &Value, ix: &Index) -> Result<Vec<Value>> {
    let case_id = &doc["id"];
    let mut work = vec![];
    for (i, row) in rows(doc, "candidates").iter().enumerate() {
        let mut item = compact(row, i, ix);
        item["kind"] = json!("candidate");
        item["status"] = json!("available");
        item["reason"] = json!(if row["selected"] == true {
            "selected_reading_plan"
        } else {
            "outside_initial_reading_plan"
        });
        item["inspection"] = json!("not_recorded_by_server");
        item["action"] = json!({"operation":"explain","arguments":{"report_id":case_id,
            "ref":item["source_ref"],"source_offset":item["source_offset"]}});
        work.push(item);
    }
    let graph = &doc["coverage"]["retrieval"]["graph_expansion"];
    if let Some(report) = graph["remaining_work"]["report_id"].as_str() {
        let retained = l.report(report)?;
        require(
            retained["index_id"] == ix.id && retained["view_id"] == ix.source_view,
            "TASK",
            "Discovery work pin differs",
        )?;
        work.extend(rows(&retained, "items").iter().cloned());
    } else if [
        "omitted_starters",
        "unexpanded_frontier_files",
        "unexpanded_frontier_keys",
    ]
    .iter()
    .any(|key| graph[key].as_u64().unwrap_or(0) > 0)
    {
        work.push(json!({"kind":"discovery_boundary","status":"unsupported","reason":"legacy_frontier_not_retained",
            "remediation":"Build a new case from this retained source view. The historical case has counts and examples, not the full frontier."}));
    }
    if let Some(lexical) = lexical_report(l, doc)? {
        let admitted = doc["coverage"]["retrieval"]["lexical_discovery"]["candidate_limit"]
            .as_u64()
            .unwrap_or(1000);
        let promoted: BTreeSet<_> = rows(doc, "lexical_candidates")
            .iter()
            .filter_map(|r| r["path"].as_str())
            .collect();
        for row in rows(&lexical, "items")
            .iter()
            .filter(|r| r["rank"].as_u64().unwrap_or(0) > admitted)
        {
            let path = text(row, "path");
            if promoted.contains(path.as_str()) {
                continue;
            }
            work.push(json!({"kind":"lexical_tail","path":path,"role":row["role"],"lexical_rank":row["rank"],
                "status":"available","reason":"lexical_admission_limit",
                "action":{"operation":"expand_case","arguments":{"case_id":case_id,"paths":[path]}}}));
        }
    }
    if ix.coverage["omitted_relationship_occurrences"]
        .as_u64()
        .unwrap_or(0)
        > 0
    {
        let adjacency = index::adjacency_coverage(ix);
        work.push(json!({"kind":"relationship_overflow","status":if adjacency["state"]=="retained" {"available"} else {"unsupported"},
            "reason":"hot_adjacency_limit","coverage":adjacency,
            "action":ix.coverage["adjacency"]["overflow_report_id"].as_str().map(|id|
                crate::architecture::reports::continuation(id,"/items",0,100))}));
    }
    for (i, producer) in rows(&ix.coverage, "producer_boundaries").iter().enumerate() {
        if producer["count"].as_u64().unwrap_or(0) > 0
            || producer["limitation_count"].as_u64().unwrap_or(0) > 0
        {
            work.push(json!({"kind":"producer_partition","target":producer["view_id"],"status":"unsupported",
                "reason":"producer_coverage_boundary","scope":producer["scope"],"count":producer["count"],
                "action":{"operation":"explain","arguments":{"report_id":ix.id,"ref":format!("/coverage/producer_boundaries/{i}")}},
                "remediation":"Inspect the pinned producer detail report; unresolved or unsupported analysis requires a new producer publication and a new case."}));
        }
    }
    for gap in rows(doc, "not_covered") {
        work.push(json!({"kind":"evidence_boundary","status":"unsupported","reason":gap["area"],"boundary":gap,
            "remediation":"Follow the boundary's retained detail or capture/producer/external-verification requirement; paging alone does not resolve unavailable evidence."}));
    }
    for item in &mut work {
        if item["action"]["operation"] == "expand_case" {
            item["action"]["arguments"]["case_id"] = case_id.clone();
        }
        item["id"] = json!(identity::id("task-work", &json!([case_id, item]))?);
    }
    Ok(work)
}

pub fn expand_case(l: &Ledger, a: &Value) -> Result<Value> {
    let id = a["case_id"]
        .as_str()
        .ok_or("ARGUMENTS: case_id is required")?;
    let (parent, mut ix) = pinned_case(l, id)?;
    require(
        parent["provenance"]["retraction_epoch"].as_u64() == Some(l.retraction_epoch()?)
            && !l.pending_source()?,
        "TASK",
        "Cannot expand historical/retracted or pending evidence; build a new pinned case",
    )?;
    let mut request: request::Request = serde_json::from_value(parent["request"].clone())?;
    let initial_max_files = parent["coverage"]["retrieval"]["revision"]["initial_max_files"]
        .as_u64()
        .or(parent["coverage"]["retrieval"]["max_files"].as_u64())
        .or(parent["request"]["max_files"].as_u64());
    if let Some(max) = initial_max_files {
        request.max_files = max as usize;
    }
    request.view = ix.source_view.clone();
    let snapshot = Snapshot::load_with(l, &ix.source_view, &serde_json::to_value(&request.scope)?)?;
    let pins: BTreeSet<_> = snapshot
        .profiles
        .iter()
        .chain(snapshot.evidence_views.iter())
        .map(|v| v.id.clone())
        .collect();
    require(
        pins == crate::architecture::contracts::strings(&ix.coverage["profile_views"])
            .into_iter()
            .collect(),
        "TASK",
        "Producer profiles changed; build a new case instead of revising the old scope",
    )?;
    let lexical = lexical_report(l, &parent)?;
    let mut paths: BTreeSet<String> = crate::architecture::contracts::strings(&a["paths"])
        .into_iter()
        .collect();
    let mut lexical_next = None;
    if let Some(report_id) = a["lexical_report_id"].as_str() {
        require(
            Some(report_id)
                == parent["coverage"]["retrieval"]["lexical_discovery"]["report_id"].as_str(),
            "TASK",
            "Lexical report belongs to another case query",
        )?;
        let report = lexical
            .as_ref()
            .ok_or("TASK: No pinned lexical discovery report")?;
        let start = a["lexical_offset"].as_u64().unwrap_or(1000) as usize;
        let count = a["lexical_limit"].as_u64().unwrap_or(20) as usize;
        require(
            start <= rows(report, "items").len(),
            "ARGUMENTS",
            "Lexical offset is outside the pinned inventory",
        )?;
        paths.extend(
            rows(report, "items")
                .iter()
                .skip(start)
                .take(count)
                .map(|r| text(r, "path")),
        );
        let end = start.saturating_add(count).min(rows(report, "items").len());
        lexical_next = (end < rows(report, "items").len()).then_some(end);
    }
    require(
        !paths.is_empty() && paths.len() <= 1000,
        "ARGUMENTS",
        "Expand one to 1000 retained paths per revision",
    )?;
    let hits: Vec<_> = lexical
        .as_ref()
        .map(|r| {
            rows(r, "items")
                .iter()
                .filter(|r| paths.contains(&text(r, "path")))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    require(
        paths.iter().all(|p| ix.files.contains_key(p)),
        "TASK",
        "Expansion paths must belong to this case's pinned retained source inventory",
    )?;
    snapshot.contents.scan(
        &snapshot.files,
        &paths.iter().map(String::as_str).collect::<Vec<_>>(),
        |_, _| {},
    )?;
    // Query/assembly may use the complete retained graph. Initial task ranking
    // continues to use the bounded hot index for baseline comparability.
    ix.links = index::query_links(l, &ix, "all")?;
    if index::adjacency_coverage(&ix)["state"] == "retained" {
        ix.coverage["assembly_relationship_scope"] = json!("all");
    }
    let initial = trace::build(l, &snapshot, &ix, &request)?;
    let mut candidates: retrieval::Candidates = initial
        .candidates
        .into_iter()
        .map(|c| (c.path.clone(), c))
        .collect();
    for row in rows(&parent, "candidates") {
        let c: retrieval::Candidate = serde_json::from_value(row.clone())?;
        candidates.entry(c.path.clone()).or_insert(c);
    }
    for path in &paths {
        candidates.entry(path.clone()).or_insert_with(|| {
            let mut candidate = retrieval::Candidate::new(path);
            candidate.authority =
                "explicit_scope_selection; not relevance or implementation proof".into();
            candidate
        });
    }
    let parent_slice: Vec<_> = source::ARTIFACT_ROLES
        .iter()
        .flat_map(|role| rows(&parent["slice"], role))
        .collect();
    for row in &parent_slice {
        let path = text(row, "path");
        candidates.entry(path.clone()).or_insert_with(|| {
            let mut candidate = retrieval::Candidate::new(&path);
            candidate.authority = "retained_parent_selection; not renewed relevance".into();
            candidate
        });
    }
    let mut lexical_candidates = initial.lexical_candidates;
    let new_hits: Vec<_> = hits
        .into_iter()
        .filter(|h| {
            !rows(&parent, "lexical_candidates")
                .iter()
                .any(|r| r["path"] == h["path"])
        })
        .collect();
    if !new_hits.is_empty() {
        require(
            request.enabled("lexical"),
            "TASK",
            "Lexical discovery was disabled for this case",
        )?;
        let report = lexical.as_ref().unwrap();
        retrieval::admit_lexical(
            &snapshot,
            &crate::architecture::contracts::strings(&report["terms"]),
            &new_hits,
            &parent["coverage"]["retrieval"]["lexical_discovery"]["report_id"],
            &mut candidates,
            &mut lexical_candidates,
        )?;
    }
    // Prior explicit lexical promotions stay in the revision even though the
    // normal discovery pass intentionally keeps its original admission bound.
    for row in rows(&parent, "lexical_candidates") {
        if !lexical_candidates.iter().any(|r| r["path"] == row["path"]) {
            lexical_candidates.push(row.clone());
        }
    }
    let mut ranked = retrieval::rank(&ix, &request, candidates);
    let mut selected: BTreeSet<_> = parent_slice.iter().map(|r| text(r, "path")).collect();
    selected.extend(paths.clone());
    let mut order: Vec<_> = parent_slice
        .iter()
        .map(|r| (r["rank"].as_u64().unwrap_or(u64::MAX), text(r, "path")))
        .collect();
    order.sort();
    let mut delivery: Vec<_> = order.into_iter().map(|(_, p)| p).collect();
    delivery.extend(
        paths
            .iter()
            .filter(|p| !delivery.contains(p))
            .cloned()
            .collect::<Vec<_>>(),
    );
    for candidate in &mut ranked {
        candidate.selected = selected.contains(&candidate.path);
        candidate.delivery_rank = delivery
            .iter()
            .position(|p| p == &candidate.path)
            .map(|i| i + 1);
    }
    let mut graph_coverage = initial.retrieval["graph_expansion"].clone();
    let mut remaining = BTreeMap::new();
    for coverage in [
        &graph_coverage,
        &parent["coverage"]["retrieval"]["graph_expansion"],
    ] {
        if let Some(id) = coverage["remaining_work"]["report_id"].as_str() {
            for item in rows(&l.report(id)?, "items") {
                remaining.insert(identity::id("discovery-work", item)?, item.clone());
            }
        }
    }
    for path in &paths {
        let item = json!({"kind":"promoted_graph_starter","target":path,"status":"available",
            "reason":"explicit_scope_expansion","action":{"operation":"neighbors","arguments":{
                "index_id":ix.id,"view":ix.source_view,"target":path,"direction":"both","relationship_scope":"all","depth":1}}});
        remaining.insert(identity::id("discovery-work", &item)?, item);
    }
    let work = crate::architecture::reports::retain_items(
        l,
        json!({"kind":"discovery_work","index_id":ix.id,
        "view_id":ix.source_view,"pinned_views":ix.coverage["profile_views"],"items":remaining.into_values().collect::<Vec<_>>()}),
        0,
    )?;
    graph_coverage["remaining_work"] = json!({"report_id":work["report_id"],"count":work["page"]["total"],
        "continue_with":crate::architecture::reports::continuation(work["report_id"].as_str().unwrap(),"/items",0,100)});
    let mut trace = trace::finish(
        l,
        &snapshot,
        &ix,
        &request,
        initial.seed,
        ranked,
        lexical_candidates,
        initial.retrieval["lexical_discovery"].clone(),
        initial.retrieval["join_discovery"].clone(),
        graph_coverage,
    )?;
    trace.retrieval["revision"] = json!({"parent_case_id":id,"explicit_paths":paths,
        "initial_max_files":initial_max_files,"assembly_files":trace.paths.len(),
        "semantics":"Explicit assembly scope; inspection and verification do not select files automatically."});
    let case = case::retain(
        l,
        case::assemble(
            &request,
            &snapshot,
            &ix,
            &trace,
            &request.duty.policy().case,
            case::LedgerState {
                watermark: l.watermark()?,
                pending_source: l.pending_source()?,
                retraction_epoch: l.retraction_epoch()?,
            },
        )?,
    )?;
    let child_pins = l.report(&case.id)?["pinned_views"].clone();
    Ok(
        json!({"kind":"case_revision","parent_case_id":id,"case_id":case.id,"report_id":case.id,
        "view_id":ix.source_view,"index_id":ix.id,"pinned_views":child_pins,
        "assembly_files":trace.paths.len(),"promoted_paths":paths.len(),
        "details":{"operation":"explain","arguments":{"report_id":case.id,"ref":"/coverage/retrieval/revision"}},
        "continue_with":{"operation":"inventory","arguments":{"case_id":case.id,"kind":"candidates","selected":true}},
        "expansion_continue_with":lexical_next.map(|offset|json!({"operation":"expand_case","arguments":{
            "case_id":case.id,"lexical_report_id":a["lexical_report_id"],"lexical_offset":offset,"lexical_limit":a["lexical_limit"].as_u64().unwrap_or(20)}}))}),
    )
}
