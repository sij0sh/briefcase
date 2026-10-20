//! Task-oriented facets over one evidence index. Scores select reading context;
//! they never create assertions, feature ownership or completeness guarantees.
use super::*;
use super::{
    request::Request,
    trace::{AnchorStatus, Reach, Seed},
};

pub(super) const FACETS: &[&str] = &[
    "structure",
    "dependency",
    "tests",
    "domain",
    "execution",
    "data",
    "runtime",
    "history",
    "sessions",
    "convergence",
    "behavior",
    "lexical",
];
const MIN_SCORE: u64 = 24;
const MAX_SIGNALS: usize = 12;
/// Distinct files admitted as lexical candidates; the remainder stays paged.
const LEXICAL_FILE_LIMIT: usize = 1000;
const GRAPH_CONTINUATIONS: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Signal {
    pub facet: String,
    pub kind: String,
    pub strength: u64,
    pub confidence: String,
    pub depth: usize,
    pub via_link: Option<String>,
    pub provenance: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Candidate {
    pub path: String,
    pub rank: usize,
    pub score: u64,
    pub selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_rank: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub authority: String,
    pub signals: Vec<Signal>,
    pub spans: Vec<Value>,
    /// Source routing is valid for unselected candidates too. Omitted in old pins.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_ref: String,
}
impl Candidate {
    pub(super) fn new(path: &str) -> Self {
        Self {
            path: path.into(),
            rank: 0,
            score: 0,
            selected: false,
            delivery_rank: None,
            role: None,
            authority: "retrieval_hint; not implementation membership".into(),
            signals: vec![],
            spans: vec![],
            source_ref: String::new(),
        }
    }
    pub fn reach(&self) -> Reach {
        let signal = self
            .signals
            .iter()
            .min_by_key(|s| (std::cmp::Reverse(s.strength), s.depth));
        Reach {
            depth: signal.map_or(0, |s| s.depth),
            via_link: signal.and_then(|s| s.via_link.clone()),
        }
    }
}
pub(super) type Candidates = BTreeMap<String, Candidate>;
pub(super) fn contribute(candidates: &mut Candidates, path: &str, signal: Signal) {
    if source::is_evaluation_artifact(path)
        && !(signal.facet == "anchors" && signal.kind == "exact_anchor")
    {
        return;
    }
    let candidate = candidates
        .entry(path.into())
        .or_insert_with(|| Candidate::new(path));
    // At most one contribution per facet/kind/link. Repeated occurrences and
    // repeated passes cannot inflate scores. Keep strongest explanations first.
    if let Some(prior) = candidate
        .signals
        .iter_mut()
        .find(|s| s.facet == signal.facet && s.kind == signal.kind && s.via_link == signal.via_link)
    {
        if signal.strength > prior.strength {
            *prior = signal;
        }
    } else {
        candidate.signals.push(signal);
    }
    candidate.signals.sort_by_key(|s| {
        (
            std::cmp::Reverse(s.strength),
            s.facet.clone(),
            s.via_link.clone(),
        )
    });
    // Reserve diversity before applying the explanation bound.
    let mut seen = BTreeSet::new();
    let diverse: Vec<_> = candidate
        .signals
        .iter()
        .filter(|s| seen.insert(s.facet.clone()))
        .cloned()
        .collect();
    let mut kept = diverse;
    for s in &candidate.signals {
        if kept.len() < MAX_SIGNALS
            && !kept
                .iter()
                .any(|k| k.facet == s.facet && k.kind == s.kind && k.via_link == s.via_link)
        {
            kept.push(s.clone());
        }
    }
    candidate.signals = kept;
    candidate.score = facet_score(&candidate.signals);
}
/// Strongest signal per facet plus a bonus per additional facet. Zero-strength
/// signals remain as explanations but do not count as a facet.
fn facet_score(signals: &[Signal]) -> u64 {
    let mut strongest = BTreeMap::<&str, u64>::new();
    for s in signals.iter().filter(|s| s.strength > 0) {
        strongest
            .entry(&s.facet)
            .and_modify(|n| *n = (*n).max(s.strength))
            .or_insert(s.strength);
    }
    strongest.values().sum::<u64>() + strongest.len().saturating_sub(1) as u64 * 8
}
fn is_import_signal(s: &Signal) -> bool {
    let basis = s.provenance["basis"].as_str().unwrap_or_default();
    match s.kind.as_str() {
        "IMPORTS" | "DEPENDS_ON" => basis == "retained_assertion",
        // A syntactic binding use connects files that already import each other.
        "REFERENCES" => basis == "retained_assertion; symbol_binding",
        "TESTS" => ["test_IMPORTS;", "test_DEPENDS_ON;", "test_SYMBOL_BINDING;"]
            .iter()
            .any(|prefix| basis.starts_with(prefix)),
        _ => false,
    }
}
fn is_binding_signal(s: &Signal) -> bool {
    let basis = s.provenance["basis"].as_str().unwrap_or_default();
    basis == "retained_assertion; symbol_binding" || basis.starts_with("test_SYMBOL_BINDING;")
}
/// Strongest lexical or anchor strength: the task relevance a file has on its own.
fn own_relevance(signals: &[Signal]) -> u64 {
    signals
        .iter()
        .filter(|s| ["lexical", "anchors"].contains(&s.facet.as_str()))
        .map(|s| s.strength)
        .max()
        .unwrap_or(0)
}
/// Import structure corroborates task relevance but cannot exceed it: an
/// import-derived signal is capped at the candidate's own lexical or anchor
/// strength. Uncapped strength stays in provenance. With `structural_reach`,
/// a one-hop binding use from a task-relevant origin may carry that origin's
/// relevance instead; relevance never compounds past one symbol hop.
fn corroborate(candidate: &mut Candidate, reach: bool) {
    let ceiling = own_relevance(&candidate.signals);
    for s in candidate.signals.iter_mut().filter(|s| is_import_signal(s)) {
        let origin = if reach && is_binding_signal(s) {
            s.provenance["origin_relevance"].as_u64().unwrap_or(0)
        } else {
            0
        };
        let ceiling = ceiling.max(origin);
        if s.strength > ceiling {
            if let Some(provenance) = s.provenance.as_object_mut() {
                provenance.insert("uncapped_strength".into(), json!(s.strength));
                provenance.insert(
                    "cap".into(),
                    json!(if origin > 0 { "origin_relevance_ceiling" } else { "task_relevance_ceiling" }),
                );
            }
            s.strength = ceiling;
        }
    }
    candidate
        .signals
        .sort_by_key(|s| (std::cmp::Reverse(s.strength), s.facet.clone(), s.via_link.clone()));
    candidate.score = facet_score(&candidate.signals);
}
/// Structural relationships between one file pair share an origin: an import,
/// the binding uses it enables and a test association derived from either are
/// one observation, not independent facets.
fn signal_origin(s: &Signal, links: &BTreeMap<&str, &Link>) -> String {
    let Some(link) = s.via_link.as_deref().and_then(|id| links.get(id)) else {
        return format!("{}:{}", s.facet, s.kind);
    };
    let structural = ["IMPORTS", "REFERENCES", "CALLS_STATIC", "DEPENDS_ON"].contains(&link.predicate.as_str())
        || link.predicate == "TESTS" && link.basis.starts_with("test_");
    if structural {
        let pair: BTreeSet<_> = link.from_files.iter().chain(&link.to_files).collect();
        format!("pair:{}", pair.into_iter().cloned().collect::<Vec<_>>().join("\0"))
    } else if !link.assertion_ids.is_empty() {
        let mut ids = link.assertion_ids.clone();
        ids.sort();
        format!("assertions:{}", ids.join(","))
    } else {
        format!("link:{}", link.id)
    }
}
/// Facet score where a facet whose every positive signal shares an origin with
/// a stronger counted facet adds only the facet bonus, not its strength.
fn fused_score(candidate: &mut Candidate, links: &BTreeMap<&str, &Link>) -> u64 {
    let mut facets = BTreeMap::<String, (u64, BTreeSet<String>)>::new();
    for s in candidate.signals.iter().filter(|s| s.strength > 0) {
        let entry = facets.entry(s.facet.clone()).or_default();
        entry.0 = entry.0.max(s.strength);
        entry.1.insert(signal_origin(s, links));
    }
    let mut order: Vec<_> = facets.into_iter().collect();
    order.sort_by_key(|(facet, (strength, _))| (std::cmp::Reverse(*strength), facet.clone()));
    let (mut counted, mut correlated) = (BTreeSet::new(), BTreeSet::new());
    let (mut score, mut independent) = (0, 0u64);
    for (facet, (strength, origins)) in order {
        if !counted.is_empty() && origins.is_subset(&counted) {
            correlated.insert(facet);
        } else {
            score += strength;
            independent += 1;
            counted.extend(origins);
        }
    }
    for s in candidate.signals.iter_mut().filter(|s| correlated.contains(&s.facet)) {
        if let Some(provenance) = s.provenance.as_object_mut() {
            provenance.insert("correlated_origin".into(), json!(true));
        }
    }
    score + independent.saturating_sub(1) * 8 + correlated.len() as u64 * 8
}
fn signal(
    facet: &str,
    kind: &str,
    strength: u64,
    confidence: &str,
    depth: usize,
    link: Option<&str>,
    provenance: Value,
) -> Signal {
    Signal {
        facet: facet.into(),
        kind: kind.into(),
        strength,
        confidence: confidence.into(),
        depth,
        via_link: link.map(str::to_owned),
        provenance,
    }
}
pub(super) fn terms(task: &str) -> Vec<String> {
    term_plan(task).0
}
/// Query terms in priority order: explicit identifiers (backticked, snake_case,
/// CamelCase, qualified or path-like) first, then their parts (which match the
/// other spelling, e.g. `parseConfig` against `parse_config`), then ordinary
/// task words in task order. Returns the bounded terms and the omitted remainder.
pub(super) fn term_plan(task: &str) -> (Vec<String>, Vec<String>) {
    const STOP: &[&str] = &[
        "the",
        "and",
        "for",
        "with",
        "from",
        "this",
        "that",
        "how",
        "what",
        "which",
        "does",
        "into",
        "through",
        "about",
        "can",
        "please",
        "would",
        "should",
        "could",
        "use",
        "using",
        "make",
        "change",
        "fix",
        "add",
        "new",
        "support",
        "called",
        "similar",
        "correctly",
    ];
    let quoted: BTreeSet<_> = task.split('`').skip(1).step_by(2).map(str::trim).collect();
    let parts = |raw: &str| {
        let mut out = vec![];
        for word in raw.split(|c: char| !c.is_alphanumeric()) {
            let mut part = String::new();
            let mut previous = ' ';
            for c in word.chars() {
                if c.is_uppercase()
                    && (previous.is_lowercase() || previous.is_ascii_digit())
                    && !part.is_empty()
                {
                    out.push(std::mem::take(&mut part));
                }
                part.push(c);
                previous = c;
            }
            if !part.is_empty() {
                out.push(part);
            }
        }
        out.into_iter()
            .map(|p| p.to_lowercase())
            .filter(|p| p.len() > 2 && !STOP.contains(&p.as_str()))
            .collect::<Vec<_>>()
    };
    let mut ranked = vec![];
    let raws = task
        .split(|c: char| c.is_whitespace() || "`,;()[]{}<>\"'!?".contains(c))
        .map(|raw| raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
        .filter(|raw| !raw.is_empty());
    for (position, raw) in raws.enumerate() {
        let camel = raw
            .chars()
            .zip(raw.chars().skip(1))
            .any(|(a, b)| a.is_lowercase() && b.is_uppercase());
        let explicit = quoted.contains(raw)
            || camel
            || raw.contains('_')
            || raw.contains("::")
            || raw.contains('/')
            || raw.contains('.') && raw.split('.').all(|piece| piece.len() > 1);
        if explicit && raw.len() > 1 {
            ranked.push((0, position, raw.to_lowercase()));
        }
        for part in parts(raw) {
            ranked.push((if explicit { 1 } else { 2 }, position, part));
        }
    }
    ranked.sort_by_key(|(priority, position, _)| (*priority, *position));
    let mut seen = BTreeSet::new();
    let ordered: Vec<_> = ranked
        .into_iter()
        .map(|(_, _, term)| term)
        .filter(|term| seen.insert(term.clone()))
        .collect();
    let (mut bounded, mut omitted) = (vec![], vec![]);
    let mut used = 0;
    for term in ordered {
        let room = 4096usize.saturating_sub(used + usize::from(!bounded.is_empty()));
        if bounded.len() >= 32 || term.len() > room {
            omitted.push(term);
            continue;
        }
        used += term.len() + usize::from(!bounded.is_empty());
        bounded.push(term);
    }
    (bounded, omitted)
}
pub(super) fn matches(text: &str, terms: &[String]) -> usize {
    let text = text.to_lowercase();
    terms.iter().filter(|t| text.contains(t.as_str())).count()
}

pub(super) fn discover(
    l: &Ledger,
    s: &Snapshot,
    ix: &Index,
    r: &Request,
    seed: &Seed,
) -> Result<(Candidates, Vec<Value>, Value)> {
    let mut candidates = Candidates::new();
    for path in &seed.paths {
        contribute(
            &mut candidates,
            path,
            signal(
                "anchors",
                if seed.status == AnchorStatus::Resolved {
                    "exact_anchor"
                } else {
                    "ambiguous_location"
                },
                if seed.status == AnchorStatus::Resolved {
                    10000
                } else {
                    90
                },
                if seed.status == AnchorStatus::Resolved {
                    "exact"
                } else {
                    "candidate"
                },
                0,
                None,
                json!({"identities":seed.candidates,"status":seed.status}),
            ),
        );
    }
    let mut lexical = vec![];
    let (query_terms, omitted_terms) = term_plan(&r.task);
    // The identifier experiment changes only the lexical representation;
    // terms, admission and scoring stay those of the baseline.
    let identifier_parts = r.selection_variant == "identifier_terms";
    let mut coverage = json!({"enabled":r.enabled("lexical"),"method":"distinct_file_fts","candidate_limit":LEXICAL_FILE_LIMIT,
        "representation":if identifier_parts {"identifier_components"} else {"original_tokens"},
        "candidate_unit":"file","query_terms":query_terms,"omitted_terms":omitted_terms,
        "candidate_limit_reached":false,"omitted_candidates":0,"matched_files":0,"state":"not_queried"});
    if r.enabled("lexical") && !query_terms.is_empty() {
        let terms = query_terms;
        let (configuration, _) = if identifier_parts {
            crate::architecture::search::ensure_identifier_index(l, &s.view)?
        } else {
            crate::architecture::search::ensure_index(l, &s.view)?
        };
        let matched_files = match source::fts_expression(&terms.join(" ")) {
            Some(expression) => {
                l.recall_files(&s.view.id, &configuration, &expression, identifier_parts)?
            }
            None => vec![],
        };
        // Every matched file stays reachable in rank order. Only the first
        // LEXICAL_FILE_LIMIT become candidates; the rest are paged, not lost.
        let mut inventory = vec![];
        for (rank, (path, unit)) in matched_files.iter().enumerate() {
            let Some(file) = s.files.get(path) else {
                continue;
            };
            let (_, _, _, start, end) = l.recall_source_unit(*unit)?;
            inventory.push(json!({"rank":rank+1,"path":path,"artifact_id":file.artifact_id,"digest":file.digest,
                "role":file.role,"byte_range":{"start":start,"end":end}}));
        }
        let total = inventory.len();
        let doc = json!({"kind":"lexical-discovery","view_id":s.view.id,"pinned_views":[s.view.id],"terms":terms,
            "omitted_terms":coverage["omitted_terms"],"configuration":configuration,"items":inventory,
            "ordering":"best retained text unit per file (bm25), then unit order; each file appears once",
            "authority":"retained_text_candidate; not implementation membership"});
        let delivered = crate::architecture::reports::retain_items(l, doc, LEXICAL_FILE_LIMIT)?;
        coverage["state"] = json!("queried");
        coverage["report_id"] = delivered["report_id"].clone();
        coverage["matched_files"] = json!(total);
        coverage["candidate_limit_reached"] = json!(total > LEXICAL_FILE_LIMIT);
        coverage["omitted_candidates"] = json!(total.saturating_sub(LEXICAL_FILE_LIMIT));
        coverage["continue_with"] = delivered["continue_with"].clone();
        admit_lexical(s, &terms, rows(&delivered, "items"), &delivered["report_id"], &mut candidates, &mut lexical)?;
    }
    if ["multi_query", "combined"].contains(&r.selection_variant.as_str()) {
        multi_query_hints(s, r, &mut candidates);
    }
    if r.enabled("domain") {
        claim_hints(s, ix, r, &mut candidates);
    }
    Ok((candidates, lexical, coverage))
}

/// Admit an explicit batch from the pinned lexical inventory using the same
/// scoring and localization as initial discovery. Reading a page alone never admits it.
pub(super) fn admit_lexical(s:&Snapshot, terms:&[String], hits:&[Value], report_id:&Value,
    candidates:&mut Candidates, lexical:&mut Vec<Value>)->Result<()> {
    let paths: Vec<_> = hits
        .iter()
        .map(|hit| text(hit, "path"))
        .collect();
    s.contents.load(
        &s.files,
        &paths.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    for hit in hits {
        let path = text(hit, "path");
        let rank = hit["rank"].as_u64().unwrap_or(1000);
        let content = s.content(&path)?.unwrap_or("");
        let matched = matches(content, terms);
        let path_matches = matches(&path, terms);
        let strength = (70 * 8 / (rank + 7)
            + ((matched * 18) / terms.len().max(1)) as u64
            + (path_matches.min(3) * 12) as u64)
            .min(110);
        let (start, end) = (
            hit["byte_range"]["start"].as_u64().unwrap_or(0) as usize,
            hit["byte_range"]["end"].as_u64().unwrap_or(0) as usize,
        );
        let first = 1 + content.as_bytes()[..start.min(content.len())]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        let last = first
            + content
                .get(start..end)
                .unwrap_or("")
                .trim_end_matches('\n')
                .matches('\n')
                .count();
        let span =
            json!({"byte_range":hit["byte_range"],"line_range":{"start":first,"end":last}});
        contribute(
            candidates,
            &path,
            signal(
                "lexical",
                "retained_text",
                strength,
                "candidate",
                0,
                None,
                json!({"report_id":report_id,"rank":rank,"matched_terms":matched}),
            ),
        );
        if let Some(candidate) = candidates.get_mut(&path) {
            candidate.spans.push(span.clone());
        }
        lexical.push(json!({"path":path,"rank":rank,"strength":strength,"span":span,
            "search_report_id":report_id,"basis":"retained text retrieval hint; no implementation membership established"}));
    }
    Ok(())
}

/// Package root for diversity: the first two path segments, or the first
/// segment for top-level files. Ownership here is a path heuristic only.
pub(super) fn package_root(path: &str) -> String {
    let segments: Vec<_> = path.split('/').collect();
    if segments.len() == 1 {
        return ".".into();
    }
    if segments.len() == 2 {
        return segments[0].into();
    }
    segments[..2].join("/")
}
fn evidence_base(path: &str) -> String {
    let stem = path.rsplit_once('.').map_or(path, |(stem, _)| stem);
    stem.strip_suffix(".test")
        .or_else(|| stem.strip_suffix(".spec"))
        .or_else(|| stem.strip_suffix("_test"))
        .unwrap_or(stem)
        .into()
}
fn is_test_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_test.rs")
        || path.contains("/tests/")
}
/// Companion tests sharing an implementation's evidence base. Pairing is a
/// labelled retrieval hint; it does not establish that the test proves behavior.
pub(super) fn companion_test_paths(path: &str, all: &[String]) -> Vec<String> {
    let base = evidence_base(path);
    let mut out: Vec<_> = all
        .iter()
        .filter(|other| {
            other.as_str() != path && evidence_base(other) == base && is_test_path(other)
        })
        .cloned()
        .collect();
    out.sort();
    out
}
fn multi_query_hints(s: &Snapshot, r: &Request, candidates: &mut Candidates) {
    let all: Vec<String> = s.files.keys().cloned().collect();
    let present: Vec<String> = candidates.keys().cloned().collect();
    for path in &present {
        for test in companion_test_paths(path, &all) {
            contribute(
                candidates,
                &test,
                signal(
                    "tests",
                    "companion_test",
                    45,
                    "candidate",
                    0,
                    None,
                    json!({"implementation":path,"basis":"shared evidence base; not proof of behavior"}),
                ),
            );
        }
        if is_test_path(path) {
            for sibling in all.iter().filter(|other| {
                other.as_str() != path
                    && evidence_base(other) == evidence_base(path)
                    && !is_test_path(other)
            }) {
                contribute(
                    candidates,
                    sibling,
                    signal(
                        "tests",
                        "companion_impl",
                        30,
                        "candidate",
                        0,
                        None,
                        json!({"test":path,"basis":"shared evidence base; not proof of behavior"}),
                    ),
                );
            }
        }
        let lowered = path.to_lowercase();
        if terms(&r.task)
            .iter()
            .any(|term| term.len() >= 5 && lowered.contains(term.as_str()))
        {
            contribute(
                candidates,
                path,
                signal(
                    "dependency",
                    "path_ownership",
                    30,
                    "candidate",
                    0,
                    None,
                    json!({"basis":"task term matches the retained path; ownership hint only"}),
                ),
            );
        }
    }
}

/// Claims stay outside authoritative adjacency. Eligible explicit module →
/// domain/feature Claims may only suggest files when their domain is requested.
fn claim_hints(s: &Snapshot, ix: &Index, r: &Request, candidates: &mut Candidates) {
    let entities: BTreeMap<_, _> = rows(&s.view.bundle, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), e))
        .collect();
    let terms = terms(&r.task);
    for a in rows(&s.view.bundle, "assertions").iter().filter(|a| {
        s.view.eligible.contains(&text(a, "id"))
            && (a["class"] == "claim" || a["support_class"] == "includes_inference")
    }) {
        let p = &a["proposition"];
        if ![
            "BELONGS_TO_DOMAIN",
            "IMPLEMENTS_FEATURE",
            "PARTICIPATES_IN",
            "ENTRYPOINT_FOR",
        ]
        .contains(&p["predicate"].as_str().unwrap_or(""))
        {
            continue;
        }
        let (Some(from), Some(to)) = (
            entities.get(&text(p, "subject_id")),
            entities.get(&text(&p["object"], "entity_version_id")),
        ) else {
            continue;
        };
        let key = text(to, "canonical_key");
        if terms.is_empty() || matches(&key, &terms) == 0 {
            continue;
        }
        if let Some(owner) = ix.entities.get(&text(from, "canonical_key")) {
            for path in &owner.files {
                contribute(
                    candidates,
                    path,
                    signal(
                        "domain",
                        "claim_hint",
                        28,
                        "inferred_candidate",
                        0,
                        None,
                        json!({"assertion_id":a["id"],"class":a["class"],"target":key,"membership_established":false}),
                    ),
                );
            }
        }
    }
}

