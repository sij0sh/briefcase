//! Presentation budgets over complete UTF-8 JSON, not just evidence text.
//! Full reports and explanations remain retained; omissions never add facts.
use super::{
    contracts::{rows, text},
    require, Result,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const DEFAULT_MAX_BYTES: usize = 24576;
pub const DEFAULT_AGENT_MAX_BYTES: usize = 8000;
pub const MIN_MAX_BYTES: usize = 256;
pub const MAX_MAX_BYTES: usize = 1024 * 1024;
/// Smallest excerpt worth keeping over dropping a lower-ranked item.
const MIN_EXCERPT_BYTES: usize = 240;

/// CLI bounded responses use compact JSON and one trailing LF.
pub fn serialized_bytes(value: &Value) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len() + 1)
}

fn omitted(value: &mut Value, path: &str, count: usize) {
    if count == 0 {
        return;
    }
    if !value["omissions"].is_object() {
        value["omissions"] = json!({});
    }
    let previous = value["omissions"][path].as_u64().unwrap_or(0);
    value["omissions"][path] = json!(previous + count as u64);
}

/// Collapse repeated path-specific diagnostics without discarding the warning.
/// The full list remains in the retained report and its explanations.
pub(crate) fn summarize_limitations(value: &mut Value) {
    for pointer in [
        "/response/limitations",
        "/limitations",
        "/coverage/capture_limitations",
    ] {
        let Some(all) = value.pointer(pointer).and_then(Value::as_array).cloned() else {
            continue;
        };
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for item in &all {
            let Some(message) = item.as_str() else {
                continue;
            };
            let prefix = if let Some((prefix, _)) = message
                .starts_with("unresolved ")
                .then(|| message.split_once(" reference for "))
                .flatten()
            {
                Some(prefix)
            } else if let Some((prefix, _)) = message.split_once(": ") {
                if prefix.contains("outside") || prefix.contains("skipped") {
                    Some(prefix)
                } else {
                    None
                }
            } else if message.starts_with("parse errors in ") {
                Some("parse errors in retained files")
            } else {
                None
            };
            groups
                .entry(prefix.unwrap_or(message).into())
                .or_default()
                .push(message.into());
        }
        let mut summaries = Vec::new();
        for (prefix, mut messages) in groups {
            messages.sort();
            messages.dedup();
            if messages.len() > 1 {
                summaries.push(json!(format!(
                    "{prefix}: {} occurrences; example: {}. Full diagnostics remain retained.",
                    messages.len(),
                    messages[0]
                )));
            } else if let Some(message) = messages.first() {
                summaries.push(json!(message));
            }
        }
        if summaries.len() < all.len() {
            *value.pointer_mut(pointer).unwrap() = json!(summaries);
            omitted(value, pointer, all.len() - summaries.len());
        }
    }
}

fn measure(
    value: &mut Value,
    max_bytes: usize,
    size_of: &dyn Fn(&Value) -> Result<usize>,
) -> Result<usize> {
    if !value["budget"].is_object() {
        value["budget"] = json!({});
    }
    value["budget"]["max_response_bytes"] = json!(max_bytes);
    value["budget"]["returned_response_bytes"] = json!(0);
    // The byte count includes itself. Reach the fixed point as its digits grow.
    for _ in 0..8 {
        let size = size_of(value)?;
        if value["budget"]["returned_response_bytes"].as_u64() == Some(size as u64) {
            return Ok(size);
        }
        value["budget"]["returned_response_bytes"] = json!(size);
    }
    Err("BUDGET: Could not measure response envelope".into())
}

fn mark_partial(value: &mut Value) {
    if value["response"].is_object() {
        value["response"]["truncated"] = json!(true);
        value["response"]["resolution_complete"] = json!(false);
        if value["response"]["outcome"] == "complete" {
            value["response"]["outcome"] = json!("partial");
        }
        if let Some(results) = value["response"]["results"].as_array_mut() {
            for row in results {
                if row["conclusion"] == "pass" {
                    row["conclusion"] = json!("unknown");
                }
            }
        }
    }
    if value["outcome"] == "complete" {
        value["outcome"] = json!("partial");
    }
}

fn drop_row(value: &mut Value) -> bool {
    let Some(row) = value
        .get_mut("response")
        .and_then(|r| r.get_mut("results"))
        .and_then(Value::as_array_mut)
        .and_then(Vec::pop)
    else {
        return false;
    };
    let id = text(&row, "id");
    if let Some(details) = value.get_mut("details").and_then(Value::as_object_mut) {
        details.remove(&id);
    }
    if let Some(index) = id.rsplit(':').next().and_then(|s| s.parse::<usize>().ok()) {
        value["response"]["continuation_token"] =
            json!(format!("{}@{index}", text(&value["response"], "id")));
        let report_id=text(&value["response"],"id");
        value["continue_with"]=super::reports::continuation(&report_id,"/response/results",index,100);
        if value["page"].is_object(){
            let offset=value["page"]["offset"].as_u64().unwrap_or(0);
            let total=value["page"]["total"].as_u64().unwrap_or(0);
            let returned=rows(&value["response"],"results").len();
            value["page"]=json!({"offset":offset,"returned":returned,"total":total,"remaining":total.saturating_sub(index as u64),"next_offset":index});
        }
    }
    omitted(value, "/response/results", 1);
    mark_partial(value);
    true
}

