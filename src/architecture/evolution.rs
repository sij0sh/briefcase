//! Retained change history: how code and captured architecture change over time
//! (ADR-0025).
//!
//! Four sections keep their different epistemic standing apart:
//! - `lineage` composes `history-lineage` between two captured views: exact
//!   `RENAMED_FROM`, inferred `LIKELY_SAME_ENTITY` and ambiguous candidates;
//! - `activity` and `coupling` are reproducible statistics over retained Git
//!   metadata. They are derived history metrics, never Evidence. Co-change is
//!   correlation, not dependency;
//! - `architecture_changes` composes `compare` between two captured views.
//!
//! A Git path rename is display continuity only, never canonical lineage.
//! The operation reads; it persists the report and its explanation handles.
use super::{
    capture,
    contracts::{rows, strings, text},
    git_history, history, identity,
    ledger::{HistoricalCommit, Ledger, View},
    query::{self, all_rows, Options, Row},
    require,
    source::module_path,
    Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const ACTIVITY_METHOD: &str = "change-activity-v1";
pub const COUPLING_METHOD: &str = "change-coupling-v1";
pub const DEFAULT_MIN_SHARED: usize = 2;
/// Newest commit OIDs kept as witnesses of each activity or coupling entry.
pub const MAX_WITNESSES: usize = 5;
const LOCK_FILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "bun.lockb",
    "go.sum",
    "go.work.sum",
    "gradle.lockfile",
    "packages.lock.json",
];
const VENDORED: &[&str] = &[
    "vendor",
    "third_party",
    "third-party",
    "bower_components",
    ".yarn",
    "Pods",
];
const GENERATED: &[&str] = &[
    ".min.js",
    ".min.css",
    ".pb.go",
    "_pb2.py",
    "_pb2_grpc.py",
    ".g.cs",
    ".designer.cs",
    ".Designer.cs",
];
const TEXT_LIMITATIONS: usize = 12;
const NOT_DEPENDENCY: &str = "Co-change is correlation over retained commits, not an architectural dependency; it never implies DEPENDS_ON.";

/// Optional comparison view, time range and coupling threshold.
#[derive(Clone, Debug)]
pub struct Request {
    pub base: Option<String>,
    /// Inclusive lower bound: Unix seconds, `YYYY-MM-DD` or RFC 3339.
    pub since: Option<String>,
    /// Exclusive upper bound, in the same forms.
    pub until: Option<String>,
    pub min_shared: usize,
}
impl Default for Request {
    fn default() -> Self {
        Self {
            base: None,
            since: None,
            until: None,
            min_shared: DEFAULT_MIN_SHARED,
        }
    }
}

/// Why a retained path is left out of activity and coupling, if it is.
pub(crate) fn noise(path: &str) -> Option<&'static str> {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or(path);
    if LOCK_FILES.contains(&name) || name.ends_with(".lock") {
        Some("lock_file")
    } else if parts.iter().any(|d| VENDORED.contains(d)) {
        Some("vendored")
    } else if parts.iter().any(|d| capture::EXCLUDED.contains(d)) {
        Some("excluded_directory")
    } else if GENERATED.iter().any(|s| name.ends_with(s)) || name.contains(".generated.") {
        Some("generated")
    } else {
        None
    }
}

pub(crate) fn configuration(min_shared: usize) -> Value {
    json!({"activity":ACTIVITY_METHOD,"coupling":COUPLING_METHOD,"exclude_merges":true,
        "exclude_incomplete_commits_from_coupling":true,"min_shared":min_shared,"max_witnesses":MAX_WITNESSES,
        "path_normalization":"literal retained Git paths; Git renames are listed, never merged",
        "excluded_paths":{"lock_files":LOCK_FILES,"lock_suffix":".lock","vendored_directories":VENDORED,
            "excluded_directories":capture::EXCLUDED,"generated_suffixes":GENERATED,"generated_infix":".generated."},
        "implementation_digest":identity::bytes_digest(include_bytes!("evolution.rs")),
        "source_implementation_digest":identity::bytes_digest(include_bytes!("source.rs")),
        "query_implementation_digest":identity::bytes_digest(include_bytes!("query.rs"))})
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Activity {
    pub path: String,
    pub commits: usize,
    pub first_seen: i64,
    pub last_seen: i64,
    pub witnesses: Vec<String>,
}
impl Activity {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            commits: 0,
            first_seen: i64::MAX,
            last_seen: i64::MIN,
            witnesses: vec![],
        }
    }
    fn bump(&mut self, c: &HistoricalCommit) {
        self.commits += 1;
        self.first_seen = self.first_seen.min(c.timestamp);
        self.last_seen = self.last_seen.max(c.timestamp);
        if self.witnesses.len() < MAX_WITNESSES {
            self.witnesses.push(c.oid.clone());
        }
    }
}