/// Each predicate has a retrieval policy, not another persisted graph product.
pub(super) fn facet(predicate: &str) -> Option<(&'static str, u64)> {
    Some(match predicate {
        "CONTAINS" | "DECLARES" => ("structure", 35),
        "TESTS" => ("tests", 100),
        "IMPORTS" | "DEPENDS_ON" | "REFERENCES" => ("dependency", 80),
        "REFERENCES_ARTIFACT" | "IMPLEMENTS_PLAN_SECTION" => ("dependency", 65),
        "BELONGS_TO_DOMAIN" | "IMPLEMENTS_FEATURE" | "REALIZES_FEATURE" | "SUBFEATURE_OF" => {
            ("domain", 90)
        }
        "ENTRYPOINT_FOR" | "PARTICIPATES_IN" | "ROUTES_TO" | "CALLS_STATIC" => ("execution", 95),
        "READS" | "WRITES" | "DERIVES_FROM" | "MAPS_TO" | "TRANSFORMS" => ("data", 85),
        "BUILDS"
        | "DEPLOYS_AS"
        | "CONNECTS_TO"
        | "CALLS_OBSERVED"
        | "CONNECTION_OBSERVED"
        | "MAPS_TO_SERVICE"
        | "DECLARES_CONNECTION"
        | "INCLUDES_SOURCE" => ("runtime", 80),
        "READS_CONFIG" | "DECLARES_CONFIG_KEY" | "INSTALLS_TO" | "REFERENCES_TOUCHPOINT" => {
            ("dependency", 70)
        }
        _ => return None,
    })
}

