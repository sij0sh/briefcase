use super::*;

/// Exhaustive literal occurrences over verified snapshot bytes. Pages are
/// contiguous in (path, byte offset) order and include overlapping matches.
pub fn occurrences(l: &Ledger, a: &Value) -> Result<Value> {
    let needle = a["text"].as_str().ok_or("ARGUMENTS: text is required")?;
    require(!needle.is_empty() && needle.len() <= 4096,"ARGUMENTS","Literal text must contain 1..4096 bytes")?;
    let s = Snapshot::source(l,a["view"].as_str().unwrap_or("latest"))?;
    let path_prefix = a["path"].as_str();
    let offset = a["offset"].as_u64().unwrap_or(0) as usize;
    let limit = a["limit"].as_u64().unwrap_or(100) as usize;
    require((1..=1000).contains(&limit),"ARGUMENTS","limit must be 1..1000")?;
    let mut total = 0usize;
    let mut items = vec![];
    let mut searched = 0usize;
    let pattern=needle.as_bytes();
    let mut prefix=vec![0;pattern.len()];
    let mut matched=0;
    for i in 1..pattern.len() {
        while matched>0 && pattern[matched]!=pattern[i] {matched=prefix[matched-1];}
        if pattern[matched]==pattern[i] {matched+=1;}
        prefix[i]=matched;
    }
    for (path,content) in &s.contents {
        if path_prefix.is_some_and(|p| !p.is_empty() && path != p && !path.starts_with(&format!("{}/",p.trim_end_matches('/')))) {continue;}
        searched += 1;
        let mut line = 1;
        let mut column = 1;
        let mut counted_until = 0;
        let mut matched=0;
        // KMP keeps overlapping enumeration linear even for long repeated
        // literals. UTF-8 needle matches necessarily start/end at boundaries.
        for (position,byte) in content.as_bytes().iter().enumerate() {
            while matched>0 && pattern[matched]!=*byte {matched=prefix[matched-1];}
            if pattern[matched]==*byte {matched+=1;}
            if matched!=pattern.len() {continue;}
            let found=position+1-pattern.len();
            if total >= offset && items.len() < limit {
                for character in content[counted_until..found].chars() {
                    if character=='\n' {line+=1;column=1;} else {column+=1;}
                }
                counted_until=found;
                items.push(json!({"path":path,"artifact_id":s.files[path].artifact_id,"digest":s.files[path].digest,
                    "role":s.files[path].role,"start_byte":found,"end_byte":found+needle.len(),
                    "line":line,"column":column}));
            }
            total += 1;
            matched=prefix[matched-1];
        }
    }
    let end = offset.saturating_add(items.len());
    let inventory = &s.view.diagnostics["capture"]["inventory"];
    let mut doc=json!({"kind":"occurrences","view_id":s.view.id,"text":needle,"path":path_prefix,"items":items,
        "page":{"offset":offset,"returned":items.len(),"total":total,"remaining":total.saturating_sub(end),"next_offset":(end<total).then_some(end)},
        "coverage":{"state":"complete_for_retained_text","searched_files":searched,"captured_files":s.files.len(),
            "tracked_inventory":inventory_summary(inventory),"captured_at_unix_ms":s.view.diagnostics["capture"]["captured_at_unix_ms"],
            "not_covered":["live edits after capture","excluded, unavailable and uncaptured files","external installed copies"]},
        "ordering":"path then byte offset, including overlapping matches; no ranking or top-k cutoff","column_unit":"Unicode scalar values",
        "continue_with":{"operation":"occurrences","arguments":{"view":s.view.id,"text":needle,"path":path_prefix,"offset":end,"limit":limit}}});
    crate::architecture::reports::pin_enumerator(l,&mut doc,"occurrences",a)?;
    Ok(doc)
}

