//! Task-scoped architectural convergence (Phase 3, change set 06).
//!
//! One cohort rule: implementation files sharing a selected file's directory
//! and language are its peers, minus peers whose declared domain differs. A
//! dependency imported by most *analyzed* peers is an observed convention
//! candidate; the selected file deviating from it is the task-local decision.
//! Prevalence is computed over the whole defined cohort, never the retrieved
//! slice, and members without dependency-producer coverage stay unresolved:
//! they are never counted as conforming. Declarations are consulted first: a
//! FORBIDS_DEPENDENCY rule contradicts the candidate and an
//! EXEMPT_FROM_CONVENTION declaration makes the deviation a permitted
//! exception. Majority is not intent or design quality, the result is never
//! an adopted policy, and no refactor or ADR is produced.
use super::findings::{evidence_of, PRODUCER};
use super::*;

pub const RULE: &str = "dependency-peer-convergence-v1";
/// Below this many analyzed peers no prevalence claim is made.
pub const MIN_ANALYZED: usize = 3;
/// Predeclared prevalence threshold: conforming * DEN >= analyzed * NUM.
const THRESHOLD: (usize, usize) = (3, 4);
const MAX_SUBJECTS: usize = 8;
const MAX_COHORT: usize = 256;
const MAX_FINDINGS: usize = 4;
const MAX_LISTED: usize = 16;
const MAX_WITNESSES: usize = 4;

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

struct Cohort {
    scope: String,
    domain: Option<String>,
    eligible: Vec<String>,
    analyzed: Vec<String>,
    unresolved: Vec<(String, &'static str)>,
    excluded_by_declaration: Vec<String>,
}

struct Evidence<'a> {
    ix: &'a Index,
    /// File → (dependency target → IMPORTS links).
    imports: BTreeMap<&'a str, BTreeMap<&'a str, Vec<&'a Link>>>,
    covered: BTreeSet<&'a str>,
    unresolved: &'a BTreeSet<String>,
}

impl<'a> Evidence<'a> {
    fn new(ix: &'a Index, unresolved: &'a BTreeSet<String>) -> Self {
        let mut imports: BTreeMap<&str, BTreeMap<&str, Vec<&Link>>> = BTreeMap::new();
        for link in ix
            .links
            .iter()
            .filter(|l| l.predicate == "IMPORTS" && l.class != "claim")
        {
            for file in &link.from_files {
                imports
                    .entry(file)
                    .or_default()
                    .entry(&link.to)
                    .or_default()
                    .push(link);
            }
        }
        // A file is analyzable only when a dependency producer retained a
        // module for it; text-only capture leaves every member unresolved.
        let covered = ix
            .entities
            .values()
            .filter(|e| e.kind == "module")
            .flat_map(|e| e.files.iter().map(String::as_str))
            .collect();
        Self {
            ix,
            imports,
            covered,
            unresolved,
        }
    }
    fn declared(&self, predicate: &str, path: &str) -> Vec<&'a Link> {
        self.ix
            .links
            .iter()
            .filter(|l| {
                l.predicate == predicate && l.class == "declaration" && l.from_files.contains(path)
            })
            .collect()
    }
    fn domain(&self, path: &str) -> Option<String> {
        let domains: BTreeSet<_> = self
            .declared("BELONGS_TO_DOMAIN", path)
            .iter()
            .map(|l| l.to.clone())
            .collect();
        // Several declared domains are not one comparable responsibility.
        (domains.len() == 1).then(|| domains.into_iter().next().unwrap())
    }
    fn analyzable(&self, path: &str) -> std::result::Result<(), &'static str> {
        if !self.covered.contains(path) {
            Err("no dependency producer coverage for this file")
        } else if self.unresolved.contains(path) {
            Err("producer reported an unresolved dependency reference in this file")
        } else {
            Ok(())
        }
    }
    fn depends(&self, path: &str, target: &str) -> bool {
        self.imports
            .get(path)
            .is_some_and(|targets| targets.contains_key(target))
    }

    fn cohort(&self, subject: &str) -> Option<Cohort> {
        let file = self.ix.files.get(subject)?;
        let dir = directory(subject);
        let domain = self.domain(subject);
        let mut eligible = vec![];
        let mut excluded_by_declaration = vec![];
        for peer in self.ix.files.values().filter(|f| {
            f.path != subject
                && f.role == "impl"
                && f.language == file.language
                && directory(&f.path) == dir
                && !source::is_evaluation_artifact(&f.path)
        }) {
            match (&domain, self.domain(&peer.path)) {
                (Some(own), Some(other)) if *own != other => {
                    excluded_by_declaration.push(peer.path.clone())
                }
                _ => eligible.push(peer.path.clone()),
            }
        }
        let (mut analyzed, mut unresolved) = (vec![], vec![]);
        for (i, peer) in eligible.iter().enumerate() {
            if i >= MAX_COHORT {
                unresolved.push((peer.clone(), "cohort analysis bound reached"));
                continue;
            }
            match self.analyzable(peer) {
                Ok(()) => analyzed.push(peer.clone()),
                Err(reason) => unresolved.push((peer.clone(), reason)),
            }
        }
        Some(Cohort {
            scope: format!(
                "{}|{}|{}",
                if dir.is_empty() { "." } else { dir },
                file.language,
                domain.as_deref().unwrap_or("-")
            ),
            domain,
            eligible,
            analyzed,
            unresolved,
            excluded_by_declaration,
        })
    }
}

