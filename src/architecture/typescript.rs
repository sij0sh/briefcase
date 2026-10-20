//! Project-aware TS/JS producer. The compiler owns resolution; the Rust
//! boundary alone constructs and validates canonical Evidence and Derived facts.
use super::{
    capture::Capture,
    contracts::{add, rows, strings, text},
    identity, knowledge,
    ledger::{Ledger, Prepared},
    native_host, passes, query, require, rust, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct ScanOptions {
    pub node: PathBuf,
    /// Explicit compiler module, independently pinned by retained byte digest.
    pub compiler: PathBuf,
    pub project: String,
    pub timeout: Duration,
    pub calls: bool,
    /// Workspace partitions only: resolve identifier uses of imported
    /// bindings to their declaring definition (syntactic REFERENCES).
    pub references: bool,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            node: "node".into(),
            compiler: std::env::var_os("BRIEFCASE_TYPESCRIPT_COMPILER")
                .map(Into::into)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("tools/typescript/node_modules/typescript/lib/typescript.js")
                }),
            project: "tsconfig.json".into(),
            timeout: Duration::from_secs(60),
            calls: false,
            references: false,
        }
    }
}
fn compiler_output(c: &Capture, options: &ScanOptions) -> Result<Value> {
    node_adapter(
        c,
        options,
        "typescript.cjs",
        include_bytes!("typescript.cjs"),
    )
}
/// Run one bounded compiler-API adapter over retained bytes. The adapter only
/// reports occurrences; canonical records are constructed in Rust.
pub(crate) fn node_adapter(
    c: &Capture,
    options: &ScanOptions,
    name: &str,
    adapter: &[u8],
) -> Result<Value> {
    let out = node_adapter_mode(c, options, name, adapter, "analyze")?;
    if let Some(error) = out.get("configuration") {
        return Err(format!("ANALYZER: {}: {}", options.project, text(error, "message")).into());
    }
    Ok(out)
}

/// Validate with the production compiler host, before sync allocates a generation.
/// Configuration failures are values; process/protocol failures remain errors.
pub(crate) fn validate_project(c: &Capture, options: &ScanOptions) -> Result<Option<Value>> {
    let out = node_adapter_mode(
        c,
        options,
        "typescript.cjs",
        include_bytes!("typescript.cjs"),
        "validate",
    )?;
    if let Some(error) = out.get("configuration") {
        require(
            matches!(
                error["reason"].as_str(),
                Some(
                    "unavailable_project"
                        | "invalid_config"
                        | "no_sources"
                        | "unsupported_references"
                )
            ) && error["message"].is_string(),
            "ANALYZER",
            "Invalid TypeScript configuration diagnostic",
        )?;
        return Ok(Some(error.clone()));
    }
    require(
        out["valid"] == true,
        "ANALYZER",
        "Invalid TypeScript preflight response",
    )?;
    Ok(None)
}