/// Exhaustive forward/reverse adjacency over the retained task index.
pub fn neighbors(l: &Ledger, a: &Value) -> Result<Value> {
    let target = a["target"].as_str().ok_or("ARGUMENTS: target is required")?;
    let ix=if let Some(id)=a["index_id"].as_str() {load_index(l,id)?} else {
        let s=Snapshot::load_with(l,a["view"].as_str().unwrap_or("latest"),a)?;index::load(l,&s)?
    };
    if a["index_id"].is_string() {require(a["view"].as_str().is_none_or(|v|v=="latest" || v==ix.source_view),"ARGUMENTS","view differs from retained index pin")?;}
    let (status,mut keys,files) = resolve(&ix,target,true);
    let direct_touchpoint = ix.links.iter().any(|e|e.from == target || e.to == target);
    if status == "unresolved" && direct_touchpoint {keys.insert(target.into());}
    let status = if direct_touchpoint && status == "unresolved" {"resolved"} else {&status};
    let direction = a["direction"].as_str().unwrap_or("both");
    require(["in","out","both"].contains(&direction),"ARGUMENTS","direction must be in, out or both")?;
    let predicates: BTreeSet<_> = a["predicate"].as_str().unwrap_or("").split(',').filter(|p|!p.is_empty()).collect();
    let offset = a["offset"].as_u64().unwrap_or(0) as usize;
    let limit = a["limit"].as_u64().unwrap_or(100) as usize;
    let depth = a["depth"].as_u64().unwrap_or(1) as usize;
    require((1..=3).contains(&depth) && (1..=1000).contains(&limit),"ARGUMENTS","depth must be 1..3 and limit 1..1000")?;
    let mut selected = BTreeMap::new();
    if status == "resolved" {
        let mut frontier_keys = keys.clone();
        let mut frontier_files = files;
        for _ in 0..depth {
            let mut next_keys = BTreeSet::new(); let mut next_files = BTreeSet::new();
            for e in &ix.links {
                if !predicates.is_empty() && !predicates.contains(e.predicate.as_str()) {continue;}
                let outgoing = direction != "in" && (frontier_keys.contains(&e.from) || !frontier_files.is_disjoint(&e.from_files));
                let incoming = direction != "out" && (frontier_keys.contains(&e.to) || !frontier_files.is_disjoint(&e.to_files));
                if outgoing || incoming {
                    selected.insert(e.id.clone(),serde_json::to_value(e)?);
                    if outgoing {next_keys.insert(e.to.clone());next_files.extend(e.to_files.clone());}
                    if incoming {next_keys.insert(e.from.clone());next_files.extend(e.from_files.clone());}
                }
            }
            frontier_keys = next_keys; frontier_files = next_files;
        }
    }
    let mut items:Vec<_> = selected.into_values().collect();
    items.sort_by(|a,b|(&a["from"].as_str(),&a["predicate"].as_str(),&a["to"].as_str(),&a["id"].as_str()).cmp(&(&b["from"].as_str(),&b["predicate"].as_str(),&b["to"].as_str(),&b["id"].as_str())));
    let mut result = page_items("neighbors",&ix.source_view,&items,offset,limit);
    result["resolution"] = json!({"status":status,"candidates":keys});
    result["target"] = json!(target);
    result["coverage"] = ix.coverage.clone();
    result["index_id"] = json!(ix.id);
    result["pinned_views"]=ix.coverage["profile_views"].clone();
    result["historical_index"]=json!(ix.retraction_epoch!=l.retraction_epoch()?);
    result["index_retraction_epoch"]=json!(ix.retraction_epoch);
    result["continue_with"] = json!({"operation":"neighbors","arguments":{"view":ix.source_view,"index_id":ix.id,"target":target,
        "direction":direction,"predicate":a["predicate"],"depth":depth,"offset":offset.saturating_add(rows(&result,"items").len()),"limit":limit}});
    crate::architecture::reports::pin_enumerator(l,&mut result,"neighbors",a)?;
    Ok(result)
}

fn load_index(l:&Ledger,id:&str) -> Result<Index> {
    require(id.starts_with("task-index:"),"TASK","A retained task-index ID is required")?;
    let index:Index=serde_json::from_value(l.report(id)?)?;
    require(index.id==id,"TASK","Retained index identity mismatch")?;
    Ok(index)
}