/// Unordered co-change of `from` and `to`, oriented from the focus when there
/// is one. Counts are over coupling-eligible commits only.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Coupling {
    pub from: String,
    pub to: String,
    pub shared: usize,
    pub from_commits: usize,
    pub to_commits: usize,
    pub witnesses: Vec<String>,
}
fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        (n as f64 / d as f64 * 10_000.0).round() / 10_000.0
    }
}
impl Coupling {
    /// P(to changes | from changes).
    pub fn forward(&self) -> f64 {
        ratio(self.shared, self.from_commits)
    }
    /// P(from changes | to changes).
    pub fn backward(&self) -> f64 {
        ratio(self.shared, self.to_commits)
    }
    pub fn jaccard(&self) -> f64 {
        ratio(
            self.shared,
            self.from_commits + self.to_commits - self.shared,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GitRename {
    pub from: String,
    pub to: String,
    /// `rename` or `copy`, as Git's rename detection reported it.
    pub kind: &'static str,
    pub commit: String,
    pub timestamp: i64,
}

#[derive(Debug, Default)]
pub(crate) struct Analysis {
    /// Commits inside the time range.
    pub considered: usize,
    pub merges: usize,
    /// Non-merge commits whose changed files exceeded the retention bound.
    pub incomplete: usize,
    /// Incomplete commits whose retained files touch the focus.
    pub focus_incomplete: usize,
    pub coupling_commits: usize,
    pub excluded: BTreeMap<&'static str, BTreeSet<String>>,
    pub activity: Vec<Activity>,
    /// The focus as one unit (each commit counted once), when there is a focus.
    pub total: Option<Activity>,
    pub coupling: Vec<Coupling>,
    pub renames: Vec<GitRename>,
}

/// The focus: a component name and the retained paths it covers.
pub(crate) type Focus<'a> = Option<(&'a str, &'a dyn Fn(&str) -> bool)>;

/// Activity and coupling over retained commits, newest first. Merges are
/// excluded (their first-parent diff repeats the merged commits). Commits
/// whose changed files exceeded the retention bound count toward activity
/// with their retained files but never toward coupling, so no pair is
/// counted from an incomplete change set. With a focus, its paths fold into
/// one node and only pairs with that node are counted.
pub(crate) fn analyze(
    commits: &[HistoricalCommit],
    since: Option<i64>,
    until: Option<i64>,
    focus: Focus,
    min_shared: usize,
) -> Analysis {
    let inside = |p: &str| focus.is_none_or(|(_, f)| f(p));
    let mut a = Analysis::default();
    let mut activity: BTreeMap<String, Activity> = BTreeMap::new();
    let mut total = focus.map(|(key, _)| Activity::new(key));
    // Paths are interned: unfocused pair counts can be large.
    let mut names: Vec<String> = Vec::new();
    let mut index: HashMap<String, u32> = HashMap::new();
    let mut changes: HashMap<u32, usize> = HashMap::new();
    let mut pairs: HashMap<(u32, u32), (usize, Vec<u32>)> = HashMap::new();
    let mut oids: Vec<&str> = Vec::new();
    for c in commits {
        if since.is_some_and(|s| c.timestamp < s) || until.is_some_and(|u| c.timestamp >= u) {
            continue;
        }
        a.considered += 1;
        if c.parents.len() > 1 {
            a.merges += 1;
            continue;
        }
        let complete = c.files_total <= c.changes.len();
        if !complete {
            a.incomplete += 1;
        }
        let mut paths = BTreeSet::new();
        for ch in &c.changes {
            if let Some(kind) = noise(&ch.path) {
                a.excluded.entry(kind).or_default().insert(ch.path.clone());
                continue;
            }
            paths.insert(ch.path.as_str());
            if let Some(old) = &ch.old_path {
                if inside(old) || inside(&ch.path) {
                    a.renames.push(GitRename {
                        from: old.clone(),
                        to: ch.path.clone(),
                        kind: if ch.status == "C" { "copy" } else { "rename" },
                        commit: c.oid.clone(),
                        timestamp: c.timestamp,
                    });
                }
            }
        }
        let mut touched = false;
        for p in paths.iter().filter(|p| inside(p)) {
            touched = true;
            activity
                .entry((*p).to_owned())
                .or_insert_with(|| Activity::new(p))
                .bump(c);
        }
        if touched {
            if let Some(t) = &mut total {
                t.bump(c);
            }
            if !complete {
                a.focus_incomplete += 1;
            }
        }
        if !complete {
            continue;
        }
        a.coupling_commits += 1;
        let commit = oids.len() as u32;
        oids.push(&c.oid);
        let mut nodes = BTreeSet::new();
        for p in &paths {
            let name = match focus {
                Some((key, f)) if f(p) => key,
                _ => p,
            };
            let id = *index.entry(name.to_owned()).or_insert_with(|| {
                names.push(name.to_owned());
                names.len() as u32 - 1
            });
            nodes.insert(id);
        }
        for n in &nodes {
            *changes.entry(*n).or_default() += 1;
        }
        let mut count = |x: u32, y: u32| {
            let e = pairs.entry((x, y)).or_default();
            e.0 += 1;
            if e.1.len() < MAX_WITNESSES {
                e.1.push(commit);
            }
        };
        match focus {
            Some((key, _)) => {
                if let Some(&f) = index.get(key).filter(|f| nodes.contains(f)) {
                    for n in nodes.iter().filter(|n| **n != f) {
                        count(f, *n);
                    }
                }
            }
            None => {
                let nodes: Vec<u32> = nodes.into_iter().collect();
                for (i, x) in nodes.iter().enumerate() {
                    for y in &nodes[i + 1..] {
                        // Unfocused pairs are ordered by path.
                        if names[*x as usize] < names[*y as usize] {
                            count(*x, *y);
                        } else {
                            count(*y, *x);
                        }
                    }
                }
            }
        }
    }
    a.activity = activity.into_values().collect();
    a.activity.sort_by(|x, y| {
        (y.commits, y.last_seen)
            .cmp(&(x.commits, x.last_seen))
            .then_with(|| x.path.cmp(&y.path))
    });
    a.total = total.filter(|t| t.commits > 0);
    a.coupling = pairs
        .into_iter()
        .filter(|(_, (shared, _))| *shared >= min_shared)
        .map(|((x, y), (shared, witnesses))| Coupling {
            from: names[x as usize].clone(),
            to: names[y as usize].clone(),
            shared,
            from_commits: changes[&x],
            to_commits: changes[&y],
            witnesses: witnesses
                .into_iter()
                .map(|w| oids[w as usize].to_owned())
                .collect(),
        })
        .collect();
    a.coupling.sort_by(|x, y| {
        let strength = |c: &Coupling| c.forward().max(c.backward());
        y.shared
            .cmp(&x.shared)
            .then_with(|| strength(y).total_cmp(&strength(x)))
            .then_with(|| (&x.from, &x.to).cmp(&(&y.from, &y.to)))
    });
    a
}

/// Unix seconds from Unix seconds, `YYYY-MM-DD` (midnight UTC) or RFC 3339.
pub(crate) fn instant(value: &str) -> Result<i64> {
    if let Ok(n) = value.trim().parse::<i64>() {
        return Ok(n);
    }
    let full = if value.len() == 10 {
        format!("{value}T00:00:00Z")
    } else {
        value.to_owned()
    };
    super::runtime_observations::seconds(&full).map_err(|_| {
        format!("EVOLUTION: Invalid time {value}; use Unix seconds, YYYY-MM-DD or RFC 3339").into()
    })
}
/// Second-precision civil UTC time.
pub(crate) fn civil(seconds: i64) -> String {
    let t = super::runtime_observations::timestamp(seconds as i128 * 1_000_000_000);
    format!("{}Z", &t[..19])
}

/// Retained files of the selected view and the files that hold each entity.
/// File keys are paths (`file::<path>` for some producers). An entity maps to
/// the files that CONTAIN or DECLARE it, or to its own key's path when that
/// is a retained file.
struct Files {
    paths: BTreeSet<String>,
    /// Entity version ID → files.
    of: BTreeMap<String, BTreeSet<String>>,
    /// Canonical key → files.
    by_key: BTreeMap<String, BTreeSet<String>>,
    /// File → canonical keys of the modules it holds.
    modules: BTreeMap<String, BTreeSet<String>>,
}
impl Files {
    fn new(v: &View) -> Self {
        let entities = rows(&v.bundle, "entity_versions");
        let mut ids = BTreeMap::new();
        for e in entities.iter().filter(|e| e["kind"] == "file") {
            let key = text(e, "canonical_key");
            ids.insert(
                text(e, "id"),
                key.strip_prefix("file::").unwrap_or(&key).to_owned(),
            );
        }
        let paths: BTreeSet<String> = ids.values().cloned().collect();
        let mut of: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (id, path) in &ids {
            of.entry(id.clone()).or_default().insert(path.clone());
        }
        for a in rows(&v.bundle, "assertions").iter().filter(|a| {
            a["class"] == "evidence"
                && ["CONTAINS", "DECLARES"]
                    .contains(&a["proposition"]["predicate"].as_str().unwrap_or_default())
        }) {
            if let Some(path) = ids.get(&text(&a["proposition"], "subject_id")) {
                of.entry(text(&a["proposition"]["object"], "entity_version_id"))
                    .or_default()
                    .insert(path.clone());
            }
        }
        for e in entities {
            let key = text(e, "canonical_key");
            let path = module_path(&key);
            if paths.contains(path) {
                of.entry(text(e, "id")).or_default().insert(path.to_owned());
            }
        }
        let mut by_key: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut modules: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for e in entities {
            if let Some(p) = of.get(&text(e, "id")) {
                let key = text(e, "canonical_key");
                if e["kind"] == "module" {
                    for path in p {
                        modules.entry(path.clone()).or_default().insert(key.clone());
                    }
                }
                by_key.entry(key).or_default().extend(p.iter().cloned());
            }
        }
        Self {
            paths,
            of,
            by_key,
            modules,
        }
    }
}

/// How `--component` selected retained paths.
struct Resolution {
    /// `all`, `path`, `subtree`, `entity`, `retained_history_path`,
    /// `ambiguous`, `no_retained_file` or `unresolved`.
    status: &'static str,
    /// The focus node's name in coupling: the path, prefix or component.
    label: Option<String>,
    prefix: Option<String>,
    paths: BTreeSet<String>,
    /// The canonical key the component resolved to in the view.
    entity: Option<String>,
    candidates: Vec<String>,
}
fn under(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}
impl Resolution {
    fn inside(&self, path: &str) -> bool {
        self.status == "all"
            || self.paths.contains(path)
            || self.prefix.as_deref().is_some_and(|p| under(path, p))
    }
    fn resolved(&self) -> bool {
        !["ambiguous", "unresolved", "no_retained_file"].contains(&self.status)
    }
}
fn resolve(
    v: &View,
    files: &Files,
    component: Option<&str>,
    history: &[HistoricalCommit],
) -> Resolution {
    let mut r = Resolution {
        status: "all",
        label: None,
        prefix: None,
        paths: BTreeSet::new(),
        entity: None,
        candidates: vec![],
    };
    let Some(raw) = component else {
        return r;
    };
    let c = raw.trim().trim_start_matches("./").trim_end_matches('/');
    r.label = Some(c.to_owned());
    if files.paths.contains(c) {
        r.status = "path";
        r.paths.insert(c.to_owned());
        return r;
    }
    if !c.is_empty() && files.paths.iter().any(|p| under(p, c)) {
        r.status = "subtree";
        r.prefix = Some(c.to_owned());
        return r;
    }
    let keys = query::entities(v);
    let ids = query::selected(v, raw);
    if ids.len() == 1 {
        r.entity = Some(keys[&ids[0]].clone());
        r.paths = files.of.get(&ids[0]).cloned().unwrap_or_default();
        r.status = if r.paths.is_empty() {
            "no_retained_file"
        } else {
            "entity"
        };
        if r.paths.len() == 1 {
            r.label = r.paths.first().cloned();
        }
    } else if ids.len() > 1 {
        r.status = "ambiguous";
        r.candidates = ids.iter().map(|id| keys[id].clone()).collect();
    } else if !c.is_empty()
        && history
            .iter()
            .flat_map(|h| &h.changes)
            .any(|ch| under(&ch.path, c) || ch.old_path.as_deref().is_some_and(|o| under(o, c)))
    {
        r.status = "retained_history_path";
        r.prefix = Some(c.to_owned());
    } else {
        r.status = "unresolved";
    }
    r
}

/// Static `DEPENDS_ON` in the selected view, lifted to the files holding
/// each side, both directions.
struct Dependencies {
    edges: BTreeMap<(String, String), Vec<String>>,
}
impl Dependencies {
    fn new(l: &Ledger, v: &View, files: &Files, historical: bool) -> Result<Self> {
        let removed = l.retracted()?;
        let none = BTreeSet::new();
        let mut edges: BTreeMap<_, Vec<String>> = BTreeMap::new();
        for a in query::eligible(v, &removed, historical, "DEPENDS_ON") {
            let p = &a["proposition"];
            let from = files.of.get(&text(p, "subject_id")).unwrap_or(&none);
            let to = files
                .of
                .get(&text(&p["object"], "entity_version_id"))
                .unwrap_or(&none);
            for f in from {
                for t in to.iter().filter(|t| *t != f) {
                    edges
                        .entry((f.clone(), t.clone()))
                        .or_default()
                        .push(text(a, "id"));
                }
            }
        }
        Ok(Self { edges })
    }
    /// Whether a static dependency is established between the focus's files and `to`.
    fn between(&self, files: &Files, from: &[String], to: &str) -> Value {
        let from: Vec<_> = from.iter().filter(|p| files.paths.contains(*p)).collect();
        if from.is_empty() || !files.paths.contains(to) {
            return json!({"status":"not_applicable","reason":"Both sides must be retained files of the selected view."});
        }
        let mut forward = Vec::new();
        let mut backward = Vec::new();
        for f in from {
            forward.extend(
                self.edges
                    .get(&(f.clone(), to.to_owned()))
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            backward.extend(
                self.edges
                    .get(&(to.to_owned(), f.clone()))
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
        }
        if forward.is_empty() && backward.is_empty() {
            return json!({"status":"not_established","reason":"No eligible DEPENDS_ON between entities of these files in the selected view; the co-change is not explained by a known static dependency."});
        }
        let direction = match (forward.is_empty(), backward.is_empty()) {
            (false, false) => "both",
            (false, true) => "from_to",
            _ => "to_from",
        };
        forward.extend(backward);
        forward.sort();
        forward.dedup();
        json!({"status":"present","direction":direction,"assertion_ids":forward})
    }
}

fn bounded(mut entries: Vec<Value>, limit: usize) -> (Vec<Value>, usize, bool) {
    let total = entries.len();
    entries.truncate(limit);
    (entries, total, total > limit)
}
fn sub_options(o: &Options, view: &str) -> Options {
    Options {
        view: view.to_owned(),
        component: None,
        limit: 1000,
        ..o.clone()
    }
}
fn handle_of(row: &Row) -> Value {
    if row.handle.is_empty() {
        Value::Null
    } else {
        json!(row.handle)
    }
}
/// Rows of a captured-snapshot report whose keys are held by a file of the
/// component in either view.
fn touches(r: &Resolution, views: &[&Files], d: &Value) -> bool {
    let keys = ["from", "to", "from_key"]
        .iter()
        .filter_map(|k| d[*k].as_str().map(str::to_owned))
        .chain(strings(&d["from_keys"]))
        .chain(strings(&d["candidate_keys"]));
    r.status == "all"
        || keys.into_iter().any(|k| {
            r.inside(module_path(&k))
                || views
                    .iter()
                    .filter_map(|f| f.by_key.get(&k))
                    .flatten()
                    .any(|p| r.inside(p))
        })
}
fn lineage_entry(row: &Row, before: &str, after: &str) -> Value {
    let d = &row.detail;
    if row.conclusion == "present" {
        json!({"predicate":d["predicate"],"certainty_class":d["certainty_class"],"change":d["change"],
            "from":d["from"],"to":d["to"],"before_view":before,"after_view":after,
            "assertion_ids":row.ids,"explanation_handle":handle_of(row),
            "method":d.get("method").cloned().unwrap_or(Value::Null),
            "inference_id":d.get("inference_id").cloned().unwrap_or(Value::Null),
            "limitations":[d["limitation"]]})
    } else {
        let from = match d["from_keys"].as_array() {
            Some(keys) => json!(keys),
            None => json!([d["from_key"]]),
        };
        json!({"predicate":Value::Null,"certainty_class":"ambiguous_candidates","change":d["change"],
            "from":from,"to":d["candidate_keys"],"before_view":before,"after_view":after,
            "assertion_ids":row.ids,"explanation_handle":handle_of(row),
            "limitations":[d["limitation"],"No candidate is promoted to lineage."]})
    }
}

/// The evolution report for one source view, optionally scoped to a
/// component and compared against an earlier captured view.
pub fn evolution(l: &Ledger, o: &Options, r: &Request) -> Result<Value> {
    require(
        r.min_shared >= 1,
        "EVOLUTION",
        "min_shared must be at least 1",
    )?;
    let since = r.since.as_deref().map(instant).transpose()?;
    let until = r.until.as_deref().map(instant).transpose()?;
    require(
        since.zip(until).is_none_or(|(s, u)| s < u),
        "EVOLUTION",
        "since must be earlier than until",
    )?;
    let v = query::loaded(l, o)?;
    require(
        v.scope == "source",
        "SCOPE",
        "--view must select a source publication",
    )?;
    let commits = l.history_commits()?;
    let tip = l.history_tip()?;
    let files = Files::new(&v);
    let resolution = resolve(&v, &files, o.component.as_deref(), &commits);
    let mut limitations: Vec<String> = query::restrictions(l, &v, o)?;
    let mut unknowns: Vec<Value> = Vec::new();
    if !resolution.resolved() {
        let reason = match resolution.status {
            "ambiguous" => format!(
                "Component {} matches {} retained identities; select one file, path prefix or entity.",
                o.component.as_deref().unwrap_or_default(),
                resolution.candidates.len()
            ),
            "no_retained_file" => {
                "The component's entity is held by no retained file of the selected view.".into()
            }
            _ => "The component matches no retained file, entity or retained Git path.".into(),
        };
        unknowns.push(json!({"kind":"component_resolution",
            "state":if resolution.status == "ambiguous" {"ambiguous"} else {"unknown"},
            "reason":reason,"candidates":resolution.candidates}));
    }
    if resolution.status == "retained_history_path" {
        limitations.push("The component is not a file of the selected view; only retained Git paths under it were matched.".into());
    }
    if resolution.status == "entity" {
        limitations.push("History is per file: the files holding the component's entity are reported, not symbol-level change.".into());
    }
    // ---- Activity and coupling over retained Git history -------------------------------
    let history = !commits.is_empty();
    let inside = |p: &str| resolution.inside(p);
    let focus: Focus = match &resolution.label {
        Some(k) if resolution.resolved() => Some((k.as_str(), &inside)),
        _ => None,
    };
    let analysis = if history && resolution.resolved() {
        analyze(&commits, since, until, focus, r.min_shared)
    } else {
        Analysis::default()
    };
    let deps = Dependencies::new(l, &v, &files, o.historical)?;
    let focus_files: Vec<String> = match focus {
        Some(_) => files
            .paths
            .iter()
            .filter(|p| resolution.inside(p))
            .cloned()
            .collect(),
        None => vec![],
    };
    if !history {
        unknowns.push(json!({"kind":"history_unavailable","state":"unknown",
            "reason":"No Git history is retained; run `briefcase sync` in a Git repository. Activity and coupling are unknown, not absent."}));
    }
    if analysis.incomplete > 0 {
        unknowns.push(json!({"kind":"incomplete_commits","state":"unknown",
            "reason":format!("{} commit(s) changed more than {} files; their unretained files are not counted and they are excluded from coupling.",
                analysis.incomplete, git_history::MAX_FILES_PER_COMMIT)}));
    }
    let activity_entries: Vec<Value> = analysis
        .activity
        .iter()
        .map(|a| json!({"path":a.path,"commits":a.commits,"first_seen":a.first_seen,"last_seen":a.last_seen,
            "first_seen_at":civil(a.first_seen),"last_seen_at":civil(a.last_seen),"witness_commits":a.witnesses,
            "class":"derived_history_metric","method":ACTIVITY_METHOD}))
        .collect();
    let (activity_entries, activity_total, activity_truncated) = bounded(activity_entries, o.limit);
    let coupling_entries: Vec<Value> = analysis
        .coupling
        .iter()
        .map(|c| {
            let from: Vec<String> = if focus.is_some() {
                focus_files.clone()
            } else {
                vec![c.from.clone()]
            };
            json!({"kind":"change_coupling","from":c.from,"to":c.to,"shared_commits":c.shared,
                "from_commits":c.from_commits,"to_commits":c.to_commits,
                "from_to":c.forward(),"to_from":c.backward(),"jaccard":c.jaccard(),
                "witness_commits":c.witnesses,"static_dependency":deps.between(&files, &from, &c.to),
                "from_modules":if focus.is_some() {BTreeSet::new()} else {files.modules.get(&c.from).cloned().unwrap_or_default()},
                "to_modules":files.modules.get(&c.to).cloned().unwrap_or_default(),
                "class":"derived_history_metric","method":COUPLING_METHOD})
        })
        .collect();
    let (coupling_entries, coupling_total, coupling_truncated) = bounded(coupling_entries, o.limit);
    let renames: Vec<Value> = analysis
        .renames
        .iter()
        .map(|g| {
            json!({"from":g.from,"to":g.to,"kind":g.kind,"commit":g.commit,"timestamp":g.timestamp,
            "at":civil(g.timestamp),"class":"git_path_rename"})
        })
        .collect();
    let (renames, renames_total, renames_truncated) = bounded(renames, o.limit);
    let total = analysis.total.as_ref().map(|t| {
        json!({"component":t.path,"commits":t.commits,"first_seen":t.first_seen,"last_seen":t.last_seen,
            "first_seen_at":civil(t.first_seen),"last_seen_at":civil(t.last_seen),"witness_commits":t.witnesses})
    });
    let evaluated = history && resolution.resolved();
    let metrics = |entries: bool| {
        if !history {
            "unavailable"
        } else if !resolution.resolved() {
            "not_evaluated"
        } else if entries {
            "available"
        } else {
            "no_changes_in_scope"
        }
    };
    let activity_complete = evaluated && analysis.focus_incomplete == 0 && !activity_truncated;
    let coupling_complete = evaluated && analysis.incomplete == 0 && !coupling_truncated;
    let mut activity = json!({"class":"derived_history_metric","method":ACTIVITY_METHOD,
        "status":metrics(activity_total > 0),"outcome":section_outcome(evaluated, activity_complete),
        "component_total":total,"entries":activity_entries,"total_entries":activity_total,"truncated":activity_truncated,
        "git_path_renames":{"class":"git_path_rename","entries":renames,"total_entries":renames_total,"truncated":renames_truncated,
            "limitation":"Git rename detection between a commit and its first parent. It is display continuity, not RENAMED_FROM: it does not establish exact content movement or conceptual identity, and activity is not merged across it."},
        "limitations":["Counts are commits (non-merge, in range) that changed each retained path; churn is not measured because retained patches are bounded."]});
    if focus.is_some() {
        activity["limitations"].as_array_mut().unwrap().push(json!(
            "component_total counts each commit once across the component's paths."
        ));
    }
    let coupling = json!({"class":"derived_history_metric","method":COUPLING_METHOD,
        "status":metrics(coupling_total > 0),"outcome":section_outcome(evaluated, coupling_complete),
        "orientation":if focus.is_some() {"from is the component (its paths folded into one node); to is a path outside it"} else {"unordered path pairs, from < to"},
        "measures":{"shared_commits":"coupling-eligible commits that changed both","from_to":"P(to changes | from changes) = shared / from_commits",
            "to_from":"P(from changes | to changes) = shared / to_commits","jaccard":"shared / (from_commits + to_commits - shared)"},
        "min_shared":r.min_shared,"entries":coupling_entries,"total_entries":coupling_total,"truncated":coupling_truncated,
        "limitations":[NOT_DEPENDENCY,"Only commits whose every changed file was retained contribute pairs; merges are excluded."]});
    // ---- Lineage and architecture changes between captured views ------------------------
    let (lineage, architecture_changes, base_id, sub_reports) = match &r.base {
        None => {
            let not = |reason: &str| json!({"status":"not_requested","outcome":"not_requested","reason":reason});
            (
                not("No base view; lineage needs two captured views (`base`)."),
                not("No base view; architecture changes need two captured views (`base`)."),
                Value::Null,
                vec![],
            )
        }
        Some(base) => {
            let before = query::loaded(l, &sub_options(o, base))?;
            let prepared = history::prepared(l, &before, &v)?;
            let before_files = Files::new(&before);
            let lineage_report =
                query::history_lineage(l, base, &sub_options(o, &v.id), "combined")?;
            let compare_report = query::compare(l, base, &sub_options(o, &v.id))?;
            let lineage_rows: Vec<Value> = all_rows(l, &lineage_report)?
                .iter()
                .filter(|row| touches(&resolution, &[&files, &before_files], &row.detail))
                .map(|row| lineage_entry(row, &before.id, &v.id))
                .collect();
            let changes: Vec<Value> = all_rows(l, &compare_report)?
                .iter()
                .filter(|row| {
                    ["added", "removed", "possibly_added", "possibly_removed"]
                        .contains(&row.detail["change"].as_str().unwrap_or_default())
                        && touches(&resolution, &[&files, &before_files], &row.detail)
                })
                .map(|row| json!({"change":row.detail["change"],"predicate":"DEPENDS_ON","from":row.detail["from"],
                    "to":row.detail["to"],"conclusion":row.conclusion,"assertion_ids":row.ids,
                    "explanation_handle":handle_of(row),"correspondence":row.detail["correspondence"]}))
                .collect();
            let (lineage_rows, lineage_total, lineage_truncated) = bounded(lineage_rows, o.limit);
            let (changes, changes_total, changes_truncated) = bounded(changes, o.limit);
            let mut certainty: BTreeMap<String, usize> = BTreeMap::new();
            for e in &lineage_rows {
                *certainty.entry(text(e, "certainty_class")).or_default() += 1;
            }
            let lineage_id = text(&lineage_report["response"], "id");
            let compare_id = text(&compare_report["response"], "id");
            let mut lineage_limits = strings(&lineage_report["response"]["limitations"]);
            lineage_limits.push("Lineage is established only between captured snapshots, not reconstructed from Git commits.".into());
            if !prepared {
                lineage_limits.push("Exact and ambiguous correspondence was not prepared for these views; only inferred overlays can appear. Prepare it with `briefcase evolution --base` or `compare --base` on the CLI.".into());
                unknowns.push(json!({"kind":"correspondence_not_prepared","state":"unknown",
                    "reason":"Retained correspondence between the base and selected views was not prepared; renames and splits are unknown, not absent."}));
            }
            let lineage = json!({"class":"captured_snapshot_lineage",
                "status":if prepared {"available"} else {"correspondence_not_prepared"},
                "outcome":if prepared {lineage_report["response"]["outcome"].clone()} else {json!("unknown")},
                "mode":"combined","report_id":lineage_id,"before_view":before.id,"after_view":v.id,
                "certainty_counts":certainty,"entries":lineage_rows,"total_entries":lineage_total,"truncated":lineage_truncated,
                "limitations":lineage_limits});
            let changes = json!({"class":"captured_snapshot_comparison","status":"available",
                "outcome":compare_report["response"]["outcome"],"report_id":compare_id,
                "before_view":before.id,"after_view":v.id,
                "dependencies_added":changes.iter().filter(|c| c["change"] == "added").count(),
                "dependencies_removed":changes.iter().filter(|c| c["change"] == "removed").count(),
                "entries":changes,"total_entries":changes_total,"truncated":changes_truncated,
                "limitations":compare_report["response"]["limitations"]});
            (
                lineage,
                changes,
                json!(before.id),
                vec![lineage_id, compare_id],
            )
        }
    };
    // ---- Coverage, outcome and identity ---------------------------------------------------
    let reachable = l.history_reachable_count()?;
    let oldest = commits.iter().map(|c| c.timestamp).min();
    let newest = commits.iter().map(|c| c.timestamp).max();
    let at_bound = reachable as usize >= git_history::MAX_COMMITS;
    if at_bound {
        limitations.push(format!("Retained history is bounded to the newest {} reachable commits; older history is not considered.", git_history::MAX_COMMITS));
    }
    if history {
        limitations.push("Retained history follows HEAD at the last sync; the selected view is a separate capture and is not tied to a commit.".into());
    }
    limitations.push(NOT_DEPENDENCY.into());
    limitations.sort();
    limitations.dedup();
    let excluded: BTreeMap<_, _> = analysis
        .excluded
        .iter()
        .map(|(k, v)| (*k, v.len()))
        .collect();
    let coverage = json!({
        "history":{"retained":history,"tip":tip.as_ref().map(|t| &t.0),"configuration":tip.as_ref().map(|t| &t.1),
            "reachable_commits":reachable,"maximum_commits":git_history::MAX_COMMITS,"at_bound":at_bound,
            "max_files_per_commit":git_history::MAX_FILES_PER_COMMIT,
            "oldest_retained_timestamp":oldest,"newest_retained_timestamp":newest},
        "change_activity":{"complete_for_retained_history":activity_complete,"commits_considered":analysis.considered,
            "excluded_merge_commits":analysis.merges,"incomplete_commits_touching_scope":analysis.focus_incomplete},
        "change_coupling":{"eligible_commits":analysis.considered - analysis.merges,"used_commits":analysis.coupling_commits,
            "excluded_merge_commits":analysis.merges,"excluded_truncated_commits":analysis.incomplete,
            "complete_for_retained_history":coupling_complete},
        "excluded_paths":excluded,
        "lineage":{"base_view":base_id,"current_view":v.id,"mode":if r.base.is_some() {json!("combined")} else {Value::Null}}});
    let outcomes = [&lineage, &activity, &coupling, &architecture_changes]
        .map(|s| s["outcome"].as_str().unwrap_or("unknown").to_owned());
    let outcome = if !resolution.resolved() || (!history && r.base.is_none()) {
        "unknown"
    } else if outcomes
        .iter()
        .all(|s| ["complete", "not_requested"].contains(&s.as_str()))
        && unknowns.is_empty()
        && !at_bound
    {
        "complete"
    } else {
        "partial"
    };
    let request = json!({"view":o.view,"component":o.component,"base":r.base,"since":r.since,"until":r.until,
        "min_shared":r.min_shared,"limit":o.limit,"historical":o.historical});
    let history_scope = json!({"tip":tip.as_ref().map(|t| &t.0),"since":since,"until":until,
        "since_at":since.map(civil),"until_at":until.map(civil),"reachable_commits":reachable,
        "considered_commits":analysis.considered});
    let resolution_json = json!({"status":resolution.status,"component":o.component,"focus":resolution.label,
        "prefix":resolution.prefix,"entity":resolution.entity,"candidates":resolution.candidates,
        "retained_files_in_scope":focus_files.len(),"files":focus_files.iter().take(o.limit).collect::<Vec<_>>()});
    let mut sections = json!({"lineage":lineage,"activity":activity,"coupling":coupling,
        "architecture_changes":architecture_changes});
    let id = identity::id(
        "evolution-report",
        &json!({"view":v.id,"base":base_id,"history":coverage["history"],"since":since,"until":until,
            "resolution":resolution_json,"configuration":configuration(r.min_shared),"report_ids":sub_reports,
            "sections":integral(&sections),"limit":o.limit,"historical":o.historical,"outcome":outcome,
            "limitations":limitations}),
    )?;
    // Activity and coupling entries are explainable: their support is the witness commits.
    let mut explanations = Vec::new();
    for section in ["activity", "coupling"] {
        let section_limits = sections[section]["limitations"].clone();
        for (i, e) in sections[section]["entries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            let handle = format!("explain:{id}:{section}:{i}");
            explanations.push((handle.clone(), json!({"report_id":id,"operation":"evolution","views":[v.id],
                "assertion_ids":[],"detail":e.clone(),"conclusion":"derived_history_metric","historical":o.historical,
                "limitations":section_limits,
                "selection_reasons":["retained Git history metadata","requested component and time range"]})));
            e["explanation_handle"] = json!(handle);
        }
    }
    let report = json!({"schema_version":"1.0","id":id,"operation":"evolution","view":v.id,"base_view":base_id,
        "component":o.component,"request":request,"resolution":resolution_json,"history_scope":history_scope,
        "configuration":configuration(r.min_shared),"sections":sections,"coverage":coverage,"unknowns":unknowns,
        "report_ids":sub_reports,"limitations":limitations,"outcome":outcome});
    l.save_report(&id, &report, &explanations)?;
    Ok(report)
}
/// Identity input without the derived ratios: they are functions of the
/// integer counts, and identities admit no floating point.
fn integral(v: &Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => Value::Null,
        Value::Array(xs) => Value::Array(xs.iter().map(integral).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), integral(v))).collect())
        }
        v => v.clone(),
    }
}
fn section_outcome(evaluated: bool, complete: bool) -> &'static str {
    match (evaluated, complete) {
        (false, _) => "unknown",
        (true, true) => "complete",
        (true, false) => "partial",
    }
}

