//! Versioned deterministic passes over explicit inputs. Outputs and snapshot
//! bindings are separate; execution IDs never enter semantic manifest keys.
use super::{
    contracts::{add, rows, strings, text, Payloads},
    identity, knowledge,
    ledger::Ledger,
    query, require, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[allow(clippy::too_many_arguments)] // One constructor owns the pinned manifest dimensions.
pub(crate) fn manifest(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    name: &str,
    inputs: Vec<String>,
    mut artifacts: Vec<String>,
    dependencies: Vec<String>,
    coverage: &Value,
    output: Value,
) -> Result<String> {
    artifacts.sort();
    artifacts.dedup();
    let predicates = rows(coverage, "guarantees")
        .iter()
        .filter(|g| g["kind"] != "syntax_only")
        .map(|g| text(g, "predicate"))
        .collect::<Vec<_>>();
    // Observed samples never become an exactness requirement.
    let requirements = rows(coverage, "guarantees")
        .iter()
        .filter(|g| {
            ["exact_supported", "sound_overapproximation"].contains(&text(g, "kind").as_str())
        })
        .map(|g| format!("{}:{}", text(g, "predicate"), text(g, "kind")))
        .collect::<Vec<_>>();
    let configuration = json!({"algorithm":name,"version":"1","ordering":"canonical ascending",
        "required_predicates":predicates,
        "build":rows(b,"build_contexts").iter().find(|r| r["id"] == ctx["build_context_id"]),
        "producer_configurations":producer_configurations(b,ctx,&predicates)});
    let implementation_digest = identity::digest(
        &json!({"pass_engine":identity::bytes_digest(include_bytes!("passes.rs")),
        "graph_algorithms":identity::bytes_digest(include_bytes!("query.rs")),
        "retained_knowledge":identity::bytes_digest(include_bytes!("knowledge.rs")),
        "identity_contract":identity::bytes_digest(include_bytes!("identity.rs"))}),
    )?;
    let input_content = if name == "module-import-dependency-v1"
        || name == "static-function-reachability-v1"
        || name == "field-write-lineage-v1"
    {
        artifacts.iter().map(|id|knowledge::row(b,"artifacts",id).map(|a|json!({"path":a["path"],"digest":a["digest"],"source_kind":a["source_kind"]}))).collect::<Result<Vec<_>>>()?
    } else {
        artifacts.iter().map(|id| json!(id)).collect()
    };
    let output_id = knowledge::artifact(
        b,
        payloads,
        "passes",
        identity::canonical(&json!({"schema_version":"1.0",
        "pass_name":name,"pass_version":"1","implementation_digest":implementation_digest,
        "configuration":semantic_configuration(b,ctx,&predicates),"input_content":input_content,"output":output}))?,
    )?;
    let mut m = json!({"schema_version":"1.0","pass_name":name,"pass_version":"1","context":ctx,
        "implementation_digest":implementation_digest,"configuration_digest":identity::digest(&configuration)?,
        "input_assertion_ids":inputs,"input_artifact_ids":artifacts,"dependencies":dependencies,
        "coverage":coverage,"coverage_requirements":requirements,
        "status":if coverage["resolution_complete"] == true {"complete"} else {"partial"},"output_artifact_ids":[output_id]});
    let id = identity::id("pass-manifest", &m)?;
    m["id"] = json!(id);
    add(b, "pass_manifests", m);
    Ok(id)
}
fn producer_configurations(b: &Value, ctx: &Value, predicates: &[String]) -> Vec<Value> {
    let selected: BTreeSet<_> = rows(b, "runs")
        .iter()
        .filter(|r| {
            r["context"] == *ctx
                && rows(&r["coverage"], "guarantees")
                    .iter()
                    .any(|g| predicates.contains(&text(g, "predicate")))
        })
        .map(|r| text(r, "analysis_config_id"))
        .collect();
    let mut configs: Vec<_> = rows(b, "analysis_configs")
        .iter()
        .filter(|c| selected.contains(&text(c, "id")))
        .cloned()
        .collect();
    configs.sort_by_key(|c| text(c, "id"));
    configs
}
fn semantic_configuration(b: &Value, ctx: &Value, predicates: &[String]) -> Value {
    let build = rows(b, "build_contexts")
        .iter()
        .find(|r| r["id"] == ctx["build_context_id"]);
    json!({"producer_configurations":producer_configurations(b,ctx,predicates),
        "settings_digest":build.map(|r|r["settings_digest"].clone()),
        "dependency_digest":build.map(|r|r["dependency_digest"].clone()),
        "generated_inputs_digest":build.map(|r|r["generated_inputs_digest"].clone())})
}
pub(crate) fn derivation(b: &mut Value, mid: &str, inputs: Vec<String>) -> Result<String> {
    let m = knowledge::row(b, "pass_manifests", mid)?;
    let mut d = json!({"pass_name":m["pass_name"],"implementation_digest":m["implementation_digest"],
        "configuration_digest":m["configuration_digest"],"input_assertion_ids":inputs,"deterministic":true,
        "support_class":"evidence_only","manifest_id":mid,"input_artifact_ids":m["input_artifact_ids"],"output_artifact_ids":m["output_artifact_ids"]});
    let id = identity::id("derivation", &d)?;
    d["id"] = json!(id);
    add(b, "derivations", d);
    Ok(id)
}

fn dependencies(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    coverage: &Value,
) -> Result<String> {
    let imports: Vec<_> = rows(b, "assertions")
        .iter()
        .filter(|a| {
            a["class"] == "evidence"
                && a["context"] == *ctx
                && a["proposition"]["predicate"] == "IMPORTS"
        })
        .cloned()
        .collect();
    let keys: BTreeMap<_, _> = rows(b, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), text(e, "canonical_key")))
        .collect();
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for a in &imports {
        let p = &a["proposition"];
        let pair = (
            text(p, "subject_id"),
            text(&p["object"], "entity_version_id"),
        );
        if pair.0 != pair.1 {
            groups.entry(pair).or_default().push(text(a, "id"));
        }
    }
    let mut dependency_manifests = Vec::new();
    let mut normalized_dependencies = Vec::new();
    for ((from, to), mut supports) in groups {
        supports.sort();
        supports.dedup();
        let sources = knowledge::sources(b, payloads, &supports)?;
        let mut artifacts: Vec<_> = sources.iter().map(|s| s.artifact_id.clone()).collect();
        // Native navigation can depend on the target body as well as the importer.
        let target_content = knowledge::module_source(b, payloads, &to)?;
        if let Some(s) = &target_content {
            artifacts.push(s.artifact_id.clone());
        }
        let output = json!({"from":keys[&from],"to":keys[&to],"predicate":"DEPENDS_ON",
            "imports":sources.iter().map(|s|json!({"path":s.path,"source_digest":identity::bytes_digest(s.text.as_bytes())})).collect::<Vec<_>>(),
            "target_content_digest":target_content.as_ref().map(|s|identity::bytes_digest(s.text.as_bytes()))});
        normalized_dependencies.push(output.clone());
        let mid = manifest(
            b,
            payloads,
            ctx,
            "module-import-dependency-v1",
            supports.clone(),
            artifacts,
            vec![],
            coverage,
            output,
        )?;
        let did = derivation(b, &mid, supports.clone())?;
        knowledge::derived(
            b,
            ctx,
            &from,
            "DEPENDS_ON",
            knowledge::target(&to),
            &did,
            supports,
        )?;
        dependency_manifests.push(mid);
    }
    let import_ids = imports
        .iter()
        .map(|a| text(a, "id"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let snapshot_files = rows(b, "snapshots")
        .iter()
        .find(|s| s["id"] == ctx["snapshot_id"])
        .map(|s| rows(s, "files"))
        .unwrap_or(&[]);
    let all_artifacts: Vec<_> = rows(b, "artifacts")
        .iter()
        .filter(|a| {
            snapshot_files
                .iter()
                .any(|f| f["path"] == a["path"] && f["digest"] == a["digest"])
        })
        .map(|a| text(a, "id"))
        .collect();
    // An aggregate manifest also exists for an empty graph. It explicitly pins
    // all module inputs; SCCs cannot make an incremental whole-graph assumption.
    normalized_dependencies.sort_by_key(|d| (text(d, "from"), text(d, "to")));
    let dependency_manifest = manifest(
        b,
        payloads,
        ctx,
        "module-dependency-overlay-v1",
        import_ids,
        all_artifacts,
        dependency_manifests,
        coverage,
        json!({"dependencies":normalized_dependencies}),
    )?;
    Ok(dependency_manifest)
}
fn scc(b: &mut Value, payloads: &mut Payloads, ctx: &Value, prerequisite: &str) -> Result<String> {
    let dependency_manifest = knowledge::row(b, "pass_manifests", prerequisite)?.clone();
    require(
        dependency_manifest["pass_name"] == "module-dependency-overlay-v1"
            && dependency_manifest["context"] == *ctx,
        "PASS_PLAN",
        "SCC requires a dependency overlay in the exact pinned context",
    )?;
    let coverage = dependency_manifest["coverage"].clone();
    let producer_manifests: BTreeSet<_> = strings(&dependency_manifest["dependencies"])
        .into_iter()
        .collect();
    let deps: Vec<_> = rows(b, "assertions")
        .iter()
        .filter(|a| {
            a["context"] == *ctx
                && a["proposition"]["predicate"] == "DEPENDS_ON"
                && knowledge::row(b, "derivations", &text(&a["origin"], "derivation_id"))
                    .is_ok_and(|d| producer_manifests.contains(&text(d, "manifest_id")))
        })
        .cloned()
        .collect();
    let keys: BTreeMap<_, _> = rows(b, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), text(e, "canonical_key")))
        .collect();
    let edges: Vec<_> = deps
        .iter()
        .map(|a| {
            (
                text(&a["proposition"], "subject_id"),
                text(&a["proposition"]["object"], "entity_version_id"),
            )
        })
        .collect();
    let groups = query::components(&edges);
    let mut named: Vec<Vec<String>> = groups
        .iter()
        .map(|g| {
            let mut names: Vec<_> = g.iter().map(|id| keys[id].clone()).collect();
            names.sort();
            names
        })
        .collect();
    named.sort();
    let input_ids = deps
        .iter()
        .map(|a| text(a, "id"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let upstream = strings(&dependency_manifest["output_artifact_ids"]);
    let scc_manifest = manifest(
        b,
        payloads,
        ctx,
        "strongly-connected-modules-v1",
        input_ids.clone(),
        upstream,
        vec![prerequisite.to_owned()],
        &coverage,
        json!({"cycles":named}),
    )?;
    let witness_edges: Vec<_> = deps
        .iter()
        .map(|a| {
            (
                text(&a["proposition"], "subject_id"),
                text(&a["proposition"]["object"], "entity_version_id"),
                text(a, "id"),
            )
        })
        .collect();
    for group in groups {
        let did = derivation(b, &scc_manifest, input_ids.clone())?;
        let mut names: Vec<_> = group.iter().map(|id| keys[id].clone()).collect();
        names.sort();
        let value = serde_json::to_string(&names)?;
        for subject in group {
            let witness = query::path(&witness_edges, &subject, &subject)
                .ok_or("PASS: Missing cycle witness")?;
            knowledge::derived(
                b,
                ctx,
                &subject,
                "IN_CYCLE",
                json!({"kind":"literal","value":value}),
                &did,
                witness,
            )?;
        }
    }
    Ok(scc_manifest)
}

/// Per-entry closures have independent content artifacts and conservative
/// source-file dependencies. Global graph bookkeeping does not define identity.
fn reachability(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    coverage: &Value,
) -> Result<String> {
    let calls: Vec<_> = rows(b, "assertions")
        .iter()
        .filter(|a| {
            a["class"] == "evidence"
                && a["context"] == *ctx
                && a["proposition"]["predicate"] == "CALLS_STATIC"
        })
        .cloned()
        .collect();
    let keys: BTreeMap<_, _> = rows(b, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), text(e, "canonical_key")))
        .collect();
    let mut edges: Vec<_> = calls
        .iter()
        .map(|a| {
            (
                text(&a["proposition"], "subject_id"),
                text(&a["proposition"]["object"], "entity_version_id"),
                text(a, "id"),
            )
        })
        .collect();
    edges.sort();
    const MAX_ENTRIES: usize = 256;
    const MAX_TARGETS: usize = 256;
    let subjects: BTreeSet<_> = edges.iter().map(|(a, _, _)| a.clone()).collect();
    let nodes: BTreeSet<_> = edges
        .iter()
        .flat_map(|(a, b, _)| [a.clone(), b.clone()])
        .collect();
    let mut adjacency: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (from, to, id) in &edges {
        adjacency
            .entry(from.clone())
            .or_default()
            .push((to.clone(), id.clone()));
    }
    for next in adjacency.values_mut() {
        next.sort_by_key(|(to, id)| (keys[to].clone(), id.clone()));
    }
    let mut coverage = coverage.clone();
    if subjects.len() > MAX_ENTRIES || nodes.len() > MAX_TARGETS {
        coverage["resolution_complete"] = json!(false);
        coverage["unsupported_count"] =
            json!(coverage["unsupported_count"].as_u64().unwrap_or(0) + 1);
    }
    let mut manifests = Vec::new();
    let mut graph = Vec::new();
    let mut subjects = subjects.into_iter().collect::<Vec<_>>();
    subjects.sort_by_key(|id| keys[id].clone());
    for from in subjects.iter().take(MAX_ENTRIES) {
        let mut paths = BTreeMap::new();
        let mut queue = std::collections::VecDeque::from([(from.clone(), Vec::<String>::new())]);
        let mut visited = BTreeSet::from([from.clone()]);
        while let Some((node, witness)) = queue.pop_front() {
            for (to, id) in adjacency.get(&node).into_iter().flatten() {
                let mut next = witness.clone();
                next.push(id.clone());
                if to == from {
                    paths.entry(to.clone()).or_insert(next);
                } else if visited.len() < MAX_TARGETS && visited.insert(to.clone()) {
                    paths.insert(to.clone(), next.clone());
                    queue.push_back((to.clone(), next));
                }
            }
        }
        if paths.is_empty() {
            continue;
        }
        // Include every outgoing edge from the reachable closure, not merely
        // one selected shortest witness. Corrections cannot leave stale proofs.
        let reachable: BTreeSet<_> = std::iter::once(from.clone())
            .chain(paths.keys().cloned())
            .collect();
        let mut inputs: BTreeSet<_> = edges
            .iter()
            .filter(|(a, _, _)| reachable.contains(a))
            .map(|(_, _, id)| id.clone())
            .collect();
        inputs.extend(
            rows(b, "assertions")
                .iter()
                .filter(|a| {
                    a["class"] == "evidence"
                        && a["context"] == *ctx
                        && a["proposition"]["predicate"] == "DECLARES"
                        && reachable
                            .contains(&text(&a["proposition"]["object"], "entity_version_id"))
                })
                .map(|a| text(a, "id")),
        );
        let inputs: Vec<_> = inputs.into_iter().collect();
        let sources = knowledge::sources(b, payloads, &inputs)?;
        let artifacts = sources.iter().map(|s| s.artifact_id.clone()).collect();
        let targets = paths
            .keys()
            .map(|to| keys[to].clone())
            .collect::<BTreeSet<_>>();
        let semantic_edges = edges
            .iter()
            .filter(|(a, _, _)| reachable.contains(a))
            .map(|(a, b, _)| (keys[a].clone(), keys[b].clone()))
            .collect::<BTreeSet<_>>();
        let output = json!({"from":keys[from],"reachable":targets,
            "calls":semantic_edges.iter().map(|(a,b)|json!({"from":a,"to":b})).collect::<Vec<_>>()});
        graph.push(output.clone());
        let mid = manifest(
            b,
            payloads,
            ctx,
            "static-function-reachability-v1",
            inputs.clone(),
            artifacts,
            vec![],
            &coverage,
            output,
        )?;
        let did = derivation(b, &mid, inputs)?;
        for (to, witness) in paths {
            knowledge::derived(
                b,
                ctx,
                from,
                "REACHES_STATIC",
                knowledge::target(&to),
                &did,
                witness,
            )?;
        }
        manifests.push(mid);
    }
    graph.sort_by_key(|entry| text(entry, "from"));
    manifest(
        b,
        payloads,
        ctx,
        "static-callgraph-overlay-v1",
        calls.iter().map(|a| text(a, "id")).collect(),
        vec![],
        manifests,
        &coverage,
        json!({"entries":graph,"limits":{"entries":MAX_ENTRIES,"targets":MAX_TARGETS}}),
    )
}
/// Request-field → column lineage. One reusable manifest per supported chain:
/// TRANSFORMS(DTO field → ORM field) at the exact ORM assignment span, the
/// WRITES occurrence at that same span, and explicit MAPS_TO(ORM field → column).
/// Chains whose retained inputs are unchanged keep their content identity.
pub(crate) fn produce_lineage(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    coverage: &Value,
) -> Result<String> {
    let evidence = |predicate: &str| -> Vec<Value> {
        rows(b, "assertions")
            .iter()
            .filter(|a| {
                a["class"] == "evidence"
                    && a["context"] == *ctx
                    && a["proposition"]["predicate"] == predicate
            })
            .cloned()
            .collect()
    };
    let transforms = evidence("TRANSFORMS");
    let writes = evidence("WRITES");
    let maps = evidence("MAPS_TO");
    let keys: BTreeMap<_, _> = rows(b, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), text(e, "canonical_key")))
        .collect();
    let object = |a: &Value| text(&a["proposition"]["object"], "entity_version_id");
    let subject = |a: &Value| text(&a["proposition"], "subject_id");
    let mut chains = Vec::new();
    let mut manifests = Vec::new();
    let mut outputs = Vec::new();
    for t in &transforms {
        let site = strings(&t["evidence_ref_ids"])
            .first()
            .cloned()
            .unwrap_or_default();
        for w in writes
            .iter()
            .filter(|w| strings(&w["evidence_ref_ids"]).first() == Some(&site))
        {
            for m in maps
                .iter()
                .filter(|m| subject(m) == object(t) && object(m) == object(w))
            {
                let mut supports = vec![text(t, "id"), text(m, "id"), text(w, "id")];
                supports.sort();
                chains.push((subject(t), object(w), supports, t.clone(), w.clone()));
            }
        }
    }
    for (source, column, supports, t, w) in chains {
        let sources = knowledge::sources(b, payloads, &supports)?;
        let artifacts = sources.iter().map(|s| s.artifact_id.clone()).collect();
        let output = json!({"from":keys[&source],"via":keys[&object(&t)],"to":keys[&column],
            "operation_function":keys[&subject(&w)],"transformation":t["transformation"],
            "witnesses":sources.iter().map(|s|json!({"path":s.path,"source_digest":identity::bytes_digest(s.text.as_bytes())})).collect::<Vec<_>>()});
        outputs.push(output.clone());
        let chain = json!({"attempted":1,"completed":1,"unsupported_count":0,"unresolved_count":0,"resolution_complete":true,
            "guarantees":[{"predicate":"WRITES","kind":"exact_supported"},{"predicate":"TRANSFORMS","kind":"exact_supported"},
                {"predicate":"MAPS_TO","kind":"exact_supported"}]});
        let mid = manifest(
            b,
            payloads,
            ctx,
            "field-write-lineage-v1",
            supports.clone(),
            artifacts,
            vec![],
            &chain,
            output,
        )?;
        let did = derivation(b, &mid, supports.clone())?;
        knowledge::derived(
            b,
            ctx,
            &source,
            "WRITES",
            knowledge::target(&column),
            &did,
            supports,
        )?;
        manifests.push(mid);
    }
    outputs.sort_by_key(|o| (text(o, "from"), text(o, "to")));
    manifests.sort();
    manifests.dedup();
    let mut inputs: Vec<_> = transforms
        .iter()
        .chain(&writes)
        .chain(&maps)
        .map(|a| text(a, "id"))
        .collect();
    inputs.sort();
    inputs.dedup();
    manifest(
        b,
        payloads,
        ctx,
        "data-lineage-overlay-v1",
        inputs,
        vec![],
        manifests,
        coverage,
        json!({"lineage":outputs}),
    )
}
/// Bounded aggregate windows over runtime Evidence. Each window is Derived
/// from its exact span-witness Evidence; counts live in the pass output, and a
/// window with no observations is simply absent, never a zero-call claim.
pub(crate) fn produce_runtime_aggregates(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    coverage: &Value,
    events_artifact: &str,
    events: &[Value],
    window: i64,
) -> Result<String> {
    let starts: BTreeMap<String, (i64, bool)> = events
        .iter()
        .map(|e| {
            let nanos = text(e, "start_unix_nano").parse::<i128>().unwrap_or(0);
            (
                text(e, "id"),
                (
                    (nanos / 1_000_000_000) as i64,
                    e["kind"] == "client" && e["status"] == "error",
                ),
            )
        })
        .collect();
    let refs: BTreeMap<String, String> = rows(b, "evidence_refs")
        .iter()
        .map(|r| (text(r, "id"), text(&r["locator"], "event_id")))
        .collect();
    let keys: BTreeMap<_, _> = rows(b, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), text(e, "canonical_key")))
        .collect();
    // (predicate, caller, callee, window) → (supporting Evidence, error witnesses)
    type Window = (String, String, String, i64);
    let mut groups: BTreeMap<Window, (Vec<String>, usize)> = BTreeMap::new();
    for a in rows(b, "assertions").iter().filter(|a| {
        a["class"] == "evidence"
            && a["context"] == *ctx
            && ["CALLS_OBSERVED", "CONNECTION_OBSERVED"]
                .contains(&a["proposition"]["predicate"].as_str().unwrap_or(""))
    }) {
        let witnesses: Vec<(i64, bool)> = strings(&a["evidence_ref_ids"])
            .iter()
            .filter_map(|r| refs.get(r))
            .filter_map(|e| starts.get(e).copied())
            .collect();
        let Some(first) = witnesses.iter().map(|w| w.0).min() else {
            continue;
        };
        let entry = groups
            .entry((
                text(&a["proposition"], "predicate"),
                text(&a["proposition"], "subject_id"),
                text(&a["proposition"]["object"], "entity_version_id"),
                first.div_euclid(window),
            ))
            .or_default();
        entry.0.push(text(a, "id"));
        entry.1 += usize::from(witnesses.iter().any(|w| w.1));
    }
    let mut manifests = Vec::new();
    let mut outputs = Vec::new();
    for ((predicate, from, to, index), (mut supports, errors)) in groups {
        supports.sort();
        let output = json!({"predicate":predicate,"from":keys[&from],"to":keys[&to],
            "window_start":super::runtime_observations::timestamp(i128::from(index * window) * 1_000_000_000),
            "window_end":super::runtime_observations::timestamp(i128::from((index + 1) * window) * 1_000_000_000),
            "count":supports.len(),"error_count":errors});
        outputs.push(output.clone());
        let mid = manifest(
            b,
            payloads,
            ctx,
            "runtime-observation-window-v1",
            supports.clone(),
            vec![events_artifact.to_owned()],
            vec![],
            coverage,
            output,
        )?;
        let did = derivation(b, &mid, supports.clone())?;
        knowledge::derived(
            b,
            ctx,
            &from,
            &predicate,
            knowledge::target(&to),
            &did,
            supports,
        )?;
        manifests.push(mid);
    }
    manifests.sort();
    let inputs: BTreeSet<String> = rows(b, "assertions")
        .iter()
        .filter(|a| a["class"] == "evidence" && a["context"] == *ctx)
        .map(|a| text(a, "id"))
        .collect();
    manifest(
        b,
        payloads,
        ctx,
        "runtime-observation-aggregate-v1",
        inputs.into_iter().collect(),
        vec![events_artifact.to_owned()],
        manifests,
        coverage,
        json!({"window_seconds":window,"windows":outputs}),
    )
}
pub(crate) fn produce_execution(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    coverage: &Value,
) -> Result<()> {
    execute(
        b,
        payloads,
        ctx,
        coverage,
        &[Node {
            id: "reachability".into(),
            operation: Operation::Reachability,
            dependencies: vec![],
        }],
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Dependencies,
    Scc,
    Reachability,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct External {
    pass: String,
    artifact_id: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum Dependency {
    Node(String),
    Pinned(External),
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Node {
    id: String,
    operation: Operation,
    dependencies: Vec<Dependency>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema_version: String,
    view: String,
    passes: Vec<Node>,
}
fn default_nodes() -> Vec<Node> {
    vec![
        Node {
            id: "dependencies".into(),
            operation: Operation::Dependencies,
            dependencies: vec![],
        },
        Node {
            id: "scc".into(),
            operation: Operation::Scc,
            dependencies: vec![Dependency::Node("dependencies".into())],
        },
    ]
}
fn schedule(nodes: &[Node]) -> Result<Vec<String>> {
    require(
        !nodes.is_empty() && nodes.len() <= 64,
        "PASS_PLAN",
        "Plan needs 1..64 nodes",
    )?;
    let mut graph = BTreeMap::new();
    for node in nodes {
        require(
            !node.id.is_empty() && !graph.contains_key(&node.id),
            "PASS_PLAN",
            "Invalid or duplicate pass ID",
        )?;
        graph.insert(
            node.id.clone(),
            node.dependencies
                .iter()
                .filter_map(|d| match d {
                    Dependency::Node(id) => Some(id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        );
    }
    fn visit(
        id: &str,
        graph: &BTreeMap<String, Vec<String>>,
        active: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
        order: &mut Vec<String>,
    ) -> Result<()> {
        require(!active.contains(id), "PASS_CYCLE", "Cyclic pass dependency")?;
        if done.contains(id) {
            return Ok(());
        }
        let deps = graph.get(id).ok_or("PASS_PLAN: Unknown dependency node")?;
        active.insert(id.into());
        for dependency in deps {
            visit(dependency, graph, active, done, order)?;
        }
        active.remove(id);
        done.insert(id.into());
        order.push(id.into());
        Ok(())
    }
    let mut order = Vec::new();
    let mut done = BTreeSet::new();
    for id in graph.keys() {
        visit(id, &graph, &mut BTreeSet::new(), &mut done, &mut order)?;
    }
    Ok(order)
}
/// Validate all prerequisites before executing any node. Node dependencies bind
/// this exact acyclic plan. External dependencies must pin completed manifests.
fn plan_nodes(plan: &Value, b: &Value, view: &str) -> Result<Vec<Node>> {
    let plan: Plan = serde_json::from_value(plan.clone())
        .map_err(|e| format!("PASS_PLAN: Invalid plan schema: {e}"))?;
    require(
        plan.schema_version == "1.0",
        "PASS_PLAN",
        "Invalid plan contract version",
    )?;
    require(
        plan.view != "latest",
        "UNPINNED",
        "Plan must pin a view, not latest",
    )?;
    require(
        plan.view == view,
        "UNPINNED",
        "Plan view must match the explicitly selected pinned view",
    )?;
    schedule(&plan.passes)?;
    let ctx = knowledge::context(b);
    for node in &plan.passes {
        if matches!(
            node.operation,
            Operation::Dependencies | Operation::Reachability
        ) {
            require(
                node.dependencies.is_empty(),
                "PASS_PLAN",
                "Source passes consume pinned canonical predicates, not another overlay",
            )?;
        } else {
            require(
                node.dependencies.len() == 1,
                "PASS_PLAN",
                "SCC requires exactly one pinned dependency prerequisite",
            )?;
        }
        for dep in &node.dependencies {
            match dep {
                Dependency::Node(id) => require(
                    plan.passes
                        .iter()
                        .any(|n| n.id == *id && n.operation == Operation::Dependencies),
                    "PASS_PLAN",
                    "SCC prerequisite must produce a dependency overlay",
                )?,
                Dependency::Pinned(external) => {
                    require(
                        external.artifact_id != "latest" && !external.artifact_id.is_empty(),
                        "UNPINNED",
                        "External dependency must pin an immutable manifest, not latest",
                    )?;
                    let m = knowledge::row(b, "pass_manifests", &external.artifact_id)?;
                    require(
                        external.pass == "dependencies"
                            && m["pass_name"] == "module-dependency-overlay-v1",
                        "PASS_PLAN",
                        "Wrong external dependency contract for SCC",
                    )?;
                    require(
                        m["status"] == "complete",
                        "COVERAGE",
                        "Partial dependency cannot satisfy a completed pass",
                    )?;
                    require(
                        m["context"] == ctx,
                        "SCOPE",
                        "External prerequisite must pin the requested snapshot/build context",
                    )?;
                }
            }
        }
    }
    Ok(plan.passes)
}
pub fn validate_plan(plan: &Value, b: &Value, view: &str) -> Result<Vec<String>> {
    schedule(&plan_nodes(plan, b, view)?)
}
fn execute(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    coverage: &Value,
    nodes: &[Node],
) -> Result<Vec<String>> {
    let order = schedule(nodes)?;
    let mut outputs = BTreeMap::new();
    let mut manifests = Vec::new();
    for id in order {
        let node = nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or("PASS_PLAN: Missing scheduled node")?;
        let output = match node.operation {
            Operation::Dependencies => dependencies(b, payloads, ctx, coverage)?,
            Operation::Reachability => {
                let calls = rows(b, "runs")
                    .iter()
                    .find(|r| {
                        r["context"] == *ctx
                            && rows(&r["coverage"], "guarantees")
                                .iter()
                                .any(|g| g["predicate"] == "CALLS_STATIC")
                    })
                    .ok_or("COVERAGE: Static execution producer has not run")?["coverage"]
                    .clone();
                reachability(b, payloads, ctx, &calls)?
            }
            Operation::Scc => {
                let input = match node
                    .dependencies
                    .first()
                    .ok_or("PASS_PLAN: Missing SCC prerequisite")?
                {
                    Dependency::Node(id) => outputs
                        .get(id)
                        .ok_or("PASS_PLAN: Dependency was not executed")?,
                    Dependency::Pinned(pinned) => &pinned.artifact_id,
                };
                scc(b, payloads, ctx, input)?
            }
        };
        outputs.insert(id, output.clone());
        manifests.push(output);
    }
    Ok(manifests)
}
pub fn produce(
    b: &mut Value,
    payloads: &mut Payloads,
    ctx: &Value,
    coverage: &Value,
) -> Result<()> {
    execute(b, payloads, ctx, coverage, &default_nodes())?;
    Ok(())
}
pub fn inspect(
    l: &mut Ledger,
    view: &str,
    plan: Option<&Value>,
    require_complete: bool,
) -> Result<Value> {
    let v = query::loaded(
        l,
        &query::Options {
            view: view.into(),
            ..Default::default()
        },
    )?;
    let order = if let Some(plan) = plan {
        validate_plan(plan, &v.bundle, view)?
    } else {
        let mut order = vec!["dependencies".into(), "scc".into()];
        if rows(&v.bundle, "pass_manifests")
            .iter()
            .any(|m| m["pass_name"] == "static-callgraph-overlay-v1")
        {
            order.push("reachability".into());
        }
        order
    };
    let ms = rows(&v.bundle, "pass_manifests");
    require(!ms.is_empty(), "PASS", "No retained pass manifests")?;
    let requested = if let Some(plan) = plan {
        Some(
            plan_nodes(plan, &v.bundle, view)?
                .into_iter()
                .map(|n| n.operation)
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    let belongs = |m: &&Value| {
        requested.as_ref().is_none_or(|ops| {
            ops.iter().any(|op| match op {
                Operation::Dependencies => [
                    "module-import-dependency-v1",
                    "module-dependency-overlay-v1",
                ]
                .contains(&text(m, "pass_name").as_str()),
                Operation::Scc => m["pass_name"] == "strongly-connected-modules-v1",
                Operation::Reachability => [
                    "static-function-reachability-v1",
                    "static-callgraph-overlay-v1",
                ]
                .contains(&text(m, "pass_name").as_str()),
            })
        })
    };
    let complete_manifest = |m: &Value| {
        m["status"] == "complete"
            && !v.stale
            && strings(&m["coverage_requirements"]).iter().all(|r| {
                r.split_once(':')
                    .is_some_and(|(predicate, _)| query::complete_for(&v, predicate))
            })
    };
    let all_complete = ms.iter().all(complete_manifest);
    if require_complete || plan.is_some() {
        require(
            ms.iter().filter(belongs).all(complete_manifest),
            "COVERAGE",
            "Partial or incomplete requested pass coverage",
        )?;
    }
    let manifest_ids = ms.iter().map(|m| text(m, "id")).collect::<Vec<_>>();
    let mut verified = false;
    if let Some(plan) = plan {
        // The registered operations run on an explicit canonical input bundle,
        // never latest or a mutable graph. Verification reuses existing content
        // if the central ledger already owns the identical semantic artifacts.
        let watermark = l.watermark()?;
        let mut b = v.bundle.clone();
        let mut payloads = l.payloads(&b)?;
        let nodes = plan_nodes(plan, &v.bundle, view)?;
        execute(
            &mut b,
            &mut payloads,
            &knowledge::context(&v.bundle),
            &v.diagnostics["coverage"],
            &nodes,
        )?;
        let old_manifests: BTreeMap<_, _> = ms.iter().map(|m| (text(m, "id"), m)).collect();
        for manifest in rows(&b, "pass_manifests") {
            require(
                old_manifests.get(&text(manifest, "id")).copied() == Some(manifest),
                "PASS_OUTPUT",
                "Pinned recomputation differs from retained deterministic manifest",
            )?;
        }
        let original: BTreeMap<_, _> = rows(&v.bundle, "assertions")
            .iter()
            .filter(|a| a["class"] == "derived")
            .map(|a| (text(a, "id"), a))
            .collect();
        for a in rows(&b, "assertions")
            .iter()
            .filter(|a| a["class"] == "derived")
        {
            require(
                original.get(&text(a, "id")).copied() == Some(a),
                "PASS_OUTPUT",
                "Pinned recomputation differs from retained assertion",
            )?;
        }
        require(
            l.watermark()? == watermark,
            "FRESHNESS",
            "Knowledge changed during pass execution",
        )?;
        super::contracts::validate(&b, &payloads)?;
        verified = true;
    }
    let execution = l.record_execution(&v.id,&json!({"operation":"passes","manifest_ids":manifest_ids,"order":order,"recomputed_and_verified":verified}))?;
    let (reused, recomputed, invalidated) = l.pass_changes(&v.id)?;
    Ok(
        json!({"context":knowledge::context(&v.bundle),"view_id":v.id,"execution_id":execution,"manifests":ms,
        "manifest_ids":manifest_ids,"order":order,"recomputed_and_verified":verified,
        "reused_artifact_ids":reused,"recomputed":recomputed,"invalidated_artifact_ids":invalidated,
        "status":if all_complete {"complete"} else {"partial"},
        "current_eligible":all_complete,
        "limitations":v.diagnostics["limitations"]}),
    )
}
