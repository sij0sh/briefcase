//! Shared canonical publication for retained compiler adapters.
//! Adapters report native occurrences. This boundary validates spans, endpoints,
//! coverage and quotas before constructing Evidence and running registered passes.
use super::{
    capture::Capture,
    contracts::{add, rows, strings, text},
    identity, knowledge,
    ledger::Prepared,
    passes, require, rust, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn span(c: &Capture, row: &Value, is_source: fn(&str) -> bool) -> Result<(String, usize, usize)> {
    let path = text(row, "path");
    let bytes = c
        .files
        .get(&path)
        .ok_or("ANALYZER: Unknown native source path")?;
    let start = row["start"]
        .as_u64()
        .ok_or("ANALYZER: Missing native span start")? as usize;
    let end = row["end"]
        .as_u64()
        .ok_or("ANALYZER: Missing native span end")? as usize;
    require(
        is_source(&path)
            && start < end
            && end <= bytes.len()
            && std::str::from_utf8(&bytes[..start]).is_ok()
            && std::str::from_utf8(&bytes[start..end]).is_ok(),
        "ANALYZER",
        "Invalid native source span",
    )?;
    Ok((path, start, end))
}
fn coverage(out: &Value, predicate: &str) -> Value {
    let unresolved = rows(out, "unresolved")
        .iter()
        .filter(|u| u["predicate"] == predicate)
        .count();
    let unsupported = rows(out, "diagnostics").len() + strings(&out["omitted"]).len();
    let sites = match predicate {
        "CALLS_STATIC" => "calls",
        "REFERENCES" => "references",
        _ => "imports",
    };
    let attempted = rows(out, sites).len() + unresolved;
    let semantic = out["provider"]["syntax_only"] == false;
    json!({"attempted":attempted,"completed":attempted,"unsupported_count":unsupported,
        "unresolved_count":unresolved,"resolution_complete":semantic && unresolved==0 && unsupported==0,
        "guarantees":[{"predicate":predicate,"kind":if semantic {"exact_supported"}else{"unknown"}}]})
}
fn run(
    b: &mut Value,
    ctx: &Value,
    config: &str,
    coverage: &Value,
    semantic: bool,
) -> Result<String> {
    let id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config,"coverage":coverage}),
    )?;
    add(
        b,
        "runs",
        json!({"id":id,"producer_kind":if semantic {"semantic"}else{"syntax"},
        "analysis_config_id":config,"context":ctx,"status":"complete","coverage":coverage}),
    );
    Ok(id)
}

