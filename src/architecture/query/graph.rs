use super::*;
/// Compatibility report shape over the common adjacency engine.
pub fn dependencies(l:&Ledger,o:&Options)->Result<Value>{
    adjacency(l,o,"dependencies",if o.include_structure {"IMPORTS,DEPENDS_ON,CONTAINS,DECLARES,REFERENCES,IN_CYCLE"} else {"DEPENDS_ON,IN_CYCLE"})
}
fn adjacency(l:&Ledger,o:&Options,operation:&str,predicates:&str)->Result<Value>{
    let v=loaded(l,o)?;
    require(!o.historical,"ARGUMENTS","Use pinned reports and explain to inspect historical adjacency")?;
    let keys=entities(&v);
    let mut subject=None;
    if let Some(key)=&o.component {
        if operation=="callgraph" {
            let candidates=execution_subject(&v,key);
            if candidates.len()!=1 {return finish(l,operation,"evidence",&[v],None,vec![execution_unknown(key,&candidates)],o,false);}
            subject=candidates.first().cloned();
        } else {
            if let Some(f)=ambiguity(&v,key) {return finish(l,operation,"evidence",&[v],None,vec![f],o,false);}
            subject=selected(&v,key).first().cloned();
        }
    }
    let target=subject.as_ref().and_then(|id|keys.get(id)).map(String::as_str).unwrap_or(".");
    let report=crate::architecture::task::neighbors(l,&json!({"target":target,
        "predicate":predicates,"direction":"out","view":v.id,"limit":1000}))?;
    let bounded=report["coverage"]["omitted_relationship_occurrences"].as_u64().unwrap_or(0)>0;
    let mut findings=Vec::new();
    let mut page=report;
    loop {
        for row in rows(&page,"items") {
            let ids=strings(&row["assertion_ids"]);
            // Compatibility reports explain against this one source view.
            if ids.is_empty() || ids.iter().any(|id|!v.eligible.contains(id)) {continue;}
            let predicate=text(row,"predicate");
            if subject.is_some() && row["from"]!=target &&
                !(o.include_structure && ["DECLARES","CONTAINS","IMPORTS","REFERENCES"].contains(&predicate.as_str()) &&
                  (row["to"]==target || (predicate=="DECLARES" && text(row,"to").starts_with(&format!("{target}::"))))) {continue;}
            let mut detail=row.clone();
            detail["support_class"]=json!("evidence_only");
            if predicate=="IN_CYCLE" {detail["component"]=row["from"].clone();detail["members"]=row["value"].clone();}
            if let Some(assertion)=rows(&v.bundle,"assertions").iter().find(|a|a["id"]==ids[0]) {
                detail["resolution"]=assertion["resolution"].clone();
                detail["source_evidence_ref_ids"]=assertion["evidence_ref_ids"].clone();
            }
            findings.push(Finding{ids,conclusion:"present",detail});
        }
        let Some(offset)=page["page"]["next_offset"].as_u64() else {break};
        page=crate::architecture::task::neighbors(l,&json!({"target":target,"predicate":predicates,
            "direction":"out","view":v.id,"index_id":page["index_id"],"offset":offset,"limit":1000}))?;
    }
    if operation=="callgraph" {
        for u in rows(&v.bundle,"unresolved_references").iter().filter(|u|u["predicate"]=="CALLS_STATIC") {
            if subject.as_ref().is_none_or(|s|u["source_entity_id"]==*s) {
                findings.push(Finding{ids:vec![],conclusion:"unknown",detail:json!({"predicate":"CALLS_STATIC",
                    "occurrence_id":u["id"],"status":u["status"],"reason":u["reason"],"evidence_ref_id":u["evidence_ref_id"]})});
            }
        }
    }
    let complete=(if operation=="callgraph" {complete_for(&v,"CALLS_STATIC")}else{complete(&v)}) && !v.stale && !bounded;
    finish(l,operation,"evidence",&[v],None,findings,o,complete)
}
pub(crate) fn execution_subject(v: &View, key: &str) -> Vec<String> {
    rows(&v.bundle, "entity_versions")
        .iter()
        .filter(|e| {
            e["context"] == context(v)
                && e["kind"] == "symbol"
                && ["compiler", "exact_mapping"].contains(&text(e, "identity_quality").as_str())
                && (e["id"] == key
                    || e["canonical_key"] == key
                    || e["canonical_key"].as_str().is_some_and(|k| {
                        k.split('#').next() == Some(key)
                            || k.split('#').next().and_then(|k| k.rsplit("::").next()) == Some(key)
                    }))
        })
        .map(|e| text(e, "id"))
        .collect()
}
pub(crate) fn execution_unknown(key: &str, candidates: &[String]) -> Finding {
    Finding {
        ids: vec![],
        conclusion: "unknown",
        detail: json!({"component":key,
        "resolution":if candidates.is_empty() {"unresolved"} else {"ambiguous"},
        "candidate_entity_version_ids":candidates,"limitation":"A unique retained native function identity is required."}),
    }
}
pub fn callgraph(l:&Ledger,o:&Options)->Result<Value>{adjacency(l,o,"callgraph","CALLS_STATIC")}
pub fn reachability(l: &Ledger, o: &Options, from: &str, to: &str) -> Result<Value> {
    let v = loaded(l, o)?;
    let removed = l.retracted()?;
    let a = execution_subject(&v, from);
    let b = execution_subject(&v, to);
    if a.len() != 1 || b.len() != 1 {
        let findings = [(from, &a), (to, &b)]
            .into_iter()
            .filter(|(_, ids)| ids.len() != 1)
            .map(|(key, ids)| execution_unknown(key, ids))
            .collect();
        return finish(
            l,
            "reachability",
            "evidence",
            &[v],
            None,
            findings,
            o,
            false,
        );
    }
    let mut findings = Vec::new();
    for assertion in eligible(&v, &removed, o.historical, "REACHES_STATIC") {
        if assertion["proposition"]["subject_id"] == a[0]
            && assertion["proposition"]["object"]["entity_version_id"] == b[0]
        {
            findings.push(Finding {ids:vec![text(assertion,"id")],conclusion:"present",detail:json!({"from":from,"to":to,
                "predicate":"REACHES_STATIC","class":"derived","support_class":"evidence_only","witness_assertion_ids":assertion["supports"]})});
        }
    }
    let bounded = rows(&v.bundle, "pass_manifests").iter().any(|m| {
        m["context"] == context(&v)
            && m["pass_name"] == "static-callgraph-overlay-v1"
            && m["status"] == "complete"
    });
    let complete = bounded && complete_for(&v, "CALLS_STATIC") && !v.stale;
    if findings.is_empty() {
        findings.push(Finding {ids:vec![],conclusion:if complete {"absent"} else {"unknown"},detail:json!({"from":from,"to":to,
            "predicate":"REACHES_STATIC","status":if complete {"no_supported_static_path"} else {"incomplete"},
            "limitation":"No path in the evaluated retained direct-call scope; this is not an observed runtime guarantee."})});
    }
    finish(
        l,
        "reachability",
        "evidence",
        &[v],
        None,
        findings,
        o,
        complete,
    )
}