/// Monorepo scope from project manifests: each file belongs to the deepest
/// manifest directory above it. Ranking stays global; projects whose
/// candidates were all left unselected get an anchored follow-up instead of
/// silently losing to larger packages.
pub(super) fn project_scope(
    s: &Snapshot,
    r: &Request,
    seed: &Seed,
    candidates: &[Candidate],
) -> Value {
    const MANIFESTS: &[&str] = &[
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "setup.py",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
    ];
    let mut roots: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in s.files.keys() {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        if MANIFESTS.contains(&name) || name.ends_with(".csproj") {
            roots.entry(dir.into()).or_default().push(name.into());
        }
    }
    let owner = |path: &str| {
        roots
            .keys()
            .filter(|root| root.is_empty() || path.starts_with(&format!("{root}/")))
            .max_by_key(|root| root.len())
            .cloned()
    };
    let mut scope = json!({"method":"manifest_ownership","projects":roots.len()});
    if roots.len() < 2 {
        scope["state"] = json!("single_project");
        return scope;
    }
    let anchors: BTreeSet<_> = seed.paths.iter().filter_map(|path| owner(path)).collect();
    let mut rows: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for c in candidates {
        let row = rows
            .entry(owner(&c.path).unwrap_or_default())
            .or_insert((c.rank, 0, 0));
        row.0 = row.0.min(c.rank);
        row.1 += 1;
        row.2 += usize::from(c.selected);
    }
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by_key(|(root, (rank, _, _))| (*rank, root.clone()));
    let unselected: Vec<_> = rows
        .iter()
        .filter(|(root, (_, _, selected))| *selected == 0 && !root.is_empty())
        .map(|(root, _)| root.clone())
        .collect();
    scope["state"] = json!(if unselected.is_empty() {
        "all_candidate_projects_selected"
    } else {
        "unselected_candidate_projects"
    });
    scope["anchor_projects"] = json!(anchors
        .iter()
        .map(|root| if root.is_empty() { "." } else { root })
        .collect::<Vec<_>>());
    scope["candidate_projects"]=json!(rows.iter().map(|(root,(rank,count,selected))|json!({"root":if root.is_empty() {"."} else {root},
        "manifests":roots.get(root).cloned().unwrap_or_default(),"best_rank":rank,"candidates":count,"selected":selected})).collect::<Vec<_>>());
    scope["unselected_projects"] = json!(unselected.len());
    scope["continue_with"]=json!(unselected.iter().take(GRAPH_CONTINUATIONS).map(|root|json!({"operation":"agent_context",
        "arguments":{"task":r.task,"target":format!("{root}/"),"view":s.view.id,"max_files":r.max_files}})).collect::<Vec<_>>());
    scope["semantics"]=json!("Ownership is the deepest manifest directory. Cross-project candidates remain in the global ranking; ownership is not a dependency edge.");
    scope
}

