//! Desired runtime producer. Retained build manifests, OCI manifests and
//! Kubernetes Deployment/Service YAML become a deployment-scoped publication:
//! Evidence of what the retained inputs say, and Declarations of the intended
//! topology. Nothing here is an observation of running behavior.
// Occurrence tuples stay local to one capture; naming each would add no meaning.
#![allow(clippy::type_complexity)]
use super::{
    contracts::{self, add, rows, strings, text, Payloads},
    identity, knowledge,
    ledger::{Ledger, Prepared, View},
    query,
    require, rust, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use yaml_rust2::{
    parser::{Event, MarkedEventReceiver, Parser},
    scanner::Marker,
};

const MAX_FILES: usize = 512;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// A YAML node with the byte offset of its first token.
#[derive(Debug, Clone)]
pub(crate) enum Node {
    Map(Vec<(String, usize, Node)>, usize),
    Seq(Vec<Node>, usize),
    Scalar(String, usize),
    Unsupported(usize),
}
impl Node {
    fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries, _) => entries.iter().find(|(k, _, _)| k == key).map(|(_, _, v)| v),
            _ => None,
        }
    }
    fn path(&self, keys: &[&str]) -> Option<&Node> {
        keys.iter().try_fold(self, |n, k| n.get(k))
    }
    fn scalar(&self) -> Option<&str> {
        match self {
            Node::Scalar(s, _) => Some(s),
            _ => None,
        }
    }
    fn offset(&self) -> usize {
        match self {
            Node::Map(_, at) | Node::Seq(_, at) | Node::Scalar(_, at) | Node::Unsupported(at) => {
                *at
            }
        }
    }
    fn string_map(&self) -> Option<Vec<(String, String, usize)>> {
        match self {
            Node::Map(entries, _) => entries
                .iter()
                .map(|(k, at, v)| v.scalar().map(|s| (k.clone(), s.to_owned(), *at)))
                .collect(),
            _ => None,
        }
    }
}
struct Builder {
    stack: Vec<(Node, Option<(String, usize)>)>,
    documents: Vec<Node>,
    offsets: Vec<usize>,
    unsupported: bool,
}
impl Builder {
    fn byte(&self, mark: &Marker) -> usize {
        // Markers count characters; map them to retained UTF-8 byte offsets.
        self.offsets
            .get(mark.index())
            .copied()
            .unwrap_or(*self.offsets.last().unwrap_or(&0))
    }
    fn push(&mut self, node: Node) {
        match self.stack.last_mut() {
            None => self.documents.push(node),
            Some((Node::Seq(items, _), _)) => items.push(node),
            Some((Node::Map(entries, _), pending)) => match pending.take() {
                Some((key, at)) => entries.push((key, at, node)),
                None => match node {
                    Node::Scalar(key, at) => *pending = Some((key, at)),
                    other => {
                        self.unsupported = true;
                        *pending = Some((String::new(), other.offset()));
                    }
                },
            },
            Some(_) => self.unsupported = true,
        }
    }
}
impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        let at = self.byte(&mark);
        match ev {
            Event::Scalar(value, _, _, _) => self.push(Node::Scalar(value, at)),
            Event::MappingStart(_, _) => self.stack.push((Node::Map(vec![], at), None)),
            Event::SequenceStart(_, _) => self.stack.push((Node::Seq(vec![], at), None)),
            Event::MappingEnd | Event::SequenceEnd => {
                if let Some((node, _)) = self.stack.pop() {
                    self.push(node);
                }
            }
            Event::Alias(_) => {
                self.unsupported = true;
                self.push(Node::Unsupported(at));
            }
            _ => {}
        }
    }
}
/// Parse a multi-document YAML stream into marked nodes. Aliases are not
/// expanded: a manifest that needs them is reported unsupported.
pub(crate) fn yaml(source: &str) -> Result<(Vec<Node>, bool)> {
    let mut offsets: Vec<usize> = source.char_indices().map(|(i, _)| i).collect();
    offsets.push(source.len());
    let mut builder = Builder {
        stack: vec![],
        documents: vec![],
        offsets,
        unsupported: false,
    };
    Parser::new_from_str(source)
        .load(&mut builder, true)
        .map_err(|e| format!("DEPLOYMENT: Invalid YAML: {e}"))?;
    Ok((builder.documents, builder.unsupported))
}
/// The retained line containing `offset`, without indentation.
fn line_span(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = source[offset..]
        .find('\n')
        .map_or(source.len(), |i| offset + i);
    let indent = source[start..end].len() - source[start..end].trim_start().len();
    let (s, e) = (
        start + indent,
        end.max(start + indent + 1).min(source.len()),
    );
    if s < e {
        (s, e)
    } else {
        (start, (start + 1).min(source.len()))
    }
}
/// `<repository>[:tag]@sha256:<hex>`; tags are locators, the digest is identity.
pub(crate) fn image_reference(image: &str) -> (Option<String>, Option<String>, Option<String>) {
    static IMAGE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = IMAGE.get_or_init(|| {
        Regex::new(r"^([^@\s]+?)(?::([\w][\w.-]{0,127}))?(?:@(sha256:[0-9a-f]{64}))?$").unwrap()
    });
    match re.captures(image) {
        Some(c) => (
            Some(c[1].to_owned()),
            c.get(2).map(|m| m.as_str().to_owned()),
            c.get(3).map(|m| m.as_str().to_owned()),
        ),
        None => (None, None, None),
    }
}
pub(crate) fn artifact_key(repository: &str, digest: &str) -> String {
    format!("oci:{repository}@{digest}")
}
pub(crate) fn deployment_key(environment: &str, namespace: &str, name: &str) -> String {
    format!("kubernetes:{environment}/{namespace}/deployment/{name}")
}
pub(crate) fn service_key(environment: &str, namespace: &str, name: &str) -> String {
    format!("service:{environment}/{namespace}/{name}")
}
fn json_span(source: &str, key: &str) -> Option<(usize, usize)> {
    let re = Regex::new(&format!(r#""{}"\s*:\s*"([^"]*)""#, regex::escape(key))).ok()?;
    let m = re.captures(source)?.get(1)?;
    Some((m.start(), m.end().max(m.start() + 1)))
}

pub struct CaptureOptions {
    pub environment: String,
    pub source_view: Option<String>,
    pub label: Option<String>,
}
struct K8sDeployment {
    file: String,
    name: String,
    namespace: String,
    labels: Vec<(String, String, usize)>,
    images: Vec<(String, usize)>,
    env: Vec<(String, String, usize)>,
    entity: String,
}
struct K8sService {
    file: String,
    name: String,
    namespace: String,
    selector: Option<Vec<(String, String, usize)>>,
    selector_at: usize,
    entity: String,
}

fn read_inputs(root: &Path) -> Result<(BTreeMap<String, Vec<u8>>, Vec<String>)> {
    let mut files = BTreeMap::new();
    let mut excluded = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .parents(false)
        .build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)?
            .to_str()
            .ok_or("DEPLOYMENT: Non-UTF-8 path")?
            .replace('\\', "/");
        let selected = rel.ends_with(".yaml")
            || rel.ends_with(".yml")
            || rel.ends_with(".json")
            || super::capture::is_dockerfile(&rel);
        if !selected {
            excluded.push(rel);
            continue;
        }
        require(
            entry.metadata()?.len() <= MAX_FILE_BYTES,
            "DEPLOYMENT",
            "Deployment input byte limit exceeded",
        )?;
        let bytes = std::fs::read(entry.path())?;
        std::str::from_utf8(&bytes).map_err(|_| format!("DEPLOYMENT: Non-UTF-8 input {rel}"))?;
        files.insert(rel, bytes);
        require(
            files.len() <= MAX_FILES,
            "DEPLOYMENT",
            "Deployment input file limit exceeded",
        )?;
    }
    require(!files.is_empty(), "DEPLOYMENT", "No deployment inputs")?;
    Ok((files, excluded))
}

