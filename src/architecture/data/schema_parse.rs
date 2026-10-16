use super::*;
pub(crate) fn line_spans(source: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        out.push((start, start + body.len(), body));
        start += line.len();
    }
    out
}
/// Bounded Prisma schema reader for models, scalar fields, enums and the
/// explicit `@map`/`@@map`/`@@schema` correspondences. Other forms are issues.
pub(crate) fn prisma(source: &str) -> Result<Schema> {
    let block = Regex::new(r"^\s*(model|enum|datasource|generator|type|view)\s+(\w+)\s*\{\s*$")?;
    let field = Regex::new(r"^\s*(\w+)\s+(\w+)(\[\])?(\?)?(.*)$")?;
    let map = Regex::new(r#"@map\(\s*"([^"]+)"\s*\)"#)?;
    let table_map = Regex::new(r#"^\s*@@map\(\s*"([^"]+)"\s*\)\s*$"#)?;
    let schema_map = Regex::new(r#"^\s*@@schema\(\s*"([^"]+)"\s*\)\s*$"#)?;
    let mut schema = Schema::default();
    let lines = line_spans(source);
    let mut i = 0;
    while i < lines.len() {
        let (start, end, line) = lines[i];
        let code = line.split("//").next().unwrap_or("").trim_end();
        if code.trim().is_empty() {
            i += 1;
            continue;
        }
        let Some(c) = block.captures(code) else {
            schema
                .issues
                .push((start, end, "unsupported top-level schema form".into()));
            i += 1;
            continue;
        };
        let kind = c[1].to_owned();
        let name = c[2].to_owned();
        let mut model = Model {
            name: name.clone(),
            table: name.clone(),
            schema: None,
            start,
            end,
            table_span: (start, end),
            fields: vec![],
        };
        i += 1;
        while i < lines.len() && lines[i].2.trim() != "}" {
            let (ls, le, raw) = lines[i];
            let code = raw.split("//").next().unwrap_or("").trim_end();
            i += 1;
            if code.trim().is_empty() || kind != "model" {
                if kind == "enum" && !code.trim().is_empty() {
                    schema.enums.insert(name.clone());
                }
                continue;
            }
            if let Some(m) = table_map.captures(code) {
                model.table = m[1].to_owned();
                model.table_span = (ls, le);
                continue;
            }
            if let Some(m) = schema_map.captures(code) {
                model.schema = Some(m[1].to_owned());
                continue;
            }
            if code.trim_start().starts_with("@@") {
                continue;
            }
            let Some(f) = field.captures(code) else {
                schema
                    .issues
                    .push((ls, le, format!("unsupported field form in {name}")));
                continue;
            };
            let rest = f.get(5).map_or("", |m| m.as_str());
            model.fields.push(Field {
                name: f[1].to_owned(),
                column: map
                    .captures(rest)
                    .map(|m| m[1].to_owned())
                    .unwrap_or_else(|| f[1].to_owned()),
                ty: f[2].to_owned(),
                list: f.get(3).is_some(),
                optional: f.get(4).is_some(),
                id: rest.contains("@id"),
                unique: rest.contains("@unique"),
                relation: false,
                start: ls,
                end: le,
            });
        }
        if i < lines.len() {
            model.end = lines[i].1;
        }
        i += 1;
        if kind == "enum" {
            schema.enums.insert(name);
        } else if kind == "model" {
            schema.models.push(model);
        } else if kind == "type" || kind == "view" {
            schema
                .issues
                .push((start, end, format!("unsupported Prisma {kind} block")));
        }
    }
    let models: BTreeSet<_> = schema.models.iter().map(|m| m.name.clone()).collect();
    let enums = schema.enums.clone();
    for model in &mut schema.models {
        for f in &mut model.fields {
            if models.contains(&f.ty) {
                f.relation = true;
            } else if !SCALARS.contains(&f.ty.as_str()) && !enums.contains(&f.ty) {
                schema.issues.push((
                    f.start,
                    f.end,
                    format!("unknown field type {} on {}", f.ty, model.name),
                ));
                f.relation = true;
            }
        }
    }
    Ok(schema)
}
/// Columns established by retained migration SQL, with the span of each name.
#[derive(Debug, Default)]
pub(crate) struct Migrations {
    pub tables: BTreeMap<String, BTreeMap<String, (String, usize, usize)>>,
    pub unsupported_tables: BTreeSet<String>,
    pub present: bool,
}
pub(crate) fn statements(sql: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (i, ch) in sql.char_indices() {
        match (quote, ch) {
            (None, '\'' | '"') => quote = Some(ch),
            (Some(q), c) if c == q => quote = None,
            (None, ';') => {
                out.push((start, &sql[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    if !sql[start..].trim().is_empty() {
        out.push((start, &sql[start..]));
    }
    out
}
pub(crate) fn migrations(files: &[(String, &str)]) -> Result<Migrations> {
    let ident = r#""?(\w+)"?"#;
    let qualified = format!(r#"(?:{ident}\.)?{ident}"#);
    let create = Regex::new(&format!(
        r"(?is)^\s*CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?{qualified}\s*\((.*)\)\s*$"
    ))?;
    let alter = Regex::new(&format!(
        r"(?is)^\s*ALTER\s+TABLE\s+(?:ONLY\s+)?{qualified}\s+(.*)$"
    ))?;
    let add = Regex::new(&format!(r"(?is)^ADD\s+(?:COLUMN\s+)?{ident}\s+"))?;
    let drop = Regex::new(&format!(r"(?is)^DROP\s+(?:COLUMN\s+)?{ident}\s*$"))?;
    let rename = Regex::new(&format!(
        r"(?is)^RENAME\s+COLUMN\s+{ident}\s+TO\s+{ident}\s*$"
    ))?;
    let drop_table = Regex::new(&format!(
        r"(?is)^\s*DROP\s+TABLE\s+(?:IF\s+EXISTS\s+)?{qualified}\s*$"
    ))?;
    let column = Regex::new(r#"^\s*"?(\w+)"?\s+\w"#)?;
    let mut m = Migrations::default();
    for (path, sql) in files {
        m.present = true;
        for (offset, statement) in statements(sql) {
            let lead = statement.len() - statement.trim_start().len();
            let base = offset + lead;
            let statement = statement.trim();
            if let Some(c) = create.captures(statement) {
                let table = c[2].to_owned();
                let body = c.get(3).unwrap();
                let mut columns = BTreeMap::new();
                let mut depth = 0;
                let mut item_start = body.start();
                let bytes = statement.as_bytes();
                for i in body.start()..=body.end() {
                    let ch = if i < body.end() { bytes[i] } else { b',' };
                    match ch {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        b',' if depth == 0 => {
                            let item = &statement[item_start..i];
                            let upper = item.trim_start().to_ascii_uppercase();
                            if !["CONSTRAINT", "PRIMARY", "UNIQUE", "FOREIGN", "CHECK"]
                                .iter()
                                .any(|k| upper.starts_with(k))
                            {
                                if let Some(cm) = column.captures(item) {
                                    let name = cm.get(1).unwrap();
                                    columns.insert(
                                        name.as_str().to_owned(),
                                        (
                                            path.clone(),
                                            base + item_start + name.start(),
                                            base + item_start + name.end(),
                                        ),
                                    );
                                }
                            }
                            item_start = i + 1;
                        }
                        _ => {}
                    }
                }
                m.tables.insert(table, columns);
            } else if let Some(c) = alter.captures(statement) {
                let table = c[2].to_owned();
                let action = c.get(3).unwrap();
                let text = action.as_str().trim();
                let upper = text.to_ascii_uppercase();
                if upper.starts_with("ADD CONSTRAINT") || upper.starts_with("ADD PRIMARY") {
                    continue;
                }
                let columns = m.tables.entry(table.clone()).or_default();
                if let Some(a) = add.captures(text) {
                    let name = a.get(1).unwrap();
                    let at = base
                        + action.start()
                        + (action.as_str().len() - action.as_str().trim_start().len());
                    columns.insert(
                        name.as_str().to_owned(),
                        (path.clone(), at + name.start(), at + name.end()),
                    );
                } else if let Some(d) = drop.captures(text) {
                    columns.remove(&d[1]);
                } else if let Some(r) = rename.captures(text) {
                    let at = base
                        + action.start()
                        + (action.as_str().len() - action.as_str().trim_start().len());
                    let to = r.get(2).unwrap();
                    columns.remove(&r[1]);
                    columns.insert(
                        to.as_str().to_owned(),
                        (path.clone(), at + to.start(), at + to.end()),
                    );
                } else {
                    m.unsupported_tables.insert(table);
                }
            } else if let Some(c) = drop_table.captures(statement) {
                m.tables.remove(&c[2]);
            }
        }
    }
    Ok(m)
}

pub(crate) fn database_key(database: &str) -> String {
    format!("postgresql://{database}")
}
pub(crate) fn schema_key(database: &str, schema: &str) -> String {
    format!("postgresql://{database}/{schema}")
}
pub(crate) fn table_key(database: &str, schema: &str, table: &str) -> String {
    format!("postgresql://{database}/{schema}/{table}")
}
pub(crate) fn column_key(database: &str, schema: &str, table: &str, column: &str) -> String {
    format!("postgresql://{database}/{schema}/{table}/{column}")
}
/// Byte span of a JSON string value for `key` in retained configuration.
pub(crate) fn json_value_span(source: &str, key: &str) -> Option<(usize, usize)> {
    let re = Regex::new(&format!(r#""{}"\s*:\s*"([^"]*)""#, regex::escape(key))).ok()?;
    let m = re.captures(source)?.get(1)?;
    Some((m.start(), m.end().max(m.start() + 1)))
}
pub(crate) fn coverage(
    attempted: usize,
    unresolved: usize,
    unsupported: usize,
    complete: bool,
    predicates: &[&str],
) -> Value {
    json!({"attempted":attempted,"completed":attempted,"unsupported_count":unsupported,"unresolved_count":unresolved,
        "resolution_complete":complete && unresolved == 0 && unsupported == 0,
        "guarantees":predicates.iter().map(|p| json!({"predicate":p,"kind":"exact_supported"})).collect::<Vec<_>>()})
}
pub(crate) fn run(
    b: &mut Value,
    ctx: &Value,
    config: &str,
    kind: &str,
    coverage: &Value,
) -> Result<String> {
    let id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config,"producer_kind":kind,"coverage":coverage}),
    )?;
    add(
        b,
        "runs",
        json!({"id":id,"producer_kind":kind,"analysis_config_id":config,"context":ctx,"status":"complete","coverage":coverage}),
    );
    Ok(id)
}
#[allow(clippy::too_many_arguments)] // One constructor owns the unresolved record dimensions.
pub(crate) fn unresolved_record(
    b: &mut Value,
    ctx: &Value,
    subject: &str,
    evidence: &str,
    predicate: &str,
    status: &str,
    reason: &str,
    candidates: Vec<String>,
) -> Result<()> {
    let candidates: Vec<String> = candidates
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let status = if status == "ambiguous" && candidates.len() < 2 {
        "unresolved"
    } else {
        status
    };
    let mut u = json!({"source_entity_id":subject,"context":ctx,"evidence_ref_id":evidence,"predicate":predicate,
        "status":status,"reason":reason,"candidate_ids":candidates});
    u["id"] = json!(identity::id("unresolved", &u)?);
    add(b, "unresolved_references", u);
    Ok(())
}

pub(crate) struct Produced {
    pub coverage: Value,
    pub limitations: Vec<String>,
}
