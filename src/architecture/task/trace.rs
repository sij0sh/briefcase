//! Facts found by tracing retained evidence. Duty policy does not create gaps here.
use super::request::Request;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum AnchorStatus {
    Resolved,
    Ambiguous,
    Unresolved,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Seed {
    pub status: AnchorStatus,
    pub candidates: BTreeSet<String>,
    pub paths: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Reach {
    pub depth: usize,
    pub via_link: Option<String>,
}
#[derive(Clone, Debug)]
pub(super) enum Boundary {
    NoHistory,
    PartialHistory(Value),
    NoSessions,
    MetadataSession(Value),
    RetractedSession(Value),
    PartialSession(Value),
    IndirectCommit(Value),
    ProfileSnapshot(Value),
    DeploymentSource(Value),
    DeploymentPublications(Value),
    RuntimeIdentity(Value),
    RuntimeSelection(Value),
    RuntimePublications(Value),
    RuntimeObservationScope(Value),
}
impl Boundary {
    fn producer(detail: Value) -> Result<Self> {
        Ok(match detail["area"].as_str() {
            Some("profile_snapshot") => Self::ProfileSnapshot(detail),
            Some("deployment_source") => Self::DeploymentSource(detail),
            Some("deployment_publications") => Self::DeploymentPublications(detail),
            Some("runtime_identity") => Self::RuntimeIdentity(detail),
            Some("runtime_selection") => Self::RuntimeSelection(detail),
            Some("runtime_publications") => Self::RuntimePublications(detail),
            _ => return Err("TASK: Unknown producer boundary area".into()),
        })
    }
}
pub(super) struct Trace {
    pub seed: Seed,
    pub depth: usize,
    pub paths: BTreeSet<String>,
    pub keys: BTreeSet<String>,
    pub links: Vec<String>,
    pub joins: joins::Joins,
    pub lexical_candidates: Vec<Value>,
    pub boundaries: Vec<Boundary>,
    pub reached_by: BTreeMap<String, Reach>,
    pub candidates: Vec<retrieval::Candidate>,
    pub retrieval: Value,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Direction {
    In,
    Out,
    Both,
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Accumulate {
    Closure,
    Frontier,
}
pub(super) struct Expansion {
    pub paths: BTreeSet<String>,
    pub keys: BTreeSet<String>,
    pub links: BTreeSet<String>,
    pub reached_by: BTreeMap<String, Reach>,
}

/// Closure expands both endpoints of every touching edge; frontier advances
/// only across the selected directions. Every layer reads a frozen frontier.
pub(super) fn expand(
    ix: &Index,
    keys: &BTreeSet<String>,
    paths: &BTreeSet<String>,
    direction: Direction,
    predicates: &[&str],
    depth: usize,
    accumulate: Accumulate,
) -> Expansion {
    let mut out = Expansion {
        paths: paths.clone(),
        keys: keys.clone(),
        links: BTreeSet::new(),
        reached_by: paths
            .iter()
            .map(|p| {
                (
                    p.clone(),
                    Reach {
                        depth: 0,
                        via_link: None,
                    },
                )
            })
            .collect(),
    };
    let (mut frontier_keys, mut frontier_paths) = (keys.clone(), paths.clone());
    for layer in 1..=depth {
        let (mut next_keys, mut next_paths) = (BTreeSet::new(), BTreeSet::new());
        for e in &ix.links {
            if !predicates.is_empty() && !predicates.contains(&e.predicate.as_str()) {
                continue;
            }
            let outgoing = !matches!(direction, Direction::In)
                && (frontier_keys.contains(&e.from) || !frontier_paths.is_disjoint(&e.from_files));
            let incoming = !matches!(direction, Direction::Out)
                && (frontier_keys.contains(&e.to) || !frontier_paths.is_disjoint(&e.to_files));
            if !outgoing && !incoming {
                continue;
            }
            out.links.insert(e.id.clone());
            if matches!(accumulate, Accumulate::Closure) || incoming {
                next_keys.insert(e.from.clone());
                for path in &e.from_files {
                    next_paths.insert(path.clone());
                    out.reached_by.entry(path.clone()).or_insert_with(|| Reach {
                        depth: layer,
                        via_link: Some(e.id.clone()),
                    });
                }
            }
            if matches!(accumulate, Accumulate::Closure) || outgoing {
                next_keys.insert(e.to.clone());
                for path in &e.to_files {
                    next_paths.insert(path.clone());
                    out.reached_by.entry(path.clone()).or_insert_with(|| Reach {
                        depth: layer,
                        via_link: Some(e.id.clone()),
                    });
                }
            }
        }
        let unchanged = next_keys.is_subset(&out.keys) && next_paths.is_subset(&out.paths);
        out.keys.extend(next_keys.iter().cloned());
        out.paths.extend(next_paths.iter().cloned());
        match accumulate {
            Accumulate::Closure => {
                if unchanged {
                    break;
                }
                frontier_keys = out.keys.clone();
                frontier_paths = out.paths.clone();
            }
            Accumulate::Frontier => {
                frontier_keys = next_keys;
                frontier_paths = next_paths;
            }
        }
    }
    out
}

pub(super) fn seed(ix: &Index, target: &str, explicit: bool) -> Seed {
    let (status, candidates, paths) = resolve(ix, target, explicit);
    Seed {
        status: match status.as_str() {
            "resolved" => AnchorStatus::Resolved,
            "ambiguous" => AnchorStatus::Ambiguous,
            _ => AnchorStatus::Unresolved,
        },
        candidates,
        paths,
    }
}

pub(super) fn build(l: &Ledger, s: &Snapshot, ix: &Index, r: &Request) -> Result<Trace> {
    let seed = seed(
        ix,
        r.target.as_deref().unwrap_or(&r.task),
        r.target.is_some(),
    );
    let (mut candidates, lexical_candidates, lexical_coverage) =
        retrieval::discover(l, s, ix, r, &seed)?;
    let mut strong: Vec<_> = candidates.values().filter(|c| c.score >= 60).collect();
    strong.sort_by_key(|c| (std::cmp::Reverse(c.score), c.path.clone()));
    let mut initial: BTreeSet<_> = strong.iter().take(8).map(|c| c.path.clone()).collect();
    initial.extend(
        seed.paths
            .iter()
            .filter(|path| retrieval::path_allowed(path, &seed))
            .cloned(),
    );
    let omitted_join_starters:Vec<_>=strong.iter().skip(8).map(|c|json!({"kind":"join_starter","target":c.path,
        "status":"available","reason":"initial_join_starter_limit",
        "action":{"operation":"expand_case","arguments":{"paths":[c.path]}}})).collect();
    let (hints, mut join_coverage) = joins::discover(l, s, r, &initial)?;
    for hint in hints {
        let path = text(&hint, "path");
        retrieval::contribute(
            &mut candidates,
            &path,
            retrieval::Signal {
                facet: text(&hint, "facet"),
                kind: text(&hint, "kind"),
                strength: hint["strength"].as_u64().unwrap_or(0),
                confidence: text(&hint, "confidence"),
                depth: 0,
                via_link: None,
                provenance: hint["provenance"].clone(),
            },
        );
    }
    let mut graph_coverage = retrieval::expand(ix, r, &seed, &mut candidates);
    let mut work=graph_coverage.as_object_mut().unwrap().remove("remaining_work").unwrap_or(json!([])).as_array().cloned().unwrap_or_default();
    work.extend(omitted_join_starters);
    work.extend(join_coverage.as_object_mut().unwrap().remove("remaining_work").unwrap_or(json!([])).as_array().cloned().unwrap_or_default());
    let work_report=crate::architecture::reports::retain_items(l,json!({"kind":"discovery_work","index_id":ix.id,
        "view_id":ix.source_view,"pinned_views":ix.coverage["profile_views"],"items":work}),0)?;
    graph_coverage["remaining_work"]=json!({"report_id":work_report["report_id"],"count":work.len(),
        "continue_with":crate::architecture::reports::continuation(work_report["report_id"].as_str().unwrap(),"/items",0,100)});
    retrieval::localize(s, r, &mut candidates)?;
    let candidates = retrieval::rank(ix, r, candidates);
    finish(l,s,ix,r,seed,candidates,lexical_candidates,lexical_coverage,join_coverage,graph_coverage)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish(l:&Ledger,s:&Snapshot,ix:&Index,r:&Request,seed:Seed,
    candidates:Vec<retrieval::Candidate>,lexical_candidates:Vec<Value>,lexical_coverage:Value,
    join_coverage:Value,graph_coverage:Value)->Result<Trace> {
    let project_scope = retrieval::project_scope(s, r, &seed, &candidates);
    let paths: BTreeSet<_> = candidates
        .iter()
        .filter(|c| c.selected)
        .map(|c| c.path.clone())
        .collect();
    let reached_by = candidates
        .iter()
        .filter(|c| c.selected)
        .map(|c| (c.path.clone(), c.reach()))
        .collect();
    let keys = ix
        .entities
        .iter()
        .filter(|(_, e)| !paths.is_disjoint(&e.files))
        .map(|(k, _)| k.clone())
        .collect();
    let links = ix
        .links
        .iter()
        .filter(|e| {
            retrieval::link_allowed(e, &seed)
                && (!paths.is_disjoint(&e.from_files) || !paths.is_disjoint(&e.to_files))
        })
        .map(|e| e.id.clone())
        .collect();
    let joins = joins::build(l, s, ix, &paths, r.enabled("sessions"))?;
    let mut boundaries = joins.boundaries.clone();
    for detail in &s.profile_boundaries {
        boundaries.push(Boundary::producer(detail.clone())?);
    }
    for producer in s.evidence_views.iter().filter(|v| v.scope == "runtime") {
        boundaries.push(Boundary::RuntimeObservationScope(json!({"view_id":producer.id,"interval":producer.diagnostics["interval"],
            "sampling":producer.diagnostics["sampling"],"reason":"Retained runtime observations describe one interval; missing observations do not establish absence."})));
    }
    let retrieval = json!({"method":"faceted-retrieval-v3","max_files":r.max_files,"minimum_score":24,
        "test_expansion":"direct associations only; shared tests are not transitive implementation bridges",
        "selection_policy":retrieval::selection_policy(r),"selection_variant":r.selection_variant,
        "evaluation_artifacts":{"policy":"excluded from automatic discovery and graph bridges; exact anchors may opt in",
            "retained_count":s.files.keys().filter(|path|source::is_evaluation_artifact(path)).count()},
        "candidate_count":candidates.len(),"selected_files":paths.len(),"omitted_candidates":candidates.len()-paths.len(),
        "excluded_facets":r.exclude_facets,"lexical_discovery":lexical_coverage,"join_discovery":join_coverage,"graph_expansion":graph_coverage,"project_scope":project_scope,
        "semantics":"Scores rank reading context, never assertion authority, implementation membership or task completion."});
    Ok(Trace {
        seed,
        depth: r.depth,
        paths,
        keys,
        links,
        joins,
        lexical_candidates,
        boundaries,
        reached_by,
        candidates,
        retrieval,
    })
}

fn words_match(haystack: &str, needle: &str) -> bool {
    !mentions(haystack, needle).is_empty()
}
fn mentions(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return vec![];
    }
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    haystack
        .match_indices(needle.as_str())
        .filter_map(|(start, _)| {
            let end = start + needle.len();
            let word = |c: char| c.is_alphanumeric() || c == '_';
            let matched = (start == 0 || !haystack[..start].chars().next_back().is_some_and(word))
                && (end == haystack.len() || !haystack[end..].chars().next().is_some_and(word));
            matched.then_some((start, end))
        })
        .collect()
}

/// Prose names an identity only through a name that cannot be an ordinary
/// word: internal capitals, underscores, letter-digit mixes, or qualification.
/// Plain-word declarations (`stop`, `resource`, `HIGH`) match task prose by
/// accident and stay lexical evidence instead of anchors.
fn identifier_shaped(name: &str) -> bool {
    let letters = name.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 2
        && (name.contains('_')
            || name.contains("::")
            || name.contains('.')
            || (letters < name.chars().count() && name.chars().any(|c| c.is_ascii_digit()))
            || name
                .chars()
                .zip(name.chars().skip(1))
                .any(|(a, b)| a.is_lowercase() && b.is_uppercase()))
}

/// Exact identity first. Mentions and terminal identifier matches are candidates;
/// several identities remain ambiguous, even when one seems more relevant.
pub(super) fn resolve(
    ix: &Index,
    target: &str,
    explicit: bool,
) -> (String, BTreeSet<String>, BTreeSet<String>) {
    let mut keys = BTreeSet::new();
    let mut files = BTreeSet::new();
    if [".", "./", "/"].contains(&target) {
        return (
            "resolved".into(),
            ix.files.keys().cloned().collect(),
            ix.files.keys().cloned().collect(),
        );
    } else if ix.files.contains_key(target) {
        keys.insert(target.into());
        files.insert(target.into());
    } else if ix.entities.contains_key(target) {
        keys.insert(target.into());
        files.extend(ix.entities[target].files.clone());
    } else if explicit && target.ends_with('/') {
        files.extend(ix.files.keys().filter(|p| p.starts_with(target)).cloned());
        keys.extend(files.clone());
        return (
            if files.is_empty() {
                "unresolved"
            } else {
                "resolved"
            }
            .into(),
            keys,
            files,
        );
    } else {
        let mut named = vec![];
        for (path, _) in &ix.files {
            if !explicit && words_match(target, path) {
                keys.insert(path.clone());
            }
        }
        for (key, e) in &ix.entities {
            if e.names.iter().any(|n| {
                if explicit {
                    n.eq_ignore_ascii_case(target)
                } else {
                    identifier_shaped(n) && words_match(target, n)
                }
            }) || (!explicit && words_match(target, key))
            {
                keys.insert(key.clone());
            }
            if !explicit {
                for name in e.names.iter().filter(|n| identifier_shaped(n)) {
                    for (start, end) in mentions(target, name) {
                        named.push((key.clone(), start, end));
                    }
                }
            }
        }
        if !explicit {
            // A short alias inside a longer exact identity mention is not an
            // independent anchor. Disjoint mentions and equal-span identities
            // remain ambiguous, as in the semantic feature router.
            let all = named.clone();
            named.retain(|(key, s, e)| {
                !all.iter()
                    .any(|(other, s2, e2)| other != key && s2 <= s && e <= e2 && e2 - s2 > e - s)
            });
            let survivors: BTreeSet<_> = named.into_iter().map(|(key, _, _)| key).collect();
            keys.retain(|key| ix.files.contains_key(key) || survivors.contains(key));
        }
        // A file mention is stronger than the modules it happens to contain.
        let paths: BTreeSet<_> = keys
            .iter()
            .filter(|k| ix.files.contains_key(*k))
            .cloned()
            .collect();
        if !paths.is_empty() {
            keys = paths;
        }
        {
            for key in &keys {
                if ix.files.contains_key(key) {
                    files.insert(key.clone());
                }
                if let Some(e) = ix.entities.get(key) {
                    files.extend(e.files.clone());
                }
            }
        }
    }
    (
        match keys.len() {
            0 => "unresolved",
            1 => "resolved",
            _ => "ambiguous",
        }
        .into(),
        keys,
        files,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|s| (*s).into()).collect()
    }
    fn link(predicate: &str, from: &str, to: &str, from_files: &[&str], to_files: &[&str]) -> Link {
        Link::new(
            predicate,
            from,
            to,
            "derived",
            "fixture",
            vec![],
            vec![],
            set(from_files),
            set(to_files),
        )
        .unwrap()
    }
    fn index() -> Index {
        let files = ["src/a.rs", "src/b.rs", "src/c.rs", "docs/usage.md"]
            .into_iter()
            .map(|p| {
                (
                    p.into(),
                    Artifact {
                        path: p.into(),
                        artifact_id: p.into(),
                        digest: "sha256:fixture".into(),
                        role: source::artifact_role(p).into(),
                        language: "rust".into(),
                        bytes: 0,
                    },
                )
            })
            .collect();
        let entities = [
            ("crate::a::Store", "src/a.rs"),
            ("crate::b::Store", "src/b.rs"),
        ]
        .into_iter()
        .map(|(key, path)| {
            (
                key.into(),
                Entity {
                    key: key.into(),
                    kind: "module".into(),
                    names: set(&[key, "Store"]),
                    files: set(&[path]),
                },
            )
        })
        .collect();
        Index {
            id: "index".into(),
            source_view: "view".into(),
            retraction_epoch: 0,
            files,
            entities,
            links: vec![
                link("IMPORTS", "a", "b", &["src/a.rs"], &["src/b.rs"]),
                link("TESTS", "b", "c", &["src/b.rs"], &["src/c.rs"]),
                link("IMPORTS", "c", "a", &["src/c.rs"], &["src/a.rs"]),
                link(
                    "REFERENCES_ARTIFACT",
                    "doc",
                    "b",
                    &["docs/usage.md"],
                    &["src/b.rs"],
                ),
            ],
            coverage: json!({}),
        }
    }
    #[test]
    fn seed_resolves_exact_entities_directories_and_aliases_conservatively() {
        let ix = index();
        for (target, explicit, status, paths) in [
            ("src/a.rs", true, AnchorStatus::Resolved, set(&["src/a.rs"])),
            (
                "crate::a::Store",
                true,
                AnchorStatus::Resolved,
                set(&["src/a.rs"]),
            ),
            (
                "src/",
                true,
                AnchorStatus::Resolved,
                set(&["src/a.rs", "src/b.rs", "src/c.rs"]),
            ),
            (
                "Store",
                true,
                AnchorStatus::Ambiguous,
                set(&["src/a.rs", "src/b.rs"]),
            ),
            ("missing", true, AnchorStatus::Unresolved, set(&[])),
            // A plain-word declaration named in prose is not an anchor.
            ("check the store", false, AnchorStatus::Unresolved, set(&[])),
            (
                "explain crate::a::Store",
                false,
                AnchorStatus::Resolved,
                set(&["src/a.rs"]),
            ),
            (
                ".",
                true,
                AnchorStatus::Resolved,
                ix.files.keys().cloned().collect(),
            ),
        ] {
            let s = seed(&ix, target, explicit);
            assert_eq!(s.status, status, "{target}");
            assert_eq!(s.paths, paths, "{target}");
        }
    }
    #[test]
    fn only_identifier_shaped_names_anchor_from_prose() {
        for name in ["useThreadFind", "ResourceTelemetry", "MAX_LINES", "v2Store", "crate::a::Store", "fs.readFile"] {
            assert!(identifier_shaped(name), "{name}");
        }
        for name in ["stop", "a", "resource", "HIGH", "Manager", "_", "x1"] {
            assert!(!identifier_shaped(name), "{name}");
        }
    }
    /// The original neighbors loop is an independent differential oracle.
    fn legacy_neighbors(
        ix: &Index,
        mut keys: BTreeSet<String>,
        mut files: BTreeSet<String>,
        direction: &str,
        predicates: &[&str],
        depth: usize,
    ) -> BTreeSet<String> {
        let mut selected = BTreeSet::new();
        for _ in 0..depth {
            let (mut next_keys, mut next_files) = (BTreeSet::new(), BTreeSet::new());
            for e in &ix.links {
                if !predicates.is_empty() && !predicates.contains(&e.predicate.as_str()) {
                    continue;
                }
                let outgoing = direction != "in"
                    && (keys.contains(&e.from) || !files.is_disjoint(&e.from_files));
                let incoming =
                    direction != "out" && (keys.contains(&e.to) || !files.is_disjoint(&e.to_files));
                if outgoing || incoming {
                    selected.insert(e.id.clone());
                    if outgoing {
                        next_keys.insert(e.to.clone());
                        next_files.extend(e.to_files.clone());
                    }
                    if incoming {
                        next_keys.insert(e.from.clone());
                        next_files.extend(e.from_files.clone());
                    }
                }
            }
            keys = next_keys;
            files = next_files;
        }
        selected
    }
    #[test]
    fn shared_frontier_preserves_directed_neighbors_for_every_depth() {
        let ix = index();
        for (direction, name) in [
            (Direction::In, "in"),
            (Direction::Out, "out"),
            (Direction::Both, "both"),
        ] {
            for predicates in [&[][..], &["IMPORTS"][..], &["TESTS"][..]] {
                for depth in 1..=3 {
                    for (keys, paths) in [
                        (set(&["src/a.rs"]), set(&["src/a.rs"])),
                        (set(&["a"]), set(&[])),
                        (set(&["missing"]), set(&[])),
                    ] {
                        let actual = expand(
                            &ix,
                            &keys,
                            &paths,
                            direction,
                            predicates,
                            depth,
                            Accumulate::Frontier,
                        );
                        let expected = legacy_neighbors(&ix, keys, paths, name, predicates, depth);
                        assert_eq!(
                            serde_json::to_vec(&actual.links).unwrap(),
                            serde_json::to_vec(&expected).unwrap(),
                            "{name} depth {depth}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn closure_matches_the_original_accumulating_loop_and_reaches_a_fixpoint() {
        let ix = index();
        let keys = set(&["src/a.rs"]);
        let paths = keys.clone();
        for depth in 1..=3 {
            let (mut expected_paths, mut expected_keys) = (paths.clone(), keys.clone());
            for _ in 0..depth {
                let before = expected_paths.clone();
                let before_keys = expected_keys.clone();
                for e in &ix.links {
                    if before_keys.contains(&e.from)
                        || before_keys.contains(&e.to)
                        || !before.is_disjoint(&e.from_files)
                        || !before.is_disjoint(&e.to_files)
                    {
                        expected_paths.extend(e.from_files.clone());
                        expected_paths.extend(e.to_files.clone());
                        expected_keys.insert(e.from.clone());
                        expected_keys.insert(e.to.clone());
                    }
                }
                if expected_paths == before && expected_keys == before_keys {
                    break;
                }
            }
            let actual = expand(
                &ix,
                &keys,
                &paths,
                Direction::Both,
                &[],
                depth,
                Accumulate::Closure,
            );
            assert_eq!(actual.paths, expected_paths);
            assert_eq!(actual.keys, expected_keys);
        }
        let one = expand(
            &ix,
            &keys,
            &paths,
            Direction::Both,
            &["IMPORTS"],
            1,
            Accumulate::Closure,
        );
        assert!(!one.paths.contains("docs/usage.md"));
        assert_eq!(one.reached_by["src/a.rs"].depth, 0);
        for (path, reach) in &one.reached_by {
            if let Some(id) = &reach.via_link {
                let e = ix.links.iter().find(|e| &e.id == id).unwrap();
                assert!(e.from_files.contains(path) || e.to_files.contains(path));
                assert_eq!(reach.depth, 1);
            }
        }
    }
    #[test]
    fn config_join_records_the_declaration_link_without_directory_fanout() {
        let mut ix = index();
        let file = Artifact {
            path: "config/app.env".into(),
            artifact_id: "env".into(),
            digest: "sha256:fixture".into(),
            role: "config".into(),
            language: "text".into(),
            bytes: 0,
        };
        ix.files.insert(file.path.clone(), file);
        ix.links = vec![
            link("READS_CONFIG", "src/a.rs", "env:TOKEN", &["src/a.rs"], &[]),
            link(
                "DECLARES_CONFIG_KEY",
                "config/app.env",
                "env:TOKEN",
                &["config/app.env"],
                &[],
            ),
        ];
        let request =
            request::parse(&json!({"task":"inspect src/a.rs","target":"src/a.rs","depth":1}))
                .unwrap()
                .0;
        let seed = seed(&ix, "src/a.rs", true);
        let mut candidates = retrieval::Candidates::new();
        retrieval::contribute(
            &mut candidates,
            "src/a.rs",
            retrieval::Signal {
                facet: "anchors".into(),
                kind: "exact_anchor".into(),
                strength: 10000,
                confidence: "exact".into(),
                depth: 0,
                via_link: None,
                provenance: json!({}),
            },
        );
        retrieval::expand(&ix, &request, &seed, &mut candidates);
        let reach = candidates["config/app.env"].reach();
        assert_eq!(
            reach,
            Reach {
                depth: 2,
                via_link: Some(ix.links[1].id.clone())
            }
        );
        assert!(!candidates.contains_key("src/b.rs"));
    }
}