/// Capture retained desired-runtime inputs as one deployment-scoped publication.
pub fn capture(l: &mut Ledger, root: &Path, o: &CaptureOptions) -> Result<Value> {
    require(
        !o.environment.is_empty() && !o.environment.contains('/'),
        "DEPLOYMENT",
        "Environment must be an explicit single segment",
    )?;
    let repository = l.repository()?;
    let (files, excluded) = read_inputs(&root.canonicalize()?)?;
    let source = match &o.source_view {
        Some(selector) => Some(l.load(&l.resolve_scoped("source", selector)?)?),
        None => None,
    };
    let generation = l.begin_scope(&repository, o.label.as_deref(), "deployment")?;
    let result = (|| {
        let prepared = produce(
            &repository,
            &files,
            &excluded,
            o,
            source.as_ref(),
            generation,
        )?;
        let view = l.commit(generation, &prepared)?;
        let projections = l.publish(&view, false)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
            json!({"view_id":view,"scope":"deployment",
            "context":knowledge::context(&prepared.bundle),"environment":o.environment,"projections":projections,
            "diagnostics":prepared.diagnostics,"assertion_count":rows(&prepared.bundle,"assertions").len(),"status":"published"}),
        )
    })();
    if let Err(e) = &result {
        l.fail(generation, &e.to_string())?;
    }
    result
}

