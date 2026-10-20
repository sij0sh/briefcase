//! Shared engineering findings (Phase 1, change sets 02-03).
//!
//! One envelope for every producer: identity/comparison key, a limited claim,
//! applicability, evidence basis, counterevidence, a small typed payload,
//! limitations and lifecycle. Findings reference retained evidence; they never
//! copy source or upgrade an interpretation into an established relationship.
//! A statically visible guard establishes that a check exists, never that it
//! passes at runtime; a test association never establishes executed coverage.
use super::*;

pub const PRODUCER: &str = "task-findings-v1";
pub const MAX_FINDINGS: usize = 32;
const MAX_CONDITIONS: usize = 8;
const MAX_GUARDS: usize = 4;

pub(super) fn evidence_of(link: &Link, role: &str) -> Value {
    json!({"link_id":link.id,"predicate":link.predicate,"class":link.class,"basis":link.basis,
        "assertion_ids":link.assertion_ids,"witnesses":link.witnesses,"role":role,
        "from":link.from,"to":link.to,
        "paths":link.from_files.iter().chain(link.to_files.iter()).chain(link.support_files.iter()).collect::<BTreeSet<_>>()})
}

/// Narrow witnessed guard extraction: an `if` whose own line or the next
/// three lines reject/return early. The relationship stays `unknown`: the
/// passage is returned as an unresolved condition candidate, not a proof.
pub fn guard_candidates(content: &str) -> Vec<(usize, usize, String)> {
    let mut out = vec![];
    let lines: Vec<&str> = content.lines().collect();
    let mut offset = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let start = offset;
        offset += line.len() + 1;
        let trimmed = line.trim();
        if !(trimmed.starts_with("if ")
            || trimmed.starts_with("if(")
            || trimmed.starts_with("} else if "))
        {
            continue;
        }
        let window: String = lines[i..(i + 4).min(lines.len())].join("\n");
        let rejects = [
            "return Err",
            "bail!",
            "return Ok(false)",
            "return false",
            "return None",
            "throw ",
            "reject(",
            "panic!",
        ]
        .iter()
        .any(|m| window.contains(m));
        if !rejects {
            continue;
        }
        out.push((
            start,
            start + line.len(),
            trimmed.chars().take(160).collect(),
        ));
        if out.len() >= MAX_GUARDS {
            break;
        }
    }
    out
}

fn condition(
    subject: &str,
    operation: &str,
    description: &str,
    consequence: &str,
    assessment: &str,
    evidence: Vec<Value>,
) -> Value {
    json!({"subject":subject,"operation":operation,"description":description,"consequence":consequence,
        "relationship_assessment":assessment,
        // Static/declaration evidence never establishes runtime satisfaction.
        "runtime_satisfaction":"unknown","evidence":evidence})
}