/// The lowest-ranked item to trim. Opt-in session hits rank after source
/// hits but hold reserved slots: the last source item yields first while
/// another source item remains.
fn trim_index(value: &Value) -> Option<usize> {
    let items = rows(value, "items");
    // A retained report cursor addresses a contiguous prefix. Removing an
    // interior row would make the next page silently skip that row.
    if value["report_id"].is_string() && value["page"].is_object() {
        return items.len().checked_sub(1);
    }
    let sources: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item["source_kind"] != "session")
        .map(|(index, _)| index)
        .collect();
    if sources.len() > 1 && sources.len() < items.len() {
        sources.last().copied()
    } else {
        items.len().checked_sub(1)
    }
}

fn drop_item(value: &mut Value) -> bool {
    let Some(index) = trim_index(value) else {
        return false;
    };
    let item = value["items"].as_array_mut().unwrap().remove(index);
    if item.get("evidence_text").is_some() {
        if rows(value, "items").is_empty() {
            mark_partial(value);
        } else {
            drop_row(value);
        }
        let count = value["omitted_evidence_count"].as_u64().unwrap_or(0);
        value["omitted_evidence_count"] = json!(count + 1);
    } else if item["source_kind"] == "session" {
        let count = value["coverage"]["sessions"]["omitted_requests"]
            .as_u64()
            .unwrap_or(0);
        value["coverage"]["sessions"]["omitted_requests"] = json!(count + 1);
        if let Some(delivered) = value["coverage"]["sessions"]["delivered_requests"].as_u64() {
            value["coverage"]["sessions"]["delivered_requests"] =
                json!(delivered.saturating_sub(1));
        }
    } else {
        let count = value["omitted_candidate_count"].as_u64().unwrap_or(0);
        value["omitted_candidate_count"] = json!(count + 1);
    }
    omitted(value, "/items", 1);
    if value["page"].is_object() {
        let offset=value["page"]["offset"].as_u64().unwrap_or(0);
        let returned=rows(value,"items").len() as u64;
        let total=value["page"]["total"].as_u64().unwrap_or(0);
        let end=offset.saturating_add(returned);
        value["page"]=json!({"offset":offset,"returned":returned,"total":total,"remaining":total.saturating_sub(end),
            "next_offset":(end<total).then_some(end)});
        if let Some(id)=value["report_id"].as_str() {
            value["continue_with"]=super::reports::continuation(id,value["section"].as_str().unwrap_or("/items"),end.try_into().unwrap(),100);
        } else {value["continue_with"]["arguments"]["offset"]=json!(end);}
    }
    true
}

/// Bytes of quoted source, request or evidence text the items carry.
fn quoted_bytes(value: &Value) -> usize {
    rows(value, "items")
        .iter()
        .flat_map(|item| ["text", "evidence_text"].map(|field| item[field].as_str()))
        .flatten()
        .map(str::len)
        .sum()
}

/// Matches existed but no quoted text fits: say how much budget would, rather
/// than returning empty items an agent could read as "no matches".
fn budget_floor(value: &mut Value, original_items: &[Value], size: usize) -> Result<()> {
    let smallest = original_items
        .iter()
        .filter(|item| {
            ["text", "evidence_text"]
                .iter()
                .any(|f| item[*f].as_str().is_some_and(|s| !s.is_empty()))
        })
        .map(|item| serialized_bytes(item).map(|n| n - 1))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .min()
        .unwrap_or(0);
    value["budget_floor"] = json!({"minimum_useful_bytes":(size + smallest).min(MAX_MAX_BYTES),
        "matched_items":original_items.len(),
        "instruction":"Matches exist but no quoted text fits this budget; returned items are not evidence of absence. Retry with max_bytes at least minimum_useful_bytes."});
    mark_partial(value);
    Ok(())
}

fn refresh_items(value: &mut Value) {
    let Some(items) = value["items"].as_array().cloned() else {
        return;
    };
    if value["reading_list"].is_array() {
        value["reading_list"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| {
                items
                    .iter()
                    .any(|item| item["path"] == entry["path"] && item["file"] == entry["file"])
            });
    }
    for (field, counter) in [
        ("text", "returned_text_bytes"),
        ("evidence_text", "returned_evidence_bytes"),
    ] {
        if value["budget"].get(counter).is_some() {
            value["budget"][counter] = json!(items
                .iter()
                .filter_map(|i| i[field].as_str())
                .map(str::len)
                .sum::<usize>());
        }
    }
}

