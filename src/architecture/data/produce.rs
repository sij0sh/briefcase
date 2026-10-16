use super::*;
/// Produce data Evidence, unresolved boundaries and the lineage pass into an
/// existing source bundle. Only entities already retained by the structure
/// producer can be data subjects; nothing is matched by display name.
pub(crate) fn produce(
    b: &mut Value,
    payloads: &mut contracts::Payloads,
    c: &Capture,
    ctx: &Value,
    options: &typescript::ScanOptions,
) -> Result<Produced> {
    let mut limitations = vec!["Data lineage covers Fastify route registration, Zod object DTOs, Prisma client create/update/findUnique/findFirst/findMany with static models and object-literal arguments, explicit Prisma schema correspondence and retained migration SQL. Raw SQL, dynamic models, spreads, computed keys, conditional values, helper functions and external wrappers are explicit boundaries.".to_owned()];
    let out = typescript::node_adapter(c, options, "data.cjs", include_bytes!("../data.cjs"))?;
    let implementation = identity::digest(
        &json!({"rust":identity::bytes_digest(&[include_bytes!("../data.rs").as_slice(), include_bytes!("schema_parse.rs").as_slice(), include_bytes!("produce.rs").as_slice(), include_bytes!("finish.rs").as_slice(), include_bytes!("analyze.rs").as_slice()].concat()),
        "adapter":identity::bytes_digest(include_bytes!("../data.cjs")),
        "launcher":identity::bytes_digest(include_bytes!("../typescript.rs")),
        "host":identity::bytes_digest(include_bytes!("../native_host.rs"))}),
    )?;
    let provider = identity::id(
        "identity-provider",
        &json!({"mapping":"retained-data-boundary-v1","implementation_digest":implementation,"producer":out["provider"]}),
    )?;
    let mut config = json!({"producer_name":"briefcase-data-lineage-v1","implementation_digest":implementation,
        "settings_digest":identity::digest(&out["provider"])?});
    let config_id = identity::id("analysis", &config)?;
    config["id"] = json!(config_id);
    add(b, "analysis_configs", config);
    knowledge::artifact(
        b,
        payloads,
        "analyzers",
        serde_json::to_vec(&json!({"producer":"briefcase-data-lineage-v1","adapter_output":out}))?,
    )?;
    let diagnostics = rows(&out, "diagnostics").len();
    let structural: BTreeMap<String, String> = rows(b, "entity_versions")
        .iter()
        .filter(|e| {
            e["context"] == *ctx && ["symbol", "module"].contains(&text(e, "kind").as_str())
        })
        .filter(|e| e["language"] == "typescript" || e["language"] == "javascript")
        .map(|e| {
            (
                format!("{}:{}", text(e, "kind"), text(e, "canonical_key")),
                text(e, "id"),
            )
        })
        .collect();
    let function = |key: &str| structural.get(&format!("symbol:{key}")).cloned();
    let module = |path: &str| structural.get(&format!("module:{path}")).cloned();

    // ---- Schema resources and explicit ORM correspondence --------------------
    let schemas: Vec<_> = c
        .files
        .keys()
        .filter(|p| p.ends_with("schema.prisma"))
        .cloned()
        .collect();
    let mut schema_unresolved = 0;
    let mut schema_attempted = 0;
    let mut schema_unsupported = 0;
    let mut models: BTreeMap<String, (Model, String)> = BTreeMap::new();
    let mut fields: BTreeMap<(String, String), (String, Option<String>)> = BTreeMap::new();
    let mut table_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut schema_records: Vec<Box<dyn FnOnce(&mut Value, &str) -> Result<()>>> = Vec::new();
    if schemas.len() > 1 {
        limitations.push(format!(
            "Multiple Prisma schemas are outside this producer scope: {schemas:?}"
        ));
        schema_unsupported += schemas.len();
    }
    let selected_schema = (schemas.len() == 1).then(|| schemas[0].clone());
    if let Some(schema_path) = &selected_schema {
        let source = std::str::from_utf8(&c.files[schema_path])?;
        let parsed = prisma(source)?;
        let dir = schema_path.trim_end_matches("schema.prisma");
        let mut migration_files: Vec<_> = c
            .files
            .iter()
            .filter(|(p, _)| p.starts_with(&format!("{dir}migrations/")) && p.ends_with(".sql"))
            .map(|(p, bytes)| Ok((p.clone(), std::str::from_utf8(bytes)?)))
            .collect::<Result<_>>()?;
        migration_files.sort();
        let migrated = migrations(&migration_files)?;
        let config_path = format!("{dir}database.json");
        let database = c
            .files
            .get(&config_path)
            .map(|bytes| {
                identity::parse(bytes)
                    .map(|v| (v, std::str::from_utf8(bytes).unwrap_or("").to_owned()))
            })
            .transpose()?;
        let file = rust::entity_for_language(
            b,
            ctx,
            "file",
            &format!("file::{schema_path}"),
            &provider,
            "syntax",
            "prisma",
        )?;
        // Retained schema witness: type, nullability and key semantics per field.
        knowledge::artifact(
            b,
            payloads,
            "analyzers",
            serde_json::to_vec(&json!({"producer":"briefcase-data-lineage-v1",
            "schema_path":schema_path,"schema_digest":identity::bytes_digest(source.as_bytes()),
            "migrations":migration_files.iter().map(|(p, s)| json!({"path":p,"digest":identity::bytes_digest(s.as_bytes())})).collect::<Vec<_>>(),
            "models":parsed.models.iter().map(|m| json!({"name":m.name,"table":m.table,"schema":m.schema,
                "fields":m.fields.iter().map(|f| json!({"name":f.name,"column":f.column,"type":f.ty,"nullable":f.optional,
                    "list":f.list,"primary_key":f.id,"unique":f.unique,"relation":f.relation})).collect::<Vec<_>>()})).collect::<Vec<_>>()}))?,
        )?;
        schema_attempted +=
            parsed.models.len() + parsed.models.iter().map(|m| m.fields.len()).sum::<usize>();
        let issue_evidence: Vec<_> = parsed
            .issues
            .iter()
            .map(|(s, e, m)| {
                Ok((
                    rust::evidence(b, schema_path, *s, (*e).max(s + 1))?,
                    m.clone(),
                ))
            })
            .collect::<Result<_>>()?;
        for (er, message) in issue_evidence {
            schema_unresolved += 1;
            limitations.push(format!("Prisma schema boundary: {message}"));
            unresolved_record(
                b,
                ctx,
                &file,
                &er,
                "MAPS_TO",
                "unsupported",
                "schema_mismatch",
                vec![],
            )?;
        }
        let Some((database, raw)) = database.filter(|(d, _)| {
            d["schema_version"] == "1.0"
                && d["logical_database"].is_string()
                && d["default_schema"].is_string()
        }) else {
            limitations.push(format!("No retained logical database configuration at {config_path}; table/column identity is unresolved, never derived from a hostname."));
            schema_unresolved += parsed.models.len();
            let er = rust::evidence(b, schema_path, 0, source.len().max(1))?;
            unresolved_record(
                b,
                ctx,
                &file,
                &er,
                "MAPS_TO",
                "unresolved",
                "missing_dependency",
                vec![],
            )?;
            return finish_without_schema(
                b,
                payloads,
                c,
                ctx,
                &out,
                &provider,
                &config_id,
                &function,
                &module,
                schema_attempted,
                schema_unresolved,
                schema_unsupported,
                diagnostics,
                limitations,
            );
        };
        let db_name = text(&database, "logical_database");
        let db_span = json_value_span(&raw, "logical_database").unwrap_or((0, raw.len()));
        let db_er = rust::evidence(b, &config_path, db_span.0, db_span.1)?;
        let db_id = rust::entity_for_language(
            b,
            ctx,
            "database",
            &database_key(&db_name),
            &provider,
            "declared",
            "postgresql",
        )?;
        let default_schema = text(&database, "default_schema");
        let schema_span = json_value_span(&raw, "default_schema").unwrap_or(db_span);
        let schema_er = rust::evidence(b, &config_path, schema_span.0, schema_span.1)?;
        let mut schema_entities: BTreeMap<String, String> = BTreeMap::new();
        for model in &parsed.models {
            let schema_name = model
                .schema
                .clone()
                .unwrap_or_else(|| default_schema.clone());
            let schema_id = rust::entity_for_language(
                b,
                ctx,
                "schema",
                &schema_key(&db_name, &schema_name),
                &provider,
                "declared",
                "postgresql",
            )?;
            schema_entities.insert(schema_name.clone(), schema_id);
            let model_key = format!("{schema_path}::{}", model.name);
            let model_id = rust::entity_for_language(
                b,
                ctx,
                "symbol",
                &model_key,
                &provider,
                "exact_mapping",
                "prisma",
            )?;
            let model_er = rust::evidence(b, schema_path, model.start, model.end)?;
            let table_key = table_key(&db_name, &schema_name, &model.table);
            let table_id = rust::entity_for_language(
                b,
                ctx,
                "table",
                &table_key,
                &provider,
                "exact_mapping",
                "postgresql",
            )?;
            table_ids.insert(table_key.clone(), table_id.clone());
            let table_er = rust::evidence(b, schema_path, model.table_span.0, model.table_span.1)?;
            let migrated_table = migrated.tables.get(&model.table);
            let table_supported = !migrated.present
                || (migrated_table.is_some()
                    && !migrated.unsupported_tables.contains(&model.table));
            let model_record = (model.clone(), model_id.clone());
            models.insert(model.name.clone(), model_record);
            let (schema_id, model_er2, table_er2, file2, db_id2, db_er2, schema_er2) = (
                schema_entities[&schema_name].clone(),
                model_er.clone(),
                table_er.clone(),
                file.clone(),
                db_id.clone(),
                db_er.clone(),
                schema_er.clone(),
            );
            let model_id2 = model_id.clone();
            let table_id2 = table_id.clone();
            let ctx2 = ctx.clone();
            schema_records.push(Box::new(move |b, run| {
                let origin = json!({"run_id":run});
                rust::assertion(
                    b,
                    &ctx2,
                    "evidence",
                    &file2,
                    "DECLARES",
                    knowledge::target(&model_id2),
                    origin.clone(),
                    vec![model_er2.clone()],
                    None,
                )?;
                rust::assertion(
                    b,
                    &ctx2,
                    "evidence",
                    &db_id2,
                    "CONTAINS",
                    knowledge::target(&schema_id),
                    origin.clone(),
                    vec![db_er2, schema_er2],
                    None,
                )?;
                rust::assertion(
                    b,
                    &ctx2,
                    "evidence",
                    &schema_id,
                    "CONTAINS",
                    knowledge::target(&table_id2),
                    origin.clone(),
                    vec![table_er2.clone()],
                    None,
                )?;
                if table_supported {
                    rust::assertion(
                        b,
                        &ctx2,
                        "evidence",
                        &model_id2,
                        "MAPS_TO",
                        knowledge::target(&table_id2),
                        origin,
                        vec![table_er2, model_er2],
                        None,
                    )?;
                }
                Ok(())
            }));
            if !table_supported {
                schema_unresolved += 1;
                unresolved_record(
                    b,
                    ctx,
                    &model_id,
                    &table_er,
                    "MAPS_TO",
                    "unresolved",
                    "schema_mismatch",
                    vec![table_id.clone()],
                )?;
                limitations.push(format!(
                    "Prisma model {} maps to table {} absent from retained migrations.",
                    model.name, model.table
                ));
            }
            for f in model.fields.iter().filter(|f| !f.relation) {
                let field_key = format!("{model_key}.{}", f.name);
                let field_id = rust::entity_for_language(
                    b,
                    ctx,
                    "symbol",
                    &field_key,
                    &provider,
                    "exact_mapping",
                    "prisma",
                )?;
                let field_er = rust::evidence(b, schema_path, f.start, f.end)?;
                let column_key = column_key(&db_name, &schema_name, &model.table, &f.column);
                let column_id = rust::entity_for_language(
                    b,
                    ctx,
                    "column",
                    &column_key,
                    &provider,
                    "exact_mapping",
                    "postgresql",
                )?;
                let migrated_column = migrated_table.and_then(|t| t.get(&f.column)).cloned();
                let column_supported =
                    table_supported && (!migrated.present || migrated_column.is_some());
                let migration_er = migrated_column
                    .map(|(path, s, e)| rust::evidence(b, &path, s, e))
                    .transpose()?;
                fields.insert(
                    (model.name.clone(), f.name.clone()),
                    (
                        field_id.clone(),
                        column_supported.then(|| column_id.clone()),
                    ),
                );
                if !column_supported {
                    schema_unresolved += 1;
                    unresolved_record(
                        b,
                        ctx,
                        &field_id,
                        &field_er,
                        "MAPS_TO",
                        "unresolved",
                        "schema_mismatch",
                        vec![column_id.clone()],
                    )?;
                    limitations.push(format!(
                        "Prisma field {}.{} maps to column {} absent from retained migrations.",
                        model.name, f.name, f.column
                    ));
                }
                let (ctx2, file2, model_id2, table_id2) = (
                    ctx.clone(),
                    file.clone(),
                    model_id.clone(),
                    table_id.clone(),
                );
                schema_records.push(Box::new(move |b, run| {
                    let origin = json!({"run_id":run});
                    rust::assertion(
                        b,
                        &ctx2,
                        "evidence",
                        &file2,
                        "DECLARES",
                        knowledge::target(&field_id),
                        origin.clone(),
                        vec![field_er.clone()],
                        None,
                    )?;
                    let _ = model_id2;
                    let mut containment = vec![field_er.clone()];
                    containment.extend(migration_er.clone());
                    rust::assertion(
                        b,
                        &ctx2,
                        "evidence",
                        &table_id2,
                        "CONTAINS",
                        knowledge::target(&column_id),
                        origin.clone(),
                        containment.clone(),
                        None,
                    )?;
                    if column_supported {
                        rust::assertion(
                            b,
                            &ctx2,
                            "evidence",
                            &field_id,
                            "MAPS_TO",
                            knowledge::target(&column_id),
                            origin,
                            containment,
                            None,
                        )?;
                    }
                    Ok(())
                }));
            }
        }
        if !migrated.present {
            limitations.push("No retained migration SQL; Prisma schema correspondence alone establishes table/column identity.".into());
        }
        let _ = db_id;
    } else if schemas.is_empty() {
        limitations.push(
            "No retained Prisma schema; ORM operations cannot be mapped to tables or columns."
                .into(),
        );
    }
    let schema_coverage = coverage(
        schema_attempted,
        schema_unresolved,
        schema_unsupported,
        c.complete && selected_schema.is_some(),
        &["MAPS_TO"],
    );
    let mut schema_coverage = schema_coverage;
    for p in ["CONTAINS", "DECLARES"] {
        schema_coverage["guarantees"]
            .as_array_mut()
            .unwrap()
            .push(json!({"predicate":p,"kind":"syntax_only"}));
    }
    let schema_run = run(b, ctx, &config_id, "schema", &schema_coverage)?;
    for record in schema_records {
        record(b, &schema_run)?;
    }
    finish_operations(
        b,
        payloads,
        c,
        ctx,
        &out,
        &provider,
        &config_id,
        &function,
        &module,
        &models,
        &fields,
        &table_ids,
        schema_coverage,
        diagnostics,
        limitations,
    )
}