/// Build behavior/condition findings for the selected slice subjects.
/// `content` yields verified retained text for a path when available.
/// Returns the findings and a bounded diagnostics record.
pub fn build(
    ix: &Index,
    source_view: &str,
    subjects: &[String],
    content: &dyn Fn(&str) -> Option<String>,
) -> (Vec<Value>, Value) {
    let mut findings = vec![];
    let mut truncated = false;
    for subject in subjects {
        if findings.len() >= MAX_FINDINGS {
            truncated = true;
            break;
        }
        let Some(file) = ix.files.get(subject) else {
            continue;
        };
        if file.role != "impl" {
            continue;
        }
        let touching: Vec<&Link> = ix
            .links
            .iter()
            .filter(|l| {
                l.from_files.contains(subject)
                    || l.to_files.contains(subject)
                    || l.support_files.contains(subject)
            })
            .collect();
        let is_anchor_evidence = !touching.is_empty() || true;
        let _ = is_anchor_evidence;
        let mut evidence = vec![];
        let mut conditions = vec![];
        let mut verification = vec![];
        let mut limitations = vec![];
        let mut counterevidence = vec![];

        // Capability: only declaration-class participation/entrypoint/feature
        // links, or a DECLARES link for a symbol owned by this file, establish
        // a declared role. A name or an import never does.
        let capability: Vec<&&Link> = touching
            .iter()
            .filter(|l| {
                [
                    "IMPLEMENTS_FEATURE",
                    "PARTICIPATES_IN",
                    "ENTRYPOINT_FOR",
                    "REALIZES_FEATURE",
                ]
                .contains(&l.predicate.as_str())
                    || (l.predicate == "FORBIDS_DEPENDENCY")
            })
            .collect();
        let declared = touching.iter().any(|l| l.class == "declaration");
        for link in &capability {
            evidence.push(evidence_of(link, "capability"));
        }

        // Conditions from configuration joins: READS_CONFIG in this file,
        // joined on the exact scoped `env:` identity to its declaration.
        let mut config_keys = BTreeSet::new();
        for link in touching
            .iter()
            .filter(|l| l.predicate == "READS_CONFIG" && l.from_files.contains(subject))
        {
            config_keys.insert(link.to.clone());
            let mut ev = vec![evidence_of(link, "condition")];
            for decl in ix
                .links
                .iter()
                .filter(|d| d.predicate == "DECLARES_CONFIG_KEY" && d.to == link.to)
            {
                ev.push(evidence_of(decl, "condition_declaration"));
            }
            if conditions.len() < MAX_CONDITIONS {
                conditions.push(condition(&link.to, subject,
                    &format!("Implementation reads configuration key `{}`; any declaring file is cited as separate evidence.", link.to),
                    "The operation may depend on this key; its value and effect are not established by retained evidence.",
                    "established", ev.clone()));
            }
            evidence.extend(ev);
        }
        // Policy contradiction: an import of a target a declaration forbids is
        // counterevidence to unconditional capability, kept visible.
        for link in touching
            .iter()
            .filter(|l| l.predicate == "IMPORTS" && l.from_files.contains(subject))
        {
            for rule in touching
                .iter()
                .filter(|r| r.predicate == "FORBIDS_DEPENDENCY")
            {
                if rule.to == link.to || rule.from == *subject {
                    counterevidence.push(evidence_of(link, "counterevidence"));
                    if conditions.len() < MAX_CONDITIONS {
                        conditions.push(condition(&link.to, subject,
                            "An import reaches a target covered by a retained FORBIDS_DEPENDENCY declaration.",
                            "Declared policy and implementation disagree; neither is erased.",
                            "contradicted", vec![evidence_of(rule, "condition"), evidence_of(link, "counterevidence")]));
                    }
                }
            }
        }
        // Guard candidates from verified source: unresolved, with witnesses.
        if let Some(text) = content(subject) {
            for (start, end, line) in guard_candidates(&text) {
                if conditions.len() >= MAX_CONDITIONS {
                    break;
                }
                let witness = json!({"path":subject,"artifact_id":file.artifact_id,"digest":file.digest,
                    "start_byte":start,"end_byte":end});
                conditions.push(condition(subject, subject,
                    &format!("Statically visible guard candidate: `{line}`"),
                    "Consequence is not established; the guard's runtime result is unknown.",
                    "unknown", vec![json!({"role":"guard_candidate","predicate":"GUARD_CANDIDATE",
                        "class":"derived","basis":"narrow witnessed guard extraction; relationship unresolved","witnesses":[witness]})]));
            }
        }
        // Consumers: bounded direct impact candidates, semantics intact.
        let consumers: Vec<&&Link> = touching
            .iter()
            .filter(|l| {
                ["IMPORTS", "CALLS_STATIC", "REFERENCES", "DEPENDS_ON"]
                    .contains(&l.predicate.as_str())
                    && l.to_files.contains(subject)
                    && !l.from_files.contains(subject)
            })
            .collect();
        for link in consumers.iter().take(4) {
            evidence.push(evidence_of(link, "consumer"));
        }

        // Verification: TESTS is a static association only.
        let tests: Vec<&&Link> = touching
            .iter()
            .filter(|l| l.predicate == "TESTS" && l.to_files.contains(subject))
            .collect();
        if tests.is_empty() {
            verification.push(json!({"kind":"relevant_test","assessment":"unknown",
                "statement":"No retained TESTS association reaches this file; an import chain does not establish that a test asserts this behavior."}));
            limitations.push(
                "No retained test association; executed test coverage is unknown.".to_owned(),
            );
        } else {
            for link in tests.iter().take(4) {
                verification.push(json!({"kind":"static_test_association","assessment":"association_established_execution_unknown",
                    "link_id":link.id,"statement":"Static TESTS association; it does not establish executed coverage or that the test asserts this condition."}));
                evidence.push(evidence_of(link, "verification"));
            }
        }
        if capability.is_empty() {
            limitations.push("No declared capability/participation relationship was retained for this file; its role is not established by its name or imports.".to_owned());
        }
        if conditions.is_empty() {
            limitations.push("No governing condition was established from retained relationships or witnessed guard candidates.".to_owned());
        }
        let claim = if !capability.is_empty() {
            format!("`{subject}` has a retained declared relationship ({}) relevant to this task; independent conditions remain separately evidenced below.",
                capability.iter().map(|l| l.predicate.as_str()).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", "))
        } else if !consumers.is_empty() {
            format!("`{subject}` is reached by {} direct retained consumer relationship(s); no declared capability was established.", consumers.len())
        } else {
            format!("The engineering role of `{subject}` is not established by retained declarations; only the cited relationships support further inspection.")
        };
        let comparison_key = format!("behavior_condition:{subject}");
        let mut finding = json!({
            "kind":"behavior_condition",
            "comparison_key":comparison_key,
            "claim":claim,
            "relevance":"Selected implementation evidence for the task; capability, governing conditions, consumers and verification are separated so none is inferred from another.",
            "subjects":[subject],
            "applicability":{"scope":"retained_source","source_view":source_view,
                "repository":source_view,"subject_status":if declared {"match"} else {"unknown"},
                "environment":"unknown","status":if declared {"match"} else {"unknown"}},
            "basis":{"producer":PRODUCER,"rule":"behavior-condition-v1","authority":if declared {"declaration_or_retained_assertion"} else {"derived_static"},
                "evidence":evidence},
            "counterevidence":counterevidence,
            "payload":{"type":"behavior_condition","conditions":conditions,"verification":verification,
                "impact_candidates":consumers.iter().map(|l| json!({"predicate":l.predicate,"from":l.from,"link_id":l.id,
                    "bound":"direct only; transitive reachability is not a behavioral-effect proof"})).collect::<Vec<_>>()},
            "limitations":limitations,
            "lifecycle":{"freshness":"current_for_pinned_inputs","dependencies":[source_view],
                "producer":PRODUCER,"superseded_by":Value::Null,"stale":false},
        });
        finding["content_id"] = json!(identity::id("finding", &finding).unwrap_or_default());
        findings.push(finding);
    }
    let count = findings.len();
    (
        findings,
        json!({"status":"evaluated","producer":PRODUCER,"subjects_considered":subjects.len(),
        "findings":count,"truncated":truncated,
        "bounds":{"max_findings":MAX_FINDINGS,"max_conditions_per_finding":MAX_CONDITIONS,"max_guards_per_file":MAX_GUARDS}}),
    )
}

