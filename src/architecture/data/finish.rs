use super::*;
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_without_schema(
    b: &mut Value,
    payloads: &mut contracts::Payloads,
    c: &Capture,
    ctx: &Value,
    out: &Value,
    provider: &str,
    config_id: &str,
    function: &dyn Fn(&str) -> Option<String>,
    module: &dyn Fn(&str) -> Option<String>,
    attempted: usize,
    unresolved: usize,
    unsupported: usize,
    diagnostics: usize,
    limitations: Vec<String>,
) -> Result<Produced> {
    let mut schema_coverage = coverage(attempted, unresolved, unsupported, false, &["MAPS_TO"]);
    schema_coverage["resolution_complete"] = json!(false);
    run(b, ctx, config_id, "schema", &schema_coverage)?;
    finish_operations(
        b,
        payloads,
        c,
        ctx,
        out,
        provider,
        config_id,
        function,
        module,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        schema_coverage,
        diagnostics,
        limitations,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_operations(
    b: &mut Value,
    payloads: &mut contracts::Payloads,
    c: &Capture,
    ctx: &Value,
    out: &Value,
    provider: &str,
    config_id: &str,
    function: &dyn Fn(&str) -> Option<String>,
    module: &dyn Fn(&str) -> Option<String>,
    models: &BTreeMap<String, (Model, String)>,
    fields: &BTreeMap<(String, String), (String, Option<String>)>,
    table_ids: &BTreeMap<String, String>,
    schema_coverage: Value,
    diagnostics: usize,
    mut limitations: Vec<String>,
) -> Result<Produced> {
    // ---- DTO identities ------------------------------------------------------
    let mut dto_fields: BTreeMap<String, String> = BTreeMap::new();
    let mut dto_declarations = Vec::new();
    for dto in rows(out, "dtos") {
        let path = text(dto, "path");
        let Some(owner) = module(&path) else {
            limitations.push(format!(
                "DTO {} lacks a retained structure module.",
                text(dto, "key")
            ));
            continue;
        };
        let dto_id = rust::entity_for_language(
            b,
            ctx,
            "symbol",
            &text(dto, "key"),
            provider,
            "compiler",
            "typescript",
        )?;
        let er = rust::evidence(
            b,
            &path,
            dto["start"].as_u64().unwrap() as usize,
            dto["end"].as_u64().unwrap() as usize,
        )?;
        dto_declarations.push((owner.clone(), dto_id, er));
        for f in rows(dto, "fields") {
            let fid = rust::entity_for_language(
                b,
                ctx,
                "symbol",
                &text(f, "key"),
                provider,
                "compiler",
                "typescript",
            )?;
            let fer = rust::evidence(
                b,
                &path,
                f["start"].as_u64().unwrap() as usize,
                f["end"].as_u64().unwrap() as usize,
            )?;
            dto_fields.insert(text(f, "key"), fid.clone());
            dto_declarations.push((owner.clone(), fid, fer));
        }
    }
    let span = |b: &mut Value, v: &Value| -> Result<String> {
        rust::evidence(
            b,
            &text(v, "path"),
            v["start"].as_u64().unwrap() as usize,
            v["end"].as_u64().unwrap() as usize,
        )
    };
    let model_of = |accessor: &str| -> Option<&(Model, String)> {
        models.values().find(|(m, _)| {
            let mut chars = m.name.chars();
            chars
                .next()
                .map(|first| first.to_lowercase().collect::<String>() + chars.as_str())
                .as_deref()
                == Some(accessor)
        })
    };
    let complete = c.complete && diagnostics == 0;
    // ---- Routes ----------------------------------------------------------------
    let mut route_unresolved = 0;
    let mut route_items = Vec::new();
    for r in rows(out, "routes") {
        match function(&text(r, "handler")) {
            Some(handler) => route_items.push((r.clone(), handler)),
            None => {
                route_unresolved += 1;
                limitations.push(format!(
                    "Route handler {} is not a retained structure symbol.",
                    text(r, "handler")
                ));
            }
        }
    }
    let route_unresolved_rows: Vec<_> = rows(out, "unresolved")
        .iter()
        .filter(|u| u["predicate"] == "ROUTES_TO")
        .cloned()
        .collect();
    let route_coverage = coverage(
        route_items.len() + route_unresolved + route_unresolved_rows.len(),
        route_unresolved + route_unresolved_rows.len(),
        0,
        complete,
        &["ROUTES_TO"],
    );
    let route_run = run(b, ctx, config_id, "semantic", &route_coverage)?;
    for (r, handler) in &route_items {
        let endpoint = rust::entity_for_language(
            b,
            ctx,
            "endpoint",
            &format!("http:{} {}", text(r, "method"), text(r, "route")),
            provider,
            "compiler",
            "http",
        )?;
        let er = span(b, r)?;
        rust::assertion(
            b,
            ctx,
            "evidence",
            &endpoint,
            "ROUTES_TO",
            knowledge::target(handler),
            json!({"run_id":route_run}),
            vec![er],
            Some(json!({"status":"resolved","method":"compiler","candidate_ids":[handler]})),
        )?;
    }
    // ---- Operations --------------------------------------------------------------
    let mut writes = Vec::new();
    let mut reads = Vec::new();
    let mut transforms = Vec::new();
    let mut write_unresolved: Vec<(Value, &'static str, &'static str, Vec<String>, String)> =
        Vec::new();
    let mut read_unresolved: Vec<(Value, &'static str, &'static str, Vec<String>, String)> =
        Vec::new();
    let mut write_attempted = 0;
    let mut read_attempted = 0;
    for op in rows(out, "operations") {
        let path = text(op, "path");
        let subject = function(&text(op, "function"));
        let model = model_of(&text(op, "model_accessor"));
        let is_write = op["kind"] == "write";
        let bucket = if is_write {
            &mut write_unresolved
        } else {
            &mut read_unresolved
        };
        let (Some(subject), Some((model, _))) = (subject.clone(), model) else {
            bucket.push((
                op.clone(),
                if is_write { "WRITES" } else { "READS" },
                "missing_dependency",
                vec![],
                subject.or_else(|| module(&path)).unwrap_or_default(),
            ));
            continue;
        };
        if is_write {
            for a in rows(op, "assignments") {
                write_attempted += 1;
                let field_name = text(a, "field");
                match fields.get(&(model.name.clone(), field_name.clone())) {
                    Some((prisma_field, Some(column))) => {
                        writes.push((subject.clone(), column.clone(), a.clone()));
                        if a["origin"] == "dto_field" {
                            let Some(source) = dto_fields.get(&text(a, "source_field")) else {
                                bucket.push((
                                    a.clone(),
                                    "TRANSFORMS",
                                    "missing_dependency",
                                    vec![],
                                    subject.clone(),
                                ));
                                continue;
                            };
                            transforms.push((source.clone(), prisma_field.clone(), a.clone()));
                        }
                    }
                    Some((prisma_field, None)) => bucket.push((
                        a.clone(),
                        "WRITES",
                        "schema_mismatch",
                        vec![prisma_field.clone()],
                        subject.clone(),
                    )),
                    None => bucket.push((
                        a.clone(),
                        "WRITES",
                        "schema_mismatch",
                        vec![],
                        subject.clone(),
                    )),
                }
            }
        } else {
            let table = table_ids
                .iter()
                .find(|(k, _)| k.ends_with(&format!("/{}", model.table)))
                .map(|(_, v)| v.clone());
            let selected: Vec<(String, Value)> = match op["selected"].as_array() {
                Some(selection) => selection
                    .iter()
                    .map(|s| (text(s, "field"), s.clone()))
                    .collect(),
                None => model
                    .fields
                    .iter()
                    .filter(|f| !f.relation)
                    .map(|f| (f.name.clone(), op.clone()))
                    .collect(),
            };
            if let Some(table) = table {
                reads.push((subject.clone(), table, op.clone()));
            }
            for (name, site) in selected {
                read_attempted += 1;
                match fields.get(&(model.name.clone(), name)) {
                    Some((_, Some(column))) => reads.push((subject.clone(), column.clone(), site)),
                    Some((prisma_field, None)) => bucket.push((
                        site,
                        "READS",
                        "schema_mismatch",
                        vec![prisma_field.clone()],
                        subject.clone(),
                    )),
                    None => {
                        bucket.push((site, "READS", "schema_mismatch", vec![], subject.clone()))
                    }
                }
            }
        }
    }
    for u in rows(out, "unresolved")
        .iter()
        .filter(|u| u["predicate"] != "ROUTES_TO")
    {
        let reason: &'static str = match text(u, "reason").as_str() {
            "dynamic_property" => "dynamic_property",
            "spread_object" => "spread_object",
            "raw_sql" => "raw_sql",
            "dynamic_model" => "dynamic_model",
            "conditional_mapping" => "conditional_mapping",
            "opaque_wrapper" => "opaque_wrapper",
            "unsupported_context" => "unsupported_context",
            "unsupported_operation" => "unsupported_operation",
            _ => "unsupported_transformation",
        };
        let predicate: &'static str = match text(u, "predicate").as_str() {
            "READS" => "READS",
            "TRANSFORMS" => "TRANSFORMS",
            _ => "WRITES",
        };
        let subject = u["caller"]
            .as_str()
            .and_then(function)
            .or_else(|| module(&text(u, "path")))
            .unwrap_or_default();
        let candidates = strings(&u["candidates"])
            .iter()
            .filter_map(|k| dto_fields.get(k).cloned())
            .collect();
        if predicate == "READS" {
            read_unresolved.push((u.clone(), predicate, reason, candidates, subject));
        } else {
            write_unresolved.push((u.clone(), predicate, reason, candidates, subject));
        }
    }
    let unsupported = |xs: &[(Value, &str, &str, Vec<String>, String)]| {
        xs.iter()
            .filter(|x| {
                [
                    "raw_sql",
                    "dynamic_model",
                    "unsupported_context",
                    "unsupported_operation",
                ]
                .contains(&x.2)
            })
            .count()
    };
    let write_coverage = coverage(
        write_attempted + write_unresolved.len(),
        write_unresolved.len() - unsupported(&write_unresolved),
        unsupported(&write_unresolved) + diagnostics,
        complete,
        &["WRITES", "TRANSFORMS"],
    );
    let mut write_coverage = write_coverage;
    write_coverage["guarantees"]
        .as_array_mut()
        .unwrap()
        .push(json!({"predicate":"DECLARES","kind":"syntax_only"}));
    let read_coverage = coverage(
        read_attempted + read_unresolved.len(),
        read_unresolved.len() - unsupported(&read_unresolved),
        unsupported(&read_unresolved) + diagnostics,
        complete,
        &["READS"],
    );
    let write_run = run(b, ctx, config_id, "semantic", &write_coverage)?;
    let read_run = run(b, ctx, config_id, "semantic", &read_coverage)?;
    for (owner, id, er) in dto_declarations {
        rust::assertion(
            b,
            ctx,
            "evidence",
            &owner,
            "DECLARES",
            knowledge::target(&id),
            json!({"run_id":write_run}),
            vec![er],
            None,
        )?;
    }
    let resolution = |target: &str| {
        Some(json!({"status":"resolved","method":"compiler","candidate_ids":[target]}))
    };
    for (subject, column, a) in &writes {
        let er = span(b, a)?;
        rust::assertion(
            b,
            ctx,
            "evidence",
            subject,
            "WRITES",
            knowledge::target(column),
            json!({"run_id":write_run}),
            vec![er],
            resolution(column),
        )?;
    }
    for (source, target, a) in &transforms {
        let mut refs = vec![span(b, a)?];
        if a["binding"].is_object() {
            refs.push(span(b, &a["binding"])?);
        }
        let mut t = json!({"context":ctx,"class":"evidence","proposition":{"subject_id":source,"predicate":"TRANSFORMS","object":knowledge::target(target)},
            "origin":{"run_id":write_run},"evidence_ref_ids":refs,"transformation":a["transformation"]});
        t["id"] = json!(identity::id("assertion", &t)?);
        add(b, "assertions", t);
    }
    for (subject, target, site) in &reads {
        let er = span(b, site)?;
        rust::assertion(
            b,
            ctx,
            "evidence",
            subject,
            "READS",
            knowledge::target(target),
            json!({"run_id":read_run}),
            vec![er],
            resolution(target),
        )?;
    }
    for (site, predicate, reason, candidates, subject) in
        write_unresolved.iter().chain(read_unresolved.iter())
    {
        if subject.is_empty() {
            continue;
        }
        let er = span(b, site)?;
        let status = if [
            "raw_sql",
            "dynamic_model",
            "unsupported_context",
            "unsupported_operation",
        ]
        .contains(reason)
        {
            "unsupported"
        } else if candidates.is_empty() {
            "unresolved"
        } else {
            "partial"
        };
        unresolved_record(
            b,
            ctx,
            subject,
            &er,
            predicate,
            status,
            reason,
            candidates.clone(),
        )?;
    }
    for u in route_unresolved_rows {
        let subject = u["caller"]
            .as_str()
            .and_then(function)
            .or_else(|| module(&text(&u, "path")))
            .unwrap_or_default();
        if !subject.is_empty() {
            let er = span(b, &u)?;
            unresolved_record(
                b,
                ctx,
                &subject,
                &er,
                "ROUTES_TO",
                "unresolved",
                "dynamic_property",
                vec![],
            )?;
        }
    }
    if diagnostics > 0 {
        limitations.push(format!(
            "{diagnostics} compiler diagnostics make data coverage partial."
        ));
    }
    passes::produce_lineage(b, payloads, ctx, &write_coverage)?;
    Ok(Produced {
        coverage: json!({"schema_resolution_complete":schema_coverage["resolution_complete"],
            "orm_write_resolution_complete":write_coverage["resolution_complete"],
            "orm_read_resolution_complete":read_coverage["resolution_complete"],
            "route_resolution_complete":route_coverage["resolution_complete"],
            "schema":schema_coverage,"writes":write_coverage,"reads":read_coverage,"routes":route_coverage}),
        limitations,
    })
}