fn produce(
    repository: &str,
    files: &BTreeMap<String, Vec<u8>>,
    excluded: &[String],
    o: &CaptureOptions,
    source: Option<&View>,
    generation: u64,
) -> Result<Prepared> {
    let env = &o.environment;
    let mut b = contracts::empty_bundle();
    let mut payloads = Payloads::new();
    let manifest: Vec<_> = files
        .iter()
        .map(|(p, bytes)| json!({"path":p,"digest":identity::bytes_digest(bytes)}))
        .collect();
    let did = identity::id(
        "deployment",
        &json!({"repository_id":repository,"environment":env,"files":manifest}),
    )?;
    add(
        &mut b,
        "deployment_contexts",
        json!({"id":did,"repository_id":repository,"environment":env,
        "manifest_digest":identity::digest(&json!(manifest))?,"files":manifest,"capture_state":"complete"}),
    );
    let ctx = json!({"kind":"deployment","deployment_context_id":did});
    let mut artifact_ids = BTreeMap::new();
    for (path, bytes) in files {
        let digest = identity::bytes_digest(bytes);
        let aid = identity::id(
            "artifact",
            &json!({"path":path,"digest":digest,"source_kind":"configuration"}),
        )?;
        add(
            &mut b,
            "artifacts",
            json!({"id":aid,"path":path,"digest":digest,"source_kind":"configuration","availability":"available"}),
        );
        payloads.insert(aid.clone(), bytes.clone());
        artifact_ids.insert(path.clone(), aid);
    }
    let implementation = identity::bytes_digest(include_bytes!("deployment.rs"));
    let settings = json!({"environment":env,"supports":["briefcase-build-manifest-v1","oci-image-manifest","kubernetes-apps/v1-Deployment","kubernetes-v1-Service"],
        "source_view_snapshot":source.map(|v| knowledge::context(&v.bundle)["snapshot_id"].clone())});
    let mut config = json!({"producer_name":"briefcase-deployment-v1","implementation_digest":implementation,"settings_digest":identity::digest(&settings)?});
    let config_id = identity::id("analysis", &config)?;
    config["id"] = json!(config_id);
    add(&mut b, "analysis_configs", config);
    let provider = identity::id(
        "identity-provider",
        &json!({"mapping":"retained-deployment-identity-v1","implementation_digest":implementation}),
    )?;
    let mut limitations = vec![format!("Desired runtime for environment {env}: declared configuration only. Kubernetes YAML never establishes observed calls.")];
    limitations.extend(
        excluded
            .iter()
            .map(|p| format!("Outside supported deployment inputs: {p}")),
    );
    let mut file_entities = BTreeMap::new();
    for path in files.keys() {
        file_entities.insert(
            path.clone(),
            rust::entity_for_language(
                &mut b,
                &ctx,
                "file",
                &format!("file::{path}"),
                &provider,
                "syntax",
                "none",
            )?,
        );
    }
    let text_of = |p: &str| std::str::from_utf8(&files[p]).unwrap_or("");
    let snapshot_files: Option<BTreeMap<String, String>> = source.map(|v| {
        let ctx = knowledge::context(&v.bundle);
        rows(&v.bundle, "snapshots")
            .iter()
            .filter(|s| s["id"] == ctx["snapshot_id"])
            .flat_map(|s| rows(s, "files"))
            .map(|f| (text(f, "path"), text(f, "digest")))
            .collect()
    });
    let source_snapshot = source.map(|v| text(&knowledge::context(&v.bundle), "snapshot_id"));
    let mut unresolved: Vec<(
        String,
        String,
        &'static str,
        &'static str,
        Vec<String>,
        String,
    )> = Vec::new();
    // ---- Build manifests: source revision → OCI artifact -----------------------
    let mut builds = Vec::new();
    let mut build_attempted = 0;
    let mut artifacts: BTreeMap<String, String> = BTreeMap::new();
    for (path, bytes) in files.iter().filter(|(p, _)| p.ends_with(".json")) {
        let Ok(m) = identity::parse(bytes) else {
            continue;
        };
        if m["kind"] != "briefcase-build-manifest" {
            continue;
        }
        build_attempted += 1;
        let raw = text_of(path);
        let at = |key: &str| json_span(raw, key).unwrap_or((0, raw.len().max(1)));
        let file = file_entities[path].clone();
        let well_formed = m["schema_version"] == "1.0"
            && m["source_snapshot_id"].is_string()
            && m["repository"].is_string()
            && m["image_digest"].as_str().is_some_and(|d| {
                d.len() == 71
                    && d.starts_with("sha256:")
                    && d[7..]
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            && m["context_files"].is_array()
            && m["dockerfile"]["path"].is_string();
        if !well_formed {
            unresolved.push((
                file,
                path.clone(),
                "BUILDS",
                "extraction_failure",
                vec![],
                "Malformed build manifest".into(),
            ));
            continue;
        }
        let digest = text(&m, "image_digest");
        let repository = text(&m, "repository");
        let context_files = rows(&m, "context_files").to_vec();
        let mut problems = Vec::new();
        if m["context_digest"] != json!(identity::digest(&json!(context_files))?) {
            problems.push("context digest differs from listed context files".to_owned());
        }
        // The immutable identity is verified against retained manifest bytes when present.
        let oci = m["oci_manifest"].as_str().map(|p| {
            let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
            if dir.is_empty() {
                p.to_owned()
            } else {
                format!("{dir}/{p}")
            }
        });
        let oci_verified = oci
            .as_ref()
            .and_then(|p| files.get(p))
            .is_some_and(|bytes| identity::bytes_digest(bytes) == digest);
        if !oci_verified {
            problems.push("retained OCI manifest bytes do not establish the image digest".into());
        }
        if let (Some(snapshot), Some(files_in)) = (&source_snapshot, &snapshot_files) {
            if text(&m, "source_snapshot_id") != *snapshot {
                problems.push(format!(
                    "build source {} is not the pinned source view {snapshot}",
                    text(&m, "source_snapshot_id")
                ));
            }
            for f in context_files
                .iter()
                .chain(std::iter::once(&m["dockerfile"]))
            {
                if files_in.get(&text(f, "path")) != Some(&text(f, "digest")) {
                    problems.push(format!(
                        "build input {} differs from the pinned snapshot",
                        text(f, "path")
                    ));
                }
            }
        } else {
            limitations.push(format!(
                "Build manifest {path}: no pinned source view; source inputs are not verified."
            ));
        }
        let artifact = rust::entity_for_language(
            &mut b,
            &ctx,
            "artifact",
            &artifact_key(&repository, &digest),
            &provider,
            if oci_verified {
                "exact_mapping"
            } else {
                "declared"
            },
            "oci",
        )?;
        artifacts.insert(digest.clone(), artifact.clone());
        let revision = rust::entity_for_language(
            &mut b,
            &ctx,
            "source_revision",
            &text(&m, "source_snapshot_id"),
            &provider,
            "exact_mapping",
            "none",
        )?;
        let mut refs = vec![
            {
                let s = at("image_digest");
                rust::evidence(&mut b, path, s.0, s.1)?
            },
            {
                let s = at("source_snapshot_id");
                rust::evidence(&mut b, path, s.0, s.1)?
            },
        ];
        if let (Some(oci), true) = (&oci, oci_verified) {
            refs.push(rust::evidence(&mut b, oci, 0, files[oci].len())?);
        }
        if problems.is_empty() {
            builds.push((revision, artifact, refs));
        } else {
            limitations.extend(
                problems
                    .iter()
                    .map(|p| format!("Build manifest {path}: {p}")),
            );
            unresolved.push((
                revision,
                path.clone(),
                "BUILDS",
                "schema_mismatch",
                vec![artifact],
                problems.join("; "),
            ));
        }
    }
    // ---- Kubernetes Deployment / Service --------------------------------------
    let mut deployments: Vec<K8sDeployment> = Vec::new();
    let mut services: Vec<K8sService> = Vec::new();
    let mut unsupported_documents = 0;
    for (path, _) in files
        .iter()
        .filter(|(p, _)| p.ends_with(".yaml") || p.ends_with(".yml"))
    {
        let raw = text_of(path);
        let file = file_entities[path].clone();
        let (documents, aliases) = match yaml(raw) {
            Ok(x) => x,
            Err(e) => {
                unresolved.push((
                    file,
                    path.clone(),
                    "DEPLOYS_AS",
                    "extraction_failure",
                    vec![],
                    e.to_string(),
                ));
                continue;
            }
        };
        if aliases {
            unresolved.push((
                file.clone(),
                path.clone(),
                "DEPLOYS_AS",
                "unsupported_generated_source",
                vec![],
                "YAML aliases are not expanded".into(),
            ));
        }
        for doc in documents {
            let kind = doc.get("kind").and_then(Node::scalar).unwrap_or("");
            let api = doc.get("apiVersion").and_then(Node::scalar).unwrap_or("");
            let name = doc
                .path(&["metadata", "name"])
                .and_then(Node::scalar)
                .map(str::to_owned);
            let namespace = doc
                .path(&["metadata", "namespace"])
                .and_then(Node::scalar)
                .map(str::to_owned);
            let supported =
                (kind == "Deployment" && api == "apps/v1") || (kind == "Service" && api == "v1");
            if !supported {
                unsupported_documents += 1;
                limitations.push(format!(
                    "Unsupported Kubernetes resource {api}/{kind} in {path}"
                ));
                continue;
            }
            let Some(name) = name else {
                unresolved.push((
                    file.clone(),
                    path.clone(),
                    "DEPLOYS_AS",
                    "extraction_failure",
                    vec![],
                    format!("{kind} without metadata.name"),
                ));
                continue;
            };
            let Some(namespace) = namespace else {
                // Never assume `default`: the applied namespace is not retained.
                let at = line_span(raw, doc.offset());
                unresolved.push((
                    file.clone(),
                    format!("{path}#{}:{}", at.0, at.1),
                    "DEPLOYS_AS",
                    "missing_namespace",
                    vec![],
                    format!("{kind} {name} has no explicit namespace"),
                ));
                continue;
            };
            if kind == "Deployment" {
                let template = doc.path(&["spec", "template"]);
                let labels = template
                    .and_then(|t| t.path(&["metadata", "labels"]))
                    .and_then(Node::string_map)
                    .unwrap_or_default();
                let mut images = Vec::new();
                let mut env = Vec::new();
                if let Some(Node::Seq(containers, _)) =
                    template.and_then(|t| t.path(&["spec", "containers"]))
                {
                    for c in containers {
                        if let Some(image) = c.get("image") {
                            if let Some(s) = image.scalar() {
                                images.push((s.to_owned(), image.offset()));
                            }
                        }
                        if let Some(Node::Seq(vars, _)) = c.get("env") {
                            for var in vars {
                                if let (Some(n), Some(value)) =
                                    (var.get("name").and_then(Node::scalar), var.get("value"))
                                {
                                    if let Some(v) = value.scalar() {
                                        env.push((n.to_owned(), v.to_owned(), value.offset()));
                                    }
                                }
                            }
                        }
                    }
                }
                let entity = rust::entity_for_language(
                    &mut b,
                    &ctx,
                    "deployment",
                    &deployment_key(&o.environment, &namespace, &name),
                    &provider,
                    "declared",
                    "kubernetes",
                )?;
                deployments.push(K8sDeployment {
                    file: path.clone(),
                    name,
                    namespace,
                    labels,
                    images,
                    env,
                    entity,
                });
            } else {
                let selector_node = doc.path(&["spec", "selector"]);
                let entity = rust::entity_for_language(
                    &mut b,
                    &ctx,
                    "service",
                    &service_key(env, &namespace, &name),
                    &provider,
                    "declared",
                    "kubernetes",
                )?;
                services.push(K8sService {
                    file: path.clone(),
                    name,
                    namespace,
                    selector: selector_node.and_then(Node::string_map),
                    selector_at: selector_node.map_or(doc.offset(), Node::offset),
                    entity,
                });
            }
        }
    }
    // Authority: one declaration record per retained manifest file.
    let authority = identity::id(
        "authority",
        &json!({"context":ctx,"name":format!("kubernetes-manifests:{env}")}),
    )?;
    add(
        &mut b,
        "authorities",
        json!({"id":authority,"context":ctx,"precedence":1}),
    );
    let mut records = BTreeMap::new();
    for path in deployments
        .iter()
        .map(|d| &d.file)
        .chain(services.iter().map(|s| &s.file))
        .collect::<BTreeSet<_>>()
    {
        let mut d = json!({"authority_id":authority,"artifact_id":artifact_ids[path],"context":ctx,
            "revision":identity::bytes_digest(&files[path]),"effective_from":"1970-01-01T00:00:00Z"});
        let id = identity::id("declaration", &d)?;
        d["id"] = json!(id);
        add(&mut b, "declaration_records", d);
        records.insert(path.clone(), id);
    }
    let mut deploys = Vec::new();
    let mut deploy_attempted = 0;
    for d in &deployments {
        let raw = text_of(&d.file);
        for (image, at) in &d.images {
            deploy_attempted += 1;
            let span = line_span(raw, *at);
            let (repo, _tag, digest) = image_reference(image);
            match (repo, digest) {
                (Some(repo), Some(digest)) => {
                    let known = artifacts.get(&digest).cloned();
                    let artifact = match known {
                        Some(a)
                            if rows(&b, "entity_versions").iter().any(|e| {
                                e["id"] == a && e["canonical_key"] == artifact_key(&repo, &digest)
                            }) =>
                        {
                            a
                        }
                        _ => rust::entity_for_language(
                            &mut b,
                            &ctx,
                            "artifact",
                            &artifact_key(&repo, &digest),
                            &provider,
                            "declared",
                            "oci",
                        )?,
                    };
                    let er = rust::evidence(&mut b, &d.file, span.0, span.1)?;
                    deploys.push((artifact, d.entity.clone(), vec![er], d.file.clone()));
                }
                _ => {
                    unresolved.push((
                        d.entity.clone(),
                        format!("{}#{}:{}", d.file, span.0, span.1),
                        "DEPLOYS_AS",
                        "mutable_tag",
                        vec![],
                        format!("Image {image} has no immutable digest; a tag is only a locator"),
                    ));
                }
            }
        }
    }
    let mut selections = Vec::new();
    let mut selector_attempted = 0;
    let mut implemented_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in &services {
        selector_attempted += 1;
        let raw = text_of(&s.file);
        let selector_span = line_span(raw, s.selector_at);
        let Some(selector) = s.selector.as_ref().filter(|x| !x.is_empty()) else {
            unresolved.push((
                s.entity.clone(),
                format!("{}#{}:{}", s.file, selector_span.0, selector_span.1),
                "DEPLOYS_AS",
                "unmatched_selector",
                vec![],
                format!("Service {} has no explicit selector", s.name),
            ));
            continue;
        };
        let matches: Vec<&K8sDeployment> = deployments
            .iter()
            .filter(|d| {
                d.namespace == s.namespace
                    && selector
                        .iter()
                        .all(|(k, v, _)| d.labels.iter().any(|(lk, lv, _)| lk == k && lv == v))
            })
            .collect();
        if matches.is_empty() {
            unresolved.push((
                s.entity.clone(),
                format!("{}#{}:{}", s.file, selector_span.0, selector_span.1),
                "DEPLOYS_AS",
                "unmatched_selector",
                vec![],
                format!("Service {} selects no retained Deployment", s.name),
            ));
            continue;
        }
        if matches.len() > 1 {
            // All candidates are retained; no exact mapping is selected.
            unresolved.push((
                s.entity.clone(),
                format!("{}#{}:{}", s.file, selector_span.0, selector_span.1),
                "DEPLOYS_AS",
                "ambiguous_selector",
                matches.iter().map(|d| d.entity.clone()).collect(),
                format!(
                    "Service {} selector matches {} Deployments",
                    s.name,
                    matches.len()
                ),
            ));
        }
        for d in matches {
            let mut refs = Vec::new();
            for (_, _, at) in selector {
                let sp = line_span(raw, *at);
                refs.push(rust::evidence(&mut b, &s.file, sp.0, sp.1)?);
            }
            let draw = text_of(&d.file);
            for (k, _, at) in &d.labels {
                if selector.iter().any(|(sk, _, _)| sk == k) {
                    let sp = line_span(draw, *at);
                    refs.push(rust::evidence(&mut b, &d.file, sp.0, sp.1)?);
                }
            }
            refs.sort();
            refs.dedup();
            implemented_by
                .entry(d.entity.clone())
                .or_default()
                .push(s.entity.clone());
            selections.push((d.entity.clone(), s.entity.clone(), refs, s.file.clone()));
        }
    }
    // Declared connections: explicit in-cluster service URLs in container env.
    let url = Regex::new(
        r"^https?://([a-z0-9]([-a-z0-9]*[a-z0-9])?)(?:\.([a-z0-9]([-a-z0-9]*[a-z0-9])?))?(?:\.svc(?:\.cluster\.local)?)?(?::\d+)?(?:/.*)?$",
    )?;
    let mut connections = Vec::new();
    let mut connection_attempted = 0;
    for d in &deployments {
        for (name, value, at) in &d.env {
            let Some(c) = url.captures(value) else {
                continue;
            };
            connection_attempted += 1;
            let namespace = c.get(3).map_or(d.namespace.as_str(), |m| m.as_str());
            let raw = text_of(&d.file);
            let span = line_span(raw, *at);
            let target = services
                .iter()
                .find(|s| s.name == c[1] && s.namespace == namespace);
            let callers = implemented_by.get(&d.entity).cloned().unwrap_or_default();
            match (target, callers.as_slice()) {
                (Some(t), [caller]) => {
                    let er = rust::evidence(&mut b, &d.file, span.0, span.1)?;
                    connections.push((caller.clone(), t.entity.clone(), vec![er], d.file.clone()));
                }
                (target, callers) => unresolved.push((d.entity.clone(), format!("{}#{}:{}", d.file, span.0, span.1), "DECLARES_CONNECTION",
                    if target.is_none() { "missing_dependency" } else { "ambiguous_selector" },
                    callers.to_vec(), format!("Env {name} on Deployment {}: target service or calling service is not uniquely declared", d.name))),
            }
        }
    }
    // ---- Runs and assertions ------------------------------------------------------
    let count = |p: &str, reasons: &[&str]| {
        unresolved
            .iter()
            .filter(|u| u.2 == p && (reasons.is_empty() || reasons.contains(&u.3)))
            .count()
    };
    let build_unresolved = count("BUILDS", &[]);
    let deploy_unresolved = count("DEPLOYS_AS", &[]);
    let connection_unresolved = count("DECLARES_CONNECTION", &[]);
    let build_coverage = json!({"attempted":build_attempted,"completed":build_attempted,"unsupported_count":0,"unresolved_count":build_unresolved,
        "resolution_complete":build_unresolved == 0 && build_attempted > 0,"guarantees":[{"predicate":"BUILDS","kind":"exact_supported"}]});
    let deploy_coverage = json!({"attempted":deploy_attempted + selector_attempted + connection_attempted,"completed":deploy_attempted + selector_attempted + connection_attempted,
        "unsupported_count":unsupported_documents,"unresolved_count":deploy_unresolved + connection_unresolved,
        "resolution_complete":deploy_unresolved + connection_unresolved + unsupported_documents == 0,
        "guarantees":[{"predicate":"DEPLOYS_AS","kind":"exact_supported"},{"predicate":"DECLARES_CONNECTION","kind":"exact_supported"}]});
    let build_run = run(&mut b, &ctx, &config_id, "build", &build_coverage)?;
    let deploy_run = run(&mut b, &ctx, &config_id, "deployment", &deploy_coverage)?;
    for (revision, artifact, refs) in builds {
        rust::assertion(
            &mut b,
            &ctx,
            "evidence",
            &revision,
            "BUILDS",
            knowledge::target(&artifact),
            json!({"run_id":build_run}),
            refs,
            None,
        )?;
    }
    for (predicate, list) in [
        (
            "DEPLOYS_AS",
            deploys.into_iter().chain(selections).collect::<Vec<_>>(),
        ),
        ("DECLARES_CONNECTION", connections),
    ] {
        for (from, to, refs, file) in list {
            rust::assertion(
                &mut b,
                &ctx,
                "evidence",
                &from,
                predicate,
                knowledge::target(&to),
                json!({"run_id":deploy_run}),
                refs,
                None,
            )?;
            rust::assertion(
                &mut b,
                &ctx,
                "declaration",
                &from,
                predicate,
                knowledge::target(&to),
                json!({"declaration_record_id":records[&file]}),
                vec![],
                None,
            )?;
        }
    }
    for (subject, site, predicate, reason, candidates, message) in &unresolved {
        let (file, range) = site
            .split_once('#')
            .map_or((site.as_str(), None), |(f, r)| (f, Some(r)));
        let (start, end) = range
            .and_then(|r| r.split_once(':'))
            .and_then(|(s, e)| Some((s.parse().ok()?, e.parse().ok()?)))
            .unwrap_or((0, files[file].len().max(1)));
        let er = rust::evidence(&mut b, file, start, end)?;
        let status = if *reason == "ambiguous_selector" && candidates.len() > 1 {
            "ambiguous"
        } else if candidates.is_empty() {
            "unresolved"
        } else {
            "partial"
        };
        let mut u = json!({"source_entity_id":subject,"context":ctx,"evidence_ref_id":er,"predicate":predicate,"status":status,"reason":reason,
            "candidate_ids":candidates.iter().cloned().collect::<BTreeSet<_>>()});
        u["id"] = json!(identity::id("unresolved", &u)?);
        add(&mut b, "unresolved_references", u);
        limitations.push(message.clone());
    }
    limitations.sort();
    limitations.dedup();
    let diagnostics = json!({"environment":env,"deployment_context_id":did,"source_view_snapshot":source_snapshot,
        "coverage":{"deployment_mapping_complete":build_coverage["resolution_complete"] == true && deploy_unresolved == 0,
            "service_selector_resolution_complete":count("DEPLOYS_AS", &["ambiguous_selector", "unmatched_selector"]) == 0,
            "declared_connection_resolution_complete":connection_unresolved == 0,"builds":build_coverage,"deployment":deploy_coverage},
        "limitations":limitations});
    Prepared::seal(b, payloads, generation, diagnostics)
}
fn run(b: &mut Value, ctx: &Value, config: &str, kind: &str, coverage: &Value) -> Result<String> {
    let id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config,"producer_kind":kind,"coverage":coverage}),
    )?;
    add(
        b,
        "runs",
        json!({"id":id,"producer_kind":kind,"analysis_config_id":config,"context":ctx,
        "status":if coverage["resolution_complete"] == true {"complete"} else {"partial"},"coverage":coverage}),
    );
    Ok(id)
}