/// Shorten an excerpt only at a character boundary and count every omitted byte.
/// Metadata and pinned identities are never sliced.
fn shorten_excerpt(
    value: &mut Value,
    pointer: &str,
    max_bytes: usize,
    size_of: &dyn Fn(&Value) -> Result<usize>,
) -> Result<bool> {
    let Some(raw) = value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(false);
    };
    if raw.is_empty() {
        return Ok(false);
    }
    let size = measure(value, max_bytes, size_of)?;
    let mut end = raw
        .len()
        .saturating_sub(size.saturating_sub(max_bytes).max(1));
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    *value.pointer_mut(pointer).unwrap() = json!(&raw[..end]);
    let (parent, field) = pointer.rsplit_once('/').unwrap();
    if field == "evidence_text" || parent.starts_with("/items/") {
        let item = value.pointer_mut(parent).unwrap();
        let counter = if field == "evidence_text" {
            "omitted_source_bytes"
        } else {
            "omitted_bytes"
        };
        item[counter] = json!(item[counter].as_u64().unwrap_or(0) + (raw.len() - end) as u64);
        let range = if field == "evidence_text" {
            "source_byte_range"
        } else {
            "byte_range"
        };
        if item[range].is_object() {
            item[range]["end"] = json!(item[range]["start"].as_u64().unwrap_or(0) + end as u64);
        }
        if item["request_byte_range"].is_object() {
            item["request_byte_range"]["end"] =
                json!(item["request_byte_range"]["start"].as_u64().unwrap_or(0) + end as u64);
        }
        if item["line_range"].is_object() {
            item["line_range"]["end"] = json!(
                item["line_range"]["start"].as_u64().unwrap_or(1)
                    + raw[..end].trim_end_matches('\n').matches('\n').count() as u64
            );
        }
    } else {
        // Resume a source page at the delivered byte after budget reduction.
        if pointer=="/text" && value["source_offset"].is_u64() && value["entry"]["bytes"].is_u64() {
            let offset=value["source_offset"].as_u64().unwrap();
            let total=value["entry"]["bytes"].as_u64().unwrap();
            let next=offset+end as u64;
            value["next_source_offset"]=json!((next<total).then_some(next));
            value["omitted_bytes"]=json!(total.saturating_sub(next));
            value["continue_with"]=json!({"operation":"explain","arguments":{
                "report_id":value["report_id"],"ref":value["ref"],"source_offset":next}});
        }
        value["omitted_text_bytes"] =
            json!(value["omitted_text_bytes"].as_u64().unwrap_or(0) + (raw.len() - end) as u64);
        if value.get("bytes").is_some() {
            value["bytes"] = json!(end);
        }
        mark_partial(value);
    }
    omitted(value, pointer, raw.len() - end);
    refresh_items(value);
    Ok(true)
}

fn notice(
    value: &Value,
    max_bytes: usize,
    original_bytes: usize,
    size_of: &dyn Fn(&Value) -> Result<usize>,
) -> Result<Value> {
    let mut result = json!({"outcome":"partial","truncated":true,"omissions":{"unrendered_response_bytes":original_bytes},
        "instruction":"Response metadata or one result exceeds the budget. Increase max_bytes; use the pinned report or explanation handle for full support."});
    for key in [
        "operation",
        "id",
        "pins",
        "provenance",
        "pinned_views",
        "view_id",
        "current_view",
        "pending_or_interrupted",
        "retracted_count",
        "ledger_watermark",
        "report_id",
        "case_id",
        "outcome_scope",
        "views",
        "freshness",
        "budget_floor",
        "kind",
        "index_id",
        "link_id",
    ] {
        if let Some(v) = value.get(key) {
            result[key] = v.clone();
        }
    }
    if value["page"].is_object() {
        let offset=value["page"]["offset"].as_u64().unwrap_or(0);
        let total=value["page"]["total"].as_u64().unwrap_or(0);
        result["page"]=json!({"offset":offset,"returned":0,"total":total,"remaining":total.saturating_sub(offset),
            "next_offset":(offset<total).then_some(offset)});
        if let Some(id)=value["report_id"].as_str() {
            result["continue_with"]=super::reports::continuation(id,value["section"].as_str().unwrap_or("/items"),offset.try_into().unwrap(),100);
        } else {
            result["continue_with"]=value["continue_with"].clone();result["continue_with"]["arguments"]["offset"]=json!(offset);
        }
    } else if value["source_offset"].is_u64() && value["ref"].is_string() {
        result["ref"]=value["ref"].clone();
        result["continue_with"]=json!({"operation":"explain","arguments":{
            "report_id":value["report_id"],"ref":value["ref"],"source_offset":value["source_offset"]}});
    } else if value["kind"] == "agent_context" {
        result["outcome"]=value["outcome"].clone();
        // Budget failure changes delivery, not retained completeness. Preserve
        // the warning and its original support instead of hiding every gap.
        let preview: Vec<_> = rows(value, "not_covered").iter().take(1).cloned().collect();
        result["not_covered"] = json!(preview);
        let mut delivery = serde_json::Map::new();
        for field in ["files", "findings", "not_covered"] {
            let total = value["coverage"]["delivery"][field]["total"]
                .as_u64().or_else(||value["sections"][format!("/{field}")].as_u64())
                .unwrap_or(rows(value, field).len() as u64);
            let returned = if field == "not_covered" { preview.len() as u64 } else { 0 };
            delivery.insert(field.into(), json!({"total":total,"returned":returned,
                "omitted":total.saturating_sub(returned)}));
        }
        result["coverage"] = json!({"completeness":value["coverage"]["completeness"],"delivery":delivery});
        result["fetch_source"] = value["fetch_source"].clone();
        let section=if value["sections"].get("/candidates").is_some() {"/candidates"} else {"/not_covered"};
        result["continue_with"]=super::reports::continuation(value["case_id"].as_str().unwrap_or(""),section,0,20);
    } else if value["kind"] == "brief" {
        if let Some(outcome)=value.get("outcome") {result["outcome"]=outcome.clone();}
        result["report_id"]=value["id"].clone();
        result["continue_with"]=json!({"operation":"page","arguments":{"report_id":value["id"],"section":"/slice/impl","offset":0}});
    } else if value["kind"] == "architectural_impact_contract" {
        result["assessment"]=value["assessment"].clone();
        result["continue_with"]=super::reports::continuation(value["report_id"].as_str().unwrap_or(""),"/affected_components",0,20);
    }
    if let Some(id) = value["response"]["id"].as_str() {
        result["report_id"] = json!(id);
        result["continuation_token"] = json!(format!("{id}@0"));
    }
    if let Some(handle) = value.get("explanation_handle").or_else(|| {
        rows(&value["response"], "results")
            .first()
            .and_then(|r| r.get("explanation_handle"))
    }) {
        result["explanation_handle"] = handle.clone();
    }
    if measure(&mut result, max_bytes, size_of)? > max_bytes {
        // Keep pins and a visible warning even when the full freshness block cannot fit.
        if result.get("freshness").is_some() {
            result["freshness"] = json!({"state":"omitted_by_budget","pending_or_interrupted":value["freshness"]["pending_or_interrupted"]});
        }
    }
    let size = measure(&mut result, max_bytes, size_of)?;
    require(
        size <= max_bytes,
        "BUDGET",
        &format!("max_bytes is too small for pinned response metadata; need at least {size} bytes"),
    )?;
    Ok(result)
}