pub(crate) struct Publisher {
    pub language: &'static str,
    pub name: &'static str,
    pub source: fn(&str) -> bool,
    pub implementation: &'static [u8],
    pub adapter: &'static [u8],
    pub limitations: &'static str,
}
impl Publisher {
    pub(crate) fn publish(
        &self,
        c: Capture,
        repository: &str,
        generation: u64,
        calls_enabled: bool,
        out: Value,
    ) -> Result<Prepared> {
        let syntax_only = out["provider"]["syntax_only"] == true;
        let is_source = self.source;
        require(c.complete, "CAPTURE", "Incomplete captured native inputs")?;
        require(
            !syntax_only || !calls_enabled,
            "ANALYZER",
            "native calls require binding resolution, not syntax-only mode",
        )?;
        require(
            rows(&out, "modules").len() <= 4096
                && rows(&out, "definitions").len() <= 4096
                && ["imports", "references", "calls", "unresolved"]
                    .iter()
                    .map(|k| rows(&out, k).len())
                    .sum::<usize>()
                    <= 16384,
            "ANALYZER",
            "native source quotas exceeded",
        )?;
        let producer = &out["provider"];
        let implementation = identity::digest(
            &json!({"publisher":identity::bytes_digest(self.implementation),
        "adapter":identity::bytes_digest(self.adapter), "publication":identity::bytes_digest(include_bytes!("native_source.rs")),
        "launcher":identity::bytes_digest(include_bytes!("native_host.rs"))}),
        )?;
        let provider = identity::id(
            "identity-provider",
            &json!({"mapping":producer,"implementation":implementation}),
        )?;
        let (mut b, mut payloads, ctx) = c.seed(repository, producer)?;
        let witness = knowledge::artifact(
            &mut b,
            &mut payloads,
            "analyzers",
            serde_json::to_vec(&out)?,
        )?;
        let mut config = json!({"producer_name":self.name,"implementation_digest":implementation,
        "settings_digest":identity::digest(producer)?});
        let config_id = identity::id("analysis", &config)?;
        config["id"] = json!(config_id);
        add(&mut b, "analysis_configs", config);
        let mut imports = coverage(&out, "IMPORTS");
        for predicate in ["CONTAINS", "DECLARES"] {
            imports["guarantees"]
                .as_array_mut()
                .unwrap()
                .push(json!({"predicate":predicate,"kind":"syntax_only"}));
        }
        let import_run = run(&mut b, &ctx, &config_id, &imports, !syntax_only)?;
        let references = coverage(&out, "REFERENCES");
        let reference_run = run(&mut b, &ctx, &config_id, &references, !syntax_only)?;
        let calls = coverage(&out, "CALLS_STATIC");
        let call_run = calls_enabled
            .then(|| run(&mut b, &ctx, &config_id, &calls, true))
            .transpose()?;
        let repo = rust::entity_for_language(
            &mut b,
            &ctx,
            "repository",
            repository,
            &provider,
            "syntax",
            "none",
        )?;
        let mut entities = BTreeMap::new();
        let mut functions = BTreeSet::new();
        for package in rows(&out, "packages") {
            let key = text(package, "key");
            let id = rust::entity_for_language(
                &mut b,
                &ctx,
                "package",
                &key,
                &provider,
                "syntax",
                self.language,
            )?;
            require(
                entities.insert(key, id).is_none(),
                "ANALYZER",
                "Duplicate native package key",
            )?;
        }
        for package in rows(&out, "packages") {
            let parent = package["parent"]
                .as_str()
                .map(|key| entities.get(key))
                .unwrap_or(Some(&repo))
                .ok_or("ANALYZER: Unknown native parent package")?;
            let path = text(package, "source");
            let bytes = c
                .files
                .get(&path)
                .ok_or("ANALYZER: Missing package source")?;
            if !bytes.is_empty() {
                let er = rust::evidence(&mut b, &path, 0, bytes.len())?;
                rust::assertion(
                    &mut b,
                    &ctx,
                    "evidence",
                    parent,
                    "CONTAINS",
                    knowledge::target(&entities[&text(package, "key")]),
                    json!({"run_id":import_run}),
                    vec![er],
                    None,
                )?;
            }
        }
        for group in rows(&out, "groups") {
            let key = text(group, "key");
            let id = rust::entity_for_language(
                &mut b,
                &ctx,
                "module",
                &key,
                &provider,
                "exact_mapping",
                self.language,
            )?;
            require(
                entities.insert(key.clone(), id).is_none(),
                "ANALYZER",
                "Duplicate native module group",
            )?;
            let parent = entities
                .get(&text(group, "parent"))
                .ok_or("ANALYZER: Unknown module group parent")?;
            let path = text(group, "source");
            let bytes = c
                .files
                .get(&path)
                .ok_or("ANALYZER: Unknown module group source")?;
            if !bytes.is_empty() {
                let er = rust::evidence(&mut b, &path, 0, bytes.len())?;
                rust::assertion(
                    &mut b,
                    &ctx,
                    "evidence",
                    parent,
                    "CONTAINS",
                    knowledge::target(&entities[&key]),
                    json!({"run_id":import_run}),
                    vec![er],
                    None,
                )?;
            }
        }
        for module in rows(&out, "modules") {
            let path = text(module, "path");
            require(
                is_source(&path) && c.files.contains_key(&path),
                "ANALYZER",
                "Unknown native module",
            )?;
            let id = rust::entity_for_language(
                &mut b,
                &ctx,
                "module",
                &path,
                &provider,
                "exact_mapping",
                self.language,
            )?;
            require(
                entities.insert(path.clone(), id.clone()).is_none(),
                "ANALYZER",
                "Duplicate native module",
            )?;
            let file = rust::entity_for_language(
                &mut b,
                &ctx,
                "file",
                &format!("file::{path}"),
                &provider,
                "syntax",
                self.language,
            )?;
            let parent = module
                .get("parent")
                .unwrap_or(&module["package"])
                .as_str()
                .map(|key| entities.get(key))
                .unwrap_or(Some(&repo))
                .ok_or("ANALYZER: Unknown native module package")?;
            let bytes = &c.files[&path];
            if !bytes.is_empty() {
                let er = rust::evidence(&mut b, &path, 0, bytes.len())?;
                for (from, to) in [(parent, &file), (&file, &id)] {
                    rust::assertion(
                        &mut b,
                        &ctx,
                        "evidence",
                        from,
                        "CONTAINS",
                        knowledge::target(to),
                        json!({"run_id":import_run}),
                        vec![er.clone()],
                        None,
                    )?;
                }
                b["entity_versions"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|e| e["id"] == id)
                    .unwrap()["source_evidence_ref_id"] = json!(er);
            }
        }
        require(
            c.files
                .keys()
                .filter(|p| is_source(p))
                .all(|p| entities.contains_key(p)),
            "ANALYZER",
            "native adapter omitted retained module structure",
        )?;
        for definition in rows(&out, "definitions") {
            let (path, start, end) = span(&c, definition, is_source)?;
            let key = text(definition, "key");
            require(
                key.starts_with(&format!("{path}::")),
                "ANALYZER",
                "native definition key is missing",
            )?;
            let callable = definition["callable"] == true && !syntax_only;
            let id = rust::entity_for_language(
                &mut b,
                &ctx,
                "symbol",
                &key,
                &provider,
                if callable { "exact_mapping" } else { "syntax" },
                self.language,
            )?;
            require(
                entities.insert(key.clone(), id.clone()).is_none(),
                "ANALYZER",
                "Duplicate native definition",
            )?;
            let er = rust::evidence(&mut b, &path, start, end)?;
            rust::assertion(
                &mut b,
                &ctx,
                "evidence",
                &entities[&path],
                "DECLARES",
                knowledge::target(&id),
                json!({"run_id":import_run}),
                vec![er],
                None,
            )?;
            if callable {
                functions.insert(key);
            }
        }
        for (predicate, sites, run) in [
            ("IMPORTS", "imports", Some(import_run.as_str())),
            ("REFERENCES", "references", Some(reference_run.as_str())),
            ("CALLS_STATIC", "calls", call_run.as_deref()),
        ] {
            let Some(run) = run else { continue };
            for site in rows(&out, sites) {
                let (path, start, end) = span(&c, site, is_source)?;
                let target = text(site, "target");
                let caller = text(site, "caller");
                require(
                    predicate != "CALLS_STATIC"
                        || (functions.contains(&caller) && functions.contains(&target)),
                    "ANALYZER",
                    "native call endpoints are not supported function definitions",
                )?;
                let from = if predicate == "CALLS_STATIC" {
                    entities.get(&caller)
                } else if predicate == "IMPORTS" {
                    entities
                        .get(&text(site, "subject"))
                        .or_else(|| entities.get(&path))
                } else {
                    entities.get(&caller).or_else(|| entities.get(&path))
                }
                .ok_or("ANALYZER: Unknown native occurrence subject")?;
                let to = entities
                    .get(&target)
                    .ok_or("ANALYZER: Unknown native occurrence target")?;
                let er = rust::evidence(&mut b, &path, start, end)?;
                rust::assertion(
                    &mut b,
                    &ctx,
                    "evidence",
                    from,
                    predicate,
                    knowledge::target(to),
                    json!({"run_id":run}),
                    vec![er],
                    Some(json!({"status":"resolved","method":"compiler","candidate_ids":[to]})),
                )?;
            }
        }
        for occurrence in rows(&out, "unresolved")
            .iter()
            .filter(|u| u["predicate"] != "CALLS_STATIC" || calls_enabled)
        {
            let (path, start, end) = span(&c, occurrence, is_source)?;
            let source = entities
                .get(&text(occurrence, "caller"))
                .or_else(|| entities.get(&path))
                .ok_or("ANALYZER: Unknown native unresolved subject")?;
            let candidates: Vec<_> = strings(&occurrence["candidate_keys"])
                .iter()
                .filter_map(|key| entities.get(key).cloned())
                .collect();
            let er = rust::evidence(&mut b, &path, start, end)?;
            let mut unknown = json!({"source_entity_id":source,"context":ctx,"predicate":occurrence["predicate"],
            "evidence_ref_id":er,"status":occurrence["status"],"reason":occurrence["reason"],"candidate_ids":candidates});
            unknown["id"] = json!(identity::id("unresolved", &unknown)?);
            add(&mut b, "unresolved_references", unknown);
        }
        passes::produce(&mut b, &mut payloads, &ctx, &imports)?;
        if calls_enabled {
            passes::produce_execution(&mut b, &mut payloads, &ctx, &calls)?;
        }
        let mut limitations = vec![self.limitations.to_owned()];
        limitations.extend(
            strings(&out["omitted"])
                .into_iter()
                .map(|p| format!("{} source outside analyzed selection: {p}", self.language)),
        );
        super::declarations::apply(&mut b, &c, &ctx, &provider, &mut limitations)?;
        let diagnostics = json!({"requested_analyses":json!({"dependencies":true,"static_calls":calls_enabled,"data":false}),"capture":c.metadata(),"provider":producer,"coverage":imports,"reference_coverage":references,
        "call_coverage":if calls_enabled {calls}else{Value::Null},format!("{}_diagnostics", self.language):out["diagnostics"],
        "files_attempted":rows(&out,"modules").len(),
        "files_completed":rows(&out,"modules").iter().filter(|m| m["parsed"]==true).count(),
        "omitted_source_files":out["omitted"],"limitations":limitations,"correspondence_artifact_id":witness,"exclusions":c.exclusions});
        Prepared::seal(b, payloads, generation, diagnostics)
    }
}
