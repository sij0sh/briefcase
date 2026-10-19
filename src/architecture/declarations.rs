//! Architecture declarations: the retained authority artifact
//! (`.briefcase-input/architecture.json`) materialized as Declaration assertions.
//!
//! Language adapters call [`apply`] once their technical entities exist. It
//! owns the domain assignments and report-only rules of the original policy
//! and the declared semantic catalog of ADR-0028: features, workflows, their
//! hierarchy, human wording and technical anchors. Semantic entities never
//! carry an analyzer language, and a technical reference that does not
//! resolve exactly is a limitation, never an invented entity.
use super::{
    capture::Capture,
    contracts::{self, add, rows, text},
    identity,
    ledger::{Ledger, Prepared},
    query, require,
    rust::{assertion, entity_for_language},
    Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Retained path of the declaration artifact inside a capture.
pub const ARTIFACT_PATH: &str = ".briefcase-input/architecture.json";
/// Entity kinds that name declared meaning rather than source structure.
pub const SEMANTIC_KINDS: [&str; 5] = ["application", "domain", "feature", "workflow", "team"];

pub(crate) fn feature_key(id: &str) -> String {
    format!("feature:{id}")
}
pub(crate) fn workflow_key(id: &str) -> String {
    format!("workflow:{id}")
}
pub(crate) fn application_key(id: &str) -> String {
    format!("application:{id}")
}
/// Declared predicates that bind technical code to a semantic entity; each
/// is reconstructed from a resolved binding resolution row.
pub const BINDING_PREDICATES: [&str; 3] =
    ["IMPLEMENTS_FEATURE", "ENTRYPOINT_FOR", "PARTICIPATES_IN"];

/// A semantic entity: declared identity keyed by a stable ID, never by its
/// display name, and never in an analyzer language.
fn semantic_entity(
    b: &mut Value,
    ctx: &Value,
    kind: &str,
    key: &str,
    provider: &str,
) -> Result<String> {
    require(
        SEMANTIC_KINDS.contains(&kind),
        "SEMANTIC_KIND",
        "Not a semantic entity kind",
    )?;
    entity_for_language(b, ctx, kind, key, provider, "declared", "none")
}

fn target(id: &str) -> Value {
    json!({"kind":"entity","entity_version_id":id})
}
fn literal(value: &str) -> Value {
    json!({"kind":"literal","value":value})
}

/// Exact technical reference resolution over the entities of one context
/// already in the bundle: an exact canonical key, or for a symbol the key
/// without its `#offset` suffix. Returns every candidate; one is a binding,
/// several are ambiguous and none is unresolved. Entities of another context
/// (a deployment service named like a source one) are never candidates.
fn resolve(b: &Value, ctx: &Value, kind: &str, key: &str) -> Vec<String> {
    let of_kind: Vec<&Value> = rows(b, "entity_versions")
        .iter()
        .filter(|e| e["kind"] == kind && e["context"] == *ctx)
        .collect();
    let exact: BTreeSet<String> = of_kind
        .iter()
        .filter(|e| e["canonical_key"] == key)
        .map(|e| text(e, "id"))
        .collect();
    if !exact.is_empty() || kind != "symbol" || key.contains('#') {
        return exact.into_iter().collect();
    }
    of_kind
        .iter()
        .filter(|e| {
            e["canonical_key"]
                .as_str()
                .and_then(|k| k.rsplit_once('#'))
                .is_some_and(|(stem, _)| stem == key)
        })
        .map(|e| text(e, "id"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Structural and cross-reference validation of the whole declaration
/// document. Errors here reject the capture: they are malformed intent, not
/// incomplete analysis.
pub fn validate(policy: &Value) -> Result<()> {
    contracts::schema(policy, "architecture-declaration", None)
        .map_err(|e| format!("POLICY: Invalid architecture declaration: {e}"))?;
    let features = policy["features"].as_array().cloned().unwrap_or_default();
    let workflows = policy["workflows"].as_array().cloned().unwrap_or_default();
    let mut parents = BTreeMap::new();
    for f in &features {
        require(
            parents
                .insert(text(f, "id"), f["parent"].as_str().map(str::to_owned))
                .is_none(),
            "POLICY",
            "Duplicate feature ID",
        )?;
    }
    for (id, parent) in &parents {
        if let Some(parent) = parent {
            require(parent != id, "POLICY", "A feature cannot be its own parent")?;
            require(
                parents.contains_key(parent),
                "POLICY",
                &format!("Feature {id} names unknown parent {parent}"),
            )?;
        }
        // One parent per feature makes the hierarchy a forest exactly when no
        // parent chain revisits a feature.
        let mut seen = BTreeSet::from([id.clone()]);
        let mut next = parent.clone();
        while let Some(p) = next {
            require(
                seen.insert(p.clone()),
                "POLICY",
                &format!("Feature hierarchy cycle through {id}"),
            )?;
            next = parents.get(&p).cloned().flatten();
        }
    }
    let mut workflow_ids = BTreeSet::new();
    for w in &workflows {
        let id = text(w, "id");
        require(
            workflow_ids.insert(id.clone()),
            "POLICY",
            "Duplicate workflow ID",
        )?;
        for f in contracts::strings(&w["features"]) {
            require(
                parents.contains_key(&f),
                "POLICY",
                &format!("Workflow {id} realizes unknown feature {f}"),
            )?;
        }
        require(
            w["entrypoints"].as_array().is_some_and(|a| !a.is_empty())
                || w["anchor_pending"] == true,
            "POLICY",
            &format!("Workflow {id} needs an entrypoint or anchor_pending"),
        )?;
        for r in contracts::rows(w, "entrypoints") {
            require(
                ["endpoint", "symbol"].contains(&r["kind"].as_str().unwrap_or("")),
                "POLICY",
                &format!("Workflow {id} entrypoints must be endpoints or symbols"),
            )?;
        }
    }
    let mut exceptions = BTreeSet::new();
    for e in rows(policy, "convention_exceptions") {
        require(
            exceptions.insert(text(e, "id")),
            "POLICY",
            "Duplicate convention exception ID",
        )?;
    }
    let mut implementations = BTreeSet::new();
    for i in policy["feature_implementations"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let feature = text(i, "feature");
        require(
            parents.contains_key(&feature),
            "POLICY",
            &format!("Implementation names unknown feature {feature}"),
        )?;
        require(
            implementations.insert(identity::canonical(i)?),
            "POLICY",
            "Duplicate feature implementation mapping",
        )?;
    }
    Ok(())
}

/// Materialize the retained declaration artifact, when the capture has one.
pub(crate) fn apply(
    b: &mut Value,
    c: &Capture,
    ctx: &Value,
    provider: &str,
    limitations: &mut Vec<String>,
) -> Result<()> {
    let Some(bytes) = c.files.get(ARTIFACT_PATH) else {
        return Ok(());
    };
    let policy = identity::parse(bytes)?;
    validate(&policy)?;
    let aid = text(
        rows(b, "artifacts")
            .iter()
            .find(|r| r["path"] == ARTIFACT_PATH)
            .ok_or("POLICY: Declaration artifact is not retained")?,
        "id",
    );
    let authority = identity::id(
        "authority",
        &json!({"context":ctx,"name":policy["authority"]}),
    )?;
    add(
        b,
        "authorities",
        json!({"id":authority,"context":ctx,"precedence":1}),
    );
    let mut d = json!({"authority_id":authority,"artifact_id":aid,"context":ctx,"revision":policy["revision"],"effective_from":"1970-01-01T00:00:00Z"});
    let did = identity::id("declaration", &d)?;
    d["id"] = json!(did);
    add(b, "declaration_records", d);
    let origin = json!({"declaration_record_id":did});
    let declare = |b: &mut Value, subject: &str, predicate: &str, object: Value| {
        assertion(
            b,
            ctx,
            "declaration",
            subject,
            predicate,
            object,
            origin.clone(),
            vec![],
            None,
        )
    };
    let modules: BTreeMap<_, _> = rows(b, "entity_versions")
        .iter()
        .filter(|r| r["kind"] == "module")
        .map(|r| (text(r, "canonical_key"), text(r, "id")))
        .collect();
    if let Some(assignments) = policy["assignments"].as_object() {
        for (module, label) in assignments {
            let Some(subject) = modules.get(module) else {
                limitations.push(format!("unresolved declared module: {module}"));
                continue;
            };
            let domain = semantic_entity(b, ctx, "domain", label.as_str().unwrap(), provider)?;
            declare(b, subject, "BELONGS_TO_DOMAIN", target(&domain))?;
        }
    }
    for rule in rows(&policy, "rules") {
        let from = text(rule, "from");
        let to = text(rule, "to");
        if let (Some(subject), Some(object)) = (modules.get(&from), modules.get(&to)) {
            declare(b, subject, "FORBIDS_DEPENDENCY", target(object))?;
        } else {
            limitations.push(format!("unresolved rule {}: {from} -> {to}", rule["id"]));
        }
    }
    // Approved exceptions to observed conventions stay declarations; the
    // reason remains in the retained declaration artifact.
    for exception in rows(&policy, "convention_exceptions") {
        let module = text(exception, "module");
        let to = text(exception, "target");
        if let (Some(subject), Some(object)) = (modules.get(&module), modules.get(&to)) {
            declare(b, subject, "EXEMPT_FROM_CONVENTION", target(object))?;
        } else {
            limitations.push(format!("unresolved convention exception {}: {module} -> {to}", exception["id"]));
        }
    }
    let binder = Binder {
        ctx: ctx.clone(),
        record_id: did,
        stage: "scan",
    };
    semantic_catalog(b, &policy, ctx, provider, &declare, &binder, limitations)
}

type Declare<'a> = dyn Fn(&mut Value, &str, &str, Value) -> Result<String> + 'a;

fn wording(b: &mut Value, subject: &str, item: &Value, declare: &Declare) -> Result<()> {
    declare(b, subject, "DISPLAY_NAME", literal(&text(item, "name")))?;
    if let Some(description) = item["description"].as_str() {
        declare(b, subject, "DESCRIPTION", literal(description))?;
    }
    for alias in contracts::strings(&item["aliases"]) {
        declare(b, subject, "ALIAS", literal(&alias))?;
    }
    Ok(())
}

fn semantic_catalog(
    b: &mut Value,
    policy: &Value,
    ctx: &Value,
    provider: &str,
    declare: &Declare,
    binder: &Binder,
    limitations: &mut Vec<String>,
) -> Result<()> {
    if policy["application"].is_object() {
        let app = &policy["application"];
        let subject = semantic_entity(
            b,
            ctx,
            "application",
            &application_key(&text(app, "id")),
            provider,
        )?;
        declare(b, &subject, "DISPLAY_NAME", literal(&text(app, "name")))?;
        if let Some(description) = app["description"].as_str() {
            declare(b, &subject, "DESCRIPTION", literal(description))?;
        }
    }
    let mut features = BTreeMap::new();
    for f in rows(policy, "features") {
        let id = text(f, "id");
        let entity = semantic_entity(b, ctx, "feature", &feature_key(&id), provider)?;
        features.insert(id, entity);
    }
    for f in rows(policy, "features") {
        let subject = features[&text(f, "id")].clone();
        wording(b, &subject, f, declare)?;
        if let Some(parent) = f["parent"].as_str() {
            declare(b, &subject, "SUBFEATURE_OF", target(&features[parent]))?;
        }
        if let Some(label) = f["domain"].as_str() {
            let domain = semantic_entity(b, ctx, "domain", label, provider)?;
            declare(b, &subject, "BELONGS_TO_DOMAIN", target(&domain))?;
        }
    }
    for w in rows(policy, "workflows") {
        let key = workflow_key(&text(w, "id"));
        let subject = semantic_entity(b, ctx, "workflow", &key, provider)?;
        wording(b, &subject, w, declare)?;
        for f in contracts::strings(&w["features"]) {
            declare(b, &subject, "REALIZES_FEATURE", target(&features[&f]))?;
        }
        if let Some(label) = w["domain"].as_str() {
            let domain = semantic_entity(b, ctx, "domain", label, provider)?;
            declare(b, &subject, "BELONGS_TO_DOMAIN", target(&domain))?;
        }
        if w["anchor_pending"] == true {
            limitations.push(format!(
                "workflow {key} is declared with its entrypoint pending"
            ));
        }
    }
    for request in requests(policy) {
        let owner = rows(b, "entity_versions")
            .iter()
            .find(|e| e["canonical_key"] == request.owner_key && e["context"] == *ctx)
            .map(|e| text(e, "id"))
            .ok_or("POLICY: Declared binding owner was not materialized")?;
        if let Some(note) = binder.bind(b, declare, &request, &owner)? {
            limitations.push(note);
        }
    }
    Ok(())
}

/// One declared technical reference of the retained declaration artifact.
#[derive(Clone, Debug)]
struct Request {
    owner_key: String,
    predicate: &'static str,
    kind: String,
    key: String,
}
/// Every technical reference the declaration makes, in document order.
fn requests(policy: &Value) -> Vec<Request> {
    let mut out = vec![];
    for w in rows(policy, "workflows") {
        let owner_key = workflow_key(&text(w, "id"));
        for (field, predicate) in [
            ("entrypoints", "ENTRYPOINT_FOR"),
            ("participants", "PARTICIPATES_IN"),
        ] {
            for r in rows(w, field) {
                out.push(Request {
                    owner_key: owner_key.clone(),
                    predicate,
                    kind: text(r, "kind"),
                    key: text(r, "key"),
                });
            }
        }
    }
    for i in rows(policy, "feature_implementations") {
        out.push(Request {
            owner_key: feature_key(&text(i, "feature")),
            predicate: "IMPLEMENTS_FEATURE",
            kind: text(&i["subject"], "kind"),
            key: text(&i["subject"], "key"),
        });
    }
    out
}

/// Binds declared technical references for one declaration record and
/// records every attempt as a `semantic_binding_resolutions` row. Only a
/// resolved attempt produces a Declaration assertion; technical bindings are
/// snapshot-specific, so one that does not resolve leaves the semantic
/// definition intact and is disclosed instead.
struct Binder {
    ctx: Value,
    record_id: String,
    stage: &'static str,
}
impl Binder {
    fn binding_key(&self, request: &Request, owner: &str) -> Result<String> {
        identity::id(
            "semantic-binding",
            &json!({"declaration_record_id":self.record_id,"owner_entity_id":owner,"predicate":request.predicate,
                "requested_kind":request.kind,"requested_key":request.key}),
        )
    }
    /// Attempt one binding; returns the limitation text when it did not bind.
    fn bind(
        &self,
        b: &mut Value,
        declare: &Declare,
        request: &Request,
        owner: &str,
    ) -> Result<Option<String>> {
        let candidates = resolve(b, &self.ctx, &request.kind, &request.key);
        let later = ["endpoint", "service"].contains(&request.kind.as_str());
        let (status, reason, assertion_id) = match candidates.len() {
            1 => (
                "resolved",
                "exactly one technical entity of the declaration's context has this key".to_owned(),
                Some(declare(b, &candidates[0], request.predicate, target(owner))?),
            ),
            0 if later && self.stage == "scan" => (
                "deferred",
                format!("{} identities are created by later source analyses; bind-declarations retries after them", request.kind),
                None,
            ),
            0 if request.kind == "service" => (
                "unresolved",
                "no service of the declaration's source context has this key; services of a deployment context are never bound to a source declaration".to_owned(),
                None,
            ),
            0 => (
                "unresolved",
                format!("no {} of the declaration's context has this key", request.kind),
                None,
            ),
            n => (
                "ambiguous",
                format!("{n} entities match; declare the full canonical key"),
                None,
            ),
        };
        let mut row = json!({"context":self.ctx,"binding_key":self.binding_key(request, owner)?,
            "declaration_record_id":self.record_id,"owner_entity_id":owner,"predicate":request.predicate,
            "requested_kind":request.kind,"requested_key":request.key,"status":status,
            "candidate_ids":if status == "resolved" || status == "ambiguous" {candidates.clone()} else {vec![]},
            "assertion_id":assertion_id,"reason":reason,"resolution_stage":self.stage});
        row["id"] = json!(identity::id("semantic-binding-resolution", &row)?);
        add(b, "semantic_binding_resolutions", row);
        let (owner_key, kind, key, predicate) = (
            &request.owner_key,
            &request.kind,
            &request.key,
            request.predicate,
        );
        Ok(match status {
            "resolved" => None,
            "deferred" => Some(format!(
                "deferred {predicate} reference for {owner_key}: {kind} {key} ({reason})"
            )),
            "ambiguous" => Some(format!(
                "ambiguous {predicate} reference for {owner_key}: {kind} {key} matches {} entities; use the full canonical key",
                candidates.len()
            )),
            _ => Some(format!(
                "unresolved {predicate} reference for {owner_key}: {kind} {key}{}",
                if later {
                    " (declare the handler symbol or module, or run bind-declarations after the analysis that creates it)"
                } else {
                    ""
                }
            )),
        })
    }
}

/// Latest binding resolution per binding key: a resolved row wins, then a
/// post-overlay attempt over the scan attempt.
pub(crate) fn current_resolutions(b: &Value) -> BTreeMap<String, Value> {
    let rank = |r: &Value| {
        (
            r["status"] == "resolved",
            r["resolution_stage"] == "post_overlay",
        )
    };
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    for r in rows(b, "semantic_binding_resolutions") {
        let key = text(r, "binding_key");
        if out.get(&key).is_none_or(|prior| rank(r) > rank(prior)) {
            out.insert(key, r.clone());
        }
    }
    out
}

/// Post-overlay semantic binding (ADR-0029): re-read each retained
/// declaration artifact of the current source view and retry its technical
/// references that did not bind at scan, against every entity the view now
/// has (for example endpoints added by `analyze-data`), in the declaration's
/// own context only. A reference that now resolves becomes the Declaration
/// the authority always made; analysis never becomes authority, and nothing
/// but this resolver writes these Declarations. Unchanged inputs reuse the
/// overlay.
pub fn rebind(l: &mut Ledger, o: &query::Options) -> Result<Value> {
    require(
        !o.historical,
        "POLICY",
        "Declaration binding needs the current source view",
    )?;
    let mut strict = o.clone();
    strict.allow_partial = false;
    let v = query::loaded(l, &strict)?;
    require(
        v.scope == "source",
        "SCOPE",
        "Declarations bind in a source view",
    )?;
    let records: Vec<Value> = rows(&v.bundle, "declaration_records").to_vec();
    if records.is_empty() {
        return Ok(
            json!({"view_id":v.id,"declarations":0,"changed":false,"reason":"The view has no retained declaration."}),
        );
    }
    let watermark = l.watermark()?;
    let entities: Vec<String> = rows(&v.bundle, "entity_versions")
        .iter()
        .map(|e| text(e, "id"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let key = identity::id(
        "declaration-overlay",
        &json!({"view_id":v.id,"declaration_record_ids":records.iter().map(|r| text(r,"id")).collect::<Vec<_>>(),
            "entities":identity::digest(&json!(entities))?,
            "implementation":identity::bytes_digest(include_bytes!("declarations.rs"))}),
    )?;
    if l.has_overlay(&key)? {
        return Ok(json!({"view_id":v.id,"overlay_id":key,"reused":true,"changed":false}));
    }
    let payloads = l.payloads(&v.bundle)?;
    let mut b = v.bundle.clone();
    let original: BTreeSet<String> = rows(&b, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect();
    let before = rows(&b, "semantic_binding_resolutions").len();
    let current = current_resolutions(&b);
    let mut resolved = vec![];
    let mut remaining = vec![];
    for record in &records {
        let ctx = record["context"].clone();
        let bytes = payloads
            .get(&text(record, "artifact_id"))
            .ok_or("POLICY: Retained declaration artifact is unavailable")?;
        let policy = identity::parse(bytes)?;
        validate(&policy)?;
        let origin = json!({"declaration_record_id":record["id"]});
        let declare = |b: &mut Value, subject: &str, predicate: &str, object: Value| {
            assertion(
                b,
                &ctx,
                "declaration",
                subject,
                predicate,
                object,
                origin.clone(),
                vec![],
                None,
            )
        };
        let binder = Binder {
            ctx: ctx.clone(),
            record_id: text(record, "id"),
            stage: "post_overlay",
        };
        for request in requests(&policy) {
            let Some(owner) = rows(&b, "entity_versions")
                .iter()
                .find(|e| e["canonical_key"] == request.owner_key && e["context"] == ctx)
                .map(|e| text(e, "id"))
            else {
                continue;
            };
            let prior = current.get(&binder.binding_key(&request, &owner)?);
            if prior.is_some_and(|r| r["status"] == "resolved") {
                continue;
            }
            let summary = json!({"owner":request.owner_key,"predicate":request.predicate,
                "kind":request.kind,"key":request.key,"before":prior.map(|r| r["status"].clone())});
            match binder.bind(&mut b, &declare, &request, &owner)? {
                None => resolved.push(summary),
                Some(note) => remaining.push(json!({"binding":summary,"limitation":note})),
            }
        }
    }
    let selected: BTreeSet<String> = rows(&b, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .filter(|id| !original.contains(id))
        .collect();
    if rows(&b, "semantic_binding_resolutions").len() == before {
        return Ok(
            json!({"view_id":v.id,"changed":false,"resolved":[],"remaining":[],
            "reason":"Every declared technical reference is already bound."}),
        );
    }
    let diagnostics = json!({"resolved":resolved,"remaining":remaining});
    let prepared = Prepared::extension(
        b,
        payloads,
        v.generation,
        &selected,
        &[contracts::Capability::Declaration],
        diagnostics.clone(),
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
        &json!({"operation":"bind-declarations","overlay_id":key,"reused":false}),
    )?;
    Ok(
        json!({"view_id":v.id,"overlay_id":key,"execution_id":execution,"reused":false,"changed":true,
        "declarations_added":selected.len(),"resolved":resolved,"remaining":remaining}),
    )
}