/// Bound the whole serialized response, including warnings, provenance and the
/// budget envelope. This changes presentation only, never retained support.
pub fn bounded(value: Value, max_bytes: usize) -> Result<Value> {
    bounded_with(value, max_bytes, &serialized_bytes)
}

/// Structured pages have no excerpt to shorten. Find a fitting contiguous
/// prefix in logarithmic measurements instead of serializing 1,000 rows once
/// for each discarded row. The first omitted row is always the next cursor.
fn fit_metadata_page(value:&mut Value,max_bytes:usize,size_of:&dyn Fn(&Value)->Result<usize>)->Result<bool> {
    let count=rows(value,"items").len();
    if count<1 || !value["page"].is_object() || !value["report_id"].is_string()
        || rows(value,"items").iter().any(|i|i.get("text").is_some()||i.get("evidence_text").is_some()) {return Ok(false);}
    fn prefix(value:&Value,count:usize,keep:usize)->Value {
        let mut trial=value.clone();trial["items"].as_array_mut().unwrap().truncate(keep);
        let removed=count-keep;
        if removed>0 {
            trial["omitted_candidate_count"]=json!(trial["omitted_candidate_count"].as_u64().unwrap_or(0)+removed as u64);
            omitted(&mut trial,"/items",removed);
        }
        let offset=trial["page"]["offset"].as_u64().unwrap_or(0) as usize;
        let total=trial["page"]["total"].as_u64().unwrap_or(0) as usize;
        let end=offset.saturating_add(keep);
        trial["page"]=json!({"offset":offset,"returned":keep,"total":total,"remaining":total.saturating_sub(end),"next_offset":(end<total).then_some(end)});
        let id=text(&trial,"report_id");let section=trial["section"].as_str().unwrap_or("/items");
        trial["continue_with"]=if end<total {super::reports::continuation(&id,section,end,100)} else {Value::Null};
        trial
    }
    // Ranking detail must not make a page unreadable. When no full row fits,
    // rows keep identity, rank and source_ref; a larger page restores detail.
    if measure(&mut prefix(value,count,1),max_bytes,size_of)?>max_bytes {
        let mut removed=0;
        for item in value["items"].as_array_mut().unwrap() {
            for key in ["signals","spans"] {
                if let Some(v)=item.as_object_mut().and_then(|o|o.remove(key)) {removed+=serialized_bytes(&v)?-1;}
            }
        }
        if removed>0 {omitted(value,"/items/ranking_detail_bytes",removed);}
        if measure(value,max_bytes,size_of)?<=max_bytes {return Ok(true);}
    }
    let (mut low,mut high)=(0,count);
    while low<high {
        let middle=(low+high+1)/2;let mut trial=prefix(value,count,middle);
        if measure(&mut trial,max_bytes,size_of)?<=max_bytes {low=middle;} else {high=middle-1;}
    }
    if low==0 {return Ok(false);}
    let mut fitted=prefix(value,count,low);measure(&mut fitted,max_bytes,size_of)?;
    *value=fitted;Ok(true)
}