/// Wrap one session-analysis draft (see `session::episodes`) in the shared
/// envelope. The task join layer owns the applicability decision; the draft
/// owns the recorded claim. Historical findings are advisory: applicability
/// of the present task stays unknown unless the join established a subject
/// match in the same repository with a complete capture, and even then the
/// recorded environment itself is not re-verified.
pub fn session_envelope(
    draft: &Value,
    source_view: &str,
    joined_subjects: &[String],
    applicability_status: &str,
    applicability_reason: &str,
) -> Value {
    let session_view = text(draft, "session_view");
    let context = text(draft, "session_context_id");
    let rule = text(draft, "rule");
    let turn = draft["payload"]["episode"]["turn_event"]
        .as_str()
        .unwrap_or("prelude");
    let primary = joined_subjects.first().cloned().unwrap_or_default();
    let comparison_key = format!(
        "session_experience:{}:{}:{}:{}",
        rule.trim_end_matches("-v1"),
        context,
        turn,
        primary
    );
    let mut payload = draft["payload"].clone();
    payload["joined_subjects"] = json!(joined_subjects);
    payload["independence"] = json!({
        "originating_sessions":1,
        "evidence_events":rows(draft, "evidence_event_ids").len(),
        "note":"Findings from one session, or overlapping episodes of it, are one origin; copied or summarized records do not add independent corroboration."});
    payload["verification"] = json!([{"kind":"current_applicability","assessment":"unknown",
        "statement":"Present-day applicability has not been verified; re-check the recorded procedure against current source and environment before acting on it."}]);
    let mut finding = json!({
        "kind":draft["kind"],
        "comparison_key":comparison_key,
        "claim":draft["claim"],
        "relevance":draft["relevance"],
        "subjects":draft["subjects"],
        "applicability":{"scope":"session_recording","source_view":source_view,
            "session_view":session_view,"repository_status":"match",
            "subject_status":"match","environment":"unknown",
            "status":applicability_status,"reason":applicability_reason,
            "current_guidance":false},
        "basis":{"producer":crate::architecture::session::episodes::PRODUCER,"rule":rule,
            "authority":draft["authority"],
            "evidence":[{"role":"session_observation","session_view":session_view,
                "session_context_id":context,"event_ids":draft["evidence_event_ids"],
                "capture_state":draft["capture_state"]}]},
        "counterevidence":[],
        "payload":payload,
        "limitations":draft["limitations"],
        "lifecycle":{"freshness":"historical_recording_current_applicability_unverified",
            "dependencies":[session_view, source_view],
            "producer":crate::architecture::session::episodes::PRODUCER,
            "superseded_by":Value::Null,"stale":"unknown"},
    });
    finding["content_id"] = json!(identity::id("finding", &finding).unwrap_or_default());
    finding
}

