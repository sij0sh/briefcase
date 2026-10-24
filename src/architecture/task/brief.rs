//! Presentation of a retained case. Brief documents are never retained.
use super::request::{self, Presentation};
use super::*;

pub fn brief(l: &Ledger, a: &Value) -> Result<Value> {
    let (case, presentation) = build_case(l, a)?;
    render(&case, presentation)
}

pub(super) fn build_case(l: &Ledger, a: &Value) -> Result<(case::Case, Presentation)> {
    let (request, presentation) = request::parse(a)?;
    let scope = serde_json::to_value(&request.scope)?;
    let snapshot = Snapshot::load_with(l, &request.view, &scope)?;
    let ix = index::load(l, &snapshot)?;
    let trace = trace::build(l, &snapshot, &ix, &request)?;
    let state = case::LedgerState {
        watermark: l.watermark()?,
        pending_source: l.pending_source()?,
        retraction_epoch: snapshot.view.retraction_watermark,
    };
    let case = case::retain(
        l,
        case::assemble(
            &request,
            &snapshot,
            &ix,
            &trace,
            &request.duty.policy().case,
            state,
        )?,
    )?;
    Ok((case, presentation))
}

pub(super) fn render(case: &case::Case, presentation: Presentation) -> Result<Value> {
    let mut doc = serde_json::to_value(case)?;
    doc.as_object_mut().unwrap().remove("request");
    doc["kind"] = json!("brief");
    doc["case_id"] = json!(case.id);
    doc["report_id"] = json!(case.id);
    doc["instruction"]=json!("Use not_covered to identify questions that could change the design or verification. Complete describes retained evidence scope; partial evidence is not a prohibition on implementation. Neither establishes runtime truth, task completion or approval. Reuse fresh context and follow pinned references as needed.");
    doc["coverage"]["completeness_contract"]["C7_delivery"]=json!("Brief presentation only; coverage.delivery counts returned and omitted rows. Page reads the retained case.");
    crate::architecture::contracts::schema(&doc, "task-brief", None)?;
    preview(&mut doc, presentation.limit);
    if let Some(max_bytes) = presentation.max_bytes {
        return crate::architecture::response::bounded(doc, max_bytes);
    }
    Ok(doc)
}

fn preview(doc: &mut Value, limit: usize) {
    let mut delivery = serde_json::Map::new();
    for pointer in super::case::sections(doc) {
        if let Some(items) = doc.pointer_mut(&pointer).and_then(Value::as_array_mut) {
            let total = items.len();
            items.truncate(limit);
            delivery.insert(
                pointer,
                json!({"total":total,"returned":items.len(),"omitted":total-items.len(),
                "next_offset":(items.len()<total).then_some(items.len())}),
            );
        }
    }
    doc["coverage"]["delivery"] = json!(delivery);
    doc["continue_with"] = doc["coverage"]["delivery"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, v)| v["omitted"].as_u64().unwrap_or(0) > 0)
        .map(|(section, v)| {
            crate::architecture::reports::continuation(
                doc["id"].as_str().unwrap(),
                section,
                v["returned"].as_u64().unwrap() as usize,
                limit,
            )
        })
        .unwrap_or(Value::Null);
}

/// Preserve the packet contract while reducing presentation. Full tables stay
/// in the retained case; every dropped row updates its section cursor.
pub(crate) fn trim(value: &mut Value) -> bool {
    if value["kind"] == "agent_context" {
        return super::agent::trim(value);
    }
    if value["kind"] != "brief" {
        return false;
    }
    let largest = super::case::sections(value)
        .into_iter()
        .filter(|p| p != "/not_covered")
        .filter_map(|p| {
            value
                .pointer(&p)
                .and_then(Value::as_array)
                .map(|a| (p, a.len()))
        })
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n);
    let Some((pointer, _)) = largest else {
        return false;
    };
    let items = value.pointer_mut(&pointer).unwrap().as_array_mut().unwrap();
    items.pop();
    let returned = items.len();
    let delivery = &mut value["coverage"]["delivery"][&pointer];
    let total = delivery["total"].as_u64().unwrap_or(returned as u64 + 1);
    *delivery = json!({"total":total,"returned":returned,"omitted":total.saturating_sub(returned as u64),"next_offset":returned});
    value["continue_with"] = json!({"operation":"page","arguments":{"report_id":value["id"],"section":pointer,"offset":returned}});
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet() -> Value {
        json!({"id":"task-case:test","case_id":"task-case:test","report_id":"task-case:test","kind":"brief",
            "slice":{"impl":[{"path":"a"},{"path":"b"},{"path":"c"},{"path":"d"}]},
            "not_covered":[{"area":"history"},{"area":"sessions"},{"area":"capture"}],
            "neighborhood":[],"coverage":{"completeness":{"slice":"established","history":"unknown"}}})
    }
    #[test]
    fn preview_and_byte_trimming_change_delivery_without_changing_evidence_states() {
        let mut doc = packet();
        let completeness = doc["coverage"]["completeness"].clone();
        preview(&mut doc, 2);
        assert_eq!(
            doc["coverage"]["delivery"]["/slice/impl"],
            json!({"total":4,"returned":2,"omitted":2,"next_offset":2})
        );
        assert_eq!(doc["coverage"]["delivery"]["/not_covered"]["total"], 3);
        let cursor: Value = serde_json::from_str(
            doc["continue_with"]["arguments"]["cursor"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(cursor["report_id"], "task-case:test");
        assert!(trim(&mut doc));
        assert_eq!(
            doc["coverage"]["delivery"]["/slice/impl"],
            json!({"total":4,"returned":1,"omitted":3,"next_offset":1})
        );
        assert_eq!(
            doc["continue_with"]["arguments"]["report_id"],
            "task-case:test"
        );
        assert_eq!(doc["continue_with"]["arguments"]["offset"], 1);
        while trim(&mut doc) {}
        assert_eq!(rows(&doc, "not_covered").len(), 2);
        assert_eq!(doc["coverage"]["completeness"], completeness);
    }
    #[test]
    fn bounded_projection_keeps_case_pins_and_the_256_byte_floor() {
        let mut doc = packet();
        doc["slice"]["impl"] = json!([{"path":"a","detail":"é".repeat(20000)}]);
        preview(&mut doc, 2);
        let result = crate::architecture::response::bounded(doc.clone(), 1600).unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() + 1 <= 1600);
        assert_eq!(result["case_id"], "task-case:test");
        assert_eq!(result["report_id"], "task-case:test");
        assert!(crate::architecture::response::bounded(doc, 255).is_err());
    }
    #[test]
    fn oversized_case_metadata_returns_a_pinned_notice_instead_of_a_budget_error() {
        let mut doc = packet();
        doc["schema_version"] = json!("1.0");
        doc["outcome"] = json!("unknown");
        doc["coverage"]["large_metadata"] = json!("x".repeat(20000));
        doc["freshness"] = json!({"views":[{"stale":true,"projection_state":"stale_or_invalidated","reason":"x".repeat(20000)}],
            "pending_or_interrupted":true,"fallback":{"used":true}});
        preview(&mut doc, 2);
        let result = crate::architecture::response::bounded(doc, 8000).unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() + 1 <= 8000);
        assert_eq!(result["case_id"], "task-case:test");
        assert_eq!(result["outcome"], "unknown");
        assert_eq!(result["freshness"]["state"], "stale");
        assert_eq!(result["freshness"]["fallback"], true);
        assert_eq!(
            result["continue_with"]["arguments"]["report_id"],
            "task-case:test"
        );
        assert_eq!(result["continue_with"]["arguments"]["offset"], 0);
    }
}
