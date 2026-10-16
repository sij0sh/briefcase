//! Opt-in Rust execution evidence over retained inputs. Syntax finds callsites;
//! native navigation must verify both free-function endpoints before publication.
use super::{
    analyzer::{self, Analyzer},
    capture::Capture,
    contracts::{self, add, rows, strings, text},
    identity, knowledge,
    ledger::{Ledger, Prepared},
    passes, query, require,
    rust::{self, Definition, ScanOptions, Syntax},
    Result,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    time::{Duration,Instant},
};
use tree_sitter::{Node, Parser};

#[derive(Clone)]
struct Site {
    path: String,
    module: String,
    caller: Option<String>,
    start: usize,
    end: usize,
    position: Option<usize>,
    reason: &'static str,
}
fn name_position(node: Node) -> Option<usize> {
    match node.kind() {
        "identifier" => Some(node.start_byte()),
        "scoped_identifier" => node.child_by_field_name("name").and_then(name_position),
        _ => None,
    }
}
struct FileSyntax<'a> {
    modules:Vec<&'a Definition>,
    functions:BTreeMap<usize,Option<&'a Definition>>,
    blocked:Vec<(usize,usize)>,
}
fn sites(c: &Capture, syntax: &Syntax) -> Result<Vec<Site>> {
    fn walk(
        node: Node,
        path: &str,
        syntax: &FileSyntax<'_>,
        caller: Option<&Definition>,
        in_closure: bool,
        out: &mut Vec<Site>,
    ) {
        let module = syntax.modules.iter()
            .filter(|d|d.start<=node.start_byte()&&d.end>=node.end_byte())
            .min_by_key(|d| d.end - d.start)
            .map(|d| d.key.clone())
            .unwrap_or_else(|| "crate".into());
        let blocked=syntax.blocked.iter().any(|(start,end)|*start<=node.start_byte()&&*end>=node.end_byte());
        if blocked
            || matches!(
                node.kind(),
                "macro_invocation" | "macro_definition" | "impl_item"
            )
        {
            out.push(Site {
                path: path.into(),
                module,
                caller: caller.map(|d| d.key.clone()),
                start: node.start_byte(),
                end: node.end_byte(),
                position: None,
                reason: "unsupported_generated_source",
            });
            return;
        }
        let caller = if node.kind() == "function_item" {
            syntax.functions.get(&node.start_byte()).copied().flatten()
        } else {
            caller
        };
        let in_closure = in_closure || node.kind() == "closure_expression";
        if node.kind() == "call_expression" {
            let function = node.child_by_field_name("function");
            let position = if in_closure {
                None
            } else {
                function.and_then(name_position)
            };
            let reason = if function.is_some_and(|f| f.kind() == "field_expression") {
                "trait_dispatch"
            } else {
                "dynamic_property"
            };
            out.push(Site {
                path: path.into(),
                module,
                caller: caller.map(|d| d.key.clone()),
                start: node.start_byte(),
                end: node.end_byte(),
                position,
                reason,
            });
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            walk(child, path, syntax, caller, in_closure, out);
        }
    }
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    let mut out = Vec::new();
    for path in &syntax.visited {
        let code = std::str::from_utf8(&c.files[path])?;
        let tree = parser
            .parse(code, None)
            .ok_or("EXECUTION: Rust parse failed")?;
        let mut functions=BTreeMap::<usize,Option<&Definition>>::new();
        for definition in syntax.definitions.iter().filter(|d|d.function&&d.path==*path) {
            functions.entry(definition.start).and_modify(|value|*value=None).or_insert(Some(definition));
        }
        let local=FileSyntax {modules:syntax.definitions.iter().filter(|d|d.kind=="module"&&d.path==*path).collect(),
            functions,blocked:syntax.occurrences.iter().filter(|o|o.path==*path&&o.kind=="unsupported_generated_source")
                .map(|o|(o.start,o.end)).collect()};
        walk(tree.root_node(),path,&local,None,false,&mut out);
        for (at,end) in rust::parse_errors(tree.root_node()) {
            if code.is_empty() {continue;}
            let mut start=at.min(code.len()-1);while !code.is_char_boundary(start) {start-=1;}
            let mut end=end.max(start+1).min(code.len());while !code.is_char_boundary(end) {end+=1;}
            out.push(Site {path:path.clone(),module:local.modules.first().map(|d|d.key.clone()).unwrap_or_else(||"crate".into()),
                caller:None,start,end,position:None,reason:"extraction_failure"});
        }
    }
    out.retain(|s| s.start < s.end);
    out.sort_by_key(|s| (s.path.clone(), s.start, s.end));
    out.dedup_by(|a, b| a.path == b.path && a.start == b.start && a.end == b.end);
    Ok(out)
}

