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
    collections::{BTreeMap, BTreeSet},
    path::Path,
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
fn sites(c: &Capture, syntax: &Syntax) -> Result<Vec<Site>> {
    fn walk(
        node: Node,
        path: &str,
        syntax: &Syntax,
        caller: Option<&Definition>,
        in_closure: bool,
        out: &mut Vec<Site>,
    ) {
        let module = syntax
            .definitions
            .iter()
            .filter(|d| {
                d.kind == "module"
                    && d.path == path
                    && d.start <= node.start_byte()
                    && d.end >= node.end_byte()
            })
            .min_by_key(|d| d.end - d.start)
            .map(|d| d.key.clone())
            .unwrap_or_else(|| "crate".into());
        let blocked = syntax.occurrences.iter().any(|o| {
            o.path == path
                && o.kind == "unsupported_generated_source"
                && o.start <= node.start_byte()
                && o.end >= node.end_byte()
        });
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
            syntax
                .definitions
                .iter()
                .find(|d| d.function && d.path == path && d.start == node.start_byte())
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
        walk(tree.root_node(), path, syntax, None, false, &mut out);
        if tree.root_node().has_error() {
            out.push(Site {
                path: path.clone(),
                module: syntax
                    .modules
                    .iter()
                    .find(|(_, p)| *p == path)
                    .map(|(m, _)| m.clone())
                    .unwrap_or_else(|| "crate".into()),
                caller: None,
                start: 0,
                end: code.len(),
                position: None,
                reason: "extraction_failure",
            });
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
        let definition = syntax
            .definitions
            .iter()
            .find(|d| d.function && d.path == relative && Some(d.name_start) == position);
        if let Some(definition) = definition {
            keys.insert(definition.key.clone());
        } else {
            unsupported = true;
        }
    }
    Ok(if !unsupported && keys.len() == 1 {
        keys.into_iter().next()
    } else {
        None
    })
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
    let mut analyzer = Analyzer::start(executable, sandbox.path(), options.timeout, &features)?;
    let producer = analyzer.identity.clone();
    let implementation = identity::bytes_digest(include_bytes!("execution.rs"));
    let key = identity::id(
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
    let mut verified = BTreeSet::new();
    let mut unverified = 0;
    const MAX_FUNCTIONS: usize = 4096;
    for (index, definition) in syntax.definitions.iter().filter(|d| d.function).enumerate() {
        if index >= MAX_FUNCTIONS {
            unverified += 1;
            continue;
        }
        let source_supported = rows(&v.bundle, "assertions").iter().any(|a| {
            a["class"] == "evidence"
                && a["proposition"]["predicate"] == "DECLARES"
                && !removed.contains(&text(a, "id"))
                && v.eligible.contains(&text(a, "id"))
                && rows(&v.bundle, "entity_versions").iter().any(|e| {
                    e["id"] == a["proposition"]["object"]["entity_version_id"]
                        && e["canonical_key"] == definition.key
                        && e["context"] == ctx
                })
        });
        if !source_supported {
            unverified += 1;
            continue;
        }
        let code = std::str::from_utf8(&c.files[&definition.path])?;
        let path = sandbox.path().join(&definition.path);
        if opened.insert(definition.path.clone()) {
            analyzer.open(&path, code)?;
        }
        match analyzer
            .definition(&path, code, definition.name_start)
            .and_then(|r| function_target(&syntax, &c, sandbox.path(), &r))
        {
            Ok(Some(key)) if key == definition.key => {
                verified.insert(key);
            }
            Ok(_) => {
                unverified += 1;
            }
            Err(error) => {
                unverified += 1;
                limitations.push(error.to_string());
            }
        }
    }
    let all_sites = sites(&c, &syntax)?;
    let mut resolved = Vec::new();
    let mut unknown = Vec::new();
    const MAX_SITES: usize = 4096;
    for (index, site) in all_sites.iter().enumerate() {
        let caller = site.caller.as_ref().filter(|key| verified.contains(*key));
        let result = if index >= MAX_SITES {
            Ok(None)
        } else if let (Some(_), Some(position)) = (caller, site.position) {
            let code = std::str::from_utf8(&c.files[&site.path])?;
            let path = sandbox.path().join(&site.path);
            if opened.insert(site.path.clone()) {
                analyzer.open(&path, code)?;
            }
            analyzer
                .definition(&path, code, position)
                .and_then(|r| function_target(&syntax, &c, sandbox.path(), &r))
        } else {
            Ok(None)
        };
        match result {
            Ok(Some(target)) if verified.contains(&target) => resolved.push((site.clone(), target)),
            Ok(_) => unknown.push((
                site.clone(),
                if index >= MAX_SITES {
                    "extraction_failure"
                } else {
                    site.reason
                },
            )),
            Err(error) => {
                limitations.push(error.to_string());
                unknown.push((site.clone(), "extraction_failure"));
            }
        }
    }
    if unverified > 0 {
        limitations.push(format!("{unverified} source function identities were not verified; the native-function limit is {MAX_FUNCTIONS}."));
    }
    let coverage = json!({"attempted":all_sites.len(),"completed":all_sites.len().min(MAX_SITES),
        "unsupported_count":unknown.iter().filter(|(_,r)| *r == "unsupported_generated_source").count(),
        "unresolved_count":unknown.iter().filter(|(_,r)| *r != "unsupported_generated_source").count() + unverified,
        "resolution_complete":c.complete && unknown.is_empty() && unverified == 0 && syntax.diagnostics.is_empty() && limitations.len() == 1,
        "guarantees":[{"predicate":"CALLS_STATIC","kind":"exact_supported"}]});
    let run_id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config_id,"coverage":coverage}),
    )?;
    add(
        &mut b,
        "runs",
        json!({"id":run_id,"producer_kind":"semantic","analysis_config_id":config_id,"context":ctx,
        "status":if all_sites.len() <= MAX_SITES {"complete"} else {"partial"},"coverage":coverage}),
    );
    let mut functions = BTreeMap::new();
    for definition in syntax
        .definitions
        .iter()
        .filter(|d| verified.contains(&d.key))
    {
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
            "status":"unsupported","reason":reason,"candidate_ids":[]});
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
    let prepared = Prepared::extension(
        b,
        payloads,
        v.generation,
        &selected,
        &[
            contracts::Capability::Evidence,
            contracts::Capability::Derived,
        ],
        json!({"coverage":coverage,"limitations":limitations}),
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
    let execution = l.record_execution(
        &v.id,
        &json!({"operation":"analyze-execution","overlay_id":key,"reused":false}),
    )?;
    Ok(
        json!({"view_id":v.id,"context":ctx,"execution_id":execution,"coverage":coverage,"limitations":limitations,"reused":false}),
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