/// Returns resumable graph coverage: starters beyond the bound and the frontier
/// left unexpanded at the depth limit, with executable `neighbors` reads.
pub(super) fn expand(ix: &Index, r: &Request, seed: &Seed, candidates: &mut Candidates) -> Value {
    let mut neighbors = BTreeMap::<String, BTreeSet<String>>::new();
    let mut key_neighbors = BTreeMap::<&str, BTreeSet<&str>>::new();
    for e in ix.traversal_links() {
        if !link_allowed(e, seed) {
            continue;
        }
        let Some((facet, _)) = facet(&e.predicate) else {
            continue;
        };
        if !r.enabled(facet)
            || !r
                .duty
                .policy()
                .trace
                .predicates
                .contains(&e.predicate.as_str())
        {
            continue;
        }
        key_neighbors.entry(&e.from).or_default().insert(&e.to);
        key_neighbors.entry(&e.to).or_default().insert(&e.from);
        for a in &e.from_files {
            for b in &e.to_files {
                if a != b {
                    neighbors.entry(a.clone()).or_default().insert(b.clone());
                    neighbors.entry(b.clone()).or_default().insert(a.clone());
                }
            }
        }
    }
    let mut starters: Vec<_> = candidates.values().filter(|c| c.score >= 50).collect();
    starters.sort_by_key(|c| (std::cmp::Reverse(c.score), c.path.clone()));
    let eligible_starters = starters.len();
    let omitted_starters: Vec<_> = starters.iter().skip(8).map(|c| c.path.clone()).collect();
    let mut frontier_paths: BTreeSet<_> = starters.iter().take(8).map(|c| c.path.clone()).collect();
    frontier_paths.extend(
        seed.paths
            .iter()
            .filter(|path| path_allowed(path, seed))
            .cloned(),
    );
    let mut frontier_keys = seed.candidates.clone();
    let (mut visited_paths, mut visited_keys) = (frontier_paths.clone(), frontier_keys.clone());
    let reach = r.selection_variant == "structural_reach";
    for depth in 1..=r.depth {
        let (mut next_paths, mut next_keys) = (BTreeSet::new(), BTreeSet::new());
        for e in ix.traversal_links() {
            if !link_allowed(e, seed) {
                continue;
            }
            let Some((facet, base)) = facet(&e.predicate) else {
                continue;
            };
            // TESTS is a direct association. A shared test suite must not
            // bridge from one implementation into every implementation it
            // mentions. Cargo ownership already supplies runner associations.
            if facet == "tests" && depth > 1 {
                continue;
            }
            if !r
                .duty
                .policy()
                .trace
                .predicates
                .contains(&e.predicate.as_str())
            {
                continue;
            }
            if !r.enabled(facet) {
                continue;
            }
            // Structure localizes an explicit identity only. It cannot turn a
            // file into a traversal bridge through every declaration it holds.
            if facet == "structure"
                && (depth > 1
                    || !seed.candidates.contains(&e.from) && !seed.candidates.contains(&e.to))
            {
                continue;
            }
            let incoming_only =
                matches!(r.duty, Duty::Impact) && ["dependency", "execution"].contains(&facet);
            let outgoing_only = matches!(r.duty, Duty::Explain | Duty::Run)
                && ["dependency", "execution"].contains(&facet);
            let outgoing = !incoming_only
                && (frontier_keys.contains(&e.from) || !frontier_paths.is_disjoint(&e.from_files));
            let incoming = !outgoing_only
                && (frontier_keys.contains(&e.to) || !frontier_paths.is_disjoint(&e.to_files));
            for (touch, origin, target, key, origin_key) in [
                (outgoing, &e.from_files, &e.to_files, &e.to, &e.from),
                (incoming, &e.to_files, &e.from_files, &e.from, &e.to),
            ] {
                if !touch {
                    continue;
                }
                let degree = origin
                    .iter()
                    .filter(|p| frontier_paths.contains(*p))
                    .map(|p| neighbors.get(p).map_or(0, BTreeSet::len))
                    .max()
                    .unwrap_or(0)
                    .max(
                        key_neighbors
                            .get(origin_key.as_str())
                            .map_or(0, BTreeSet::len),
                    );
                let hub_penalty = if degree > 8 {
                    (usize::BITS - (degree as usize).leading_zeros()) as u64 * 9
                } else {
                    0
                };
                let fanout_penalty = target.len().saturating_sub(1).min(20) as u64 * 3;
                let strength =
                    base.saturating_sub((depth - 1) as u64 * 24 + hub_penalty + fanout_penalty);
                if strength < 12 {
                    continue;
                }
                let mut provenance = json!({"index_id":ix.id,"class":e.class,"basis":e.basis,"hub_degree":degree,"hub_penalty":hub_penalty});
                if reach && depth == 1
                    && (e.basis == "retained_assertion; symbol_binding" || e.basis.starts_with("test_SYMBOL_BINDING;"))
                {
                    let relevance = origin
                        .iter()
                        .filter(|p| frontier_paths.contains(*p))
                        .filter_map(|p| candidates.get(p))
                        .map(|c| own_relevance(&c.signals))
                        .max()
                        .unwrap_or(0);
                    provenance["origin_relevance"] = json!(relevance);
                }
                for path in target.iter().filter(|p| ix.files.contains_key(*p)) {
                    contribute(
                        candidates,
                        path,
                        signal(
                            facet,
                            &e.predicate,
                            strength,
                            "retained_relationship",
                            depth,
                            Some(&e.id),
                            provenance.clone(),
                        ),
                    );
                    if facet != "structure" && strength >= 32 && !visited_paths.contains(path) {
                        next_paths.insert(path.clone());
                    }
                }
                if facet != "structure" && strength >= 32 && !visited_keys.contains(key) {
                    next_keys.insert(key.clone());
                }
            }
        }
        visited_paths.extend(next_paths.clone());
        visited_keys.extend(next_keys.clone());
        frontier_paths = next_paths;
        frontier_keys = next_keys;
        if frontier_paths.is_empty() && frontier_keys.is_empty() {
            break;
        }
    }
    // Whatever the last layer reached but did not expand is resumable, not absent.
    let resume = |targets: Vec<String>| {
        targets.into_iter().take(GRAPH_CONTINUATIONS).map(|target|json!({"operation":"neighbors",
        "arguments":{"index_id":ix.id,"view":ix.source_view,"target":target,"direction":"both","depth":1,"limit":100}})).collect::<Vec<_>>()
    };
    let frontier: Vec<_> = frontier_paths.iter().cloned().collect();
    let coverage = json!({"depth_limit":r.depth,"starter_limit":8,"eligible_starters":eligible_starters,
        "omitted_starters":omitted_starters.len(),"unexpanded_frontier_files":frontier.len(),"unexpanded_frontier_keys":frontier_keys.len(),
        "frontier_examples":frontier.iter().take(GRAPH_CONTINUATIONS).collect::<Vec<_>>(),
        "continue_with":resume(frontier.iter().cloned().chain(omitted_starters.iter().cloned()).collect()),
        "remaining_work":frontier.iter().map(|path|("graph_frontier",path))
            .chain(frontier_keys.iter().map(|key|("graph_frontier_key",key)))
            .chain(omitted_starters.iter().map(|path|("graph_starter",path)))
            .map(|(kind,target)|json!({"kind":kind,"target":target,"status":"available",
                "reason":if kind=="graph_starter" {"starter_limit"} else {"depth_limit"},
                "action":{"operation":"neighbors","arguments":{"index_id":ix.id,"view":ix.source_view,
                    "target":target,"direction":"both","depth":1,"relationship_scope":"all"}}})).collect::<Vec<_>>(),
        "semantics":"Unexpanded frontier and omitted starters are retrieval bounds, not absence; each continuation is an exhaustive neighbors read."});
    // A narrow, explicit config-key join still works for depth-one duties.
    if r.enabled("dependency") {
        let readers: BTreeSet<_> = candidates
            .values()
            .filter(|c| c.score >= 50)
            .map(|c| c.path.clone())
            .collect();
        let keys: BTreeSet<_> = ix
            .traversal_links()
            .filter(|e| e.predicate == "READS_CONFIG" && !readers.is_disjoint(&e.from_files))
            .map(|e| e.to.clone())
            .collect();
        for e in ix.traversal_links().filter(|e| {
            link_allowed(e, seed) && e.predicate == "DECLARES_CONFIG_KEY" && keys.contains(&e.to)
        }) {
            for path in &e.from_files {
                contribute(
                    candidates,
                    path,
                    signal(
                        "dependency",
                        "config_key_join",
                        65,
                        "retained_relationship",
                        2,
                        Some(&e.id),
                        json!({"key":e.to,"index_id":ix.id}),
                    ),
                );
            }
        }
    }
    coverage
}

