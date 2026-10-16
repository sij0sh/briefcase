use super::*;
/// Historical lineage with explicit certainty classes. Exact
/// `RENAMED_FROM` movement and inference-backed `LIKELY_SAME_ENTITY`
/// continuity are never collapsed: every finding carries its class, and
/// ambiguous candidates stay unknown. Modes: exact, likely, combined,
/// ambiguous.
pub fn history_lineage(l: &Ledger, base: &str, o: &Options, mode: &str) -> Result<Value> {
    require(
        ["exact", "likely", "combined", "ambiguous"].contains(&mode),
        "LINEAGE",
        "Unknown lineage mode; use exact, likely, combined, or ambiguous",
    )?;
    let mut bo = o.clone();
    bo.view = base.into();
    let mut bv = loaded(l, &bo)?;
    let mut av = loaded(l, o)?;
    require(
        context(&bv)["snapshot_id"] != context(&av)["snapshot_id"],
        "COMPARE",
        "Lineage requires two distinct captured snapshots",
    )?;
    let correspondence = super::super::history::load(l, &bv, &av)?;
    let before_snapshot = text(&context(&bv), "snapshot_id");
    let after_snapshot = text(&context(&av), "snapshot_id");
    let inferred = l.lineage_overlays(&before_snapshot, &after_snapshot)?;
    let mut watermark = correspondence.watermark;
    for overlay in &inferred {
        watermark = watermark.max(overlay["watermark"].as_u64().unwrap_or(0));
    }
    bv.overlay_watermark = watermark;
    av.overlay_watermark = watermark;
    let before_keys = entities(&bv);
    let after_keys = entities(&av);
    let removed = l.retracted()?;
    let mut findings = Vec::new();
    if ["exact", "combined"].contains(&mode) {
        for a in &correspondence.exact {
            let id = text(a, "id");
            if !o.historical && removed.contains(&id) {
                continue;
            }
            let from = text(&a["proposition"]["object"], "entity_version_id");
            let to = text(&a["proposition"], "subject_id");
            findings.push(Finding{ids:vec![id.clone()],conclusion:"present",detail:json!({"change":"renamed","certainty_class":"exact_content_move",
                "from":before_keys[&from],"to":after_keys[&to],"predicate":"RENAMED_FROM","assertion_id":id,
                "from_entity_version_id":from,"to_entity_version_id":to,"method":"unique-retained-module-content-v1",
                "limitation":"Exact retained-content movement between distinct versions; not eternal identity."})});
            bv.eligible.insert(id.clone());
            av.eligible.insert(id);
        }
    }
    if ["likely", "combined"].contains(&mode) {
        for overlay in &inferred {
            for id in strings(&overlay["claim_ids"]) {
                if !o.historical && removed.contains(&id) {
                    continue;
                }
                let a = super::super::knowledge::row(&overlay["bundle"], "assertions", &id)?;
                if text(&a["proposition"], "predicate") != "LIKELY_SAME_ENTITY" {
                    continue;
                }
                let from = text(&a["proposition"]["object"], "entity_version_id");
                let to = text(&a["proposition"], "subject_id");
                let from_key = before_keys.get(&from).cloned().unwrap_or_default();
                let to_key = after_keys.get(&to).cloned().unwrap_or_default();
                // Scores are uncalibrated metadata, not identity inputs: only the
                // semantics tag enters the finding; the value stays on the
                // retained Claim and its explanation.
                let score_semantics = a["model_score"]["semantics"].clone();
                findings.push(Finding{ids:vec![id.clone()],conclusion:"present",detail:json!({"change":"likely_continuity","certainty_class":"inferred_continuity",
                    "from":from_key,"to":to_key,"predicate":"LIKELY_SAME_ENTITY","assertion_id":id,
                    "from_entity_version_id":from,"to_entity_version_id":to,
                    "inference_id":a["origin"]["inference_id"],"supports":a["supports"],"score_semantics":score_semantics,
                    "overlay_id":overlay["overlay_id"],
                    "limitation":"Inference-backed proposal from bounded candidate evidence; similarity cannot establish identity."})});
                bv.eligible.insert(id.clone());
                av.eligible.insert(id);
            }
        }
    }
    if ["ambiguous", "combined"].contains(&mode) {
        for detail in super::super::history::ambiguous_details(l, &correspondence)? {
            let mut detail = detail;
            detail["certainty_class"] = json!("ambiguous_candidates");
            detail["change"] = if detail["status"] == "ambiguous" {
                json!("ambiguous_correspondence")
            } else {
                detail["status"].clone()
            };
            let ids = strings(&detail["input_assertion_ids"]);
            if !o.historical && ids.iter().any(|id| removed.contains(id)) {
                continue;
            }
            findings.push(Finding {
                ids,
                conclusion: "unknown",
                detail,
            });
        }
    }
    super::super::knowledge::merge(&mut bv.bundle, &correspondence.bundle)?;
    for overlay in &inferred {
        super::super::knowledge::merge(&mut bv.bundle, &overlay["bundle"])?;
        super::super::knowledge::merge(&mut av.bundle, &overlay["bundle"])?;
    }
    let ctx = json!({"kind":"comparison","snapshot_ids":[before_snapshot, after_snapshot]});
    finish(
        l,
        "history-lineage",
        "combined",
        &[bv, av],
        Some(ctx),
        findings,
        o,
        false,
    )
}
// Breadth-first search gives a shortest, bounded witness. Exhaustion is independent of presentation limits.
pub(crate) fn path(
    edges: &[(String, String, String)],
    from: &str,
    to: &str,
) -> Option<Vec<String>> {
    let mut q = VecDeque::from([(from.to_owned(), Vec::new())]);
    let mut seen = BTreeSet::from([from.to_owned()]);
    while let Some((node, ids)) = q.pop_front() {
        for (a, b, id) in edges {
            if a == &node {
                let mut ids = ids.clone();
                ids.push(id.clone());
                if b == to {
                    return Some(ids);
                }
                if seen.insert(b.clone()) {
                    q.push_back((b.clone(), ids));
                }
            }
        }
    }
    None
}
pub fn policy(l: &Ledger, o: &Options) -> Result<Value> {
    let v = loaded(l, o)?;
    let removed = l.retracted()?;
    let payloads = l.payloads(&v.bundle)?;
    let es = entities(&v);
    let edges: Vec<_> = eligible(&v, &removed, o.historical, "DEPENDS_ON")
        .iter()
        .map(|a| {
            let p = &a["proposition"];
            (
                text(p, "subject_id"),
                text(&p["object"], "entity_version_id"),
                text(a, "id"),
            )
        })
        .collect();
    let declaration = rows(&v.bundle, "declaration_records").first();
    if rows(&v.bundle, "declaration_records").len() > 1 {
        return finish(
            l,
            "policy",
            "combined",
            &[v],
            None,
            vec![Finding {
                ids: vec![],
                conclusion: "unknown",
                detail: json!({"reason":"Multiple applicable declarations require an explicit registered precedence; this MVP accepts one policy source per view."}),
            }],
            o,
            false,
        );
    }
    let mut findings = Vec::new();
    if let Some(d) = declaration {
        let p = identity::parse(&payloads[&text(d, "artifact_id")])?;
        for r in rows(&p, "rules") {
            let from = text(r, "from");
            let to = text(r, "to");
            let candidates = eligible(&v, &removed, o.historical, "FORBIDS_DEPENDENCY");
            let declarations: Vec<_> = candidates
                .iter()
                .filter(|a| {
                    es[&text(&a["proposition"], "subject_id")] == from
                        && es[&text(&a["proposition"]["object"], "entity_version_id")] == to
                })
                .copied()
                .collect();
            let mut ids: Vec<_> = declarations.iter().map(|a| text(a, "id")).collect();
            let mut witness = Vec::new();
            let conclusion = if declarations.len() != 1 {
                "unknown"
            } else {
                let a = declarations[0];
                let from_id = text(&a["proposition"], "subject_id");
                let to_id = text(&a["proposition"]["object"], "entity_version_id");
                if let Some(found) = path(&edges, &from_id, &to_id) {
                    witness = found.clone();
                    ids.extend(found);
                    "violation"
                } else if complete(&v) && !v.stale {
                    "pass"
                } else {
                    "unknown"
                }
            };
            findings.push(Finding{ids,conclusion,detail:json!({"rule_id":r["id"],"from":from,"to":to,"mode":"report-only","authority":p["authority"],"policy_revision":d["revision"],"declaration_record_id":d["id"],"policy_artifact_id":d["artifact_id"],"witness_assertion_ids":witness,"required_predicates":["IMPORTS"],"coverage_scope":"Captured supported explicit imports; no claim about calls, generated code or runtime."})});
        }
    } else {
        findings.push(Finding {
            ids: vec![],
            conclusion: "unknown",
            detail: json!({"reason":"No registered architecture policy; scan with --policy."}),
        });
    }
    let c = complete(&v) && !v.stale && findings.iter().all(|f| f.conclusion != "unknown");
    finish(l, "policy", "combined", &[v], None, findings, o, c)
}
/// Interpretations and declarations coexist. No precedence or score averaging
/// converts a Claim into architecture intent.
pub fn domains(l: &Ledger, o: &Options) -> Result<Value> {
    let v = loaded(l, o)?;
    let es = entities(&v);
    let removed = l.retracted()?;
    if let Some(key) = &o.component {
        if let Some(f) = ambiguity(&v, key) {
            return finish(l, "domains", "combined", &[v], None, vec![f], o, false);
        }
    }
    let subject = o.component.as_ref().map(|key| selected(&v, key)[0].clone());
    let semantic: BTreeSet<String> = rows(&v.bundle, "entity_versions")
        .iter()
        .filter(|e| e["kind"] == "feature" || e["kind"] == "workflow")
        .map(|e| text(e, "id"))
        .collect();
    let mut findings = Vec::new();
    for a in rows(&v.bundle, "assertions").iter().filter(|a| {
        a["proposition"]["predicate"] == "BELONGS_TO_DOMAIN"
            && v.eligible.contains(&text(a, "id"))
            && (o.historical || !removed.contains(&text(a, "id")))
    }) {
        let sid = text(&a["proposition"], "subject_id");
        let tid = text(&a["proposition"]["object"], "entity_version_id");
        // Feature and workflow domains belong to the semantic catalog
        // (optional feature declarations); this report keeps its technical scope.
        if semantic.contains(&sid) {
            continue;
        }
        if subject.as_ref().is_some_and(|s| s != &sid) {
            continue;
        }
        // A Claim whose candidate Evidence population has moved is stale:
        // it is reported as unknown, never as a current assignment.
        if a["class"] == "claim" {
            match super::super::inference::claim_freshness(l, &v, a)? {
                super::super::inference::ClaimFreshness::Current => {}
                freshness => {
                    findings.push(Finding{ids:vec![text(a,"id")],conclusion:"unknown",detail:json!({"component":es[&sid],
                        "kind":"stale_inference","freshness":format!("{freshness:?}").to_lowercase(),
                        "reason":"An earlier inferred classification exists, but its Evidence inputs no longer match the current analysis."})});
                    continue;
                }
            }
        }
        findings.push(Finding{ids:vec![text(a,"id")],conclusion:"present",detail:json!({"component":es[&sid],"domain":es[&tid],
            "subject_id":sid,"domain_entity_version_id":tid,"class":a["class"],"origin":a["origin"],
            "authority":if a["class"] == "declaration" {"declared architecture"} else {"advisory interpretation"}})});
    }
    // Put interpretations first so their proof is immediately inspectable.
    findings.sort_by_key(|f| (f.detail["class"] != "claim", f.ids.clone()));
    finish(l, "domains", "combined", &[v], None, findings, o, false)
}
pub fn drift(l: &Ledger, o: &Options) -> Result<Value> {
    let v = loaded(l, o)?;
    let es = entities(&v);
    let removed = l.retracted()?;
    if let Some(key) = &o.component {
        if let Some(f) = ambiguity(&v, key) {
            return finish(l, "drift", "combined", &[v], None, vec![f], o, false);
        }
    }
    let subject = o.component.as_ref().map(|key| selected(&v, key)[0].clone());
    let assignments: Vec<_> = rows(&v.bundle, "assertions")
        .iter()
        .filter(|a| {
            a["proposition"]["predicate"] == "BELONGS_TO_DOMAIN"
                && v.eligible.contains(&text(a, "id"))
                && (o.historical || !removed.contains(&text(a, "id")))
        })
        .filter(|a| {
            subject
                .as_ref()
                .is_none_or(|s| a["proposition"]["subject_id"] == *s)
        })
        .collect();
    // Stale Claims cannot disagree with a declaration as current findings.
    let mut current_claims = Vec::new();
    for claim in assignments.iter().filter(|a| a["class"] == "claim") {
        if super::super::inference::claim_freshness(l, &v, claim)?
            == super::super::inference::ClaimFreshness::Current
        {
            current_claims.push(*claim);
        }
    }
    let mut findings = Vec::new();
    for claim in current_claims.iter() {
        for declaration in assignments.iter().filter(|a| {
            a["class"] == "declaration"
                && a["proposition"]["subject_id"] == claim["proposition"]["subject_id"]
                && a["proposition"]["object"] != claim["proposition"]["object"]
        }) {
            let claim_id = text(claim, "id");
            let declaration_id = text(declaration, "id");
            findings.push(Finding{ids:vec![claim_id.clone(),declaration_id.clone()],conclusion:"present",detail:json!({"kind":"domain_intent_disagreement",
                "status":"disagreement","component":es[&text(&claim["proposition"],"subject_id")],"claim_assertion_id":claim_id,
                "declaration_assertion_id":declaration_id,"interpreted_domain":es[&text(&claim["proposition"]["object"],"entity_version_id")],
                "declared_domain":es[&text(&declaration["proposition"]["object"],"entity_version_id")],
                "authority":"Declaration remains authoritative; the Claim is advisory."})});
        }
    }
    finish(l, "drift", "combined", &[v], None, findings, o, false)
}
pub(crate) fn required_predicates(operation: &str) -> Vec<&'static str> {
    match operation {
        "policy" => vec!["IMPORTS"],
        "callgraph" | "reachability" => vec!["CALLS_STATIC"],
        "data-writes" | "lineage" => vec!["WRITES", "TRANSFORMS", "MAPS_TO"],
        "data-reads" => vec!["READS"],
        "deployment" => vec!["BUILDS", "DEPLOYS_AS"],
        "desired-connections" => vec!["DECLARES_CONNECTION"],
        _ => vec![],
    }
}
pub(crate) fn merge_bundle(views: &[View], projections: &[Value]) -> Result<Value> {
    let mut b = contracts::empty_bundle();
    // Deduplicate by id with one index per table instead of a linear scan per
    // row. First view wins on collision, exactly like the previous scan.
    let mut seen: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    // Inference packets also refer to earlier pinned projection manifests.
    // Retain those references for validation; only the response-selected
    // manifests determine current result eligibility and compatibility.
    for v in views {
        for t in contracts::TABLES {
            let ids = seen.entry(t).or_default();
            let out = b[t].as_array_mut().expect("owned bundle table");
            for r in rows(&v.bundle, t) {
                if ids.insert(text(r, "id")) {
                    out.push(r.clone());
                }
            }
        }
    }
    let ids = seen.entry("projections").or_default();
    let out = b["projections"].as_array_mut().expect("owned bundle table");
    for p in projections {
        if ids.insert(text(p, "id")) {
            out.push(p.clone());
        }
    }
    Ok(b)
}
