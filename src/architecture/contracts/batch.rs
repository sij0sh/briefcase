use super::*;
/// Declared semantic bindings (ADR-0029). A binding resolution row is
/// provenance about one retained declaration's technical reference, scoped
/// to that declaration's own context; a declared membership assertion
/// exists only where a resolved row reconstructs it from the authority.
pub(crate) fn semantic_bindings(b: &Value, ix: &Index) -> Result<()> {
    let mut produced = BTreeSet::new();
    for r in rows(b, "semantic_binding_resolutions") {
        let mut input = r.clone();
        input.as_object_mut().unwrap().remove("id");
        require(
            r["id"] == identity::id("semantic-binding-resolution", &input)?,
            "IDENTITY",
            "Binding resolution identity mismatch",
        )?;
        require(
            r["binding_key"]
                == identity::id(
                    "semantic-binding",
                    &json!({"declaration_record_id":r["declaration_record_id"],"owner_entity_id":r["owner_entity_id"],
                        "predicate":r["predicate"],"requested_kind":r["requested_kind"],"requested_key":r["requested_key"]}),
                )?,
            "IDENTITY",
            "Binding key mismatch",
        )?;
        let d = ix.field("declaration_records", r, "declaration_record_id")?;
        require(
            d["context"] == r["context"],
            "SCOPE",
            "Binding resolution exceeds its declaration scope",
        )?;
        let owner = ix.field("entity_versions", r, "owner_entity_id")?;
        let owner_kind = if r["predicate"] == "IMPLEMENTS_FEATURE" {
            "feature"
        } else {
            "workflow"
        };
        require(
            owner["kind"] == owner_kind && owner["context"] == r["context"],
            "SCOPE",
            "Binding owner is not a semantic entity of the declaration scope",
        )?;
        let candidates = strings(&r["candidate_ids"]);
        for cid in &candidates {
            let c = ix.get("entity_versions", cid)?;
            require(
                c["context"] == r["context"] && c["kind"] == r["requested_kind"],
                "SCOPE",
                "Binding candidate outside the declaration scope or kind",
            )?;
        }
        let expected = match r["status"].as_str() {
            Some("resolved") => candidates.len() == 1,
            Some("ambiguous") => candidates.len() >= 2,
            _ => candidates.is_empty(),
        };
        require(
            expected,
            "RESOLUTION",
            "Binding status disagrees with its candidates",
        )?;
        if r["status"] == "resolved" {
            let a = ix.field("assertions", r, "assertion_id")?;
            let p = &a["proposition"];
            require(
                a["class"] == "declaration"
                    && a["origin"]["declaration_record_id"] == r["declaration_record_id"]
                    && p["subject_id"] == json!(candidates[0])
                    && p["predicate"] == r["predicate"]
                    && p["object"]["entity_version_id"] == r["owner_entity_id"],
                "RESOLUTION",
                "Resolved binding differs from its Declaration",
            )?;
            produced.insert(text(a, "id"));
        } else {
            require(
                r["assertion_id"].is_null(),
                "RESOLUTION",
                "Only a resolved binding produces a Declaration",
            )?;
        }
    }
    for a in rows(b, "assertions").iter().filter(|a| {
        a["class"] == "declaration"
            && ["IMPLEMENTS_FEATURE", "ENTRYPOINT_FOR", "PARTICIPATES_IN"]
                .contains(&a["proposition"]["predicate"].as_str().unwrap_or(""))
    }) {
        require(
            produced.contains(&text(a, "id")),
            "CAPABILITY",
            "Declared semantic binding lacks a resolved binding resolution",
        )?;
    }
    Ok(())
}

pub fn validate_batch(
    batch: &Value,
    bundle: &Value,
    payloads: &Payloads,
    capability: Capability,
) -> Result<()> {
    schema(batch, "batch", None)?;
    validate(bundle, payloads)?;
    require(
        batch["class"] == capability.class().name(),
        "CAPABILITY",
        "Caller cannot write this class",
    )?;
    let ix = Index::new(bundle)?;
    let mut records = Vec::new();
    for id in strings(&batch["record_ids"]) {
        let r = ix.get("assertions", &id)?;
        require(r["class"] == batch["class"], "CLASS", "Batch mixes classes")?;
        require(
            r["context"] == batch["context"],
            "SCOPE",
            "Batch mixes scopes",
        )?;
        records.push(r.clone());
    }
    records.sort_by_key(|r| text(r, "id"));
    let a = ix.field("artifacts", batch, "payload_artifact_id")?;
    require(
        a["availability"] == "available",
        "UNAVAILABLE",
        "Transport must be retained",
    )?;
    require(
        a["digest"] == batch["payload_digest"],
        "DIGEST",
        "Transport digest mismatch",
    )?;
    let payload = identity::parse(&payloads[&text(a, "id")])?;
    require(
        payload
            == json!({"context":batch["context"],"class":batch["class"],"producer_id":batch["producer_id"],"records":records}),
        "DIGEST",
        "Transport differs from submitted records",
    )
}

