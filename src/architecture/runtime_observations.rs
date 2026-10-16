//! Bounded OTLP trace ingestion. The OpenTelemetry Collector collects; this
//! adapter retains the exact export, normalizes span events, and publishes
//! runtime Evidence between explicitly identified services. Span names never
//! identify source symbols, and missing observations never establish absence.
use super::{
    contracts::{self, add, date_time, rows, strings, text, Payloads},
    identity, knowledge,
    ledger::{Ledger, Prepared, View},
    passes,
    query,
    require, rust, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_SPANS: usize = 200_000;
const WINDOW_SECONDS: i64 = 300;
const RESOURCE_KEYS: &[&str] = &[
    "service.name",
    "service.namespace",
    "service.instance.id",
    "deployment.environment.name",
    "deployment.environment",
    "k8s.namespace.name",
    "k8s.deployment.name",
    "k8s.pod.uid",
    "container.image.id",
    "container.image.name",
];
const SPAN_KEYS: &[&str] = &[
    "http.request.method",
    "http.response.status_code",
    "http.route",
    "url.path",
    "server.address",
    "server.port",
    "db.system",
    "db.system.name",
    "db.namespace",
    "db.name",
    "peer.service",
    "error.type",
];

pub struct IngestOptions {
    pub environment: String,
    pub interval_start: String,
    pub interval_end: String,
    pub collector: String,
    pub sampling: String,
    pub label: Option<String>,
    /// Locator of the retained export, e.g. its original file name.
    pub source_name: String,
}

fn attribute(v: &Value) -> Option<String> {
    let v = v.as_object()?;
    let (kind, value) = v.iter().next()?;
    match kind.as_str() {
        "stringValue" => value.as_str().map(str::to_owned),
        "intValue" => value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_i64().map(|i| i.to_string())),
        "boolValue" => value.as_bool().map(|b| b.to_string()),
        "doubleValue" => value.as_f64().map(|f| f.to_string()),
        _ => None,
    }
}
fn attributes(list: &Value, keys: &[&str]) -> BTreeMap<String, String> {
    rows(list, "attributes")
        .iter()
        .filter(|a| keys.contains(&a["key"].as_str().unwrap_or("")))
        .filter_map(|a| Some((text(a, "key"), attribute(&a["value"])?)))
        .collect()
}
fn hex_id(v: &Value, len: usize) -> Option<String> {
    let s = v.as_str()?;
    (s.len() == len && s.bytes().all(|c| c.is_ascii_hexdigit()) && s.bytes().any(|c| c != b'0'))
        .then(|| s.to_ascii_lowercase())
}
/// Civil UTC timestamp from Unix nanoseconds, keeping full precision.
pub(crate) fn timestamp(nanos: i128) -> String {
    let secs = nanos.div_euclid(1_000_000_000) as i64;
    let frac = nanos.rem_euclid(1_000_000_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{frac:09}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
/// Unix seconds. The contract parser counts from 0001-01-01T00:00:00Z.
pub(crate) fn seconds(value: &str) -> Result<i64> {
    Ok(date_time(value)?.0 - 62_135_596_800)
}
pub(crate) fn service_key(
    environment: &str,
    resource: &BTreeMap<String, String>,
) -> Option<String> {
    let name = resource.get("service.name")?;
    let namespace = resource
        .get("service.namespace")
        .map_or("-", String::as_str);
    Some(format!("otel:{environment}/{namespace}/{name}"))
}
fn external_key(environment: &str, span: &BTreeMap<String, String>) -> Option<String> {
    let address = span.get("server.address");
    if let Some(system) = span.get("db.system").or_else(|| span.get("db.system.name")) {
        let database = span.get("db.namespace").or_else(|| span.get("db.name"));
        return Some(format!(
            "db:{environment}/{system}/{}/{}",
            address.map_or("-", String::as_str),
            database.map_or("-", String::as_str)
        ));
    }
    let address = address?;
    Some(format!(
        "net:{environment}/{address}:{}",
        span.get("server.port").map_or("-", String::as_str)
    ))
}

/// Normalize one OTLP JSON export (a document or Collector JSON lines).
pub(crate) fn normalize(raw: &[u8], o: &IngestOptions) -> Result<(Vec<Value>, Value)> {
    require(
        raw.len() <= MAX_BYTES,
        "RUNTIME",
        "OTLP export byte limit exceeded",
    )?;
    let text_raw = std::str::from_utf8(raw)?;
    let documents: Vec<Value> = if text_raw.trim_start().starts_with('[') {
        identity::parse(raw)?
            .as_array()
            .cloned()
            .unwrap_or_default()
    } else {
        text_raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| identity::parse(l.as_bytes()))
            .collect::<Result<_>>()?
    };
    let (start, end) = (seconds(&o.interval_start)?, seconds(&o.interval_end)?);
    require(start <= end, "INTERVAL", "Runtime interval reversed")?;
    let mut events = Vec::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut resource_index = 0;
    for doc in &documents {
        for rs in rows(doc, "resourceSpans") {
            let resource = attributes(&rs["resource"], RESOURCE_KEYS);
            let index = resource_index;
            resource_index += 1;
            let declared_env = resource
                .get("deployment.environment.name")
                .or_else(|| resource.get("deployment.environment"));
            for ss in rows(rs, "scopeSpans") {
                for s in rows(ss, "spans") {
                    *counts.entry("spans_seen").or_default() += 1;
                    require(events.len() < MAX_SPANS, "RUNTIME", "Span limit exceeded")?;
                    let (Some(trace), Some(span)) =
                        (hex_id(&s["traceId"], 32), hex_id(&s["spanId"], 16))
                    else {
                        *counts.entry("invalid_spans").or_default() += 1;
                        continue;
                    };
                    let nanos = |k: &str| {
                        s[k].as_str()
                            .and_then(|v| v.parse::<i128>().ok())
                            .or_else(|| s[k].as_i64().map(i128::from))
                    };
                    let (Some(t0), Some(t1)) =
                        (nanos("startTimeUnixNano"), nanos("endTimeUnixNano"))
                    else {
                        *counts.entry("invalid_spans").or_default() += 1;
                        continue;
                    };
                    if t1 < t0 {
                        *counts.entry("invalid_spans").or_default() += 1;
                        continue;
                    }
                    if declared_env.is_some_and(|e| *e != o.environment) {
                        *counts.entry("environment_mismatch").or_default() += 1;
                        continue;
                    }
                    if (t0 / 1_000_000_000) < start as i128 || (t1 / 1_000_000_000) > end as i128 {
                        *counts.entry("outside_interval").or_default() += 1;
                        continue;
                    }
                    let kind = match s["kind"].as_i64().unwrap_or(0) {
                        1 => "internal",
                        2 => "server",
                        3 => "client",
                        4 => "producer",
                        5 => "consumer",
                        _ => "unspecified",
                    };
                    let status = match s["status"]["code"].as_i64().unwrap_or(0) {
                        1 => "ok",
                        2 => "error",
                        _ => "unset",
                    };
                    events.push(json!({"id":format!("span:{trace}/{span}"),"trace_id":trace,"span_id":span,
                        "parent_span_id":hex_id(&s["parentSpanId"], 16),"kind":kind,"name":s["name"],
                        "start":timestamp(t0),"end":timestamp(t1),"start_unix_nano":t0.to_string(),"status":status,
                        "resource_index":index,"resource":resource,"attributes":attributes(s, SPAN_KEYS),
                        "scope":ss["scope"]["name"]}));
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    let before = events.len();
    events.retain(|e| seen.insert(text(e, "id")));
    *counts.entry("duplicate_spans").or_default() += before - events.len();
    Ok((events, json!(counts)))
}

/// Ingest one retained OTLP export for one environment and interval.
pub fn ingest(l: &mut Ledger, raw: &[u8], o: &IngestOptions) -> Result<Value> {
    require(
        !o.environment.is_empty() && !o.collector.is_empty(),
        "RUNTIME",
        "Environment and collector identity are required",
    )?;
    require(
        ["sampled", "complete"].contains(&o.sampling.as_str()),
        "RUNTIME",
        "Sampling must be sampled or complete",
    )?;
    let repository = l.repository()?;
    let (events, counts) = normalize(raw, o)?;
    let generation = l.begin_scope(&repository, o.label.as_deref(), "runtime")?;
    let result = (|| {
        let prepared = produce(raw, events, counts, o, generation)?;
        let view = l.commit(generation, &prepared)?;
        let projections = l.publish(&view, false)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(json!({"view_id":view,"scope":"runtime",
            "context":knowledge::context(&prepared.bundle),"projections":projections,"diagnostics":prepared.diagnostics,
            "assertion_count":rows(&prepared.bundle,"assertions").len(),"status":"published"}))
    })();
    if let Err(e) = &result {
        l.fail(generation, &e.to_string())?;
    }
    result
}

fn produce(
    raw: &[u8],
    events: Vec<Value>,
    counts: Value,
    o: &IngestOptions,
    generation: u64,
) -> Result<Prepared> {
    let env = &o.environment;
    let mut b = contracts::empty_bundle();
    let mut payloads = Payloads::new();
    let raw_digest = identity::bytes_digest(raw);
    let safe_name: String = o
        .source_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || ".-_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let raw_path = format!("runtime/{env}/{}-{safe_name}", &raw_digest[7..19]);
    let raw_id = identity::id(
        "artifact",
        &json!({"path":raw_path,"digest":raw_digest,"source_kind":"runtime"}),
    )?;
    add(
        &mut b,
        "artifacts",
        json!({"id":raw_id,"path":raw_path,"digest":raw_digest,"source_kind":"runtime","availability":"available"}),
    );
    payloads.insert(raw_id.clone(), raw.to_vec());
    let image_digests: BTreeSet<String> = events
        .iter()
        .filter_map(|e| e["resource"]["container.image.id"].as_str())
        .filter_map(|id| {
            id.rsplit('@')
                .next()
                .filter(|d| d.len() == 71 && d.starts_with("sha256:"))
                .map(str::to_owned)
        })
        .collect();
    let mut runtime = json!({"environment":env,"start":o.interval_start,"end":o.interval_end,"sampling":o.sampling,
        "mapping_quality":"service","collector_id":o.collector,"artifact_digests":image_digests,
        // Several workloads share one interval: the context pins the digest of their observed image set.
        "deployed_artifact_digest":identity::digest(&json!(image_digests))?});
    let rid = identity::id(
        "runtime",
        &json!({"context":runtime,"observation_digest":raw_digest}),
    )?;
    runtime["id"] = json!(rid);
    add(&mut b, "runtime_contexts", runtime);
    let ctx = json!({"kind":"runtime","runtime_context_id":rid});
    let normalized = json!({"schema_version":"1.0","normalization":"briefcase-otlp-span-events-v1","environment":env,
        "interval":{"start":o.interval_start,"end":o.interval_end},"collector_id":o.collector,"sampling":o.sampling,
        "source_artifact_id":raw_id,"source_digest":raw_digest,"counts":counts,"events":events});
    let normalized_bytes = identity::canonical(&normalized)?;
    let events_digest = identity::bytes_digest(&normalized_bytes);
    let events_path = format!(".briefcase/runtime/{}.json", &events_digest[7..]);
    let events_id = identity::id(
        "artifact",
        &json!({"path":events_path,"digest":events_digest,"source_kind":"runtime"}),
    )?;
    add(
        &mut b,
        "artifacts",
        json!({"id":events_id,"path":events_path,"digest":events_digest,"source_kind":"runtime","availability":"available"}),
    );
    payloads.insert(events_id.clone(), normalized_bytes);
    let implementation = identity::bytes_digest(include_bytes!("runtime_observations.rs"));
    let settings = json!({"environment":env,"collector":o.collector,"sampling":o.sampling,"transport":"otlp-json","window_seconds":WINDOW_SECONDS});
    let mut config = json!({"producer_name":"briefcase-otlp-ingest-v1","implementation_digest":implementation,"settings_digest":identity::digest(&settings)?});
    let config_id = identity::id("analysis", &config)?;
    config["id"] = json!(config_id);
    add(&mut b, "analysis_configs", config);
    let provider = identity::id(
        "identity-provider",
        &json!({"mapping":"otel-resource-identity-v1","implementation_digest":implementation}),
    )?;
    let event_ref = |b: &mut Value, id: &str| -> Result<String> {
        let locator = json!({"kind":"event","event_id":id});
        let er = identity::id(
            "evidence",
            &json!({"artifact_id":events_id,"locator":locator}),
        )?;
        add(
            b,
            "evidence_refs",
            json!({"id":er,"artifact_id":events_id,"locator":locator}),
        );
        Ok(er)
    };
    let by_span: BTreeMap<(String, String), &Value> = events
        .iter()
        .map(|e| ((text(e, "trace_id"), text(e, "span_id")), e))
        .collect();
    let mut services = BTreeMap::new();
    let mut unresolved_spans = Vec::new();
    for e in &events {
        let resource: BTreeMap<String, String> = serde_json::from_value(e["resource"].clone())?;
        match service_key(env, &resource) {
            Some(key) => {
                if let std::collections::btree_map::Entry::Vacant(slot) = services.entry(key) {
                    let id = rust::entity_for_language(
                        &mut b,
                        &ctx,
                        "service",
                        slot.key(),
                        &provider,
                        "observed",
                        "otel",
                    )?;
                    slot.insert(id);
                }
            }
            None => unresolved_spans.push(e.clone()),
        }
    }
    let service_of = |e: &Value| -> Option<String> {
        let resource: BTreeMap<String, String> =
            serde_json::from_value(e["resource"].clone()).ok()?;
        services.get(&service_key(env, &resource)?).cloned()
    };
    let mut calls = Vec::new();
    let mut connections = Vec::new();
    let mut unsupported = Vec::new();
    let mut counted: BTreeMap<&str, usize> = BTreeMap::new();
    let children: BTreeMap<(String, String), Vec<&Value>> = events
        .iter()
        .filter(|e| e["kind"] == "server" && !e["parent_span_id"].is_null())
        .fold(BTreeMap::new(), |mut m, e| {
            m.entry((text(e, "trace_id"), text(e, "parent_span_id")))
                .or_insert_with(Vec::new)
                .push(e);
            m
        });
    for e in &events {
        match e["kind"].as_str().unwrap_or("") {
            "client" => {
                let Some(caller) = service_of(e) else {
                    continue;
                };
                let servers: Vec<_> = children
                    .get(&(text(e, "trace_id"), text(e, "span_id")))
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|s| service_of(s).map(|svc| (s, svc)))
                    .collect();
                if let Some((server, callee)) = servers.first() {
                    calls.push((
                        caller,
                        callee.clone(),
                        vec![text(e, "id"), text(server, "id")],
                    ));
                    continue;
                }
                let span: BTreeMap<String, String> =
                    serde_json::from_value(e["attributes"].clone())?;
                match external_key(env, &span) {
                    Some(key) => connections.push((caller, key, vec![text(e, "id")])),
                    None => unsupported.push((caller, text(e, "id"))),
                }
            }
            "server" => {
                let parent = e["parent_span_id"]
                    .as_str()
                    .map(|p| (text(e, "trace_id"), p.to_owned()));
                if parent
                    .as_ref()
                    .and_then(|p| by_span.get(p))
                    .is_none_or(|p| p["kind"] != "client")
                {
                    *counted.entry("uncorrelated_server_spans").or_default() += 1;
                }
            }
            "producer" | "consumer" => {
                if let Some(svc) = service_of(e) {
                    unsupported.push((svc, text(e, "id")));
                }
            }
            _ => *counted.entry("non_communication_spans").or_default() += 1,
        }
    }
    let attempted = calls.len() + connections.len() + unsupported.len() + unresolved_spans.len();
    let coverage = json!({"attempted":attempted,"completed":attempted,"unsupported_count":unsupported.len(),
        "unresolved_count":unresolved_spans.len(),"resolution_complete":false,
        "guarantees":[{"predicate":"CALLS_OBSERVED","kind":"observed_sample"},{"predicate":"CONNECTION_OBSERVED","kind":"observed_sample"}]});
    let run_id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config_id,"coverage":coverage}),
    )?;
    add(
        &mut b,
        "runs",
        json!({"id":run_id,"producer_kind":"runtime","analysis_config_id":config_id,"context":ctx,"status":"complete","coverage":coverage}),
    );
    for (caller, callee, witnesses) in calls {
        let refs = witnesses
            .iter()
            .map(|w| event_ref(&mut b, w))
            .collect::<Result<Vec<_>>>()?;
        rust::assertion(
            &mut b,
            &ctx,
            "evidence",
            &caller,
            "CALLS_OBSERVED",
            knowledge::target(&callee),
            json!({"run_id":run_id}),
            refs,
            None,
        )?;
    }
    for (caller, key, witnesses) in connections {
        let target = rust::entity_for_language(
            &mut b,
            &ctx,
            "external_system",
            &key,
            &provider,
            "observed",
            "otel",
        )?;
        let refs = witnesses
            .iter()
            .map(|w| event_ref(&mut b, w))
            .collect::<Result<Vec<_>>>()?;
        rust::assertion(
            &mut b,
            &ctx,
            "evidence",
            &caller,
            "CONNECTION_OBSERVED",
            knowledge::target(&target),
            json!({"run_id":run_id}),
            refs,
            None,
        )?;
    }
    for (service, span) in &unsupported {
        let er = event_ref(&mut b, span)?;
        let mut u = json!({"source_entity_id":service,"context":ctx,"evidence_ref_id":er,"predicate":"CALLS_OBSERVED",
            "status":"unsupported","reason":"unsupported_span","candidate_ids":[]});
        u["id"] = json!(identity::id("unresolved", &u)?);
        add(&mut b, "unresolved_references", u);
    }
    let mut limitations = vec![format!("Observed sample for environment {env}, interval {} .. {}, collector {}. No matching observation is not evidence of absence.", o.interval_start, o.interval_end, o.collector)];
    if !unresolved_spans.is_empty() {
        limitations.push(format!(
            "{} spans lack service.name and remain unattributed.",
            unresolved_spans.len()
        ));
    }
    passes::produce_runtime_aggregates(
        &mut b,
        &mut payloads,
        &ctx,
        &coverage,
        &events_id,
        &events,
        WINDOW_SECONDS,
    )?;
    let mut all_counts = counts.clone();
    for (k, v) in counted {
        all_counts[k] = json!(v);
    }
    let diagnostics = json!({"environment":env,"interval":{"start":o.interval_start,"end":o.interval_end},"collector_id":o.collector,
        "sampling":o.sampling,"runtime_context_id":rid,"raw_artifact_id":raw_id,"events_artifact_id":events_id,
        "coverage":{"runtime_observation_interval_complete":false,"observed":coverage,"counts":all_counts,
            "unattributed_spans":unresolved_spans.len(),"unsupported_span_forms":unsupported.len(),
            "runtime_resource_identity_complete":unresolved_spans.is_empty()},
        "limitations":limitations});
    Prepared::seal(b, payloads, generation, diagnostics)
}

// ---- Runtime identity mapping (query time) -----------------------------------

/// How one observed runtime service joins desired identity in a pinned
/// deployment publication. Joins use explicit attributes only.
pub(crate) fn mappings(
    l: &Ledger,
    runtime: &View,
    desired: Option<&View>,
) -> Result<BTreeMap<String, Value>> {
    let ctx = knowledge::context(&runtime.bundle);
    let rc = knowledge::row(
        &runtime.bundle,
        "runtime_contexts",
        &text(&ctx, "runtime_context_id"),
    )?;
    let env = text(rc, "environment");
    let events = runtime_events(l, runtime)?;
    let mut resources: BTreeMap<String, Vec<BTreeMap<String, String>>> = BTreeMap::new();
    for e in events.values() {
        let resource: BTreeMap<String, String> = serde_json::from_value(e["resource"].clone())?;
        if let Some(key) = service_key(&env, &resource) {
            let list = resources.entry(key).or_default();
            if !list.contains(&resource) {
                list.push(resource);
            }
        }
    }
    let mut out = BTreeMap::new();
    let desired_index = desired.map(|d| DesiredIndex::new(l, d)).transpose()?;
    for e in rows(&runtime.bundle, "entity_versions")
        .iter()
        .filter(|e| e["kind"] == "service" && e["context"] == ctx)
    {
        let key = text(e, "canonical_key");
        let attrs = resources.get(&key).cloned().unwrap_or_default();
        let detail = match &desired_index {
            None => {
                json!({"status":"not_evaluated","limitation":"No deployment publication was pinned for identity mapping."})
            }
            Some(index) if index.environment != env => {
                json!({"status":"unmapped","reason":"environment_mismatch",
                "runtime_environment":env,"desired_environment":index.environment})
            }
            Some(index) => index.map(&attrs),
        };
        let mut detail = detail;
        detail["runtime_service"] = json!(key);
        detail["resource_attributes"] = json!(attrs);
        out.insert(text(e, "id"), detail);
    }
    Ok(out)
}
pub(crate) fn runtime_events(l: &Ledger, v: &View) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for a in rows(&v.bundle, "artifacts").iter().filter(|a| {
        a["source_kind"] == "runtime" && text(a, "path").starts_with(".briefcase/runtime/")
    }) {
        let payload = l.payloads_for(&[text(a, "id")])?;
        let doc = identity::parse(&payload[&text(a, "id")])?;
        for e in rows(&doc, "events") {
            out.insert(text(e, "id"), e.clone());
        }
    }
    Ok(out)
}
struct DesiredIndex {
    environment: String,
    view: String,
    keys: BTreeMap<String, String>,
    artifacts: BTreeMap<String, Vec<String>>,
    deploys: BTreeMap<String, Vec<(String, Vec<String>)>>,
    ambiguous: BTreeSet<String>,
}
impl DesiredIndex {
    fn new(l: &Ledger, d: &View) -> Result<Self> {
        let ctx = knowledge::context(&d.bundle);
        let dc = knowledge::row(
            &d.bundle,
            "deployment_contexts",
            &text(&ctx, "deployment_context_id"),
        )?;
        let removed = l.retracted()?;
        let keys = query::entities(d);
        let payloads = l.payloads(&d.bundle)?;
        // OCI manifest digest and the config digest it declares both identify
        // the artifact; anything else is not an image identity.
        let mut artifacts: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in rows(&d.bundle, "entity_versions")
            .iter()
            .filter(|e| e["kind"] == "artifact")
        {
            let key = text(e, "canonical_key");
            if let Some(digest) = key.rsplit('@').next() {
                artifacts
                    .entry(digest.to_owned())
                    .or_default()
                    .push(text(e, "id"));
                for a in rows(&d.bundle, "artifacts")
                    .iter()
                    .filter(|a| a["digest"] == digest)
                {
                    if let Some(manifest) = payloads
                        .get(&text(a, "id"))
                        .and_then(|b| identity::parse(b).ok())
                    {
                        if let Some(config) = manifest["config"]["digest"].as_str() {
                            artifacts
                                .entry(config.to_owned())
                                .or_default()
                                .push(text(e, "id"));
                        }
                    }
                }
            }
        }
        let mut deploys: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
        for a in rows(&d.bundle, "assertions").iter().filter(|a| {
            a["proposition"]["predicate"] == "DEPLOYS_AS"
                && d.eligible.contains(&text(a, "id"))
                && !removed.contains(&text(a, "id"))
        }) {
            let from = text(&a["proposition"], "subject_id");
            let to = text(&a["proposition"]["object"], "entity_version_id");
            let list = deploys.entry(from).or_default();
            match list.iter_mut().find(|(t, _)| *t == to) {
                Some((_, ids)) => ids.push(text(a, "id")),
                None => list.push((to, vec![text(a, "id")])),
            }
        }
        let ambiguous = rows(&d.bundle, "unresolved_references")
            .iter()
            .filter(|u| u["reason"] == "ambiguous_selector")
            .flat_map(|u| strings(&u["candidate_ids"]))
            .collect();
        Ok(Self {
            environment: text(dc, "environment"),
            view: d.id.clone(),
            keys,
            artifacts,
            deploys,
            ambiguous,
        })
    }
    fn next(&self, from: &str, kind_prefix: &str) -> Vec<(String, Vec<String>)> {
        self.deploys
            .get(from)
            .map(|xs| {
                xs.iter()
                    .filter(|(t, _)| self.keys[t].starts_with(kind_prefix))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    fn map(&self, resources: &[BTreeMap<String, String>]) -> Value {
        let distinct = |k: &str| {
            resources
                .iter()
                .filter_map(|r| r.get(k).cloned())
                .collect::<BTreeSet<_>>()
        };
        let images = distinct("container.image.id");
        let namespaces = distinct("k8s.namespace.name");
        let names = distinct("k8s.deployment.name");
        let mut used = Vec::new();
        let mut support: Vec<String> = Vec::new();
        let mut deployments: BTreeSet<String> = BTreeSet::new();
        let mut method = "";
        if !images.is_empty() {
            used.push("container.image.id");
            for image in &images {
                let digest = image.rsplit('@').next().unwrap_or(image);
                for artifact in self.artifacts.get(digest).into_iter().flatten() {
                    for (deployment, ids) in self.next(artifact, "kubernetes:") {
                        deployments.insert(deployment);
                        support.extend(ids);
                    }
                }
            }
            method = "image_digest";
        }
        if namespaces.len() == 1 && names.len() == 1 {
            used.push("k8s.namespace.name");
            used.push("k8s.deployment.name");
            let key = super::deployment::deployment_key(
                &self.environment,
                namespaces.first().unwrap(),
                names.first().unwrap(),
            );
            let explicit: BTreeSet<_> = self
                .keys
                .iter()
                .filter(|(_, k)| **k == key)
                .map(|(id, _)| id.clone())
                .collect();
            if method.is_empty() {
                deployments = explicit;
                method = "k8s_attributes";
            } else if !explicit.is_empty() && !deployments.is_empty() && explicit != deployments {
                return json!({"status":"unresolved","reason":"conflicting_identity_attributes","attributes_used":used,"desired_view":self.view});
            } else if deployments.is_empty() {
                deployments = explicit;
                method = "k8s_attributes";
            }
        }
        if method.is_empty() || deployments.is_empty() {
            // service.name alone is a name, not an identity join.
            let names = distinct("service.name");
            let candidates: Vec<_> = self
                .keys
                .values()
                .filter(|k| {
                    (k.starts_with("service:") || k.starts_with("kubernetes:"))
                        && names.iter().any(|n| k.rsplit('/').next() == Some(n))
                })
                .cloned()
                .collect();
            return json!({"status":"unmapped","reason":if used.is_empty() {"insufficient_identity_attributes"} else {"no_known_artifact_or_deployment"},
                "attributes_used":used,"name_only_candidates":candidates,"desired_view":self.view,
                "limitation":"Runtime service retained; deployment mapping unresolved. service.name alone never selects a Deployment."});
        }
        if deployments.len() > 1 {
            return json!({"status":"ambiguous","method":method,"attributes_used":used,"desired_view":self.view,
                "candidates":deployments.iter().map(|d| &self.keys[d]).collect::<Vec<_>>()});
        }
        let deployment = deployments.into_iter().next().unwrap();
        let services = self.next(&deployment, "service:");
        for (_, ids) in &services {
            support.extend(ids.clone());
        }
        support.sort();
        support.dedup();
        let (status, service) = match services.as_slice() {
            [(s, _)] if !self.ambiguous.contains(&deployment) => {
                ("exact", Some(self.keys[s].clone()))
            }
            [] => ("deployment_only", None),
            _ => ("ambiguous", None),
        };
        json!({"status":status,"method":method,"attributes_used":used,"desired_view":self.view,
            "deployment":self.keys[&deployment],"service":service,
            "service_candidates":services.iter().map(|(s, _)| &self.keys[s]).collect::<Vec<_>>(),
            "desired_assertion_ids":support})
    }
}