pub(super) fn path_allowed(path: &str, seed: &Seed) -> bool {
    !source::is_evaluation_artifact(path)
        || seed.status == AnchorStatus::Resolved && seed.paths.contains(path)
}
pub(super) fn link_allowed(link: &Link, seed: &Seed) -> bool {
    link.from_files
        .iter()
        .chain(&link.to_files)
        .all(|path| path_allowed(path, seed))
}

/// Keep evidence scores/ranks intact. A separate, deterministic reading queue
/// protects anchors and interleaves implementation, tests and supporting roles.
pub(super) fn selection_policy(r: &Request) -> Value {
    json!({"method":match r.selection_variant.as_str() {"baseline"=>if r.duty.delivery_cycle().is_empty(){"evidence_rank"}else{"duty_roles_v1"},
            "role_diverse"=>"role_diverse_v1","reserved_floor"=>"reserved_floor_v1","multi_query"=>"multi_query_v1",
            "linked_diversity"=>"linked_diversity_v1","combined"=>"combined_v1","docs_disabled"=>"docs_disabled_v1",
            "identifier_terms"=>"identifier_terms_v1","origin_fusion"=>"origin_fusion_v1","structural_reach"=>"structural_reach_v1","exact_keys"=>"exact_keys_v1",_=>"unknown"},
        "variant":r.selection_variant,"cycle":r.duty.delivery_cycle(),"anchors":"exact anchors first",
        "minimum_score":MIN_SCORE,"reserved_floor":12,
        "rank_semantics":"candidates.rank is evidence rank; selected slice rank is delivery order"})
}

const ROLE_DIVERSE_CYCLE: &[&str] = &["impl", "impl", "test", "impl", "test", "support"];
fn diversify(order: Vec<usize>, ranked: &[Candidate]) -> Vec<usize> {
    let anchors = order
        .iter()
        .take_while(|&&index| {
            ranked[index]
                .signals
                .iter()
                .any(|s| s.facet == "anchors" && s.kind == "exact_anchor")
        })
        .count();
    let (head, tail) = order.split_at(anchors);
    let mut groups: Vec<(String, std::collections::VecDeque<usize>)> = vec![];
    for &index in tail {
        let root = package_root(&ranked[index].path);
        if let Some((_, queue)) = groups.iter_mut().find(|(name, _)| *name == root) {
            queue.push_back(index);
        } else {
            groups.push((root, std::collections::VecDeque::from([index])));
        }
    }
    let mut out: Vec<_> = head.to_vec();
    loop {
        let before = out.len();
        for (_, queue) in &mut groups {
            if let Some(index) = queue.pop_front() {
                out.push(index);
            }
        }
        if out.len() == before {
            break;
        }
    }
    out
}
fn delivery_order(ix: &Index, r: &Request, ranked: &[Candidate]) -> Vec<usize> {
    let variant = r.selection_variant.as_str();
    let mut order = vec![];
    let mut queues = BTreeMap::<&str, std::collections::VecDeque<usize>>::new();
    let diverse = ["role_diverse", "combined"].contains(&variant);
    let cycle: &[&str] = if diverse {
        ROLE_DIVERSE_CYCLE
    } else {
        r.duty.delivery_cycle()
    };
    for (index, candidate) in ranked
        .iter()
        .enumerate()
        .filter(|(_, c)| c.score >= MIN_SCORE)
    {
        if candidate
            .signals
            .iter()
            .any(|s| s.facet == "anchors" && s.kind == "exact_anchor")
        {
            order.push(index);
            continue;
        }
        let role = ix.files[&candidate.path].role.as_str();
        if variant == "docs_disabled" && role == "doc" {
            continue;
        }
        let band = if cycle.is_empty() {
            "evidence"
        } else if ["impl", "test"].contains(&role) {
            role
        } else {
            "support"
        };
        queues.entry(band).or_default().push_back(index);
    }
    if cycle.is_empty() {
        order.extend(queues.remove("evidence").unwrap_or_default());
    } else {
        loop {
            let before = order.len();
            for band in cycle {
                if let Some(index) = queues.get_mut(*band).and_then(|queue| queue.pop_front()) {
                    order.push(index);
                }
            }
            if order.len() == before {
                break;
            }
        }
    }
    if ["reserved_floor", "combined"].contains(&variant) {
        let cap = r.max_files;
        let slots = if cap >= 4 { (cap / 8).clamp(1, 4) } else { 0 };
        if slots > 0 {
            let mut reserved: Vec<usize> = ranked
                .iter()
                .enumerate()
                .filter(|(index, c)| {
                    !order.contains(index)
                        && c.score >= 12
                        && c.signals
                            .iter()
                            .any(|s| !["lexical", "anchors"].contains(&s.facet.as_str()))
                })
                .map(|(index, _)| index)
                .collect();
            reserved.sort_by_key(|&index| {
                (
                    std::cmp::Reverse(ranked[index].score),
                    ranked[index].path.clone(),
                )
            });
            reserved.truncate(slots);
            if !reserved.is_empty() {
                let keep = order.len().min(cap.saturating_sub(reserved.len()));
                let mut next: Vec<_> = order[..keep].to_vec();
                let tail: Vec<_> = order[keep..]
                    .iter()
                    .copied()
                    .filter(|index| !reserved.contains(index))
                    .collect();
                next.extend(reserved);
                next.extend(tail);
                order = next;
            }
        }
    }
    if ["linked_diversity", "combined"].contains(&variant) {
        order = diversify(order, ranked);
    }
    order
}

pub(super) fn rank(ix: &Index, r: &Request, candidates: Candidates) -> Vec<Candidate> {
    let mut ranked: Vec<_> = candidates
        .into_values()
        .filter(|c| ix.files.contains_key(&c.path))
        .collect();
    let reach = r.selection_variant == "structural_reach";
    let links: BTreeMap<&str, &Link> = if r.selection_variant == "origin_fusion" {
        ix.traversal_links().map(|link| (link.id.as_str(), link)).collect()
    } else {
        BTreeMap::new()
    };
    for candidate in &mut ranked {
        corroborate(candidate, reach);
        if !links.is_empty() {
            candidate.score = fused_score(candidate, &links);
        }
        candidate.role = Some(ix.files[&candidate.path].role.clone());
        if r.duty
            .policy()
            .case
            .required_roles
            .contains(&ix.files[&candidate.path].role.as_str())
        {
            candidate.score += 6;
        }
    }
    ranked.sort_by_key(|c| (std::cmp::Reverse(c.score), c.path.clone()));
    for (i, candidate) in ranked.iter_mut().enumerate() {
        candidate.rank = i + 1;
        candidate.source_ref = format!("/candidates/{i}");
        candidate.selected = false;
        candidate.delivery_rank = None;
    }
    let order = delivery_order(ix, r, &ranked);
    for (position, index) in order.into_iter().take(r.max_files).enumerate() {
        ranked[index].selected = true;
        ranked[index].delivery_rank = Some(position + 1);
    }
    ranked
}