/// (cohort, deviating selected subjects, co-occurring equivalent targets),
/// keyed by (cohort scope, convention target).
type Group = (Cohort, Vec<String>, Vec<String>);

/// Build convergence findings for the selected implementation subjects.
/// `unresolved` lists paths with producer-unresolved dependency references.
pub fn build(
    ix: &Index,
    source_view: &str,
    subjects: &[String],
    unresolved: &BTreeSet<String>,
) -> (Vec<Value>, Value) {
    let ev = Evidence::new(ix, unresolved);
    // One finding per (cohort, target): several selected files deviating from
    // the same convention are one issue, not separately corroborated ones.
    let mut grouped: BTreeMap<(String, String), Group> = BTreeMap::new();
    let mut skipped = vec![];
    let mut suppressed = 0usize;
    for subject in subjects
        .iter()
        .filter(|s| ix.files.get(*s).is_some_and(|f| f.role == "impl"))
        .take(MAX_SUBJECTS)
    {
        if let Err(reason) = ev.analyzable(subject) {
            skipped.push(json!({"subject":subject,"reason":reason}));
            continue;
        }
        let Some(cohort) = ev.cohort(subject) else {
            continue;
        };
        if cohort.analyzed.len() < MIN_ANALYZED {
            skipped.push(json!({"subject":subject,"cohort_scope":cohort.scope,
                "eligible":cohort.eligible.len(),"analyzed":cohort.analyzed.len(),"unresolved":cohort.unresolved.len(),
                "reason":"insufficient analyzed peers; no prevalence claim is made"}));
            continue;
        }
        let targets: BTreeSet<&str> = cohort
            .analyzed
            .iter()
            .filter_map(|p| ev.imports.get(p.as_str()))
            .flat_map(|t| t.keys().copied())
            .collect();
        // Targets imported by exactly the same conforming peers are one
        // observed dependency (a package and its module, or one statement's
        // several resolutions); counting each separately would double count.
        let mut equivalent: BTreeMap<Vec<&str>, Vec<&str>> = BTreeMap::new();
        for target in targets {
            // The subject's own module is not a dependency convention for it.
            if ix
                .entities
                .get(target)
                .is_some_and(|e| e.files.contains(subject))
                || ev.depends(subject, target)
            {
                continue;
            }
            let conforming: Vec<&str> = cohort
                .analyzed
                .iter()
                .filter(|p| ev.depends(p, target))
                .map(String::as_str)
                .collect();
            if conforming.len() * THRESHOLD.1 < cohort.analyzed.len() * THRESHOLD.0 {
                continue;
            }
            equivalent.entry(conforming).or_default().push(target);
        }
        for mut targets in equivalent.into_values() {
            // The most specific target (fewest owning files, then key) names
            // the convention; the rest are recorded as co-occurring.
            let files = |t: &str| ix.entities.get(t).map_or(usize::MAX, |e| e.files.len().max(1));
            targets.sort_by_key(|t| (files(t), *t));
            let key = (cohort.scope.clone(), targets[0].to_owned());
            if let Some((_, deviating, _)) = grouped.get_mut(&key) {
                deviating.push(subject.clone());
                suppressed += 1;
            } else {
                let cohort = ev.cohort(subject).unwrap();
                let co: Vec<String> = targets[1..].iter().map(|t| (*t).to_owned()).collect();
                grouped.insert(key, (cohort, vec![subject.clone()], co));
            }
        }
    }
    let mut findings = vec![];
    let candidates = grouped.len();
    for ((_, target), (cohort, deviating, co)) in grouped.into_iter().take(MAX_FINDINGS) {
        findings.push(finding(&ev, source_view, &cohort, &target, &deviating, &co));
    }
    let count = findings.len();
    (
        findings,
        json!({"status":"evaluated","rule":RULE,"subjects_considered":subjects.len().min(MAX_SUBJECTS),
            "candidates":candidates,"findings":count,"truncated":candidates > count,
            "suppressed_duplicate_subjects":suppressed,"skipped":skipped.into_iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "bounds":{"min_analyzed_peers":MIN_ANALYZED,"threshold":format!("{}/{}",THRESHOLD.0,THRESHOLD.1),
                "max_subjects":MAX_SUBJECTS,"max_cohort":MAX_COHORT,"max_findings":MAX_FINDINGS}}),
    )
}