/// Findings delta for Case comparison, keyed by the logical comparison key.
/// Content changes on a stable key are `changed`, never silently identical;
/// a vanished key is `removed` (which may be lost coverage, not improvement).
pub fn compare(before: &[Value], after: &[Value]) -> Value {
    fn keyed(rows: &[Value]) -> BTreeMap<String, &Value> {
        rows.iter()
            .map(|f| (text(f, "comparison_key"), f))
            .collect()
    }
    let (old, new) = (keyed(before), keyed(after));
    json!({
        "added":new.iter().filter(|(k,_)|!old.contains_key(*k)).map(|(_,f)|f).collect::<Vec<_>>(),
        "removed":old.iter().filter(|(k,_)|!new.contains_key(*k)).map(|(_,f)|f).collect::<Vec<_>>(),
        "changed":new.iter().filter(|(k,f)|old.get(*k).is_some_and(|o| o["content_id"] != f["content_id"]))
            .map(|(k,f)| json!({"comparison_key":k,"before_content_id":old[k]["content_id"],"after_content_id":f["content_id"]})).collect::<Vec<_>>(),
        "cohort_changes":super::convergence::cohort_changes(before,after),
        "interpretation":"Comparison by logical comparison key. A removed finding may mean lost evidence coverage, not an improvement; convergence cohorts that shrank are reduced coverage, not improved conformity."})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link(
        predicate: &str,
        class: &str,
        from: &str,
        to: &str,
        from_files: &[&str],
        to_files: &[&str],
    ) -> Link {
        Link::new(
            predicate,
            from,
            to,
            class,
            "fixture",
            vec![],
            vec![],
            from_files.iter().map(|s| s.to_string()).collect(),
            to_files.iter().map(|s| s.to_string()).collect(),
        )
        .unwrap()
    }
    fn index(links: Vec<Link>) -> Index {
        let files = [
            "src/adapter.rs",
            "src/consumer.rs",
            "tests/adapter_test.rs",
            "config/app.env",
        ]
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
        Index {
            id: "task-index:fixture".into(),
            source_view: "view:fixture".into(),
            retraction_epoch: 0,
            files,
            entities: BTreeMap::new(),
            links,
            overflow_links: vec![],
            coverage: json!({}),
        }
    }
    const ADAPTER: &str = "pub fn rollback() -> Result<(),String> {\n    if !restore_is_safe() { return Err(\"unsafe\".into()); }\n    Ok(())\n}\n";

    #[test]
    fn capability_guard_config_and_runtime_stay_separate() {
        let ix = index(vec![
            link(
                "PARTICIPATES_IN",
                "declaration",
                "src/adapter.rs",
                "workflow:rollback",
                &["src/adapter.rs"],
                &[],
            ),
            link(
                "READS_CONFIG",
                "derived",
                "src/adapter.rs",
                "env:ROLLBACK_TOKEN",
                &["src/adapter.rs"],
                &[],
            ),
            link(
                "DECLARES_CONFIG_KEY",
                "derived",
                "config/app.env",
                "env:ROLLBACK_TOKEN",
                &["config/app.env"],
                &[],
            ),
            link(
                "IMPORTS",
                "evidence",
                "src/consumer.rs",
                "src/adapter.rs",
                &["src/consumer.rs"],
                &["src/adapter.rs"],
            ),
        ]);
        let (findings, diag) = build(&ix, "view:fixture", &["src/adapter.rs".into()], &|_| {
            Some(ADAPTER.into())
        });
        assert_eq!(diag["status"], "evaluated");
        let f = &findings[0];
        assert_eq!(f["kind"], "behavior_condition");
        assert_eq!(f["applicability"]["status"], "match");
        let conditions = rows(&f["payload"], "conditions");
        // Config condition is established statically, runtime still unknown.
        let config = conditions
            .iter()
            .find(|c| c["subject"] == "env:ROLLBACK_TOKEN")
            .unwrap();
        assert_eq!(config["relationship_assessment"], "established");
        assert_eq!(config["runtime_satisfaction"], "unknown");
        assert_eq!(
            rows(config, "evidence").len(),
            2,
            "read + declaration witnesses"
        );
        // Guard is only an unresolved candidate with an exact witness.
        let guard = conditions
            .iter()
            .find(|c| c["relationship_assessment"] == "unknown")
            .unwrap();
        assert!(guard["description"]
            .as_str()
            .unwrap()
            .contains("restore_is_safe"));
        assert_eq!(
            guard["evidence"][0]["witnesses"][0]["path"],
            "src/adapter.rs"
        );
        // No TESTS link: verification stays unknown, never inferred from imports.
        assert_eq!(f["payload"]["verification"][0]["assessment"], "unknown");
        assert_eq!(f["payload"]["impact_candidates"][0]["predicate"], "IMPORTS");
        assert!(f["content_id"].as_str().unwrap().starts_with("finding:"));
    }

    #[test]
    fn test_association_is_not_executed_coverage_and_forbidden_import_contradicts() {
        let ix = index(vec![
            link(
                "TESTS",
                "derived",
                "tests/adapter_test.rs",
                "src/adapter.rs",
                &["tests/adapter_test.rs"],
                &["src/adapter.rs"],
            ),
            link(
                "FORBIDS_DEPENDENCY",
                "declaration",
                "src/adapter.rs",
                "legacy",
                &["src/adapter.rs"],
                &[],
            ),
            link(
                "IMPORTS",
                "evidence",
                "src/adapter.rs",
                "legacy",
                &["src/adapter.rs"],
                &[],
            ),
        ]);
        let (findings, _) = build(&ix, "view:fixture", &["src/adapter.rs".into()], &|_| None);
        let f = &findings[0];
        assert_eq!(
            f["payload"]["verification"][0]["assessment"],
            "association_established_execution_unknown"
        );
        assert!(rows(&f["payload"], "conditions")
            .iter()
            .any(|c| c["relationship_assessment"] == "contradicted"));
        assert!(!rows(f, "counterevidence").is_empty());
    }

    #[test]
    fn guard_extraction_requires_an_early_rejection() {
        assert!(guard_candidates("if ready { return 1; }\n").is_empty());
        assert_eq!(guard_candidates(ADAPTER).len(), 1);
        let (findings, _) = build(
            &index(vec![]),
            "view:fixture",
            &["src/adapter.rs".into()],
            &|_| None,
        );
        // No relationships at all: role stays unestablished, explicitly.
        assert!(findings[0]["claim"]
            .as_str()
            .unwrap()
            .contains("not established"));
        assert_eq!(findings[0]["applicability"]["status"], "unknown");
    }

    #[test]
    fn comparison_keys_survive_content_changes() {
        let (a, _) = build(
            &index(vec![]),
            "view:fixture",
            &["src/adapter.rs".into()],
            &|_| None,
        );
        let mut b = a.clone();
        b[0]["content_id"] = json!("finding:changed");
        let delta = compare(&a, &b);
        assert_eq!(
            delta["changed"][0]["comparison_key"],
            "behavior_condition:src/adapter.rs"
        );
        assert!(rows(&delta, "added").is_empty() && rows(&delta, "removed").is_empty());
        let delta = compare(&a, &[]);
        assert_eq!(rows(&delta, "removed").len(), 1);
    }
}