/// Transports supply their final serializer, including wrappers, JSON escaping
/// and the trailing LF. Reductions and byte accounting remain shared.
pub(crate) fn bounded_with(
    mut value: Value,
    max_bytes: usize,
    size_of: &dyn Fn(&Value) -> Result<usize>,
) -> Result<Value> {
    require(
        (MIN_MAX_BYTES..=MAX_MAX_BYTES).contains(&max_bytes),
        "BUDGET",
        "Response budget must be 256..1048576 bytes",
    )?;
    summarize_limitations(&mut value);
    super::freshness::filter_returned_hits(&mut value);
    if value["kind"] == "agent_context" {
        // Agent projections arrive unbudgeted. Compact the actual freshness
        // envelope (including fallback) before measuring or removing files.
        // Retain the full diagnostic locally to recompute delivered overlap.
        let freshness = value.get("freshness").cloned();
        if let Some(full) = &freshness {
            value["freshness"] = super::freshness::agent_summary(full, &value);
        }
        let original = measure(&mut value, max_bytes, size_of)?;
        loop {
            if measure(&mut value, max_bytes, size_of)? <= max_bytes { return Ok(value); }
            if !super::task::brief::trim(&mut value) { return notice(&value, max_bytes, original, size_of); }
            if let Some(full) = &freshness {
                value["freshness"] = super::freshness::agent_summary(full, &value);
            }
        }
    }
    let original_items = rows(&value, "items").to_vec();
    let original = measure(&mut value, max_bytes, size_of)?;
    let brief_freshness=if value["kind"]=="brief" && original>max_bytes {
        value.get("freshness").cloned()
    }else{None};
    if let Some(full)=&brief_freshness {
        value["freshness"]=super::freshness::brief_summary(full,&value);
    }
    while measure(&mut value,max_bytes,size_of)? > max_bytes && super::task::brief::trim(&mut value) {
        if let Some(full)=&brief_freshness {
            value["freshness"]=super::freshness::brief_summary(full,&value);
        }
    }
    while measure(&mut value,max_bytes,size_of)? > max_bytes && super::task::trim_impact(&mut value) {}
    if measure(&mut value,max_bytes,size_of)? <= max_bytes {return Ok(value);}
    // Required case metadata can itself exceed a small presentation budget.
    // Return its pins and a restart, rather than making the retained case
    // inaccessible because the brief has a schema_version/sessions field.
    if value["kind"]=="brief" || value["kind"]=="architectural_impact_contract" {return notice(&value,max_bytes,original,size_of);}
    if original <= max_bytes {
        return Ok(value);
    }
    for key in ["profile", "configuration"] {
        if let Some(v) = value.as_object_mut().and_then(|o| o.remove(key)) {
            omitted(&mut value, &format!("/{key}"), serialized_bytes(&v)? - 1);
        }
        if measure(&mut value, max_bytes, size_of)? <= max_bytes {
            return Ok(value);
        }
    }
    if value["items"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item.get("evidence_text").is_some()))
    {
        if value["response"]["context"] == value["context"] {
            value.as_object_mut().unwrap().remove("context");
            omitted(&mut value, "/context", 1);
        }

        if let Some(details) = value.as_object_mut().and_then(|o| o.remove("details")) {
            omitted(
                &mut value,
                "/details",
                details.as_object().map_or(0, |d| d.len()),
            );
        }
    }
    if measure(&mut value, max_bytes, size_of)? > max_bytes {
        if let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) {
            let mut removed = 0;
            for item in items {
                if item.get("evidence_text").is_none() {
                    continue;
                }
                // Context is already pinned by the response. Declaration
                // routing identities remain, but repeated authority records
                // are read through the item's explanation handle.
                for key in [
                    "context",
                    "schema_version",
                    "token_count",
                    "relevance_score",
                    "routing_text",
                ] {
                    if let Some(v) = item.as_object_mut().unwrap().remove(key) {
                        removed += serialized_bytes(&v)? - 1;
                    }
                }
                if let Some(supports) = item["routing_support"].as_array_mut() {
                    for support in supports {
                        if let Some(v) = support.as_object_mut().and_then(|o| o.remove("origin")) {
                            removed += serialized_bytes(&v)? - 1;
                        }
                    }
                }
            }
            omitted(&mut value, "/items/metadata_bytes", removed);
        }
    }
    // Quoted content outranks secondary metadata: repeated diagnostics,
    // manifest summaries and rank prose yield before any excerpt is cut.
    if quoted_bytes(&value) > 0 {
        for pointer in [
            "/response/limitations",
            "/limitations",
            "/coverage/capture_limitations",
        ] {
            while measure(&mut value, max_bytes, size_of)? > max_bytes
                && value
                    .pointer(pointer)
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.len() > 1)
            {
                value
                    .pointer_mut(pointer)
                    .and_then(Value::as_array_mut)
                    .unwrap()
                    .pop();
                omitted(&mut value, pointer, 1);
            }
        }
        for key in ["projection_manifests", "rank_semantics"] {
            if measure(&mut value, max_bytes, size_of)? <= max_bytes {
                break;
            }
            if let Some(v) = value.as_object_mut().and_then(|o| o.remove(key)) {
                omitted(&mut value, &format!("/{key}"), serialized_bytes(&v)? - 1);
            }
        }
        while measure(&mut value, max_bytes, size_of)? > max_bytes
            && value
                .get_mut("graph_context")
                .and_then(Value::as_array_mut)
                .and_then(Vec::pop)
                .is_some()
        {
            omitted(&mut value, "/graph_context", 1);
            value["omitted_graph_count"] =
                json!(value["omitted_graph_count"].as_u64().unwrap_or(0) + 1);
        }
        // Evidence items keep their assertion, evidence-reference and
        // explanation identities; projection-local IDs, entity endpoints and
        // ranking diagnostics are read through the explanation handle.
        if measure(&mut value, max_bytes, size_of)? > max_bytes {
            let mut removed = 0;
            for item in value["items"].as_array_mut().into_iter().flatten() {
                if item.get("evidence_text").is_none() {
                    continue;
                }
                for key in [
                    "id",
                    "artifact_id",
                    "entity_version_ids",
                    "selection_reasons",
                    "routing_support",
                ] {
                    if let Some(v) = item.as_object_mut().unwrap().remove(key) {
                        removed += serialized_bytes(&v)? - 1;
                    }
                }
            }
            omitted(&mut value, "/items/identity_bytes", removed);
        }
    }
    if measure(&mut value, max_bytes, size_of)? > max_bytes {
        if let Some(pins) = value["freshness"]
            .get_mut("views")
            .and_then(Value::as_array_mut)
        {
            for pin in pins {
                if pin
                    .as_object_mut()
                    .unwrap()
                    .remove("repository_root")
                    .is_some()
                {
                    pin["root_metadata_omitted"] = json!(true);
                }
            }
        }
    }
    if measure(&mut value, max_bytes, size_of)? > max_bytes
        && rows(&value["freshness"], "views").len() == 1
    {
        let pin = value["freshness"]["views"][0].clone();
        // The primary time/age/state already appears once in the envelope.
        value["freshness"]["views"] = json!([{ "view_id":pin["view_id"],"scope":pin["scope"],
            "projection_check":pin["projection_check"],"projection_state":pin["projection_state"],"invalidated":pin["invalidated"] }]);
        omitted(&mut value, "/freshness/duplicate_view_metadata", 1);
    }
    // Page rows and source bytes outrank the full freshness diagnostic, which
    // otherwise takes most of a small page. Pins stay at the top level.
    if measure(&mut value,max_bytes,size_of)?>max_bytes && (value["page"].is_object()||value["source_offset"].is_u64())
        && value["freshness"]["views"].is_array() {
        let full=value["freshness"].clone();
        value["freshness"]=super::freshness::agent_summary(&full,&value);
        value["freshness"]["presentation"]=json!("summary_by_budget");
        let removed=serialized_bytes(&full)?.saturating_sub(serialized_bytes(&value["freshness"])?);
        omitted(&mut value,"/freshness",removed);
    }
    let original_quoted = quoted_bytes(&json!({ "items": original_items })) > 0;
    if measure(&mut value,max_bytes,size_of)?>max_bytes && fit_metadata_page(&mut value,max_bytes,size_of)? {return Ok(value);}
    while measure(&mut value, max_bytes, size_of)? > max_bytes {
        if original_quoted && value.get("budget_floor").is_none() && quoted_bytes(&value) == 0 {
            let size = measure(&mut value, max_bytes, size_of)?;
            budget_floor(&mut value, &original_items, size)?;
            continue;
        }
        if value
            .get_mut("graph_context")
            .and_then(Value::as_array_mut)
            .and_then(Vec::pop)
            .is_some()
        {
            omitted(&mut value, "/graph_context", 1);
            value["omitted_graph_count"] =
                json!(value["omitted_graph_count"].as_u64().unwrap_or(0) + 1);
            continue;
        }
        // If metadata alone cannot fit, drop the lowest-ranked item before
        // spending all of the budget on empty excerpts of several items.
        if value["items"]
            .as_array()
            .is_some_and(|items| items.len() > 1)
        {
            let mut metadata = value.clone();
            for item in metadata["items"].as_array_mut().unwrap() {
                for field in ["text", "evidence_text"] {
                    if item[field].is_string() {
                        item[field] = json!("");
                    }
                }
            }
            if measure(&mut metadata, max_bytes, size_of)? > max_bytes && drop_item(&mut value) {
                refresh_items(&mut value);
                continue;
            }
        }
        // Trim from the lowest-ranked item. A low-ranked item whose excerpt
        // would fall below a readable size is dropped instead, so top hits
        // keep quoted text rather than every hit keeping an empty excerpt.
        let count = value["items"].as_array().map_or(0, Vec::len);
        if let Some(index) = trim_index(&value) {
            let last = &value["items"][index];
            let excerpt = ["text", "evidence_text"].into_iter().find_map(|field| {
                last[field]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| (s.len(), field))
            });
            let overflow = measure(&mut value, max_bytes, size_of)? - max_bytes;
            match excerpt {
                Some((len, _))
                    if count > 1
                        && len.saturating_sub(overflow) < MIN_EXCERPT_BYTES.min(len / 2) =>
                {
                    drop_item(&mut value);
                    refresh_items(&mut value);
                    continue;
                }
                Some((_, field)) => {
                    shorten_excerpt(
                        &mut value,
                        &format!("/items/{index}/{field}"),
                        max_bytes,
                        size_of,
                    )?;
                    continue;
                }
                None if quoted_bytes(&value) > 0 => {
                    drop_item(&mut value);
                    refresh_items(&mut value);
                    continue;
                }
                None => {}
            }
        }
        if drop_item(&mut value) {
            refresh_items(&mut value);
            continue;
        }
        if rows(&value["response"], "results").len() > 1 && drop_row(&mut value) {
            continue;
        }
        if let Some(row) = rows(&value["response"], "results").first().cloned() {
            let id = text(&row, "id");
            if value["details"][&id]["omitted_by_budget"] != true {
                let bytes = serialized_bytes(&value["details"][&id])? - 1;
                value["details"][&id] = json!({"omitted_by_budget":true,"detail_bytes":bytes,
                    "instruction":"Use this row's explanation_handle for retained support; increase max_bytes to render the full detail."});
                omitted(&mut value, "/details", 1);
                mark_partial(&mut value);
                continue;
            }
        }
        if let Some(sessions) = value.get_mut("sessions").and_then(Value::as_array_mut) {
            if sessions.len() > 1 {
                sessions.pop();
                let end = value["offset"].as_u64().unwrap_or(0)
                    + value["sessions"].as_array().unwrap().len() as u64;
                value["next_offset"] = json!(end);
                value["omitted_publications"] = json!(value["total_publications"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(end));
                omitted(&mut value, "/sessions", 1);
                continue;
            }
        }
        if let Some(support) = value.get_mut("support").and_then(Value::as_array_mut) {
            let len = support.len();
            if len > 1 {
                support.pop();
                let start = value["support_count"]
                    .as_u64()
                    .unwrap_or(len as u64)
                    .saturating_sub(
                        value["omitted_support_count"].as_u64().unwrap_or(0) + len as u64,
                    );
                value["next_offset"] = json!(start + len as u64 - 1);
                value["omitted_support_count"] =
                    json!(value["omitted_support_count"].as_u64().unwrap_or(0) + 1);
                omitted(&mut value, "/support", 1);
                continue;
            }
        }
        if let Some(first) = rows(&value, "support").first().cloned() {
            if first["omitted_by_budget"] != true {
                let bytes = serialized_bytes(&first)? - 1;
                let mut summary = json!({"omitted_by_budget":true,"currently_retracted":first["currently_retracted"],
                    "instruction":"Support detail exceeds this presentation budget. Increase max_bytes on the same explanation_handle and offset for complete retained support."});
                for key in ["assertion", "unresolved_source"] {
                    if let Some(v) = first.get(key) {
                        summary[key] = v.clone();
                    }
                }
                // Sources keep their immutable IDs and exact locators. Text is
                // omitted, never replaced or claimed to have been read.
                if let Some(sources) = first["sources"].as_array() {
                    summary["source_refs"] = json!(sources
                        .iter()
                        .map(|s| json!({"evidence_ref":s["evidence_ref"],"artifact":s["artifact"]}))
                        .collect::<Vec<_>>());
                }
                value["support"][0] = summary;
                value["omitted_support_detail_bytes"] = json!(bytes);
                value["support_presentation"] = json!("partial");
                omitted(&mut value, "/support/0/detail_bytes", bytes);
                continue;
            }
        }
        if value["text"].is_string()
            && (value["format"].is_null() || value["format"] == "llm")
            && shorten_excerpt(&mut value, "/text", max_bytes, size_of)?
        {
            continue;
        }
        let pointer = [
            "/response/limitations",
            "/limitations",
            "/coverage/capture_limitations",
        ]
        .into_iter()
        .find(|p| {
            value
                .pointer(p)
                .and_then(Value::as_array)
                .is_some_and(|a| a.len() > 1)
        });
        if let Some(pointer) = pointer {
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_array_mut()
                .unwrap()
                .pop();
            omitted(&mut value, pointer, 1);
            continue;
        }
        if value.get("response").is_some()
            || value.get("support").is_some()
            || value.get("sessions").is_some()
            || value.get("schema_version").is_some()
        {
            let needed = measure(&mut value, max_bytes, size_of)?;
            return Err(format!("BUDGET: max_bytes is too small for required pinned metadata or one record; need at least {needed} bytes. Increase max_bytes. Pins: {}; report: {}; explanation: {}",
                value.get("pinned_views").or_else(||value.get("views")).unwrap_or(&Value::Null),
                value.get("id").or_else(||value.get("report_id")).unwrap_or(&value["response"]["id"]),
                value.get("explanation_handle").unwrap_or(&Value::Null)).into());
        }
        return notice(&value, max_bytes, original, size_of);
    }
    refresh_items(&mut value);
    super::freshness::filter_returned_hits(&mut value);
    if original_quoted && value.get("budget_floor").is_none() && quoted_bytes(&value) == 0 {
        let size = measure(&mut value, max_bytes, size_of)?;
        budget_floor(&mut value, &original_items, size)?;
        return bounded_with(value, max_bytes, size_of);
    }
    if value["response"].is_object() {
        super::contracts::schema(&value["response"], "query", Some("Response"))?;
    }
    // Refreshing counters can change the final envelope's digit count.
    if measure(&mut value, max_bytes, size_of)? > max_bytes {
        return bounded_with(value, max_bytes, size_of);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_counts_utf8_json_escaping_metadata_and_itself() {
        for raw in ["é😀".repeat(4000), "\"\\\n\t".repeat(4000)] {
            for cap in [1024, 5500, 16384] {
                let result = bounded(json!({"operation":"recall","view_id":"view:1", "items":[{"path":"src/a.rs","source_kind":"code","text":raw}],
                    "budget":{"returned_text_bytes":raw.len()},"configuration":{"noise":"x".repeat(8000)}}), cap).unwrap();
                let encoded = serde_json::to_vec(&result).unwrap();
                assert!(encoded.len() + 1 <= cap, "{cap}: {}", encoded.len());
                assert_eq!(
                    result["budget"]["returned_response_bytes"],
                    encoded.len() + 1
                );
                assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), result);
                assert!(result.get("omissions").is_some());
            }
        }
    }

    #[test]
    fn repeated_limitations_keep_the_warning_and_visible_counts() {
        let mut result = json!({"response":{"limitations":(0..80).map(|i| format!("Rust file outside supported Cargo module roots: fixture/{i}.rs")).collect::<Vec<_>>()}});
        summarize_limitations(&mut result);
        assert_eq!(
            result["response"]["limitations"].as_array().unwrap().len(),
            1
        );
        assert_eq!(result["omissions"]["/response/limitations"], 79);
        assert!(result["response"]["limitations"][0]
            .as_str()
            .unwrap()
            .contains("80 occurrences"));
    }

    #[test]
    fn unresolved_reference_limitations_collapse_by_predicate() {
        let mut result = json!({"response":{"limitations":(0..15).map(|i| format!("unresolved ENTRYPOINT_FOR reference for workflow:w{i}: symbol s{i}")).collect::<Vec<_>>()}});
        summarize_limitations(&mut result);
        let limitations = result["response"]["limitations"].as_array().unwrap();
        assert_eq!(limitations.len(), 1);
        assert!(limitations[0].as_str().unwrap().contains("15 occurrences"));
    }

    #[test]
    fn evidence_outranks_diagnostics_manifests_and_item_identities() {
        let item = |i: usize| {
            json!({"id":format!("retrieval-unit:{}", "u".repeat(64)),"artifact_id":"a".repeat(80),
            "entity_version_ids":["e".repeat(70),"f".repeat(70)],"selection_reasons":[{"RrfRank":i}],
            "routing_support":[{"assertion_id":"r".repeat(80),"domain":"Knowledge"}],
            "assertion_ids":[format!("assertion:{i}")],"evidence_ref_ids":[format!("evidence:{i}")],
            "explanation_handle":format!("explain:report:x:{i}"),"evidence_class":"evidence",
            "evidence_text":format!("pub struct Item{i} {{ field: u8 }}"),"source_locator":"src/a.rs"})
        };
        let result = bounded(json!({"operation":"retrieve","pinned_views":["view:1"],"items":[item(0),item(1)],
            "projection_manifests":[{"id":"p".repeat(400)}],"rank_semantics":"r".repeat(100),
            "graph_context":(0..5).map(|i| json!({"assertion_id":format!("{i}{}", "g".repeat(200))})).collect::<Vec<_>>(),
            "limitations":(0..20).map(|i| format!("diagnostic {i} {}", "d".repeat(80))).collect::<Vec<_>>(),
            "budget":{"returned_evidence_bytes":0}}), 1500).unwrap();
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "{result}");
        assert!(items
            .iter()
            .all(|i| i["evidence_text"].as_str().unwrap().ends_with("u8 }")
                && i["explanation_handle"].is_string()
                && i["assertion_ids"].is_array()
                && i["evidence_ref_ids"].is_array()));
        assert!(result.get("projection_manifests").is_none());
        assert!(serialized_bytes(&result).unwrap() <= 1500);
    }

    #[test]
    fn trimming_preserves_top_ranked_text_and_states_a_floor() {
        let items: Vec<Value> = (0..5)
            .map(|i| json!({"rank":i+1,"path":format!("src/{i}.rs"),"source_kind":"code","text":format!("{i}{}", "x".repeat(1400))}))
            .collect();
        let result = bounded(
            json!({"operation":"recall","view_id":"view:1","items":items.clone(),
            "budget":{"returned_text_bytes":7005}}),
            3000,
        )
        .unwrap();
        let kept = result["items"].as_array().unwrap();
        assert_eq!(kept[0]["path"], "src/0.rs");
        assert!(kept
            .iter()
            .all(|i| i["text"].as_str().unwrap().len() >= MIN_EXCERPT_BYTES));
        assert!(serialized_bytes(&result).unwrap() <= 3000);

        let starved = bounded(
            json!({"operation":"recall","view_id":"view:1","items":items,
            "coverage":{"noise":"n".repeat(1200)},"budget":{"returned_text_bytes":7005}}),
            1500,
        )
        .unwrap();
        assert_eq!(quoted_bytes(&starved), 0);
        assert!(
            starved["budget_floor"]["minimum_useful_bytes"]
                .as_u64()
                .unwrap()
                > 1500
        );
        assert!(serialized_bytes(&starved).unwrap() <= 1500);
    }

    #[test]
    fn a_large_row_keeps_its_handle_and_never_a_false_pass() {
        let result = bounded(json!({"pinned_views":["view:1"],"response":{"schema_version":"1.0","operation":"dependencies","context":{"kind":"source","snapshot_id":"snapshot:1","build_context_id":"build:1"},"view":"evidence","projection_ids":["projection:1"],"ledger_watermark":1,"limitations":[],"truncated":false,"required_predicates":[],"id":"report:test","outcome":"complete","resolution_complete":true,
            "results":[{"id":"report:test:row:0","assertion_ids":[],"conclusion":"pass","explanation_handle":"explain:report:test:0"}]},
            "details":{"report:test:row:0":{"body":"x".repeat(10000)}}}), 1600).unwrap();
        assert_eq!(
            result["response"]["results"][0]["explanation_handle"],
            "explain:report:test:0"
        );
        assert_eq!(
            result["details"]["report:test:row:0"]["omitted_by_budget"],
            true
        );
        assert_eq!(result["response"]["resolution_complete"], false);
        assert_eq!(result["pinned_views"], json!(["view:1"]));
        assert!(serialized_bytes(&result).unwrap() <= 1600);
    }
}

