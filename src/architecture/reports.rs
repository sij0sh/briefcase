//! One report paging and explanation protocol for every context read.
use super::{contracts::rows,identity,ledger::Ledger,query,require,task,Result};
use serde_json::{json,Value};

pub fn cursor(report:&str,section:&str,offset:usize) -> String {
    json!({"report_id":report,"section":section,"offset":offset}).to_string()
}
pub(crate) fn continuation(report:&str,section:&str,offset:usize,limit:usize) -> Value {
    json!({"operation":"page","arguments":{"report_id":report,"cursor":cursor(report,section,offset),"limit":limit}})
}
/// A bounded ranked result is retained before presentation truncates any rows.
pub(crate) fn retain_items(l:&Ledger,mut doc:Value,limit:usize) -> Result<Value> {
    doc["retraction_epoch"]=json!(l.retraction_epoch()?);
    let id=identity::id("context-report",&doc)?;
    doc["report_id"]=json!(id);
    l.save_report(&id,&doc,&[])?;
    let total=rows(&doc,"items").len();
    doc["items"].as_array_mut().ok_or("REPORT: Missing items")?.truncate(limit);
    delivery(&mut doc,&id,"/items",0,total,limit);
    Ok(doc)
}
/// Exhaustive enumerators retain their immutable view/index recipe, rather
/// than allocating an unbounded copy of every literal occurrence.
pub(crate) fn pin_enumerator(l:&Ledger,doc:&mut Value,operation:&str,args:&Value) -> Result<()> {
    let mut pinned=args.clone();
    pinned["view"]=doc["view_id"].clone();
    if operation=="neighbors" {pinned["index_id"]=doc["index_id"].clone();}
    for field in ["offset","limit","max_bytes"] {pinned.as_object_mut().unwrap().remove(field);}
    let record=json!({"enumerator":operation,"arguments":pinned,"view_id":doc["view_id"],
        "pinned_views":doc["pinned_views"],"index_id":doc["index_id"],"retraction_epoch":l.retraction_epoch()?});
    let id=identity::id("context-report",&record)?;
    l.save_report(&id,&record,&[])?;
    doc["report_id"]=json!(id);
    let offset=doc["page"]["offset"].as_u64().unwrap_or(0) as usize;
    let total=doc["page"]["total"].as_u64().unwrap_or(0) as usize;
    let limit=args["limit"].as_u64().unwrap_or(100) as usize;
    delivery(doc,&id,"/items",offset,total,limit);
    Ok(())
}
fn delivery(doc:&mut Value,id:&str,section:&str,offset:usize,total:usize,limit:usize) {
    let returned=rows(doc,"items").len();let end=offset.saturating_add(returned);
    doc["page"]=json!({"offset":offset,"returned":returned,"total":total,
        "remaining":total.saturating_sub(end),"next_offset":(end<total).then_some(end)});
    doc["continue_with"]=if end<total {continuation(id,section,end,limit)} else {Value::Null};
}
pub fn page(l:&Ledger,a:&Value) -> Result<Value> {
    if let Some(token)=a["token"].as_str() {return query::continuation(l,token);}
    let id=a["report_id"].as_str().ok_or("ARGUMENTS: report_id is required")?;
    let mut section=a["section"].as_str().unwrap_or("").to_owned();
    let mut offset=a["offset"].as_u64().unwrap_or(0) as usize;
    if let Some(raw)=a["cursor"].as_str() {
        let c:Value=serde_json::from_str(raw)?;
        require(c["report_id"]==id,"PAGINATION","Cursor belongs to another report")?;
        section=c["section"].as_str().ok_or("PAGINATION: Missing section")?.into();
        offset=c["offset"].as_u64().ok_or("PAGINATION: Missing offset")? as usize;
    }
    let limit=a["limit"].as_u64().unwrap_or(100) as usize;
    require((1..=1000).contains(&limit),"PAGINATION","Limit must be 1..1000")?;
    let doc=load_report(l,id)?;
    let sections=if id.starts_with("task-index:") {
        // Entity name/file arrays are not a useful section inventory. Listing
        // thousands of them would itself exhaust the graph response budget.
        ["/links","/coverage/producer_boundaries","/coverage/unresolved_bindings","/coverage/analysis_profiles","/coverage/languages","/coverage/profile_views"]
            .into_iter().filter(|p|doc.pointer(p).is_some_and(Value::is_array)).map(str::to_owned).collect()
    } else {match doc["kind"].as_str() {Some("case")=>task::case::sections(&doc),Some("brief")=>task::report_sections(&doc),
        Some("relationship_proof")=>{let mut sections=report_sections(&doc);sections.extend(super::contracts::strings(&doc["page_sections"]));sections},
        _=>report_sections(&doc)}};
    if section.is_empty() {
        section=if doc["enumerator"].is_string()||doc["items"].is_array() {"/items".into()}
            else if doc["response"].is_object(){"/response/results".into()}
            else{sections.iter().find(|p|doc.pointer(p).and_then(Value::as_array).is_some_and(|a|!a.is_empty())).cloned().ok_or("REPORT: No array sections")?};
    }
    if let Some(operation)=doc["enumerator"].as_str() {
        require(section=="/items","PAGINATION","Unknown enumerator section")?;
        let mut args=doc["arguments"].clone();args["offset"]=json!(offset);args["limit"]=json!(limit);
        return match operation {"occurrences"=>task::occurrences(l,&args),"neighbors"=>task::neighbors(l,&args),_=>Err("REPORT: Unknown enumerator".into())};
    }
    if doc["response"].is_object() {
        require(section=="/items"||section=="/response/results","PAGINATION","Unknown query section")?;
        return query::report_page(l,std::rc::Rc::unwrap_or_clone(doc),offset,limit);
    }
    require(sections.contains(&section),"PAGINATION","Unknown report section")?;
    let items=doc.pointer(&section).and_then(Value::as_array).ok_or("REPORT: Section is not an array")?;
    require(offset<=items.len(),"PAGINATION","Offset outside retained report")?;
    let end=offset.saturating_add(limit).min(items.len());
    let mut out=json!({"kind":"page","report_id":id,"section":section,"view_id":doc["view_id"],
        "index_id":doc["index_id"],"available_sections":sections,"pinned_views":doc["pinned_views"],"items":&items[offset..end],
        "historical_packet":doc["retraction_epoch"].as_u64().or(doc["provenance"]["retraction_epoch"].as_u64())!=Some(l.retraction_epoch()?),
        "current_retraction_epoch":l.retraction_epoch()?});
    if doc["kind"]=="task_inventory" {
        for key in ["case_id","inventory_kind","filters","fetch_source","interpretation","coverage"] {out[key]=doc[key].clone();}
    }
    delivery(&mut out,id,&section,offset,items.len(),limit);
    Ok(out)
}
pub fn explain(l:&Ledger,a:&Value) -> Result<Value> {
    let reference=a["ref"].as_str().or(a["link_id"].as_str()).or(a["handle"].as_str())
        .ok_or("ARGUMENTS: ref or handle is required")?;
    if reference.starts_with("explain:") {
        return query::explain_page(l,reference,a["limit"].as_u64().unwrap_or(100) as usize,
            a["offset"].as_u64().unwrap_or(0) as usize,a["source_offset"].as_u64().unwrap_or(0) as usize);
    }
    let report=a["report_id"].as_str();
    let doc=report.map(|id|load_report(l,id)).transpose()?;
    let index=a["index_id"].as_str().or_else(||doc.as_ref().and_then(|d|d["index_id"].as_str()))
        .or_else(||report.filter(|id|id.starts_with("task-index:")));
    if reference.starts_with("task-link:") {
        return task::explain(l,&json!({"index_id":index.ok_or("ARGUMENTS: Report has no relationship index")?,
            "link_id":reference,"limit":a["limit"],"offset":a["offset"]}));
    }
    let doc=doc.ok_or("ARGUMENTS: report_id is required for this reference")?;
    if let Some(row)=rows(&doc["response"],"results").iter().find(|r|r["id"]==reference) {
        let mut args=a.clone();args["handle"]=row["explanation_handle"].clone();args["ref"]=row["explanation_handle"].clone();
        return explain(l,&args);
    }
    let entry=doc.pointer(reference).ok_or("EXPLAIN: Unknown report reference")?;
    // A candidate is a reading hint, whether selected or not. Resolve its
    // source through the same pinned index rather than consulting live files.
    let candidate_source=if reference.strip_prefix("/candidates/").is_some_and(|n|n.parse::<usize>().is_ok())
        && doc["kind"]=="case" {
        let index=l.report(doc["index_id"].as_str().ok_or("EXPLAIN: Missing pinned index")?)?;
        require(index["source_view"]==doc["view_id"],"EXPLAIN","Candidate index has a different source view")?;
        Some(index["files"].get(entry["path"].as_str().ok_or("EXPLAIN: Candidate has no path")?)
            .ok_or("EXPLAIN: Candidate source is absent from its pinned index")?.clone())
    }else{None};
    let row=candidate_source.as_ref().unwrap_or(entry);
    if let Some(assertion)=row["assertion_id"].as_str() {
        // Search annotations use the same retained assertion proof traversal as
        // query reports. No second evidence hydrator or authority rule.
        let handle=identity::id("explain",&json!([report,reference,assertion]))?;
        let proof=json!({"report_id":report,"operation":"search","views":doc["pinned_views"],
            "assertion_ids":[assertion],"detail":row,"selection_reasons":["attached assertion on a retained text span"],
            "interpretation":"The assertion keeps its original class, scope and producer coverage; current retractions are annotated."});
        l.save_report(report.unwrap(),&doc,&[(handle.clone(),proof)])?;
        return query::explain_page(l,&handle,a["limit"].as_u64().unwrap_or(100) as usize,
            a["offset"].as_u64().unwrap_or(0) as usize,a["source_offset"].as_u64().unwrap_or(0) as usize);
    }
    // Source references return verified retained bytes, never live-file fallback.
    if let Some(artifact)=row["artifact_id"].as_str() {
        let payload=l.payloads_for(&[artifact.to_owned()])?;
        let bytes=payload.get(artifact).ok_or("UNKNOWN_REFERENCE: Missing retained source payload")?;
        let digest=row["digest"].as_str().ok_or("EXPLAIN: Source reference has no retained digest")?;
        require(identity::bytes_digest(bytes)==digest,"DIGEST","Report source is corrupted")?;
        let source=std::str::from_utf8(bytes)?;
        let offset=a["source_offset"].as_u64().unwrap_or(0) as usize;
        require(offset<=source.len()&&source.is_char_boundary(offset),"EXPLAIN","Invalid source byte offset")?;
        let length=a["source_bytes"].as_u64().unwrap_or(4000) as usize;
        require((4..=65536).contains(&length),"ARGUMENTS","source_bytes must be 4..65536")?;
        let mut end=offset.saturating_add(length).min(source.len());while !source.is_char_boundary(end) {end-=1;}
        let epoch=l.retraction_epoch()?;
        let source_view=doc.get("view_id").or_else(||doc.get("source_view"));
        let source_index=report.filter(|id|id.starts_with("task-index:")).map(|id|json!(id)).unwrap_or_else(||doc["index_id"].clone());
        let pins=doc.get("pinned_views").unwrap_or(&doc["coverage"]["profile_views"]);
        return Ok(json!({"kind":"explain","report_id":report,"ref":reference,"entry":row,"text":&source[offset..end],
            "view_id":source_view,"index_id":source_index,"pinned_views":pins,
            "historical_packet":doc["provenance"]["retraction_epoch"].as_u64().or(doc["retraction_epoch"].as_u64())
                .is_some_and(|pin|pin!=epoch),
            "source_offset":offset,"next_source_offset":(end<source.len()).then_some(end),"omitted_bytes":source.len()-end,
            "continue_with":(end<source.len()).then(||json!({"operation":"explain","arguments":{
                "report_id":report,"ref":reference,"source_offset":end,"source_bytes":length}}))}));
    }
    Ok(json!({"kind":"explain","report_id":report,"ref":reference,"entry":row,
        "interpretation":"Retained report entry; its class and basis keep their original authority."}))
}