/// Only a unique native definition at a retained free-function name is a target.
/// Parameters, local values, methods, external libraries and generated targets
/// cannot acquire a compiler function identity through this mapping.
fn function_target(
    syntax: &Syntax,
    c: &Capture,
    root: &Path,
    response: &Value,
) -> Result<Option<String>> {
    let candidates = if let Some(xs) = response.as_array() {
        xs.clone()
    } else if response.is_object() {
        vec![response.clone()]
    } else {
        vec![]
    };
    let mut keys = BTreeSet::new();
    let mut unsupported = false;
    for candidate in candidates {
        let uri = candidate["targetUri"]
            .as_str()
            .or_else(|| candidate["uri"].as_str())
            .ok_or("EXECUTION: Native definition has no URI")?;
        let path = analyzer::path_from_uri(uri)?;
        let Some(relative) = path.strip_prefix(root).ok().and_then(Path::to_str) else {
            unsupported = true;
            continue;
        };
        let Some(bytes) = c.files.get(relative) else {
            unsupported = true;
            continue;
        };
        let code = std::str::from_utf8(bytes)?;
        let range = candidate
            .get("targetSelectionRange")
            .or_else(|| candidate.get("range"))
            .ok_or("EXECUTION: Native definition has no range")?;
        let position = rust::byte_position(code, &range["start"]);
        let definitions:Vec<_>=syntax.definitions.iter().filter(|d|d.function&&d.path==relative&&Some(d.name_start)==position).collect();
        if definitions.len()==1 {keys.insert(definitions[0].key.clone());}else{unsupported=true;}
    }
    Ok(if !unsupported && keys.len() == 1 {
        keys.into_iter().next()
    } else {
        None
    })
}
#[derive(Default)]
struct Resolution {
    verified:BTreeSet<String>,
    failed_callers:BTreeSet<String>,
    resolved:Vec<(Site,String)>,
    unknown:Vec<(Site,&'static str)>,
    limitations:Vec<String>,
    completed:usize,
    native_requests:usize,
}

/// Resolve only endpoints needed by actual calls. No fixed function/site cap,
/// no up-front native sweep, and one cached identity check per caller. A native
/// call-target lookup itself verifies that target's retained free-function key.
fn resolve_sites<F>(sites:&[Site],syntax:&Syntax,declared:&BTreeSet<String>,deadline:Instant,mut lookup:F)->Resolution
where F:FnMut(&str,usize)->Result<Option<String>> {
    let definitions:BTreeMap<_,_>=syntax.definitions.iter().filter(|d|d.function).map(|d|(d.key.as_str(),d)).collect();
    let mut queues=BTreeMap::<&str,VecDeque<&Site>>::new();
    for site in sites {queues.entry(site.path.as_str()).or_default().push_back(site);}
    let mut result=Resolution::default();
    loop {
        let mut visited=false;
        for queue in queues.values_mut() {
            let Some(site)=queue.pop_front() else {continue};visited=true;
            let (Some(caller),Some(position))=(site.caller.as_deref(),site.position) else {
                result.completed+=1;result.unknown.push((site.clone(),site.reason));continue;
            };
            if Instant::now()>=deadline {result.unknown.push((site.clone(),"extraction_failure"));continue;}
            result.completed+=1;
            if !declared.contains(caller) || !definitions.contains_key(caller) {
                result.unknown.push((site.clone(),"unsupported_generated_source"));continue;
            }
            if !result.verified.contains(caller) && !result.failed_callers.contains(caller) {
                let definition=definitions[caller];result.native_requests+=1;
                match lookup(&definition.path,definition.name_start) {
                    Ok(Some(key)) if key==caller => {result.verified.insert(key);},
                    Ok(_) => {result.failed_callers.insert(caller.into());},
                    Err(error) => {result.failed_callers.insert(caller.into());result.limitations.push(error.to_string());},
                }
            }
            if !result.verified.contains(caller) {result.unknown.push((site.clone(),"extraction_failure"));continue;}
            if Instant::now()>=deadline {result.unknown.push((site.clone(),"extraction_failure"));continue;}
            result.native_requests+=1;
            match lookup(&site.path,position) {
                Ok(Some(target)) if declared.contains(&target)&&definitions.contains_key(target.as_str()) => {
                    result.failed_callers.remove(&target);
                    result.verified.insert(target.clone());result.resolved.push((site.clone(),target));
                },
                Ok(_) => result.unknown.push((site.clone(),site.reason)),
                Err(error) => {result.limitations.push(error.to_string());result.unknown.push((site.clone(),"extraction_failure"));},
            }
        }
        if !visited {break;}
    }
    result.resolved.sort_by_key(|(site,target)|(site.path.clone(),site.start,target.clone()));
    result.unknown.sort_by_key(|(site,reason)|(site.path.clone(),site.start,*reason));
    result
}

pub(crate) fn retained_capture(l: &Ledger, bundle: &Value) -> Result<Capture> {
    let ctx = knowledge::context(bundle);
    let snapshot = knowledge::row(bundle, "snapshots", &text(&ctx, "snapshot_id"))?;
    let payloads = l.payloads(bundle)?;
    let mut files = BTreeMap::new();
    for file in rows(snapshot, "files") {
        let artifact = rows(bundle, "artifacts")
            .iter()
            .find(|a| {
                a["path"] == file["path"]
                    && a["digest"] == file["digest"]
                    && a["source_kind"] != "transport"
            })
            .ok_or("EXECUTION: Missing retained snapshot artifact")?;
        let bytes = &payloads[&text(artifact, "id")];
        require(
            identity::bytes_digest(bytes) == file["digest"],
            "DIGEST",
            "Retained execution input is corrupted",
        )?;
        files.insert(text(file, "path"), bytes.clone());
    }
    Ok(Capture {
        root: ".".into(),
        captured_at_unix_ms: None,
            inventory: serde_json::Value::Null,
        files,
        diagnostics: vec![],
        exclusions: vec![],
        complete: snapshot["capture_state"] == "complete",
    })
}

pub fn analyze(l: &mut Ledger, o: &query::Options, options: &ScanOptions) -> Result<Value> {
    let deadline=Instant::now()+options.timeout;
    // Keep bounded time for serializing/publishing a partial result under the
    // same outer deadline instead of consuming it all on native requests.
    let native_deadline=deadline-(options.timeout/10).min(Duration::from_secs(5));
    require(
        !o.historical,
        "EXECUTION",
        "Execution extraction needs current eligible source inputs",
    )?;
    let mut strict = o.clone();
    strict.allow_partial = false;
    let v = query::loaded(l, &strict)?;
    require(
        rows(&v.bundle, "entity_versions").iter().any(|e| e["kind"] == "module" && e["language"] == "rust"),
        "EXECUTION",
        "Rust execution requires retained Rust modules; select --language typescript for a TS/JS view",
    )?;
    let watermark = l.watermark()?;
    let ctx = knowledge::context(&v.bundle);
    let c = retained_capture(l, &v.bundle)?;
    let syntax = rust::syntax(&c)?;
    let sandbox = c.materialize()?;
    let executable = options
        .analyzer
        .as_ref()
        .ok_or("EXECUTION: Native analyzer is required")?;
    let settings = &v.diagnostics["provider"]["settings"];
    let features = strings(
        settings["cargo"]
            .get("features")
            .unwrap_or(&settings["features"]),
    );
    require(
        options.features.is_empty() || options.features == features,
        "SCOPE",
        "Execution must inherit the captured build's feature selection",
    )?;
    let mut analyzer = Analyzer::start(executable, sandbox.path(), native_deadline.saturating_duration_since(Instant::now()), &features)?;
    let producer = analyzer.identity.clone();
    let implementation = identity::bytes_digest(include_bytes!("execution.rs"));
    let mut key = identity::id(
        "execution-overlay",
        &json!({"view_id":v.id,"context":ctx,"producer":producer,
        "implementation_digest":implementation,"pass_engine":identity::bytes_digest(include_bytes!("passes.rs"))}),
    )?;
    if l.has_overlay(&key)? {
        let execution = l.record_execution(
            &v.id,
            &json!({"operation":"analyze-execution","overlay_id":key,"reused":true}),
        )?;
        return Ok(json!({"view_id":v.id,"context":ctx,"execution_id":execution,"reused":true}));
    }
    let removed = l.retracted()?;
    let mut b = v.bundle.clone();
    let original: BTreeSet<_> = rows(&b, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect();
    let mut payloads = l.payloads(&b)?;
    let provider = identity::id(
        "identity-provider",
        &json!({"mapping":"retained-rust-free-functions-v1",
        "implementation_digest":implementation,"producer":producer}),
    )?;
    let mut config = json!({"producer_name":"rust-static-execution-v1","implementation_digest":implementation,
        "settings_digest":identity::digest(&producer)?});
    let config_id = identity::id("analysis", &config)?;
    config["id"] = json!(config_id);
    add(&mut b, "analysis_configs", config);
    let mut limitations = vec!["Static execution covers native-resolved direct calls between retained free functions, not observed runtime behavior. Methods, dynamic dispatch, external targets, generated code and conditional source remain unsupported.".to_owned()];
    let mut opened = BTreeSet::new();
    let source_entities:BTreeMap<_,_>=rows(&v.bundle,"entity_versions").iter().filter(|e|e["context"]==ctx)
        .map(|e|(text(e,"id"),e)).collect();
    let declared:BTreeSet<_>=rows(&v.bundle,"assertions").iter().filter(|a|a["class"]=="evidence"
        && a["proposition"]["predicate"]=="DECLARES" && !removed.contains(&text(a,"id")) && v.eligible.contains(&text(a,"id")))
        .filter_map(|a|source_entities.get(&text(&a["proposition"]["object"],"entity_version_id"))).map(|e|text(e,"canonical_key")).collect();
    let all_sites=sites(&c,&syntax)?;
    let result=resolve_sites(&all_sites,&syntax,&declared,native_deadline,|file,position| {
        let code=std::str::from_utf8(&c.files[file])?;let path=sandbox.path().join(file);
        if opened.insert(file.to_owned()) {analyzer.open(&path,code)?;}
        analyzer.definition(&path,code,position).and_then(|response|function_target(&syntax,&c,sandbox.path(),&response))
    });
    let Resolution {verified,failed_callers,resolved,unknown,limitations:native_limitations,completed,native_requests}=result;
    limitations.extend(native_limitations);
    let unverified=failed_callers.len();
    if unverified>0 {limitations.push(format!("{unverified} required caller identities could not be verified."));}
    let resolution_complete=c.complete && analyzer.workspace_ready() && unknown.is_empty() && unverified == 0 && syntax.diagnostics.is_empty() && limitations.len() == 1;
    let native_analysis=json!({"native_requests":native_requests,"native_functions":{"retained":syntax.definitions.iter().filter(|d|d.function).count(),
            "verified_endpoints":verified.len(),"unverified_required_callers":unverified},
        "scheduling":"round_robin_by_file; cached on-demand endpoints; deadline only",
        "rust_syntax":rust::syntax_diagnostics(&syntax)});
    let coverage = json!({"attempted":all_sites.len(),"completed":completed,
        "unsupported_count":unknown.iter().filter(|(_,r)| *r == "unsupported_generated_source").count(),
        "unresolved_count":unknown.iter().filter(|(_,r)| *r != "unsupported_generated_source").count(),
        "resolution_complete":resolution_complete,
        "guarantees":[{"predicate":"CALLS_STATIC","kind":"exact_supported",
            "scope":if resolution_complete {"retained_free_function_scope"} else {"native_resolved_subset"}}]});
    if !resolution_complete {
        // Partial results are retryable. Stable identical outcomes deduplicate,
        // while improved native resolution can publish a new additive overlay.
        let mut correspondence=json!(analyzer.witnesses);
        rust::normalize_native(&mut correspondence,sandbox.path());
        key=identity::id("execution-overlay",&json!({"base":key,"coverage":coverage,"correspondence":correspondence}))?;
        if l.has_overlay(&key)? {
            return Ok(json!({"view_id":v.id,"context":ctx,"coverage":coverage,"native_analysis":native_analysis,"reused":true,"state":"partial","retryable":true}));
        }
    }
    let run_id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config_id,"coverage":coverage}),
    )?;
    add(
        &mut b,
        "runs",
        json!({"id":run_id,"producer_kind":"semantic","analysis_config_id":config_id,"context":ctx,
        "status":if resolution_complete {"complete"} else {"partial"},"coverage":coverage}),
    );
    let mut functions = BTreeMap::new();
    for definition in syntax
        .definitions
        .iter()
        .filter(|d| verified.contains(&d.key))
    {
        super::ledger::recovery::check_deadline(deadline)?;
        let id = rust::entity(
            &mut b,
            &ctx,
            "symbol",
            &definition.key,
            &provider,
            "compiler",
        )?;
        let er = rust::evidence(&mut b, &definition.path, definition.start, definition.end)?;
        let file = rows(&v.bundle, "entity_versions")
            .iter()
            .find(|e| {
                e["kind"] == "file" && e["canonical_key"] == definition.path && e["context"] == ctx
            })
            .ok_or("EXECUTION: Missing retained file entity")?;
        rust::assertion(
            &mut b,
            &ctx,
            "evidence",
            &text(file, "id"),
            "DECLARES",
            knowledge::target(&id),
            json!({"run_id":run_id}),
            vec![er],
            None,
        )?;
        functions.insert(definition.key.clone(), id);
    }
    for (site, target) in resolved {
        super::ledger::recovery::check_deadline(deadline)?;
        let er = rust::evidence(&mut b, &site.path, site.start, site.end)?;
        let from = &functions[site.caller.as_ref().unwrap()];
        let to = &functions[&target];
        rust::assertion(
            &mut b,
            &ctx,
            "evidence",
            from,
            "CALLS_STATIC",
            knowledge::target(to),
            json!({"run_id":run_id}),
            vec![er],
            Some(json!({"status":"resolved","method":"compiler","candidate_ids":[to]})),
        )?;
    }
    for (site, reason) in unknown {
        super::ledger::recovery::check_deadline(deadline)?;
        let er = rust::evidence(&mut b, &site.path, site.start, site.end)?;
        let module = rows(&v.bundle, "entity_versions")
            .iter()
            .find(|e| {
                e["kind"] == "module" && e["canonical_key"] == site.module && e["context"] == ctx
            })
            .ok_or("EXECUTION: Missing source module")?;
        let subject = site
            .caller
            .as_ref()
            .and_then(|key| functions.get(key))
            .cloned()
            .unwrap_or_else(|| text(module, "id"));
        let mut u = json!({"source_entity_id":subject,"context":ctx,"evidence_ref_id":er,"predicate":"CALLS_STATIC",
            "status":if reason=="extraction_failure" {"unresolved"}else{"unsupported"},"reason":reason,"candidate_ids":[]});
        u["id"] = json!(identity::id("unresolved", &u)?);
        add(&mut b, "unresolved_references", u);
    }
    let mut witness = json!({"provider":producer,"mapping":"retained-rust-free-functions-v1","context":ctx,"correspondence":analyzer.witnesses});
    rust::normalize_native(&mut witness, sandbox.path());
    knowledge::artifact(
        &mut b,
        &mut payloads,
        "analyzers",
        serde_json::to_vec(&witness)?,
    )?;
    passes::produce_execution(&mut b, &mut payloads, &ctx, &coverage)?;
    let selected = rows(&b, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .filter(|id| !original.contains(id))
        .collect();
    limitations.sort();limitations.dedup();
    // Native work and serialization must share the caller's deadline. Never
    // start an expensive publication after its budget has expired.
    super::ledger::recovery::check_deadline(deadline)?;
    let prepared = Prepared::extension(
        b,
        payloads,
        v.generation,
        &selected,
        &[
            contracts::Capability::Evidence,
            contracts::Capability::Derived,
        ],
        json!({"coverage":coverage,"native_analysis":native_analysis,"limitations":limitations,"workspace_status":analyzer.workspace_status}),
    )?;
    super::ledger::recovery::check_deadline(deadline)?;
    l.commit_extension(
        &key,
        "source",
        std::slice::from_ref(&v.id),
        &prepared,
        &selected,
        watermark,
    )?;
    l.rebuild(&v.id)?;
    let execution = l.record_execution(
        &v.id,
        &json!({"operation":"analyze-execution","overlay_id":key,"reused":false}),
    )?;
    Ok(
        json!({"view_id":v.id,"context":ctx,"execution_id":execution,"coverage":coverage,"native_analysis":native_analysis,"limitations":limitations,
            "state":if resolution_complete {"complete"} else {"partial"},"retryable":!resolution_complete,
            "workspace_status":analyzer.workspace_status,"reused":false}),
    )
}