// ---- Queries ------------------------------------------------------------------

fn by_predicate<'a>(
    v: &'a View,
    removed: &BTreeSet<String>,
    historical: bool,
    predicate: &str,
) -> Vec<&'a Value> {
    rows(&v.bundle, "assertions")
        .iter()
        .filter(|a| {
            a["proposition"]["predicate"] == predicate
                && a["context"] == knowledge::context(&v.bundle)
                && v.eligible.contains(&text(a, "id"))
                && (historical || !removed.contains(&text(a, "id")))
        })
        .collect()
}
/// One desired-runtime chain per explicit join: source revision → artifact →
/// Deployment → Service, each step labeled with its Evidence and Declaration.
pub(crate) fn chains(v: &View, removed: &BTreeSet<String>, historical: bool) -> Vec<Value> {
    let keys = query::entities(v);
    let kinds: BTreeMap<_, _> = rows(&v.bundle, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), text(e, "kind")))
        .collect();
    let mut edges: BTreeMap<(String, String, String), (Vec<String>, Vec<String>)> = BTreeMap::new();
    for predicate in ["BUILDS", "DEPLOYS_AS"] {
        for a in by_predicate(v, removed, historical, predicate) {
            let key = (
                predicate.to_owned(),
                text(&a["proposition"], "subject_id"),
                text(&a["proposition"]["object"], "entity_version_id"),
            );
            let entry = edges.entry(key).or_default();
            if a["class"] == "declaration" {
                entry.1.push(text(a, "id"))
            } else {
                entry.0.push(text(a, "id"))
            }
        }
    }
    let step = |k: &(String, String, String), e: &(Vec<String>, Vec<String>)| {
        json!({"predicate":k.0,"from":keys[&k.1],"to":keys[&k.2],
        "evidence_assertion_ids":e.0,"declaration_assertion_ids":e.1,
        "label":if e.1.is_empty() {"evidence"} else if e.0.is_empty() {"declaration"} else {"evidence+declaration"}})
    };
    let mut out = Vec::new();
    let artifacts: BTreeSet<_> = kinds
        .iter()
        .filter(|(_, k)| *k == "artifact")
        .map(|(id, _)| id.clone())
        .collect();
    for artifact in artifacts {
        let builds: Vec<_> = edges
            .iter()
            .filter(|(k, _)| k.0 == "BUILDS" && k.2 == artifact)
            .collect();
        let deploys: Vec<_> = edges
            .iter()
            .filter(|(k, _)| k.0 == "DEPLOYS_AS" && k.1 == artifact)
            .collect();
        let build_steps: Vec<Option<(&(String, String, String), &(Vec<String>, Vec<String>))>> =
            if builds.is_empty() {
                vec![None]
            } else {
                builds.into_iter().map(Some).collect()
            };
        for build in &build_steps {
            let deploy_steps: Vec<Option<_>> = if deploys.is_empty() {
                vec![None]
            } else {
                deploys.iter().cloned().map(Some).collect()
            };
            for deploy in &deploy_steps {
                let services: Vec<Option<_>> = match deploy {
                    Some((k, _)) => {
                        let s: Vec<_> = edges
                            .iter()
                            .filter(|(e, _)| {
                                e.0 == "DEPLOYS_AS"
                                    && e.1 == k.2
                                    && kinds.get(&e.2).is_some_and(|x| x == "service")
                            })
                            .collect();
                        if s.is_empty() {
                            vec![None]
                        } else {
                            s.into_iter().map(Some).collect()
                        }
                    }
                    None => vec![None],
                };
                let ambiguity = deploy.and_then(|(k, _)| {
                    let candidates: Vec<_> = services
                        .iter()
                        .flatten()
                        .map(|(e, _)| e.2.clone())
                        .collect();
                    let competing: Vec<_> = rows(&v.bundle, "unresolved_references")
                        .iter()
                        .filter(|u| {
                            u["reason"] == "ambiguous_selector"
                                && strings(&u["candidate_ids"]).contains(&k.2)
                                && candidates.contains(&text(u, "source_entity_id"))
                        })
                        .map(|u| text(u, "id"))
                        .collect();
                    (!competing.is_empty()).then_some(competing)
                });
                for service in &services {
                    let mut ids = Vec::new();
                    let mut steps = Vec::new();
                    for (k, e) in [build, deploy, service].into_iter().flatten() {
                        ids.extend(e.0.clone());
                        ids.extend(e.1.clone());
                        steps.push(step(k, e));
                    }
                    let complete = build.is_some()
                        && deploy.is_some()
                        && service.is_some()
                        && ambiguity.is_none();
                    out.push(json!({"assertion_ids":ids,"steps":steps,
                        "source_revision":build.map(|(k, _)| keys[&k.1].clone()),"artifact":keys[&artifact],
                        "deployment":deploy.map(|(k, _)| keys[&k.2].clone()),"service":service.map(|(k, _)| keys[&k.2].clone()),
                        "mapping":if ambiguity.is_some() {"ambiguous"} else if complete {"exact"} else {"incomplete"},
                        "ambiguity_occurrence_ids":ambiguity}));
                }
            }
        }
    }
    out
}