pub fn validate_query(response: &Value, b: &Value, payloads: &Payloads) -> Result<()> {
    schema(response, "query", Some("Response"))?;
    let taint = validate(b, payloads)?;
    validate_query_with_taint(response, b, &taint)
}

/// Response-scoped checks over an already-validated bundle. The caller must
/// have established full bundle validity for these exact bytes (directly or
/// through a content-addressed verdict); taint must come from the same logic.
pub fn validate_query_with_taint(
    response: &Value,
    b: &Value,
    taint: &BTreeMap<String, bool>,
) -> Result<()> {
    schema(response, "query", Some("Response"))?;
    let ix = Index::new(b)?;
    let mut selected = Vec::new();
    for id in strings(&response["projection_ids"]) {
        let p = ix.get("projections", &id)?;
        require(
            p["context"] == response["context"],
            "SCOPE",
            "Query/projection scope differs",
        )?;
        selected.push(p);
    }
    let compatible = selected.iter().all(|p| {
        p["ledger_watermark"] == response["ledger_watermark"]
            && p["generation"] == selected[0]["generation"]
            && strings(&p["input_assertion_ids"])
                .into_iter()
                .collect::<BTreeSet<_>>()
                == strings(&selected[0]["input_assertion_ids"])
                    .into_iter()
                    .collect::<BTreeSet<_>>()
    });
    if response["outcome"] == "complete" {
        require(
            compatible,
            "FRESHNESS",
            "Complete query combines incompatible projections",
        )?;
    } else if !compatible {
        require(
            !strings(&response["limitations"]).is_empty(),
            "FRESHNESS",
            "Mixed projections need limitation",
        )?;
    }
    let allowed: &[&str] = match response["view"].as_str().unwrap() {
        "evidence" => &["evidence", "derived"],
        "intent" => &["declaration"],
        "interpretation" => &["claim", "derived"],
        _ => &["evidence", "derived", "claim", "declaration"],
    };
    let available: BTreeSet<_> = selected
        .iter()
        .flat_map(|p| strings(&p["input_assertion_ids"]))
        .collect();
    for r in rows(response, "results") {
        for id in strings(&r["assertion_ids"]) {
            require(
                available.contains(&id),
                "UNKNOWN_REFERENCE",
                "Result not in selected projections",
            )?;
            let a = ix.get("assertions", &id)?;
            require(
                allowed.contains(&a["class"].as_str().unwrap()),
                "VIEW",
                "Result class forbidden",
            )?;
            if response["view"] == "evidence" {
                require(!taint[&id], "TAINT", "Inference in evidence view")?;
            }
        }
        if r["conclusion"] == "absent" || r["conclusion"] == "pass" {
            require(
                response["resolution_complete"] == true
                    && response["truncated"] == false
                    && response["outcome"] == "complete"
                    && !strings(&response["required_predicates"]).is_empty(),
                "COVERAGE",
                "Negative/compliance lacks complete resolution",
            )?;
            if response["context"]["kind"] == "source" {
                let s = ix.field("snapshots", &response["context"], "snapshot_id")?;
                require(
                    s["capture_state"] == "complete",
                    "COVERAGE",
                    "Partial capture cannot establish absence",
                )?;
            }
            for predicate in strings(&response["required_predicates"]) {
                require(
                    rows(b, "runs").iter().any(|run| {
                        run["context"] == response["context"]
                            && run["coverage"]["resolution_complete"] == true
                            && rows(&run["coverage"], "guarantees").iter().any(|g| {
                                g["predicate"] == predicate
                                    && (g["kind"] == "exact_supported"
                                        || g["kind"] == "sound_overapproximation")
                            })
                    }),
                    "COVERAGE",
                    "Predicate-specific negative guarantee missing",
                )?;
            }
        }
    }
    if response["truncated"] == true {
        require(
            response["continuation_token"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "PAGINATION",
            "Truncated query needs pinned continuation",
        )?;
    }
    Ok(())
}
