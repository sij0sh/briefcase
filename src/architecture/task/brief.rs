use super::*;

/// One fixed-schema task packet composed from retained source, architecture
/// assertions, feature declarations, Git and session recordings.
pub fn brief(l: &Ledger, a: &Value) -> Result<Value> {
    let task=a["task"].as_str().ok_or("ARGUMENTS: task is required")?;
    require(!task.trim().is_empty() && task.len()<=16384,"ARGUMENTS","task must contain 1..16384 bytes")?;
    let selected_duty=if a["duty"].is_string() {serde_json::from_value::<Duty>(a["duty"].clone())?} else {Duty::infer(task)};
    let template=selected_duty.template();
    let s=Snapshot::load_with(l,a["view"].as_str().unwrap_or("latest"),a)?;
    let ix=index::load(l,&s)?;
    let target=a["target"].as_str().unwrap_or(task);
    let (status,keys,anchors)=resolve(&ix,target,a["target"].is_string());
    let mut paths=anchors.clone();
    let depth=a["depth"].as_u64().unwrap_or(template.depth as u64) as usize;
    require((1..=3).contains(&depth),"ARGUMENTS","depth must be 1..3")?;
    // Expand artifact neighborhoods; CONTAINS/DECLARES are deliberately not
    // fanout operations from a repository or package into the whole tree.
    let mut expanded_keys=keys.clone();
    for _ in 0..depth {
        let before=paths.clone();
        let before_keys=expanded_keys.clone();
        for edge in &ix.links {
            if !template.relationship_predicates.contains(&edge.predicate.as_str()) {continue;}
            if before_keys.contains(&edge.from) || before_keys.contains(&edge.to) || !before.is_disjoint(&edge.from_files) || !before.is_disjoint(&edge.to_files) {
                paths.extend(edge.from_files.clone());paths.extend(edge.to_files.clone());
                expanded_keys.insert(edge.from.clone());expanded_keys.insert(edge.to.clone());
            }
        }
        if paths == before && expanded_keys == before_keys {break;}
    }
    let config_keys:BTreeSet<_>=ix.links.iter().filter(|e|e.predicate == "READS_CONFIG" && !paths.is_disjoint(&e.from_files)).map(|e|e.to.clone()).collect();
    for e in ix.links.iter().filter(|e|e.predicate == "DECLARES_CONFIG_KEY" && config_keys.contains(&e.to)) {paths.extend(e.from_files.clone());}
    let mut neighborhood=vec![];
    let mut external=vec![];
    for edge in &ix.links {
        if !paths.is_disjoint(&edge.from_files) || !paths.is_disjoint(&edge.to_files) || expanded_keys.contains(&edge.from) || expanded_keys.contains(&edge.to) {
            let row=serde_json::to_value(edge)?;
            if ["READS_CONFIG","REFERENCES_TOUCHPOINT","DECLARES_CONFIG_KEY","INSTALLS_TO"].contains(&edge.predicate.as_str()) {external.push(row);}
            else {neighborhood.push(row);}
        }
    }
    let joins=joins::build(l,&s,&ix,&paths,&template)?;
    let mut slice=serde_json::Map::new();
    for role in source::ARTIFACT_ROLES {slice.insert((*role).into(),json!([]));}
    for path in &paths {
        let Some(file)=s.files.get(path) else {continue};
        let mut row=serde_json::to_value(file)?;
        row["selection"]=json!(if anchors.contains(path) {"resolved_anchor"} else {"evidence_neighborhood"});
        row["text_is_untrusted_repository_content"]=json!(true);
        slice.get_mut(&file.role).unwrap().as_array_mut().unwrap().push(row);
    }
    let mut not_covered=joins.not_covered;
    not_covered.extend(s.profile_boundaries.clone());
    for producer in &s.evidence_views {
        if producer.scope=="runtime" {
            not_covered.push(json!({"area":"runtime_observation_scope","view_id":producer.id,"interval":producer.diagnostics["interval"],
                "sampling":producer.diagnostics["sampling"],"reason":"Retained runtime observations describe one interval; missing observations do not establish absence."}));
        }
    }
    let artifact_paths:BTreeMap<_,_>=rows(&s.view.bundle,"artifacts").iter().map(|a|(text(a,"id"),text(a,"path"))).collect();
    let reference_paths:BTreeMap<_,_>=rows(&s.view.bundle,"evidence_refs").iter()
        .filter_map(|r|artifact_paths.get(&text(r,"artifact_id")).map(|p|(text(r,"id"),p.clone()))).collect();
    let entity_keys:BTreeMap<_,_>=rows(&s.view.bundle,"entity_versions").iter().map(|e|(text(e,"id"),text(e,"canonical_key"))).collect();
    let (mut outside_boundaries,mut unlocalized_boundaries)=(0usize,0usize);
    for producer in rows(&ix.coverage,"producer_boundaries") {
        for boundary in rows(producer,"unresolved") {
            let path=reference_paths.get(&text(boundary,"evidence_ref_id"));
            let key=entity_keys.get(&text(boundary,"source_entity_id"));
            let related=path.is_some_and(|p|paths.contains(p)) || key.is_some_and(|k|expanded_keys.contains(k)
                || ix.entities.get(k).is_some_and(|e|!e.files.is_disjoint(&paths)));
            if !related {
                if path.is_some() || key.is_some_and(|k|ix.entities.get(k).is_some_and(|e|!e.files.is_empty())) {outside_boundaries+=1;}
                else {unlocalized_boundaries+=1;}
                continue;
            }
            not_covered.push(json!({"area":"producer_boundary","view_id":producer["view_id"],"scope":producer["scope"],"boundary":boundary,
                "reason":"A retained producer occurrence remains unresolved or unsupported; missing links do not establish absence."}));
        }
    }
    if unlocalized_boundaries>0 {
        not_covered.push(json!({"area":"producer_boundary_scope","count":unlocalized_boundaries,
            "reason":"Retained producer boundaries without file/entity associations could not be localized to this task. Inspect the retained index or unresolved report."}));
    }
    for binding in rows(&ix.coverage,"unresolved_bindings") {
        not_covered.push(json!({"area":"semantic_binding","binding":binding,
            "reason":"Declaration binding is unresolved, ambiguous, deferred or withdrawn within this profile scope; another compatible profile may bind it."}));
    }
    if ix.coverage["omitted_relationship_occurrences"].as_u64().unwrap_or(0)>0 {
        not_covered.push(json!({"area":"relationship_index","reason":"Relationship index bound was reached; reverse/forward results do not cover omitted occurrences.",
            "omitted_occurrences":ix.coverage["omitted_relationship_occurrences"]}));
    }
    if ix.coverage["propagation"]["feature_membership_complete"]==false || ix.coverage["propagation"]["file_associations_complete"]==false {
        not_covered.push(json!({"area":"relationship_propagation","reason":"File association propagation reached its iteration bound; associations can be incomplete.","coverage":ix.coverage["propagation"]}));
    }
    let required_roles=template.required_roles;
    let mut role_coverage=BTreeMap::new();
    for role in source::ARTIFACT_ROLES {
        let captured=s.files.values().filter(|f|f.role == *role).count();
        let selected=slice.get(*role).and_then(Value::as_array).map_or(0,Vec::len);
        role_coverage.insert(*role,json!({"captured_files":captured,"selected_files":selected,
            "state":if selected>0 {"associations_established"} else {"no_association_established"}}));
        if required_roles.contains(role) && selected==0 {
            not_covered.push(json!({"area":format!("artifact_role:{role}"),"reason":"This duty requires considering this role; no retained association established.","captured_files":captured}));
        }
    }
    if template.external_required && !external.is_empty() {
        not_covered.push(json!({"area":"external_installations","reason":"Referenced installations/configuration are desired state. Use explicit external inspection to verify selected installed files; active runtime selection remains unknown."}));
    }
    if template.test_execution_required {
        not_covered.push(json!({"area":"executed_test_coverage","reason":"Static test associations do not establish executed test results."}));
    }
    if template.plan_outcomes_required && paths.iter().any(|p|s.files.get(p).is_some_and(|f|f.role=="plan"||f.role=="adr")) {
        not_covered.push(json!({"area":"plan_outcomes","reason":"Plan references establish declared intent; outcomes require implementing evidence."}));
    }
    if template.relationship_predicates.contains(&"CALLS_STATIC") && !rows(&ix.coverage,"analysis_profiles").iter().any(|p|
        !rows(p,"static_call_runs").is_empty()) {
        not_covered.push(json!({"area":"static_calls","reason":"No static-call producer guarantees are present in the selected profiles."}));
    }
    if status != "resolved" {
        not_covered.push(json!({"area":"anchors","reason":"No unique retained identity resolved. Candidates are not selected as implementation; supply an exact path, canonical key or declared feature ID."}));
    }
    let inventory=&s.view.diagnostics["capture"]["inventory"];
    let inventory_digest=inventory_summary(inventory);
    if inventory["tracked_text_complete"] != true {
        not_covered.push(json!({"area":"capture","reason":"Full tracked-file coverage is unestablished; inspect the inventory omissions.","inventory":inventory_digest}));
    }
    for path in rows(inventory,"unretained_tracked_paths") {
        not_covered.push(json!({"area":"capture_path","path":path,"reason":"Tracked path was not retained; see capture exclusions."}));
    }
    for reason in rows(inventory,"exclusions") {
        not_covered.push(json!({"area":"capture_exclusion","reason":reason}));
    }
    for reason in rows(inventory,"diagnostics") {
        not_covered.push(json!({"area":"capture_diagnostic","reason":reason}));
    }
    for language in rows(&ix.coverage,"languages").iter().filter(|v|v["semantic_profile"] == "not_run") {
        not_covered.push(json!({"area":"language_profile","language":language["language"],"reason":"Text is retained but this language's semantic profile is not present in the selected view."}));
    }
    if s.view.diagnostics["coverage"]["resolution_complete"] != true {
        not_covered.push(json!({"area":"semantic_analysis","reason":"Producer coverage does not establish complete resolution; missing edges do not establish absence.","coverage":s.view.diagnostics["coverage"]}));
    }
    let lexical_candidates:Vec<Value>=if status=="resolved" {vec![]} else {
        let mut end=task.len().min(4096);while !task.is_char_boundary(end) {end-=1;}
        let search=crate::architecture::search::search(l,&json!({"text":&task[..end],"view":s.view.id,
            "history":false,"sessions":false,"classes":"text","limit":1000}))?;
        rows(&search,"items").iter().map(|hit|json!({"path":hit["path"],"role":hit["path"].as_str().and_then(|p|s.files.get(p)).map(|f|&f.role),
            "rank":hit["rank"],"search_report_id":search["report_id"],"basis":"shared ranked search; candidate only, no membership inferred"})).collect()
    };
    let mut pinned:Vec<_>=s.profiles.iter().chain(s.evidence_views.iter()).map(|p|p.id.clone()).collect();pinned.extend(joins.session_views.clone());pinned.sort();pinned.dedup();
    let uncovered_areas:BTreeSet<_>=not_covered.iter().map(|g|text(g,"area")).collect();
    let mut doc=json!({"schema_version":"1.0","kind":"brief","method":METHOD,"outcome":"partial",
        "task":task,"duty":selected_duty,"view_id":s.view.id,"index_id":ix.id,"pinned_views":pinned,
        "anchors":{"status":status,"candidates":keys,"paths":anchors},"slice":slice,"neighborhood":neighborhood,
        "history":joins.history,"sessions":joins.sessions,"errors":joins.errors,"joins":joins.links,
        "cochange":joins.cochange,"external_touchpoints":external,"lexical_candidates":lexical_candidates,"not_covered":not_covered,
        "coverage":{"capture":inventory_digest,"profiles":ix.coverage["languages"],"analysis_profiles":ix.coverage["analysis_profiles"],"producer":s.view.diagnostics["coverage"],
            "artifact_roles":role_coverage,"required_roles":required_roles,"neighborhood_depth":depth,
            "producer_boundary_scope":{"outside_selected_slice":outside_boundaries,"unlocalized":unlocalized_boundaries},
            "not_covered_areas":uncovered_areas,
            "history_tip":joins.history_tip,"session_publications_considered":joins.sessions_considered,
            "completeness_contract":{"C1_anchors":status,"C2_slice":"roles explicitly enumerated",
                "C3_neighborhood":"retained forward/reverse relationships with basis","C4_history":"retained Git history; live capture/HEAD comparison is advisory freshness",
                "C5_sessions":"imported recordings, no causal attribution","C6_uncovered":"explicit not_covered inventory",
                "C7_delivery":"retained full packet and paginated sections"}},
        "readiness":{"source_text":"digest_verified","graph":"not_required; eligible durable assertions used",
            "pending_source_capture":l.pending_source()?,"retraction_epoch":s.view.retraction_watermark},
        "provenance":{"source_view":s.view.id,"ledger_watermark":l.watermark()?,"retraction_epoch":s.view.retraction_watermark},
        "instruction":"Use not_covered as the remaining evidence checklist. Static relationships and recorded activity do not establish runtime truth or task completion."});
    let id=identity::id("task-brief",&doc)?;doc["id"]=json!(id);doc["report_id"]=json!(id);
    crate::architecture::contracts::schema(&doc,"task-brief",None)?;
    l.save_report(&id,&doc,&[])?;
    let limit=a["limit"].as_u64().unwrap_or(10) as usize;
    require((1..=1000).contains(&limit),"ARGUMENTS","limit must be 1..1000")?;
    preview(&mut doc,limit);
    Ok(doc)
}