#[cfg(test)]
mod metadata_page_tests {
    use super::*;
    #[test]
    fn large_metadata_pages_fit_with_logarithmic_measurements_and_a_contiguous_cursor() {
        let doc=json!({"kind":"occurrences","report_id":"context-report:fixture","view_id":"view:fixture",
            "items":(11..1011).map(|n|json!({"start_byte":n,"path":"src/a.rs","detail":"x".repeat(180)})).collect::<Vec<_>>(),
            "page":{"offset":11,"returned":1000,"total":1011}});
        let calls=std::cell::Cell::new(0usize);
        let measure_with=|value:&Value| {calls.set(calls.get()+1);serialized_bytes(value)};
        let result=bounded_with(doc,8192,&measure_with).unwrap();
        let returned=rows(&result,"items").len();assert!(returned>0&&returned<1000);
        assert!(serialized_bytes(&result).unwrap()<=8192);
        assert_eq!(result["items"][0]["start_byte"],11);
        assert_eq!(result["items"][returned-1]["start_byte"],10+returned);
        assert_eq!(result["page"]["next_offset"],11+returned);
        let cursor:Value=serde_json::from_str(result["continue_with"]["arguments"]["cursor"].as_str().unwrap()).unwrap();
        assert_eq!(cursor["offset"],11+returned);assert_eq!(cursor["section"],"/items");
        assert!(calls.get()<100,"must not reserialize once per discarded row: {}",calls.get());
    }
    #[test]
    fn an_oversized_agent_manifest_preserves_a_pinned_restart() {
        let doc=json!({"kind":"agent_context","case_id":"task-case:fixture","report_id":"task-case:fixture","outcome":"partial",
            "sections":{"/candidates":1},"files":[{"path":"src/a.rs","detail":"x".repeat(20000)}]});
        let result=bounded(doc,1600).unwrap();
        assert_eq!(result["case_id"],"task-case:fixture");
        let cursor:Value=serde_json::from_str(result["continue_with"]["arguments"]["cursor"].as_str().unwrap()).unwrap();
        assert_eq!(cursor["report_id"],"task-case:fixture");assert_eq!(cursor["section"],"/candidates");assert_eq!(cursor["offset"],0);
    }
}