pub fn unresolved(l: &Ledger, o: &Options) -> Result<Value> {
    let v = loaded(l, o)?;
    let es = entities(&v);
    let mut findings = Vec::new();
    if let Some(key) = &o.component {
        if let Some(f) = ambiguity(&v, key) {
            return finish(l, "unresolved", "evidence", &[v], None, vec![f], o, false);
        }
    }
    let subject = o.component.as_ref().map(|k| selected(&v, k)[0].clone());
    for u in rows(&v.bundle, "unresolved_references") {
        let sid = text(u, "source_entity_id");
        if subject.as_ref().is_none_or(|s| s == &sid) {
            findings.push(Finding{ids:vec![],conclusion:"unknown",detail:json!({"occurrence_id":u["id"],"predicate":u["predicate"],"from":es[&sid],"status":u["status"],"reason":u["reason"],"candidate_ids":u["candidate_ids"],"evidence_ref_id":u["evidence_ref_id"]})});
        }
    }
    finish(l, "unresolved", "evidence", &[v], None, findings, o, false)
}
pub fn compare(l: &Ledger, base: &str, o: &Options) -> Result<Value> {
    let mut bo = o.clone();
    bo.view = base.into();
    let mut bv = loaded(l, &bo)?;
    let mut av = loaded(l, o)?;
    require(
        context(&bv)["snapshot_id"] != context(&av)["snapshot_id"],
        "COMPARE",
        "Comparison requires two distinct captured snapshots",
    )?;
    let correspondence = super::super::history::load(l, &bv, &av)?;
    bv.overlay_watermark = correspondence.watermark;
    av.overlay_watermark = correspondence.watermark;
    let mut remap = BTreeMap::new();
    let before_keys = entities(&bv);
    let after_keys = entities(&av);
    let mut lineage_findings = Vec::new();
    let removed = l.retracted()?;
    for a in &correspondence.exact {
        let id = text(a, "id");
        if !o.historical && removed.contains(&id) {
            continue;
        }
        let from = text(&a["proposition"]["object"], "entity_version_id");
        let to = text(&a["proposition"], "subject_id");
        remap.insert(after_keys[&to].clone(), before_keys[&from].clone());
        lineage_findings.push(Finding{ids:vec![id.clone()],conclusion:"present",detail:json!({"change":"renamed",
            "from":before_keys[&from],"to":after_keys[&to],"predicate":"RENAMED_FROM","assertion_id":id,
            "from_entity_version_id":from,"to_entity_version_id":to,"method":"unique-retained-module-content-v1",
            "limitation":"Exact retained-content movement between distinct versions; not eternal identity."})});
        bv.eligible.insert(id.clone());
        av.eligible.insert(id);
    }
    for detail in super::super::history::ambiguous_details(l, &correspondence)? {
        let mut detail = detail;
        detail["change"] = if detail["status"] == "ambiguous" {
            json!("ambiguous_correspondence")
        } else {
            detail["status"].clone()
        };
        let ids = strings(&detail["input_assertion_ids"]);
        if !o.historical && ids.iter().any(|id| removed.contains(id)) {
            continue;
        }
        lineage_findings.push(Finding {
            ids,
            conclusion: "unknown",
            detail,
        });
    }
    super::super::knowledge::merge(&mut bv.bundle, &correspondence.bundle)?;
    super::super::knowledge::merge(&mut av.bundle, &correspondence.bundle)?;
    pub(crate) fn keyed(
        v: &View,
        removed: &BTreeSet<String>,
        historical: bool,
        remap: &BTreeMap<String, String>,
    ) -> BTreeMap<(String, String), Vec<String>> {
        let es = entities(v);
        let mut out: BTreeMap<_, Vec<String>> = BTreeMap::new();
        for a in eligible(v, removed, historical, "DEPENDS_ON") {
            let p = &a["proposition"];
            out.entry((
                remap
                    .get(&es[&text(p, "subject_id")])
                    .unwrap_or(&es[&text(p, "subject_id")])
                    .clone(),
                remap
                    .get(&es[&text(&p["object"], "entity_version_id")])
                    .unwrap_or(&es[&text(&p["object"], "entity_version_id")])
                    .clone(),
            ))
            .or_default()
            .push(text(a, "id"));
        }
        out
    }
    let before = keyed(&bv, &removed, o.historical, &BTreeMap::new());
    let after = keyed(&av, &removed, o.historical, &remap);
    let mut findings = lineage_findings;
    for (pair, ids) in &after {
        if !before.contains_key(pair) {
            findings.push(Finding{ids:ids.clone(),conclusion:if complete(&bv)&&!bv.stale{"present"}else{"unknown"},detail:json!({"change":if complete(&bv)&&!bv.stale{"added"}else{"possibly_added"},"from":pair.0,"to":pair.1,"correspondence":"equal canonical source/module keys; rename/split/merge is not established"})});
        }
    }
    for (pair, ids) in &before {
        if !after.contains_key(pair) {
            findings.push(Finding{ids:ids.clone(),conclusion:if complete(&av)&&!av.stale{"present"}else{"unknown"},detail:json!({"change":if complete(&av)&&!av.stale{"removed"}else{"possibly_removed"},"from":pair.0,"to":pair.1,"correspondence":"equal canonical source/module keys; rename/split/merge is not established"})});
        }
    }
    let c = complete(&bv) && complete(&av) && !bv.stale && !av.stale;
    let ctx = json!({"kind":"comparison","snapshot_ids":[context(&bv)["snapshot_id"],context(&av)["snapshot_id"]]});
    finish(
        l,
        "compare",
        "evidence",
        &[bv, av],
        Some(ctx),
        findings,
        o,
        c,
    )
}
pub fn prepare_compare(l: &mut Ledger, base: &str, o: &Options) -> Result<()> {
    let mut bo = o.clone();
    bo.view = base.into();
    let before = loaded(l, &bo)?;
    let after = loaded(l, o)?;
    super::super::history::prepare(l, &before, &after)
}