/// Compact text for people and agents: every number names its scope and the
/// metric sections say what they are not.
pub fn render_text(report: &Value) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let s = &report["sections"];
    let _ = writeln!(
        out,
        "EVOLUTION {} view={} component={} outcome={}",
        text_of(&report["id"]),
        text_of(&report["view"]),
        report["component"].as_str().unwrap_or("(all)"),
        text_of(&report["outcome"])
    );
    let h = &report["coverage"]["history"];
    let _ = writeln!(
        out,
        "history: tip={} reachable={} (max {}) considered={} merges_excluded={} truncated_excluded={}",
        h["tip"].as_str().map_or("none", |t| &t[..t.len().min(12)]),
        h["reachable_commits"],
        h["maximum_commits"],
        report["coverage"]["change_activity"]["commits_considered"],
        report["coverage"]["change_coupling"]["excluded_merge_commits"],
        report["coverage"]["change_coupling"]["excluded_truncated_commits"]
    );
    let keys = |v: &Value| match v {
        Value::Array(xs) => xs.iter().map(text_of).collect::<Vec<_>>().join(" | "),
        v => text_of(v),
    };
    let lineage = &s["lineage"];
    let _ = writeln!(out, "\n[lineage] {}", text_of(&lineage["status"]));
    for e in rows(lineage, "entries") {
        let _ = writeln!(
            out,
            "  {} -> {} {} ({}) {}",
            keys(&e["from"]),
            keys(&e["to"]),
            e["predicate"].as_str().unwrap_or("candidates"),
            text_of(&e["certainty_class"]),
            text_of(&e["explanation_handle"])
        );
    }
    let activity = &s["activity"];
    let _ = writeln!(
        out,
        "\n[activity] {} (derived_history_metric)",
        text_of(&activity["status"])
    );
    if let Some(t) = activity["component_total"].as_object() {
        let _ = writeln!(
            out,
            "  component {}: {} commits, {} .. {}",
            text_of(&t["component"]),
            t["commits"],
            text_of(&t["first_seen_at"]),
            text_of(&t["last_seen_at"])
        );
    }
    for e in rows(activity, "entries") {
        let _ = writeln!(
            out,
            "  {} {} commits, last {}",
            text_of(&e["path"]),
            e["commits"],
            text_of(&e["last_seen_at"])
        );
    }
    for g in rows(&activity["git_path_renames"], "entries") {
        let commit = text_of(&g["commit"]);
        let _ = writeln!(
            out,
            "  git {} {} -> {} at {} (not RENAMED_FROM)",
            text_of(&g["kind"]),
            text_of(&g["from"]),
            text_of(&g["to"]),
            &commit[..commit.len().min(12)]
        );
    }
    let coupling = &s["coupling"];
    let _ = writeln!(
        out,
        "\n[coupling] {} (co-change, not dependency)",
        text_of(&coupling["status"])
    );
    for e in rows(coupling, "entries") {
        let _ = writeln!(
            out,
            "  {} ~ {}: {}/{} from-changes also touched to ({:.0}%), {}/{} reverse ({:.0}%); static dependency {}",
            text_of(&e["from"]),
            text_of(&e["to"]),
            e["shared_commits"],
            e["from_commits"],
            e["from_to"].as_f64().unwrap_or(0.0) * 100.0,
            e["shared_commits"],
            e["to_commits"],
            e["to_from"].as_f64().unwrap_or(0.0) * 100.0,
            text_of(&e["static_dependency"]["status"])
        );
    }
    let changes = &s["architecture_changes"];
    let _ = writeln!(
        out,
        "\n[architecture_changes] {}",
        text_of(&changes["status"])
    );
    for e in rows(changes, "entries") {
        let sign = match e["change"].as_str() {
            Some("added") => "+",
            Some("removed") => "-",
            _ => "?",
        };
        let _ = writeln!(
            out,
            "  {sign} {} -> {} {} ({})",
            text_of(&e["from"]),
            text_of(&e["to"]),
            text_of(&e["predicate"]),
            text_of(&e["change"])
        );
    }
    if !rows(report, "unknowns").is_empty() {
        let _ = writeln!(out, "\n[unknown]");
        for u in rows(report, "unknowns") {
            let _ = writeln!(
                out,
                "  {} {}: {}",
                text_of(&u["state"]),
                text_of(&u["kind"]),
                text_of(&u["reason"])
            );
        }
    }
    let _ = writeln!(out, "\n[limitations]");
    let limitations = strings(&report["limitations"]);
    for x in limitations.iter().take(TEXT_LIMITATIONS) {
        let _ = writeln!(out, "  {x}");
    }
    if limitations.len() > TEXT_LIMITATIONS {
        let _ = writeln!(
            out,
            "  … {} more in the JSON report",
            limitations.len() - TEXT_LIMITATIONS
        );
    }
    out
}
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        v => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::architecture::ledger::HistoricalChange;

    fn commit(oid: &str, t: i64, parents: usize, paths: &[&str], total: usize) -> HistoricalCommit {
        HistoricalCommit {
            oid: oid.into(),
            parents: (0..parents).map(|i| format!("p{i}")).collect(),
            timestamp: t,
            files_total: total.max(paths.len()),
            message: format!("commit {oid}"),
            changes: paths
                .iter()
                .map(|p| HistoricalChange {
                    path: (*p).into(),
                    old_path: None,
                    status: "M".into(),
                })
                .collect(),
        }
    }
    /// Newest first, as the ledger returns them.
    fn fixture() -> Vec<HistoricalCommit> {
        vec![
            commit("c4", 4, 1, &["C"], 0),
            commit("c3", 3, 1, &["A", "B"], 0),
            commit("c2", 2, 1, &["A", "B"], 0),
            commit("c1", 1, 0, &["A"], 0),
        ]
    }

    #[test]
    fn activity_and_conditional_coupling_are_exact() {
        let a = analyze(&fixture(), None, None, None, 2);
        let count = |p: &str| a.activity.iter().find(|x| x.path == p).unwrap().commits;
        assert_eq!((count("A"), count("B"), count("C")), (3, 2, 1));
        assert_eq!(a.activity[0].witnesses, ["c3", "c2", "c1"]);
        assert_eq!(a.coupling.len(), 1);
        let c = &a.coupling[0];
        assert_eq!((c.from.as_str(), c.to.as_str(), c.shared), ("A", "B", 2));
        assert_eq!((c.forward(), c.backward()), (0.6667, 1.0));
        assert_eq!(c.witnesses, ["c3", "c2"]);
        // Thresholds and time bounds (since inclusive, until exclusive).
        assert!(analyze(&fixture(), None, None, None, 3).coupling.is_empty());
        let late = analyze(&fixture(), Some(3), None, None, 1);
        assert_eq!(late.considered, 2);
        assert_eq!(late.coupling[0].shared, 1);
        assert_eq!(analyze(&fixture(), None, Some(3), None, 1).considered, 2);
    }

    #[test]
    fn merges_and_incomplete_commits_never_contribute_pairs() {
        let mut commits = fixture();
        commits.insert(0, commit("m", 6, 2, &["X", "Y"], 0));
        commits.insert(0, commit("big", 7, 1, &["X", "Y"], 70));
        let a = analyze(&commits, None, None, None, 1);
        assert_eq!((a.merges, a.incomplete, a.coupling_commits), (1, 1, 4));
        assert!(a.coupling.iter().all(|c| c.from != "X" && c.to != "Y"));
        // The incomplete commit still counts its retained files as activity.
        assert_eq!(
            a.activity.iter().find(|x| x.path == "X").unwrap().commits,
            1
        );
    }

    #[test]
    fn a_focus_folds_its_paths_and_orients_pairs() {
        let commits = vec![
            commit(
                "c3",
                3,
                1,
                &["src/orders/a.rs", "src/orders/b.rs", "src/tax.rs"],
                0,
            ),
            commit("c2", 2, 1, &["src/orders/a.rs", "src/tax.rs"], 0),
            commit("c1", 1, 1, &["src/orders/b.rs", "src/pay.rs"], 0),
        ];
        let inside = |p: &str| p.starts_with("src/orders/");
        let a = analyze(&commits, None, None, Some(("src/orders", &inside)), 1);
        assert_eq!(a.total.as_ref().unwrap().commits, 3);
        assert_eq!(a.activity.len(), 2);
        let tax = a.coupling.iter().find(|c| c.to == "src/tax.rs").unwrap();
        assert_eq!(
            (
                tax.from.as_str(),
                tax.shared,
                tax.from_commits,
                tax.to_commits
            ),
            ("src/orders", 2, 3, 2)
        );
        assert!(a.coupling.iter().all(|c| c.from == "src/orders"));
    }

    #[test]
    fn noise_paths_and_times() {
        assert_eq!(noise("Cargo.lock"), Some("lock_file"));
        assert_eq!(noise("web/package-lock.json"), Some("lock_file"));
        assert_eq!(noise("vendor/x/y.go"), Some("vendored"));
        assert_eq!(noise("target/debug/a.rs"), Some("excluded_directory"));
        assert_eq!(noise("api/foo.pb.go"), Some("generated"));
        assert_eq!(noise("src/lock.rs"), None);
        assert_eq!(instant("2026-01-01").unwrap(), 1_767_225_600);
        assert_eq!(instant("1767225600").unwrap(), 1_767_225_600);
        assert_eq!(instant("2026-01-01T00:00:00Z").unwrap(), 1_767_225_600);
        assert!(instant("January").is_err());
        assert_eq!(civil(1_767_225_600), "2026-01-01T00:00:00Z");
    }
}