/// Shared retained-source execution publication for non-LSP language producers.
/// The producer cannot change source context, revive retractions or replace pins.
pub(crate) fn analyze_retained(
    l: &mut Ledger,
    o: &query::Options,
    languages: &[&str],
    producer: impl FnOnce(Capture, &str, u64) -> Result<Prepared>,
) -> Result<Value> {
    require(
        !o.historical,
        "EXECUTION",
        "Execution needs current eligible source inputs",
    )?;
    let mut strict = o.clone();
    strict.allow_partial = false;
    let v = query::loaded(l, &strict)?;
    require(
        rows(&v.bundle, "entity_versions")
            .iter()
            .any(|e| e["kind"] == "module" && languages.contains(&text(e, "language").as_str())),
        "EXECUTION",
        "Execution language must match the retained view",
    )?;
    let watermark = l.watermark()?;
    let ctx = knowledge::context(&v.bundle);
    let snapshot = knowledge::row(&v.bundle, "snapshots", &text(&ctx, "snapshot_id"))?;
    let payloads = l.payloads(&v.bundle)?;
    let capture = retained_capture(l, &v.bundle)?;
    let produced = producer(capture, &text(snapshot, "repository_id"), v.generation)?;
    require(
        knowledge::context(&produced.bundle) == ctx,
        "EXECUTION",
        "Interpreter/compiler, project and configuration must match the captured view",
    )?;
    require(
        rows(&produced.bundle, "assertions")
            .iter()
            .filter(|a| {
                ["CONTAINS", "DECLARES"].contains(&text(&a["proposition"], "predicate").as_str())
            })
            .all(|a| v.eligible.contains(&text(a, "id"))),
        "EXECUTION",
        "Retraction or producer identity changed required source structure; capture inputs again before analysis",
    )?;
    let original: BTreeSet<_> = rows(&v.bundle, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect();
    let selected: BTreeSet<_> = rows(&produced.bundle, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .filter(|id| !original.contains(id))
        .collect();
    let reused = rows(&v.bundle, "runs").iter().any(|r| {
        r["context"] == ctx
            && rows(&r["coverage"], "guarantees")
                .iter()
                .any(|g| g["predicate"] == "CALLS_STATIC")
    });
    let key = identity::id(
        "execution-overlay",
        &json!({"view_id":v.id,"context":ctx,
        "configuration":produced.diagnostics["provider"],"analysis":produced.bundle["analysis_configs"]}),
    )?;
    if !reused {
        let mut bundle = v.bundle.clone();
        knowledge::merge(&mut bundle, &produced.bundle)?;
        let mut payloads = payloads;
        payloads.extend(produced.payloads);
        let prepared = Prepared::extension(
            bundle,
            payloads,
            v.generation,
            &selected,
            &[
                contracts::Capability::Evidence,
                contracts::Capability::Derived,
            ],
            produced.diagnostics.clone(),
        )?;
        l.commit_extension(
            &key,
            "source",
            std::slice::from_ref(&v.id),
            &prepared,
            &selected,
            watermark,
        )?;
        l.rebuild(&v.id)?;
    }
    let execution = l.record_execution(
        &v.id,
        &json!({"operation":"analyze-execution","overlay_id":key,"reused":reused}),
    )?;
    Ok(
        json!({"view_id":v.id,"context":ctx,"execution_id":execution,"coverage":produced.diagnostics["call_coverage"],
        "reused":reused,"limitations":produced.diagnostics["limitations"]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition(path:&str,key:&str,position:usize)->Definition {
        Definition {path:path.into(),start:0,end:100,name_start:position,key:key.into(),kind:"symbol",function:true}
    }
    fn site(path:&str,caller:&str,position:usize)->Site {
        Site {path:path.into(),module:"crate".into(),caller:Some(caller.into()),start:position,end:position+1,
            position:Some(position),reason:"dynamic_property"}
    }
    #[test]
    fn more_than_four_thousand_functions_and_calls_do_not_hit_a_fixed_ceiling() {
        let mut syntax=Syntax::default();
        syntax.definitions=(0..5000).map(|index|definition("src/unused.rs",&format!("unused_{index}"),index)).collect();
        syntax.definitions.extend([definition("src/z.rs","caller",0),definition("src/z.rs","callee",1)]);
        let declared=syntax.definitions.iter().map(|definition|definition.key.clone()).collect();
        let calls:Vec<_>=(2..5003).map(|position|site("src/z.rs","caller",position)).collect();
        let result=resolve_sites(&calls,&syntax,&declared,Instant::now()+Duration::from_secs(30),|path,position| {
            assert_eq!(path,"src/z.rs","unused functions must not cause native requests");
            Ok(Some((if position==0 {"caller"}else{"callee"}).into()))
        });
        assert_eq!(result.resolved.len(),5001);assert!(result.unknown.is_empty());
        assert_eq!(result.native_requests,5002);assert_eq!(result.verified.len(),2);
        assert!(result.failed_callers.is_empty());
    }
    #[test]
    fn native_work_is_fair_across_files_and_reuses_caller_identity() {
        let mut syntax=Syntax::default();
        syntax.definitions=vec![definition("src/a.rs","a",0),definition("src/z.rs","z",0),definition("src/end.rs","target",0)];
        let declared=syntax.definitions.iter().map(|definition|definition.key.clone()).collect();
        let mut calls:Vec<_>=(10..210).map(|position|site("src/a.rs","a",position)).collect();calls.push(site("src/z.rs","z",10));
        let mut requested=vec![];
        let result=resolve_sites(&calls,&syntax,&declared,Instant::now()+Duration::from_secs(30),|path,position| {
            requested.push((path.to_owned(),position));
            Ok(Some((if position!=0 {"target"}else if path=="src/a.rs" {"a"}else{"z"}).into()))
        });
        assert_eq!(result.resolved.len(),201);
        assert!(requested.iter().take(4).any(|(path,_)|path=="src/z.rs"));
        assert_eq!(requested.iter().filter(|(_,position)|*position==0).count(),2);
    }
    #[test]
    fn failed_caller_checks_are_cached_and_expired_work_makes_no_native_requests() {
        let mut syntax=Syntax::default();syntax.definitions=vec![definition("src/a.rs","a",0)];
        let declared=BTreeSet::from(["a".into()]);let calls:Vec<_>=(10..20).map(|position|site("src/a.rs","a",position)).collect();
        let result=resolve_sites(&calls,&syntax,&declared,Instant::now()+Duration::from_secs(30),|_,_|Ok(None));
        assert_eq!(result.native_requests,1);assert_eq!(result.failed_callers.len(),1);assert_eq!(result.unknown.len(),calls.len());
        let result=resolve_sites(&calls,&syntax,&declared,Instant::now()-Duration::from_millis(1),|_,_|panic!("expired lookup"));
        assert_eq!(result.native_requests,0);assert_eq!(result.completed,0);
        assert!(result.unknown.iter().all(|(_,reason)|*reason=="extraction_failure"));
    }
    #[test]
    fn native_locations_cannot_choose_arbitrarily_between_shared_target_contexts() {
        let root=Path::new("/captured-source");
        let capture=Capture {root:root.into(),files:BTreeMap::from([("src/a.rs".into(),b"fn work() {}\n".to_vec())]),
            diagnostics:vec![],exclusions:vec![],complete:true,inventory:Value::Null,captured_at_unix_ms:None};
        let mut syntax=Syntax::default();syntax.definitions=vec![definition("src/a.rs","test::a::work",3),definition("src/a.rs","test::b::work",3)];
        let response=json!({"uri":analyzer::uri(&root.join("src/a.rs")),"range":{"start":{"line":0,"character":3}}});
        assert!(function_target(&syntax,&capture,root,&response).unwrap().is_none());
        syntax.definitions.pop();assert_eq!(function_target(&syntax,&capture,root,&response).unwrap(),Some("test::a::work".into()));
    }
    #[test]
    fn localized_parse_errors_keep_valid_sibling_calls_available() {
        let capture=Capture {root:".".into(),files:BTreeMap::from([
            ("Cargo.toml".into(),b"[package]\nname='demo'\nversion='0.1.0'\n".to_vec()),
            ("src/lib.rs".into(),b"fn before() {}\n@\nfn after() { before(); }\n".to_vec())]),
            diagnostics:vec![],exclusions:vec![],complete:true,inventory:Value::Null,captured_at_unix_ms:None};
        let syntax=rust::syntax(&capture).unwrap();let found=sites(&capture,&syntax).unwrap();
        assert!(found.iter().any(|site|site.position.is_some()&&site.caller.as_ref().is_some_and(|caller|caller.starts_with("after#"))));
        assert!(found.iter().filter(|site|site.reason=="extraction_failure").all(|site|site.start>0&&site.end<capture.files["src/lib.rs"].len()));
    }
}