fn report_sections(doc:&Value)->Vec<String>{
    fn visit(value:&Value,prefix:&str,out:&mut Vec<String>){
        if value.is_array(){out.push(prefix.into());}
        else if let Some(object)=value.as_object(){for (key,value) in object {
            let key=key.replace('~',"~0").replace('/',"~1");visit(value,&format!("{prefix}/{key}"),out);
        }}
    }
    let mut sections=vec![];visit(doc,"",&mut sections);sections
}

thread_local! {
    /// Parsed content-addressed task indexes; long-lived processes only.
    static INDEX_DOCS: std::cell::RefCell<Vec<(String,std::rc::Rc<Value>)>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn load_report(l:&Ledger,id:&str) -> Result<std::rc::Rc<Value>> {
    if id.starts_with("task-case:") {return task::case::load_doc(l,id).map(Into::into);}
    if !id.starts_with("task-index:") || !crate::architecture::ledger::process_caches() {return l.report(id).map(Into::into);}
    if !l.has_report(id)? {return l.report(id).map(Into::into);}
    if let Some(doc)=INDEX_DOCS.with(|d|d.borrow().iter().find(|(k,_)|k==id).map(|(_,v)|v.clone())) {return Ok(doc);}
    let doc=std::rc::Rc::new(l.report(id)?);
    INDEX_DOCS.with(|d|{let mut d=d.borrow_mut(); if d.len()>=2 {d.remove(0);} d.push((id.into(),doc.clone()));});
    Ok(doc)
}

#[cfg(test)]
mod proof_pages {
    use super::*;
    #[test]
    fn large_proof_arrays_have_exact_registered_nested_pages() {
        let temp=tempfile::tempdir().unwrap();let ledger=Ledger::open(&temp.path().join("proof.db")).unwrap();
        let section="/assertions/0/derivation/input_assertion_ids";
        let doc=json!({"kind":"relationship_proof","retraction_epoch":ledger.retraction_epoch().unwrap(),
            "page_sections":[section],"assertions":[{"derivation":{"input_assertion_ids":["a","b","c"]}}]});
        let id=identity::id("task-proof",&doc).unwrap();ledger.save_report(&id,&doc,&[]).unwrap();
        let first=page(&ledger,&json!({"report_id":id,"section":section,"limit":1})).unwrap();
        assert_eq!(first["items"],json!(["a"]));assert_eq!(first["page"]["total"],3);
        let second=page(&ledger,&first["continue_with"]["arguments"]).unwrap();
        assert_eq!(second["items"],json!(["b"]));assert_eq!(second["page"]["offset"],1);
        assert_eq!(ledger.report(&id).unwrap(),doc);
    }
}