pub(crate) fn table_paths(doc:&Value) -> Vec<String> {
    let mut tables=source::ARTIFACT_ROLES.iter().map(|r|format!("/slice/{r}")).collect::<Vec<_>>();
    for field in ["neighborhood","history","sessions","errors","joins","cochange","external_touchpoints","lexical_candidates","not_covered"] {
        if doc[field].is_array() {tables.push(format!("/{field}"));}
    }
    tables
}
fn preview(doc:&mut Value,limit:usize) {
    let mut delivery=serde_json::Map::new();
    for pointer in table_paths(doc) {
        if let Some(items)=doc.pointer_mut(&pointer).and_then(Value::as_array_mut) {
            let total=items.len();items.truncate(limit);
            delivery.insert(pointer,json!({"total":total,"returned":items.len(),"omitted":total-items.len(),
                "next_offset":(items.len()<total).then_some(items.len())}));
        }
    }
    doc["coverage"]["delivery"]=json!(delivery);
    doc["continue_with"]=doc["coverage"]["delivery"].as_object().unwrap().iter()
        .find(|(_,v)|v["omitted"].as_u64().unwrap_or(0)>0)
        .map(|(section,v)|crate::architecture::reports::continuation(doc["id"].as_str().unwrap(),section,v["returned"].as_u64().unwrap() as usize,limit))
        .unwrap_or(Value::Null);
}

/// Unadvertised compatibility wrapper for the shared report protocol.
pub fn page(l:&Ledger,a:&Value)->Result<Value>{crate::architecture::reports::page(l,a)}