fn node_adapter_mode(
    c: &Capture,
    options: &ScanOptions,
    name: &str,
    adapter: &[u8],
    mode: &str,
) -> Result<Value> {
    require(
        (1..=600).contains(&options.timeout.as_secs()),
        "ANALYZER",
        "Compiler timeout must be 1..600 seconds",
    )?;
    let deadline = Instant::now() + options.timeout;
    require(
        workspace(options) || c.files.contains_key(&options.project),
        "ANALYZER",
        "Project configuration must be a retained input",
    )?;
    let compiler = options.compiler.canonicalize()?;
    let digest = identity::bytes_digest(&std::fs::read(&compiler)?);
    let temp = native_host::scratch()?;
    let script = temp.path().join(name);
    std::fs::write(&script, adapter)?;
    // Workspace partitions are loaded by the adapter from its own directory.
    std::fs::write(
        temp.path().join("typescript_workspace.cjs"),
        WORKSPACE_ADAPTER,
    )?;
    let files: BTreeMap<_, _> = c
        .files
        .iter()
        .map(|(p, b)| Ok((p, std::str::from_utf8(b)?)))
        .collect::<Result<_>>()?;
    let mut request = json!({"schema_version":"1.0","project":options.project,"files":files,"mode":mode});
    if options.references && workspace(options) {
        request["references"] = json!(true);
    }
    let input = serde_json::to_vec(&request)?;
    // Keep Node selection and environment filtering language-specific.
    let mut command = Command::new(&options.node);
    command
        .arg(&script)
        .arg(&compiler)
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .current_dir(temp.path());
    // Workspace partitions bound files, definitions, imports and references
    // row by row; their total can exceed the default output limit.
    let output = if workspace(options) {
        native_host::run_with_stdout_limit(command, &input, deadline, WORKSPACE_STDOUT_LIMIT)?
    } else {
        native_host::run(command, &input, deadline)?
    };
    let out = identity::parse(&output.stdout)?;
    require(
        out["schema_version"] == "1.0"
            && out
                .get("compiler_digest")
                .unwrap_or(&out["provider"]["compiler_digest"])
                == &digest,
        "ANALYZER",
        "Compiler identity changed while analyzing",
    )?;
    Ok(out)
}
const WORKSPACE_ADAPTER: &[u8] = include_bytes!("typescript_workspace.cjs");
const WORKSPACE_STDOUT_LIMIT: u64 = 256 * 1024 * 1024;
/// `workspace` (every retained tsconfig.json) or a comma list of configs selects
/// partitioned, module-level analysis; calls are classified, never resolved.
pub(crate) fn workspace(options: &ScanOptions) -> bool {
    options.project == "workspace" || options.project.contains(',')
}
/// One limitation per top-level directory, not per omitted file.
fn omitted_limitations(omitted: &[String], label: &str) -> Vec<String> {
    let mut groups: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for p in omitted {
        let top = p.split('/').next().unwrap_or(p);
        let entry = groups.entry(top).or_insert((0, p));
        entry.0 += 1;
    }
    groups
        .into_iter()
        .map(|(top, (n, example))| format!("{label}: {n} file(s) under {top} (e.g. {example})"))
        .collect()
}
fn coverage(out: &Value, predicate: &str) -> Value {
    let unresolved = rows(out, "unresolved")
        .iter()
        .filter(|u| u["predicate"] == predicate)
        .count();
    let unsupported = strings(&out["omitted"]).len() + rows(out, "diagnostics").len();
    let attempted = if predicate == "IMPORTS" {
        rows(out, "imports").len()
    } else {
        rows(out, "calls").len()
    } + unresolved;
    json!({"attempted":attempted,"completed":attempted,"unsupported_count":unsupported,
        "unresolved_count":unresolved,"resolution_complete":unresolved==0 && unsupported==0,
        "guarantees":[{"predicate":predicate,"kind":"exact_supported"}]})
}
fn run(b: &mut Value, ctx: &Value, config: &str, coverage: &Value) -> Result<String> {
    let id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config,"coverage":coverage}),
    )?;
    add(
        b,
        "runs",
        json!({"id":id,"producer_kind":"semantic","analysis_config_id":config,
        "context":ctx,"status":"complete","coverage":coverage}),
    );
    Ok(id)
}
pub fn produce(
    c: Capture,
    repository: &str,
    generation: u64,
    options: &ScanOptions,
) -> Result<Prepared> {
    require(c.complete, "CAPTURE", "Incomplete captured TS/JS inputs")?;
    let out = compiler_output(&c, options)?;
    let producer = &out["provider"];
    let adapter = identity::digest(
        &json!({"rust":identity::bytes_digest(include_bytes!("typescript.rs")),
        "compiler_adapter":identity::bytes_digest(include_bytes!("typescript.cjs")),
        "workspace_adapter":identity::bytes_digest(WORKSPACE_ADAPTER),
        "launcher":identity::bytes_digest(include_bytes!("native_host.rs"))}),
    )?;
    let (mut b, mut payloads, ctx) = c.seed(repository, producer)?;
    let witness = knowledge::artifact(
        &mut b,
        &mut payloads,
        "analyzers",
        serde_json::to_vec(&out)?,
    )?;
    let provider = identity::id(
        "identity-provider",
        &json!({"mapping":producer,"adapter":adapter}),
    )?;
    let mut config = json!({"producer_name":"typescript-compiler-api","implementation_digest":adapter,
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
    let import_run = run(&mut b, &ctx, &config_id, &imports)?;
    let calls = coverage(&out, "CALLS_STATIC");
    let partitioned = workspace(options);
    let resolve_calls = options.calls && !partitioned;
    let call_run = if resolve_calls {
        Some(run(&mut b, &ctx, &config_id, &calls)?)
    } else {
        None
    };
    let repo = rust::entity_for_language(
        &mut b,
        &ctx,
        "repository",
        repository,
        &provider,
        "syntax",
        "none",
    )?;
    let reference_run = if options.references && partitioned {
        let unresolved = out["workspace"]["references"]["unresolved_targets"].as_u64().unwrap_or(0) as usize;
        let attempted = rows(&out, "references").len() + unresolved;
        Some(run(&mut b, &ctx, &config_id, &json!({"attempted":attempted,"completed":attempted,
            "unsupported_count":strings(&out["omitted"]).len() + rows(&out, "diagnostics").len(),
            "unresolved_count":unresolved,"resolution_complete":false,
            "guarantees":[{"predicate":"REFERENCES","kind":"syntax_only","scope":super::task::BINDING_SCOPE}]}))?)
    } else {
        None
    };
    let mut modules = BTreeMap::new();
    let mut functions = BTreeMap::new();
    let mut symbols = BTreeMap::new();
    for m in rows(&out, "modules") {
        let path = text(m, "path");
        let language = text(m, "language");
        // Import-only targets outside analyzed partitions are module identities
        // without declarations; their file containment is still syntax.
        let module = rust::entity_for_language(
            &mut b,
            &ctx,
            "module",
            &path,
            &provider,
            if m["analyzed"] == false {
                "syntax"
            } else {
                "compiler"
            },
            &language,
        )?;
        let file = rust::entity_for_language(
            &mut b,
            &ctx,
            "file",
            &format!("file::{path}"),
            &provider,
            "syntax",
            &language,
        )?;
        modules.insert(path.clone(), module.clone());
        let end = c.files[&path].len();
        if end > 0 {
            let er = rust::evidence(&mut b, &path, 0, end)?;
            for (from, to) in [(&repo, &file), (&file, &module)] {
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
                .find(|e| e["id"] == module)
                .unwrap()["source_evidence_ref_id"] = json!(er);
        }
        for d in rows(&out, "definitions")
            .iter()
            .filter(|d| d["path"] == path)
        {
            let key = text(d, "key");
            let id = rust::entity_for_language(
                &mut b,
                &ctx,
                "symbol",
                &key,
                &provider,
                if d["callable"] == true {
                    "compiler"
                } else {
                    "syntax"
                },
                &language,
            )?;
            let er = rust::evidence(
                &mut b,
                &path,
                d["start"].as_u64().unwrap() as usize,
                d["end"].as_u64().unwrap() as usize,
            )?;
            rust::assertion(
                &mut b,
                &ctx,
                "evidence",
                &module,
                "DECLARES",
                knowledge::target(&id),
                json!({"run_id":import_run}),
                vec![er],
                None,
            )?;
            if d["callable"] == true {
                functions.insert(key.clone(), id.clone());
            }
            symbols.insert(key, id);
        }
    }
    for (predicate, sites, rid) in [
        ("IMPORTS", rows(&out, "imports"), Some(import_run.as_str())),
        ("CALLS_STATIC", rows(&out, "calls"), call_run.as_deref()),
    ] {
        let Some(rid) = rid else { continue };
        for s in sites {
            let er = rust::evidence(
                &mut b,
                &text(s, "path"),
                s["start"].as_u64().unwrap() as usize,
                s["end"].as_u64().unwrap() as usize,
            )?;
            let (from, to) = if predicate == "IMPORTS" {
                (&modules[&text(s, "path")], &modules[&text(s, "target")])
            } else {
                (
                    &functions[&text(s, "caller")],
                    &functions[&text(s, "target")],
                )
            };
            rust::assertion(
                &mut b,
                &ctx,
                "evidence",
                from,
                predicate,
                knowledge::target(to),
                json!({"run_id":rid}),
                vec![er],
                Some(
                    json!({"status":"resolved","method":match s["method"].as_str() {
                    None | Some("compiler") => "compiler",
                    // Manifest, explicit-path and re-export resolution read retained syntax.
                    Some(_) => "syntax",
                },"candidate_ids":[to]}),
                ),
            )?;
        }
    }
    // One assertion per (enclosing definition or module, target definition):
    // a syntactic binding use, never a call or behavioral dependency.
    if let Some(rid) = &reference_run {
        for r in rows(&out, "references") {
            let path = text(r, "path");
            let (Some(to), Some(from)) = (
                symbols.get(&text(r, "target")),
                r["caller"]
                    .as_str()
                    .map_or(modules.get(&path), |caller| symbols.get(caller)),
            ) else {
                continue;
            };
            let er = rust::evidence(
                &mut b,
                &path,
                r["start"].as_u64().unwrap() as usize,
                r["end"].as_u64().unwrap() as usize,
            )?;
            rust::assertion(
                &mut b,
                &ctx,
                "evidence",
                from,
                "REFERENCES",
                knowledge::target(to),
                json!({"run_id":rid}),
                vec![er],
                Some(json!({"status":"resolved","method":"syntax","candidate_ids":[to]})),
            )?;
        }
    }
    for u in rows(&out, "unresolved")
        .iter()
        .filter(|u| u["predicate"] == "IMPORTS" || resolve_calls)
    {
        let er = rust::evidence(
            &mut b,
            &text(u, "path"),
            u["start"].as_u64().unwrap() as usize,
            u["end"].as_u64().unwrap() as usize,
        )?;
        let subject = functions
            .get(&text(u, "caller"))
            .unwrap_or(&modules[&text(u, "path")]);
        let mut u = json!({"source_entity_id":subject,"context":ctx,"predicate":u["predicate"],
            "evidence_ref_id":er,"status":"unresolved","reason":u["reason"],"candidate_ids":[]});
        u["id"] = json!(identity::id("unresolved", &u)?);
        add(&mut b, "unresolved_references", u);
    }
    passes::produce(&mut b, &mut payloads, &ctx, &imports)?;
    if resolve_calls {
        passes::produce_execution(&mut b, &mut payloads, &ctx, &calls)?;
    }
    let mut limitations = vec![if partitioned {
        "Captured TS/JS workspace partitions; module-level imports only (compiler, retained workspace manifests, explicit paths, syntactic re-exports). No external/ambient targets and no call resolution; call constructs are counted by kind.".to_owned()
    } else {
        "Captured TS/JS project only; no external/ambient targets, project references, methods, callbacks, function values, dynamic dispatch or runtime guarantee.".to_owned()
    }];
    limitations.extend(omitted_limitations(
        &strings(&out["omitted"]),
        if partitioned {
            "Source not analyzed (outside partitions or bounds)"
        } else {
            "Source outside configured project"
        },
    ));
    super::declarations::apply(&mut b, &c, &ctx, &provider, &mut limitations)?;
    let diagnostics = json!({"requested_analyses":json!({"dependencies":true,"static_calls":options.calls,"binding_references":reference_run.is_some(),"data":false}),"capture":c.metadata(),"provider":producer,"coverage":imports,"call_coverage":if resolve_calls {calls}else{Value::Null},"workspace":out["workspace"],
        "compiler_diagnostics":out["diagnostics"],"omitted_source_files":out["omitted"],
        "limitations":limitations,"correspondence_artifact_id":witness,"exclusions":c.exclusions});
    Prepared::seal(b, payloads, generation, diagnostics)
}

pub fn analyze(l: &mut Ledger, o: &query::Options, options: &ScanOptions) -> Result<Value> {
    let mut options = options.clone();
    options.calls = true;
    super::execution::analyze_retained(
        l,
        o,
        &["typescript", "javascript"],
        |capture, repository, generation| produce(capture, repository, generation, &options),
    )
}