fn finding(
    ev: &Evidence,
    source_view: &str,
    cohort: &Cohort,
    target: &str,
    deviating_subjects: &[String],
    co_occurring: &[String],
) -> Value {
    let ix = ev.ix;
    let (mut conforming, mut deviating, mut permitted) = (vec![], vec![], vec![]);
    let exempt = |path: &str| -> Option<&Link> {
        ev.declared("EXEMPT_FROM_CONVENTION", path)
            .into_iter()
            .find(|l| l.to == target)
    };
    for peer in &cohort.analyzed {
        if ev.depends(peer, target) {
            conforming.push(peer.clone());
        } else if let Some(link) = exempt(peer) {
            permitted.push(json!({"path":peer,"status":"permitted_exception","declaration_link_id":link.id}));
        } else {
            deviating.push(json!({"path":peer,"status":"deviates"}));
        }
    }
    // Declarations first: a rule forbidding the target for the subject or a
    // cohort member contradicts the convention; the declaration stays
    // authoritative and the observed majority remains visible as drift.
    let members: BTreeSet<&str> = deviating_subjects
        .iter()
        .chain(cohort.eligible.iter())
        .map(String::as_str)
        .collect();
    let forbidding: Vec<&Link> = ix
        .links
        .iter()
        .filter(|l| {
            l.predicate == "FORBIDS_DEPENDENCY"
                && l.class == "declaration"
                && l.to == target
                && l.from_files.iter().any(|f| members.contains(f.as_str()))
        })
        .collect();
    let subject_status: Vec<Value> = deviating_subjects
        .iter()
        .map(|s| match exempt(s) {
            Some(link) => {
                json!({"path":s,"status":"permitted_exception","declaration_link_id":link.id})
            }
            None => json!({"path":s,"status":"deviates"}),
        })
        .collect();
    let all_exempt = subject_status
        .iter()
        .all(|s| s["status"] == "permitted_exception");
    let status = if !forbidding.is_empty() {
        "contradicted_by_declaration"
    } else if all_exempt {
        "permitted_exception"
    } else {
        "candidate"
    };
    let analyzed = cohort.analyzed.len();
    let unresolved = cohort.unresolved.len();
    let cohort_status = if unresolved == 0 {
        "complete_for_producer_scope"
    } else {
        "partial"
    };
    let dir = cohort.scope.split('|').next().unwrap_or(".");
    let subjects_text = deviating_subjects
        .iter()
        .map(|s| format!("`{s}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let claim = format!(
        "Observed convention candidate: {} of {analyzed} analyzed peers in `{dir}` import `{target}`; {subjects_text} {} not; {unresolved} peer(s) unresolved of {} eligible. Prevalence is not intent or design quality.",
        conforming.len(),
        if deviating_subjects.len() == 1 { "does" } else { "do" },
        cohort.eligible.len()
    );
    let mut evidence: Vec<Value> = conforming
        .iter()
        .filter_map(|p| ev.imports.get(p.as_str())?.get(target)?.first())
        .take(MAX_WITNESSES)
        .map(|l| evidence_of(l, "conforming_peer"))
        .collect();
    let mut declarations = vec![];
    for path in deviating_subjects.iter().chain(&cohort.eligible) {
        for l in ev.declared("BELONGS_TO_DOMAIN", path) {
            if declarations.len() < MAX_WITNESSES {
                declarations.push(evidence_of(l, "peer_selection_declaration"));
            }
        }
    }
    let mut counterevidence: Vec<Value> = forbidding
        .iter()
        .map(|l| evidence_of(l, "contradicting_declaration"))
        .collect();
    for path in deviating_subjects.iter().chain(cohort.analyzed.iter()) {
        if let Some(l) = exempt(path) {
            evidence.push(evidence_of(l, "permitted_exception"));
        }
    }
    let minority: Vec<&Value> = deviating.iter().take(MAX_LISTED).collect();
    if !deviating.is_empty() {
        counterevidence.push(json!({"role":"deviating_peers","paths":minority.iter().map(|d|&d["path"]).collect::<Vec<_>>(),
            "count":deviating.len(),"effect":"Peers without the dependency show the convention is not universal; the minority implementation may be preferable."}));
    }
    let mut limitations = vec![
        "Prevalence among comparable peers is an observation, not adopted policy, author intent or design quality.".to_owned(),
        "Peers are compared by retained module imports only; calls, runtime wiring and generated code are outside this rule.".to_owned(),
    ];
    if unresolved > 0 {
        limitations.push(format!("{unresolved} eligible peer(s) could not be analyzed and are not counted as conforming."));
    }
    if cohort.domain.is_none() {
        limitations.push("The subject has no single declared domain; comparability rests on directory, language and role only.".to_owned());
    }
    let (decision, checks): (&str, Vec<&str>) = match status {
        "contradicted_by_declaration" => (
            "Do not adopt the observed majority: an adopted declaration forbids this dependency. Peers that import it are drift against that declaration, reported by policy.",
            vec!["Run the existing policy report for the forbidding rule; it owns the violation.",
                 "Confirm with the declaration's authority before changing the rule or the peers."],
        ),
        "permitted_exception" => (
            "Preserve the deviation: it is a declared approved exception to this convention. No change is suggested.",
            vec!["Confirm the exception's recorded reason still applies to the planned change."],
        ),
        _ => (
            "Decide whether the planned change should add this dependency, deliberately keep the deviation (and declare it as an exception), or whether the convention itself should change. No action is also valid.",
            vec!["Inspect a conforming peer and the subject to confirm comparable responsibility.",
                 "Inspect unresolved peers before treating the convention as settled.",
                 "Run tests covering the subject after any change; this finding does not establish safety or minimality."],
        ),
    };
    let mut finding = json!({
        "kind":"architectural_convergence",
        "comparison_key":format!("architectural_convergence:{RULE}:{}:{target}", cohort.scope),
        "claim":claim,
        "relevance":"A selected implementation differs from a dependency shared by its comparable peers; a change to it may need to follow, or deliberately not follow, that convention.",
        "subjects":deviating_subjects.iter().cloned().chain(std::iter::once(target.to_owned())).collect::<Vec<_>>(),
        "applicability":{"scope":"cohort","source_view":source_view,"repository":source_view,
            "subject_status":"match","environment":"not_applicable",
            "status":if cohort_status == "partial" {"unknown"} else {"match"},"cohort_status":cohort_status},
        "basis":{"producer":PRODUCER,"rule":RULE,"authority":"observed_convention",
            "evidence":evidence,"declarations":declarations},
        "counterevidence":counterevidence,
        "payload":{"type":"cohort_convergence","rule":RULE,"status":status,
            "convention":{"relation":"IMPORTS","target":target,"target_files":ix.entities.get(target).map(|e|e.files.clone()).unwrap_or_default(),
                "co_occurring_targets":co_occurring},
            "cohort":{"scope":cohort.scope,
                "selection_rule":{"role":"impl","same_directory":dir,"same_language":true,
                    "declared_domain":cohort.domain,"excludes":"peers with a different declared domain; the subject itself"},
                "eligible":cohort.eligible.len(),"analyzed":analyzed,"conforming":conforming.len(),
                "deviating":deviating.len(),"permitted_exceptions":permitted.len(),"unresolved":unresolved,
                "excluded_by_declaration":cohort.excluded_by_declaration.len(),
                "denominator":"analyzed peers; retrieval ranking and unresolved peers never enter it"},
            "subject_status":subject_status,
            "exceptions":deviating.iter().chain(permitted.iter()).take(MAX_LISTED).collect::<Vec<_>>(),
            "unresolved_members":cohort.unresolved.iter().take(MAX_LISTED).map(|(p,r)|json!({"path":p,"reason":r})).collect::<Vec<_>>(),
            "excluded_members":cohort.excluded_by_declaration.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "decision":{"candidate":decision,"automatic_change":false,"adr":"not produced"},
            "verification":checks.iter().map(|c|json!({"kind":"before_adopting","assessment":"unknown","statement":c})).collect::<Vec<_>>()},
        "limitations":limitations,
        "lifecycle":{"freshness":"current_for_pinned_inputs","dependencies":[source_view],
            "producer":PRODUCER,"rule":RULE,"superseded_by":Value::Null,"stale":false},
    });
    finding["content_id"] = json!(identity::id("finding", &finding).unwrap_or_default());
    finding
}

/// Before/after cohort deltas for convergence findings on a stable key.
/// A smaller cohort, fewer analyzed peers or more unresolved peers is reduced
/// coverage, never an architectural improvement; a lost declared exception is
/// reported as such; a vanished finding is not evidence of conformity.
pub fn cohort_changes(before: &[Value], after: &[Value]) -> Vec<Value> {
    let keyed = |rows: &[Value]| -> BTreeMap<String, Value> {
        rows.iter()
            .filter(|f| f["kind"] == "architectural_convergence")
            .map(|f| {
                let mut c = f["payload"]["cohort"].clone();
                c["status"] = f["payload"]["status"].clone();
                (text(f, "comparison_key"), c)
            })
            .collect()
    };
    let (old, new) = (keyed(before), keyed(after));
    let count = |c: &Value, k: &str| c[k].as_u64().unwrap_or(0);
    let counts = |c: &Value| {
        json!({"status":c["status"],"eligible":c["eligible"],"analyzed":c["analyzed"],"conforming":c["conforming"],
            "deviating":c["deviating"],"permitted_exceptions":c["permitted_exceptions"],"unresolved":c["unresolved"]})
    };
    let mut changes: Vec<Value> = old
        .iter()
        .filter_map(|(key, a)| {
            let Some(b) = new.get(key) else {
                return Some(json!({"comparison_key":key,"assessment":"no_longer_reported",
                    "before":counts(a),"after":Value::Null,
                    "interpretation":"The candidate was not emitted after the change. This may reflect lost coverage, a cohort below the analysis minimum, or a selection change; it is not evidence of conformity."}));
            };
            if a == b {
                return None;
            }
            let shrank = count(b, "eligible") < count(a, "eligible")
                || count(b, "analyzed") < count(a, "analyzed")
                || count(b, "unresolved") > count(a, "unresolved");
            let assessment = if shrank {
                "reduced_coverage"
            } else if count(b, "permitted_exceptions") < count(a, "permitted_exceptions")
                || (a["status"] == "permitted_exception" && b["status"] != "permitted_exception")
            {
                "declared_exception_lost"
            } else if count(b, "deviating") < count(a, "deviating") {
                "conformity_increased_on_stable_or_larger_cohort"
            } else {
                "changed"
            };
            Some(json!({"comparison_key":key,"assessment":assessment,"before":counts(a),"after":counts(b)}))
        })
        .collect();
    changes.extend(new.iter().filter(|(k, _)| !old.contains_key(*k)).map(|(key, b)| {
        json!({"comparison_key":key,"assessment":"newly_reported","before":Value::Null,"after":counts(b)})
    }));
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link(predicate: &str, class: &str, from: &str, to: &str, from_files: &[&str]) -> Link {
        Link::new(
            predicate,
            from,
            to,
            class,
            "fixture",
            vec![],
            vec![],
            from_files.iter().map(|s| s.to_string()).collect(),
            BTreeSet::new(),
        )
        .unwrap()
    }
    /// Five handlers; four import `audit`, `handlers/e.py` does not.
    fn index(extra: Vec<Link>, uncovered: &[&str]) -> Index {
        let paths = [
            "handlers/a.py",
            "handlers/b.py",
            "handlers/c.py",
            "handlers/d.py",
            "handlers/e.py",
            "lib/audit.py",
        ];
        let files = paths
            .iter()
            .map(|p| {
                (
                    p.to_string(),
                    Artifact {
                        path: p.to_string(),
                        artifact_id: p.to_string(),
                        digest: "sha256:fixture".into(),
                        role: "impl".into(),
                        language: "python".into(),
                        bytes: 0,
                    },
                )
            })
            .collect();
        let entities = paths
            .iter()
            .filter(|p| !uncovered.contains(p))
            .map(|p| {
                (
                    p.to_string(),
                    Entity {
                        key: p.to_string(),
                        kind: "module".into(),
                        names: BTreeSet::new(),
                        files: BTreeSet::from([p.to_string()]),
                    },
                )
            })
            .collect();
        let mut links: Vec<Link> = ["a", "b", "c", "d"]
            .iter()
            .map(|h| {
                let path = format!("handlers/{h}.py");
                link("IMPORTS", "evidence", &path, "lib/audit.py", &[&path])
            })
            .collect();
        links.extend(extra);
        Index {
            id: "task-index:fixture".into(),
            source_view: "view:fixture".into(),
            retraction_epoch: 0,
            files,
            entities,
            links,
            overflow_links: vec![],
            coverage: json!({}),
        }
    }
    fn run(ix: &Index, unresolved: &[&str]) -> (Vec<Value>, Value) {
        let unresolved = unresolved.iter().map(|s| s.to_string()).collect();
        build(ix, "view:fixture", &["handlers/e.py".into()], &unresolved)
    }

    #[test]
    fn deviation_is_a_candidate_over_the_analyzed_cohort() {
        let (findings, diag) = run(&index(vec![], &[]), &[]);
        assert_eq!(diag["findings"], 1);
        let f = &findings[0];
        assert_eq!(f["kind"], "architectural_convergence");
        assert_eq!(f["basis"]["authority"], "observed_convention");
        let p = &f["payload"];
        assert_eq!(p["status"], "candidate");
        assert_eq!(p["convention"]["target"], "lib/audit.py");
        assert_eq!(p["cohort"]["eligible"], 4);
        assert_eq!(p["cohort"]["analyzed"], 4);
        assert_eq!(p["cohort"]["conforming"], 4);
        assert_eq!(p["subject_status"][0]["status"], "deviates");
        assert_eq!(p["decision"]["automatic_change"], false);
        assert!(f["claim"].as_str().unwrap().contains("4 of 4 analyzed"));
        assert_eq!(rows(&f["basis"], "evidence").len(), 4);
    }

    #[test]
    fn unresolved_peers_never_count_as_conforming() {
        // One peer lacks producer coverage, one has an unresolved import:
        // 2 of 2 analyzed is below the minimum, so no prevalence is claimed.
        let (findings, diag) = run(&index(vec![], &["handlers/a.py"]), &["handlers/b.py"]);
        assert!(findings.is_empty());
        assert_eq!(diag["skipped"][0]["unresolved"], 2);
        // With one unresolved peer, 3 of 3 analyzed is a partial cohort.
        let (findings, _) = run(&index(vec![], &["handlers/a.py"]), &[]);
        let p = &findings[0]["payload"];
        assert_eq!(
            (p["cohort"]["analyzed"].as_u64(), p["cohort"]["unresolved"].as_u64()),
            (Some(3), Some(1))
        );
        assert_eq!(findings[0]["applicability"]["cohort_status"], "partial");
        assert_eq!(findings[0]["applicability"]["status"], "unknown");
        assert!(findings[0]["claim"]
            .as_str()
            .unwrap()
            .contains("3 of 3 analyzed"));
        // An unanalyzable subject is skipped, not compared.
        let (findings, diag) = run(&index(vec![], &["handlers/e.py"]), &[]);
        assert!(findings.is_empty());
        assert!(diag["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("coverage"));
    }

    #[test]
    fn declared_exception_and_forbidding_declaration_take_precedence() {
        let exempt = link(
            "EXEMPT_FROM_CONVENTION",
            "declaration",
            "handlers/e.py",
            "lib/audit.py",
            &["handlers/e.py"],
        );
        let (findings, _) = run(&index(vec![exempt], &[]), &[]);
        assert_eq!(findings[0]["payload"]["status"], "permitted_exception");
        assert_eq!(
            findings[0]["payload"]["subject_status"][0]["status"],
            "permitted_exception"
        );
        let forbids = link(
            "FORBIDS_DEPENDENCY",
            "declaration",
            "handlers/e.py",
            "lib/audit.py",
            &["handlers/e.py"],
        );
        let (findings, _) = run(&index(vec![forbids], &[]), &[]);
        let f = &findings[0];
        assert_eq!(f["payload"]["status"], "contradicted_by_declaration");
        assert_eq!(
            f["counterevidence"][0]["role"],
            "contradicting_declaration"
        );
        assert_eq!(f["basis"]["authority"], "observed_convention");
    }

    #[test]
    fn different_declared_responsibility_leaves_the_cohort() {
        let domain = |path: &str, d: &str| link("BELONGS_TO_DOMAIN", "declaration", path, d, &[path]);
        let ix = index(
            vec![
                domain("handlers/e.py", "domain:billing"),
                domain("handlers/a.py", "domain:audit"),
                domain("handlers/b.py", "domain:audit"),
            ],
            &[],
        );
        let (findings, diag) = run(&ix, &[]);
        // Two peers remain eligible; no prevalence claim below the minimum.
        assert!(findings.is_empty());
        assert_eq!(diag["skipped"][0]["eligible"], 2);
    }

    #[test]
    fn minority_peer_stays_visible_and_conforming_subject_gets_nothing() {
        let mut ix = index(vec![], &[]);
        ix.links.retain(|l| !l.from_files.contains("handlers/d.py"));
        let (findings, _) = run(&ix, &[]);
        let f = &findings[0];
        assert!(f["claim"].as_str().unwrap().contains("3 of 4 analyzed"));
        assert_eq!(f["payload"]["exceptions"][0]["path"], "handlers/d.py");
        assert_eq!(
            rows(f, "counterevidence")
                .iter()
                .find(|c| c["role"] == "deviating_peers")
                .unwrap()["count"],
            1
        );
        // A subject that follows the convention is not burdened with peers'
        // exceptions.
        let (findings, _) = build(
            &ix,
            "view:fixture",
            &["handlers/a.py".into()],
            &BTreeSet::new(),
        );
        assert!(findings.is_empty());
    }

    #[test]
    fn deterministic_and_shrinking_cohort_is_reduced_coverage() {
        let (a, _) = run(&index(vec![], &[]), &[]);
        let (again, _) = run(&index(vec![], &[]), &[]);
        assert_eq!(a, again);
        let mut smaller = index(vec![], &[]);
        smaller.files.remove("handlers/d.py");
        let (b, _) = run(&smaller, &[]);
        assert_eq!(a[0]["comparison_key"], b[0]["comparison_key"]);
        let changes = cohort_changes(&a, &b);
        assert_eq!(changes[0]["assessment"], "reduced_coverage");
        assert!(cohort_changes(&a, &a).is_empty());
        // Losing producer coverage below the minimum removes the candidate;
        // that is reported as no longer reported, never as conformity.
        let (gone, _) = run(&index(vec![], &["handlers/a.py", "handlers/b.py"]), &[]);
        assert!(gone.is_empty());
        assert_eq!(cohort_changes(&a, &gone)[0]["assessment"], "no_longer_reported");
    }

    #[test]
    fn losing_a_declared_exception_is_not_improved_conformity() {
        let exempt = |p: &str| link("EXEMPT_FROM_CONVENTION", "declaration", p, "lib/audit.py", &[p]);
        let mut with = index(vec![exempt("handlers/d.py")], &[]);
        with.links.retain(|l| !(l.predicate == "IMPORTS" && l.from_files.contains("handlers/d.py")));
        let (a, _) = run(&with, &[]);
        assert_eq!(a[0]["payload"]["cohort"]["permitted_exceptions"], 1);
        with.links.retain(|l| l.predicate != "EXEMPT_FROM_CONVENTION");
        let (b, _) = run(&with, &[]);
        assert_eq!(b[0]["payload"]["cohort"]["deviating"], 1);
        assert_eq!(cohort_changes(&a, &b)[0]["assessment"], "declared_exception_lost");
    }
}