/// Retained declaration spans supplement lexical excerpts for graph-only
/// candidates. A declaration in a holding file does not relocate its target.
pub(super) fn localize(s: &Snapshot, r: &Request, candidates: &mut Candidates) -> Result<()> {
    let artifacts: BTreeMap<_, _> = rows(&s.view.bundle, "artifacts")
        .iter()
        .map(|a| (text(a, "id"), text(a, "path")))
        .collect();
    let references: BTreeMap<_, _> = rows(&s.view.bundle, "evidence_refs")
        .iter()
        .map(|e| (text(e, "id"), e))
        .collect();
    let entities: BTreeMap<_, _> = rows(&s.view.bundle, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), e))
        .collect();
    let terms = terms(&r.task);
    let mut spans = BTreeMap::<String, Vec<(usize, Value)>>::new();
    for a in rows(&s.view.bundle, "assertions").iter().filter(|a| {
        s.view.eligible.contains(&text(a, "id"))
            && a["class"] != "claim"
            && a["support_class"] != "includes_inference"
            && a["proposition"]["predicate"] == "DECLARES"
    }) {
        let Some(entity) = entities.get(&text(&a["proposition"]["object"], "entity_version_id"))
        else {
            continue;
        };
        if !["symbol", "function"].contains(&entity["kind"].as_str().unwrap_or("")) {
            continue;
        }
        for id in crate::architecture::contracts::strings(&a["evidence_ref_ids"]) {
            let Some(reference) = references.get(&id) else {
                continue;
            };
            let Some(path) = artifacts
                .get(&text(reference, "artifact_id"))
                .filter(|p| candidates.contains_key(*p))
            else {
                continue;
            };
            let Some(content) = s.content(path)? else {
                continue;
            };
            let locator = &reference["locator"];
            let (Some(start), Some(end)) = (locator["start"].as_u64(), locator["end"].as_u64())
            else {
                continue;
            };
            let (start, end) = (start as usize, end as usize);
            if locator["kind"] != "span"
                || start > end
                || end > content.len()
                || !content.is_char_boundary(start)
                || !content.is_char_boundary(end)
            {
                continue;
            }
            let symbol = text(entity, "canonical_key");
            let relevance = matches(&symbol, &terms);
            spans.entry(path.clone()).or_default().push((
                relevance,
                json!({"symbol":symbol,"evidence_ref_id":id,"assertion_id":a["id"],
                "byte_range":{"start":start,"end":end}}),
            ));
        }
    }
    for (path, mut located) in spans {
        located.sort_by_key(|(score, span)| {
            (
                std::cmp::Reverse(*score),
                span["byte_range"]["start"].as_u64(),
            )
        });
        let candidate = candidates.get_mut(&path).unwrap();
        for (_, mut span) in located.into_iter().take(3) {
            let content = s.content(&path)?.unwrap_or("");
            let start = span["byte_range"]["start"].as_u64().unwrap() as usize;
            let end = span["byte_range"]["end"].as_u64().unwrap() as usize;
            span["line_range"] = json!({"start":1+content[..start].bytes().filter(|b|*b==b'\n').count(),
                "end":1+content[..end].bytes().filter(|b|*b==b'\n').count()});
            if !candidate.spans.contains(&span) {
                candidate.spans.push(span);
            }
        }
        candidate.spans.truncate(4);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ix() -> Index {
        let files = ["src/a.rs", "src/b.rs", "src/proof.rs"]
            .into_iter()
            .map(|p| {
                (
                    p.into(),
                    Artifact {
                        path: p.into(),
                        artifact_id: p.into(),
                        digest: "sha256:fixture".into(),
                        role: "impl".into(),
                        language: "rust".into(),
                        bytes: 0,
                    },
                )
            })
            .collect();
        let mut link = Link::new(
            "CALLS_STATIC",
            "a",
            "b",
            "evidence",
            "fixture",
            vec![],
            vec![],
            BTreeSet::from(["src/a.rs".into()]),
            BTreeSet::from(["src/b.rs".into()]),
        )
        .unwrap();
        link.support_files.insert("src/proof.rs".into());
        Index {
            id: "ix".into(),
            source_view: "v".into(),
            retraction_epoch: 0,
            files,
            entities: BTreeMap::new(),
            links: vec![link],
            overflow_links: vec![],
            coverage: json!({}),
        }
    }

    #[test]
    fn coding_selection_balances_roles_without_rewriting_evidence_ranks_or_scores() {
        let mut ix = ix();
        let mut candidates = Candidates::new();
        for (role, count, strength) in [("doc", 30, 200), ("impl", 20, 50), ("test", 10, 70)] {
            for index in 0..count {
                let path = format!("{role}/{index}.txt");
                ix.files.insert(
                    path.clone(),
                    Artifact {
                        path: path.clone(),
                        artifact_id: path.clone(),
                        digest: "digest".into(),
                        role: role.into(),
                        language: "text".into(),
                        bytes: 0,
                    },
                );
                contribute(
                    &mut candidates,
                    &path,
                    signal("lexical", "text", strength, "candidate", 0, None, json!({})),
                );
            }
        }
        contribute(
            &mut candidates,
            "doc/0.txt",
            signal(
                "anchors",
                "exact_anchor",
                10000,
                "exact",
                0,
                None,
                json!({}),
            ),
        );
        let mut previous = Vec::<String>::new();
        for bound in [1, 4, 12, 32] {
            let request = super::super::request::parse(
                &json!({"task":"repair","duty":"debug","max_files":bound}),
            )
            .unwrap()
            .0;
            let ranked = rank(&ix, &request, candidates.clone());
            assert!(ranked.windows(2).all(|pair| pair[0].score >= pair[1].score));
            let mut selected: Vec<_> = ranked
                .iter()
                .filter(|candidate| candidate.selected)
                .collect();
            selected.sort_by_key(|candidate| candidate.delivery_rank);
            assert_eq!(selected.len(), bound);
            assert_eq!(selected[0].path, "doc/0.txt");
            let paths: Vec<_> = selected
                .iter()
                .map(|candidate| candidate.path.clone())
                .collect();
            assert!(paths.starts_with(&previous));
            previous = paths;
            if bound == 12 {
                assert!(
                    selected
                        .iter()
                        .filter(|candidate| ix.files[&candidate.path].role == "impl")
                        .count()
                        >= 5
                );
                assert!(
                    selected
                        .iter()
                        .filter(|candidate| ix.files[&candidate.path].role == "test")
                        .count()
                        >= 3
                );
                assert!(ranked
                    .iter()
                    .any(|candidate| !candidate.selected && candidate.path.starts_with("doc/")));
                assert!(selected.iter().any(|candidate| candidate.rank > 12));
            }
        }
        let request = super::super::request::parse(
            &json!({"task":"explain","duty":"explain","max_files":12}),
        )
        .unwrap()
        .0;
        let ranked = rank(&ix, &request, candidates);
        assert!(ranked.iter().take(12).all(|candidate| candidate.selected));
    }

    #[test]
    fn evaluation_paths_cannot_enter_through_hints_or_bridge_task_adjacency() {
        let mut ix = ix();
        let path = "EVALUATION_RERUN.md";
        ix.files.insert(
            path.into(),
            Artifact {
                path: path.into(),
                artifact_id: path.into(),
                digest: "digest".into(),
                role: "doc".into(),
                language: "text".into(),
                bytes: 0,
            },
        );
        ix.links.push(
            Link::new(
                "REFERENCES_ARTIFACT",
                path,
                "src/a.rs",
                "derived",
                "answer key",
                vec![],
                vec![],
                BTreeSet::from([path.into()]),
                BTreeSet::from(["src/a.rs".into()]),
            )
            .unwrap(),
        );
        let request = super::super::request::parse(&json!({"task":"repair","target":"src/a.rs"}))
            .unwrap()
            .0;
        let seed = super::super::trace::seed(&ix, "src/a.rs", true);
        let mut candidates = Candidates::new();
        for facet in ["lexical", "history", "sessions", "dependency"] {
            contribute(
                &mut candidates,
                path,
                signal(facet, "hint", 100, "candidate", 0, None, json!({})),
            );
        }
        contribute(
            &mut candidates,
            "src/a.rs",
            signal(
                "anchors",
                "exact_anchor",
                10000,
                "exact",
                0,
                None,
                json!({}),
            ),
        );
        expand(&ix, &request, &seed, &mut candidates);
        assert!(!candidates.contains_key(path));
        let seed = super::super::trace::seed(&ix, path, true);
        contribute(
            &mut candidates,
            path,
            signal(
                "anchors",
                "exact_anchor",
                10000,
                "exact",
                0,
                None,
                json!({}),
            ),
        );
        assert!(path_allowed(path, &seed));
        assert!(candidates.contains_key(path));
    }

    #[test]
    fn legacy_candidates_omit_the_new_optional_delivery_field_on_roundtrip() {
        let candidate = Candidate::new("src/a.rs");
        let original = serde_json::to_value(candidate).unwrap();
        assert!(original.get("delivery_rank").is_none());
        let decoded: Candidate = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), original);
    }
    #[test]
    fn shared_tests_are_direct_associations_not_transitive_implementation_bridges() {
        let mut ix = ix();
        let test = "tests/shared.rs";
        ix.files.insert(
            test.into(),
            Artifact {
                path: test.into(),
                artifact_id: test.into(),
                digest: "digest".into(),
                role: "test".into(),
                language: "rust".into(),
                bytes: 0,
            },
        );
        ix.links = ["src/a.rs", "src/b.rs"]
            .into_iter()
            .map(|implementation| {
                Link::new(
                    "TESTS",
                    test,
                    implementation,
                    "derived",
                    "shared suite",
                    vec![],
                    vec![],
                    BTreeSet::from([test.into()]),
                    BTreeSet::from([implementation.into()]),
                )
                .unwrap()
            })
            .collect();
        for anchor in ["src/a.rs", test] {
            let request = super::super::request::parse(
                &json!({"task":"repair","target":anchor,"duty":"extend","depth":2}),
            )
            .unwrap()
            .0;
            let seed = super::super::trace::seed(&ix, anchor, true);
            let mut candidates = Candidates::new();
            contribute(
                &mut candidates,
                anchor,
                signal(
                    "anchors",
                    "exact_anchor",
                    10000,
                    "exact",
                    0,
                    None,
                    json!({}),
                ),
            );
            expand(&ix, &request, &seed, &mut candidates);
            assert!(candidates.contains_key(test));
            assert!(candidates.contains_key("src/a.rs"));
            assert_eq!(
                candidates.contains_key("src/b.rs"),
                anchor == test,
                "{anchor}"
            );
        }
    }
    #[test]
    fn disabled_facets_do_not_penalize_enabled_facets_through_hub_degrees() {
        let mut ix = ix();
        ix.links.clear();
        ix.links.push(
            Link::new(
                "IMPORTS",
                "src/a.rs",
                "src/b.rs",
                "derived",
                "dependency",
                vec![],
                vec![],
                BTreeSet::from(["src/a.rs".into()]),
                BTreeSet::from(["src/b.rs".into()]),
            )
            .unwrap(),
        );
        for index in 0..20 {
            let path = format!("tests/{index}.rs");
            ix.files.insert(
                path.clone(),
                Artifact {
                    path: path.clone(),
                    artifact_id: path.clone(),
                    digest: "digest".into(),
                    role: "test".into(),
                    language: "rust".into(),
                    bytes: 0,
                },
            );
            ix.links.push(
                Link::new(
                    "TESTS",
                    &path,
                    "src/a.rs",
                    "derived",
                    "test",
                    vec![],
                    vec![],
                    BTreeSet::from([path.clone()]),
                    BTreeSet::from(["src/a.rs".into()]),
                )
                .unwrap(),
            );
        }
        let request = super::super::request::parse(
            &json!({"task":"repair","target":"src/a.rs","duty":"debug",
            "exclude_facets":["tests"]}),
        )
        .unwrap()
        .0;
        let seed = super::super::trace::seed(&ix, "src/a.rs", true);
        let mut candidates = Candidates::new();
        contribute(
            &mut candidates,
            "src/a.rs",
            signal(
                "anchors",
                "exact_anchor",
                10000,
                "exact",
                0,
                None,
                json!({}),
            ),
        );
        expand(&ix, &request, &seed, &mut candidates);
        let dependency = candidates["src/b.rs"]
            .signals
            .iter()
            .find(|signal| signal.facet == "dependency")
            .unwrap();
        assert_eq!(dependency.strength, 80);
        assert_eq!(dependency.provenance["hub_degree"], 1);
        assert!(candidates.keys().all(|path| !path.starts_with("tests/")));
    }
    #[test]
    fn provenance_files_are_never_traversal_endpoints_and_facets_can_be_ablated() {
        let ix = ix();
        for disabled in [false, true] {
            let mut args = json!({"task":"repair anchor","target":"src/a.rs","duty":"debug"});
            if disabled {
                args["exclude_facets"] = json!(["execution"]);
            }
            let request = super::super::request::parse(&args).unwrap().0;
            let seed = super::super::trace::seed(&ix, "src/a.rs", true);
            let mut candidates = Candidates::new();
            contribute(
                &mut candidates,
                "src/a.rs",
                signal(
                    "anchors",
                    "exact_anchor",
                    10000,
                    "exact",
                    0,
                    None,
                    json!({}),
                ),
            );
            expand(&ix, &request, &seed, &mut candidates);
            assert_eq!(candidates.contains_key("src/b.rs"), !disabled);
            assert!(!candidates.contains_key("src/proof.rs"));
        }
    }
    #[test]
    fn repeated_signals_do_not_outvote_independent_evidence_and_ties_are_stable() {
        let mut candidates = Candidates::new();
        for i in 0..100 {
            contribute(
                &mut candidates,
                "src/a.rs",
                signal(
                    "dependency",
                    "IMPORTS",
                    30,
                    "retained_relationship",
                    1,
                    Some(&format!("link:{i}")),
                    json!({}),
                ),
            );
        }
        assert_eq!(candidates["src/a.rs"].score, 30);
        contribute(
            &mut candidates,
            "src/b.rs",
            signal("lexical", "text", 20, "candidate", 0, None, json!({})),
        );
        contribute(
            &mut candidates,
            "src/b.rs",
            signal(
                "history",
                "cochange",
                20,
                "observational_candidate",
                0,
                None,
                json!({}),
            ),
        );
        let request = super::super::request::parse(&json!({"task":"repair anchor","max_files":1}))
            .unwrap()
            .0;
        let ranked = rank(&ix(), &request, candidates);
        assert_eq!(ranked[0].path, "src/b.rs");
        assert!(ranked[0].selected);
        assert!(!ranked[1].selected);
        assert!(ranked[0].score > ranked[1].score);
    }
    #[test]
    fn import_signals_corroborate_but_cannot_exceed_task_relevance() {
        let mut candidates = Candidates::new();
        let import = |strength| {
            signal(
                "dependency",
                "IMPORTS",
                strength,
                "retained_relationship",
                1,
                Some("link:hub"),
                json!({"basis":"retained_assertion"}),
            )
        };
        // a.rs: only reached through an import hub; b.rs: weakly matched text.
        contribute(&mut candidates, "src/a.rs", import(80));
        contribute(&mut candidates, "src/b.rs", import(80));
        contribute(
            &mut candidates,
            "src/b.rs",
            signal("lexical", "retained_text", 30, "candidate", 0, None, json!({})),
        );
        let request = super::super::request::parse(&json!({"task":"repair anchor"}))
            .unwrap()
            .0;
        let ranked = rank(&ix(), &request, candidates);
        let a = ranked.iter().find(|c| c.path == "src/a.rs").unwrap();
        let b = ranked.iter().find(|c| c.path == "src/b.rs").unwrap();
        assert!(!a.selected, "import-only reach is not task relevance");
        assert_eq!(b.score - a.score, 30 + 30 + 8);
        let capped = b.signals.iter().find(|s| s.kind == "IMPORTS").unwrap();
        assert_eq!(capped.strength, 30);
        assert_eq!(capped.provenance["uncapped_strength"], 80);
    }
    #[test]
    fn binding_reach_carries_one_hop_origin_relevance_and_baseline_keeps_the_cap() {
        let binding = |kind: &str, basis: &str, origin: u64| {
            signal("dependency", kind, 80, "retained_relationship", 1, Some("link:bind"),
                json!({"basis":basis,"origin_relevance":origin}))
        };
        let mut reached = Candidate::new("src/a.rs");
        reached.signals = vec![binding("REFERENCES", "retained_assertion; symbol_binding", 46)];
        let mut baseline = reached.clone();
        corroborate(&mut reached, true);
        corroborate(&mut baseline, false);
        assert_eq!((reached.score, baseline.score), (46, 0));
        assert_eq!(reached.signals[0].provenance["cap"], "origin_relevance_ceiling");
        // A compiler-resolved REFERENCES is not import-derived; a plain import
        // never borrows origin relevance.
        let mut other = Candidate::new("src/b.rs");
        other.signals = vec![binding("REFERENCES", "retained_assertion", 0), binding("IMPORTS", "retained_assertion", 46)];
        corroborate(&mut other, true);
        assert_eq!(other.score, 80);
        let mut test = Candidate::new("src/a.test.rs");
        test.signals = vec![signal("tests", "TESTS", 100, "retained_relationship", 1, Some("link:t"),
            json!({"basis":"test_SYMBOL_BINDING; static association, not executed coverage","origin_relevance":40}))];
        corroborate(&mut test, true);
        assert_eq!(test.score, 40);
    }
    #[test]
    fn common_origin_facets_add_only_the_facet_bonus() {
        let pair = BTreeSet::from(["src/a.rs".to_owned()]);
        let other = BTreeSet::from(["src/b.rs".to_owned()]);
        let calls = Link::new("CALLS_STATIC", "a::f", "b::g", "evidence", "retained_assertion",
            vec!["assertion:1".into()], vec![], pair.clone(), other.clone()).unwrap();
        let imports = Link::new("IMPORTS", "a", "b", "evidence", "retained_assertion",
            vec!["assertion:2".into()], vec![], pair.clone(), other.clone()).unwrap();
        let plan = Link::new("REFERENCES_ARTIFACT", "doc.md", "src/b.rs", "derived", "path_token",
            vec![], vec![], BTreeSet::from(["doc.md".to_owned()]), other.clone()).unwrap();
        let links: BTreeMap<&str, &Link> = [&calls, &imports, &plan].into_iter().map(|l| (l.id.as_str(), l)).collect();
        let mut candidate = Candidate::new("src/b.rs");
        candidate.signals = vec![
            signal("execution", "CALLS_STATIC", 95, "retained_relationship", 1, Some(&calls.id), json!({})),
            signal("dependency", "IMPORTS", 40, "retained_relationship", 1, Some(&imports.id), json!({})),
            signal("lexical", "retained_text", 40, "candidate", 0, None, json!({})),
        ];
        assert_eq!(facet_score(&candidate.signals), 95 + 40 + 40 + 16);
        assert_eq!(fused_score(&mut candidate, &links), 95 + 40 + 8 + 8);
        let correlated = candidate.signals.iter().find(|s| s.kind == "IMPORTS").unwrap();
        assert_eq!(correlated.provenance["correlated_origin"], true);
        // A distinct origin in the same facet keeps the facet independent.
        candidate.signals.push(signal("dependency", "REFERENCES_ARTIFACT", 30, "retained_relationship", 1, Some(&plan.id), json!({})));
        assert_eq!(fused_score(&mut candidate, &links), 95 + 40 + 40 + 16);
    }
    #[test]
    fn overflow_only_binding_link_reaches_a_candidate() {
        let mut ix = ix();
        ix.links.clear();
        ix.overflow_links = vec![Link::new(
            "REFERENCES", "a::f", "b::g", "evidence",
            "retained_assertion; symbol_binding",
            vec!["assertion:overflow".into()], vec![],
            BTreeSet::from(["src/a.rs".into()]),
            BTreeSet::from(["src/b.rs".into()]),
        ).unwrap()];
        let request = super::super::request::parse(&json!({"task":"repair","target":"src/a.rs"}))
            .unwrap().0;
        let seed = super::super::trace::seed(&ix, "src/a.rs", true);
        let mut candidates = Candidates::new();
        contribute(&mut candidates, "src/a.rs",
            signal("lexical", "retained_text", 100, "candidate", 0, None, json!({})));
        expand(&ix, &request, &seed, &mut candidates);
        let reached = candidates.get("src/b.rs")
            .expect("overflow binding edge must reach its target file");
        assert!(reached.signals.iter().any(|s| s.kind == "REFERENCES"
            && s.provenance["basis"] == "retained_assertion; symbol_binding"));
        // Same edge absent from both sets reaches nothing.
        ix.overflow_links.clear();
        let mut candidates = Candidates::new();
        contribute(&mut candidates, "src/a.rs",
            signal("lexical", "retained_text", 100, "candidate", 0, None, json!({})));
        expand(&ix, &request, &seed, &mut candidates);
        assert!(!candidates.contains_key("src/b.rs"));
    }
    fn variant_ix() -> (Index, Candidates) {
        let mut ix = ix();
        let mut candidates = Candidates::new();
        let add = |ix: &mut Index,
                   candidates: &mut Candidates,
                   path: &str,
                   role: &str,
                   strength: u64,
                   facet: &str| {
            ix.files.insert(
                path.into(),
                Artifact {
                    path: path.into(),
                    artifact_id: path.into(),
                    digest: "d".into(),
                    role: role.into(),
                    language: "text".into(),
                    bytes: 0,
                },
            );
            contribute(
                candidates,
                path,
                signal(facet, "fixture", strength, "candidate", 0, None, json!({})),
            );
        };
        // Strong docs crowd the baseline cap; one weak structural impl is below floor 24.
        for i in 0..6 {
            add(
                &mut ix,
                &mut candidates,
                &format!("docs/d{i}.md"),
                "doc",
                100,
                "lexical",
            );
        }
        add(
            &mut ix,
            &mut candidates,
            "pkgs/a/impl.ts",
            "impl",
            90,
            "lexical",
        );
        add(
            &mut ix,
            &mut candidates,
            "pkgs/b/impl.ts",
            "impl",
            80,
            "lexical",
        );
        add(
            &mut ix,
            &mut candidates,
            "pkgs/a/impl.test.ts",
            "test",
            70,
            "lexical",
        );
        add(
            &mut ix,
            &mut candidates,
            "pkgs/c/weak.ts",
            "impl",
            15,
            "dependency",
        );
        (ix, candidates)
    }
    fn selected(ranked: &[Candidate]) -> Vec<String> {
        let mut rows: Vec<_> = ranked.iter().filter(|c| c.selected).collect();
        rows.sort_by_key(|c| c.delivery_rank);
        rows.iter().map(|c| c.path.clone()).collect()
    }

    #[test]
    fn variant_reserved_floor_admits_low_score_structural_candidates() {
        let (ix, candidates) = variant_ix();
        let request = super::super::request::parse(&json!({"task": "repair", "duty": "debug", "max_files": 4, "selection_variant": "reserved_floor"})).unwrap().0;
        assert!(selected(&rank(&ix, &request, candidates.clone()))
            .contains(&"pkgs/c/weak.ts".to_string()));
        let baseline = super::super::request::parse(
            &json!({"task": "repair", "duty": "debug", "max_files": 4}),
        )
        .unwrap()
        .0;
        assert!(
            !selected(&rank(&ix, &baseline, candidates)).contains(&"pkgs/c/weak.ts".to_string())
        );
    }

    #[test]
    fn variant_role_diverse_and_docs_disabled_change_explain_selection() {
        let (ix, candidates) = variant_ix();
        let diverse = super::super::request::parse(&json!({"task": "explain", "duty": "explain", "max_files": 4, "selection_variant": "role_diverse"})).unwrap().0;
        let picked = selected(&rank(&ix, &diverse, candidates.clone()));
        assert!(picked.iter().any(|p| p.ends_with(".test.ts")), "{picked:?}");
        let nodocs = super::super::request::parse(&json!({"task": "explain", "duty": "explain", "max_files": 4, "selection_variant": "docs_disabled"})).unwrap().0;
        assert!(selected(&rank(&ix, &nodocs, candidates))
            .iter()
            .all(|p| !p.starts_with("docs/")));
    }

    #[test]
    fn variant_linked_diversity_interleaves_packages() {
        let (ix, candidates) = variant_ix();
        let request = super::super::request::parse(&json!({"task": "repair", "duty": "explain", "max_files": 3, "selection_variant": "linked_diversity"})).unwrap().0;
        let picked = selected(&rank(&ix, &request, candidates));
        assert_ne!(
            package_root(&picked[1]),
            package_root(&picked[0]),
            "{picked:?}"
        );
    }

    #[test]
    fn companion_tests_pair_by_evidence_base() {
        let all: Vec<String> = [
            "src/foo.ts",
            "src/foo.test.ts",
            "src/foo.spec.tsx",
            "src/bar.ts",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            companion_test_paths("src/foo.ts", &all),
            vec![
                "src/foo.spec.tsx".to_string(),
                "src/foo.test.ts".to_string()
            ]
        );
        assert_eq!(package_root("apps/server/src/x.ts"), "apps/server");
        assert_eq!(package_root("top.rs"), ".");
    }

    #[test]
    fn term_plan_keeps_late_identifiers_and_their_parts_ahead_of_prose() {
        let prose: Vec<_> = (0..40).map(|i| format!("word{i:02}x")).collect();
        let (terms, omitted) =
            term_plan(&format!("{} fix `zz_late` in parseConfig", prose.join(" ")));
        assert_eq!(
            terms[..5],
            ["zz_late", "parseconfig", "late", "parse", "config"]
        );
        assert_eq!(terms.len(), 32);
        assert_eq!(omitted.len(), 40 + 5 - 32);
        assert!(omitted.iter().all(|t| t.starts_with("word")));
    }
}