/// Resolve an edge's exact text or original assertions without changing its
/// authority. Old pins remain inspectable with current retractions annotated.
pub fn explain(l:&Ledger,a:&Value) -> Result<Value> {
    let id=a["index_id"].as_str().ok_or("ARGUMENTS: index_id is required")?;
    let link_id=a["link_id"].as_str().ok_or("ARGUMENTS: link_id is required")?;
    let ix=load_index(l,id)?;
    let edge=ix.links.iter().find(|e|e.id==link_id).ok_or("TASK: Unknown relationship in this index")?;
    let limit=a["limit"].as_u64().unwrap_or(20) as usize;
    require((1..=1000).contains(&limit),"ARGUMENTS","limit must be 1..1000")?;
    let offset=a["offset"].as_u64().unwrap_or(0) as usize;
    let mut witnesses=vec![];
    for w in edge.witnesses.iter().skip(offset).take(limit) {
        let payloads=l.payloads_for(std::slice::from_ref(&w.artifact_id))?;
        let bytes=&payloads[&w.artifact_id];
        require(identity::bytes_digest(bytes)==w.digest,"DIGEST","Relationship witness source is corrupted")?;
        let range=bytes.get(w.start_byte..w.end_byte).ok_or("TASK: Invalid witness span")?;
        let content=std::str::from_utf8(range)?;
        witnesses.push(json!({"witness":w,"text":content.chars().take(2000).collect::<String>(),
            "omitted_characters":content.chars().count().saturating_sub(2000)}));
    }
    let removed=l.retracted()?;
    let mut assertions=BTreeMap::new();
    for pin in crate::architecture::contracts::strings(&ix.coverage["profile_views"]) {
        let view=l.retained(&pin)?;
        let all:BTreeMap<_,_>=rows(&view.bundle,"assertions").iter().map(|v|(text(v,"id"),v)).collect();
        for aid in edge.assertion_ids.iter().skip(offset).take(limit) {
            if let Some(assertion)=all.get(aid) {
                let references:Vec<_>=crate::architecture::contracts::strings(&assertion["evidence_ref_ids"]);
                let evidence:Vec<_>=rows(&view.bundle,"evidence_refs").iter().filter(|r|references.contains(&text(r,"id"))).cloned().collect();
                let support:Vec<_>=crate::architecture::contracts::strings(&assertion["supports"]).iter().filter_map(|id|all.get(id).map(|a|(*a).clone())).collect();
                let declaration=rows(&view.bundle,"declaration_records").iter().find(|r|r["id"]==assertion["origin"]["declaration_record_id"]);
                let derivation=rows(&view.bundle,"derivations").iter().find(|r|r["id"]==assertion["origin"]["derivation_id"]);
                assertions.insert(aid.clone(),json!({"source_view":pin,"assertion":assertion,"currently_retracted":removed.contains(aid),
                    "evidence_refs":evidence,"support_assertions":support.iter().take(limit).collect::<Vec<_>>(),
                    "omitted_support_assertions":support.len().saturating_sub(limit),"declaration_record":declaration,"derivation":derivation}));
            }
        }
    }
    Ok(json!({"kind":"explain","index_id":id,"link_id":link_id,"link":edge,"view_id":ix.source_view,"pinned_views":ix.coverage["profile_views"],
        "observation_reports":edge.report_ids.iter().map(|id|l.report(id)).collect::<Result<Vec<_>>>()?,
        "witnesses":witnesses,"assertions":assertions.into_values().collect::<Vec<_>>(),"current_retraction_epoch":l.retraction_epoch()?,
        "historical_index":ix.retraction_epoch!=l.retraction_epoch()?,"offset":offset,"omitted_witnesses":edge.witnesses.len().saturating_sub(offset.saturating_add(limit)),
        "omitted_assertions":edge.assertion_ids.len().saturating_sub(offset.saturating_add(limit))}))
}
