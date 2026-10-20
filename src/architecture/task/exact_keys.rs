//! Opt-in literal association discovery. Exact text plus a scoped identity is
//! a reading hint, never a call, runtime dependency, read/write or guarantee.
use super::*;
use regex::Regex;

#[derive(Clone, Debug)]
struct Use {
    path: String,
    key: String,
    family: &'static str,
    role: &'static str,
    start: usize,
    end: usize,
}

fn package(path: &str) -> String {
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() >= 3 && ["apps", "packages", "infra"].contains(&parts[0]) {
        parts[..2].join("/")
    } else {
        // Standalone repositories share their repository scope.
        ".".into()
    }
}

fn uses(path: &str, text: &str, literals: &Regex, events: &Regex, headers: &Regex) -> Vec<Use> {
    let mut out = vec![];
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trim = line.trim_start();
        if !["//", "/*", "*", "import ", "export *", "export {"]
            .iter()
            .any(|p| trim.starts_with(p))
        {
            for capture in literals.captures_iter(line) {
                let Some(m) = capture.get(1).or_else(|| capture.get(2)) else {
                    continue;
                };
                let key = m.as_str();
                let family = if events.is_match(key) {
                    "event"
                } else if headers.is_match(key) {
                    "header"
                } else {
                    continue;
                };
                let prefix = &line[..m.start()];
                let role =
                    if prefix.contains("Schema.Literal(") || prefix.contains("Schema.Literals(") {
                        "declaration"
                    } else if prefix.contains("case ")
                        || prefix.contains(".get(")
                        || prefix.contains(".has(")
                        || prefix.contains("headers[")
                    {
                        "consumer"
                    } else if prefix.contains("type:")
                        || prefix.contains(".set(")
                        || prefix.contains(".append(")
                    {
                        "producer"
                    } else {
                        "literal_occurrence"
                    };
                out.push(Use {
                    path: path.into(),
                    key: key.into(),
                    family,
                    role,
                    start: offset + m.start(),
                    end: offset + m.end(),
                });
            }
        }
        offset += line.len();
    }
    out
}

pub(super) fn discover(
    l: &Ledger,
    snapshot: &Snapshot,
    ix: &Index,
    seed: &trace::Seed,
    candidates: &mut retrieval::Candidates,
) -> Result<Value> {
    let literals = Regex::new(r#"\"([^\"\\\s]{4,120})\"|'([^'\\\s]{4,120})'"#)?;
    let events = Regex::new(r"^[a-z][a-z0-9_-]*(?:\.[a-z][a-z0-9_-]*){2,}$")?;
    let headers = Regex::new(r"^x-[a-z0-9]+(?:-[a-z0-9]+){2,}$")?;
    let paths: Vec<_> = snapshot
        .files
        .keys()
        .filter(|p| {
            retrieval::path_allowed(p, seed)
                && ["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"]
                    .contains(&p.rsplit('.').next().unwrap_or(""))
        })
        .map(String::as_str)
        .collect();
    let mut grouped = BTreeMap::<String, BTreeMap<String, Use>>::new();
    snapshot
        .contents
        .scan(&snapshot.files, &paths, |path, text| {
            for occurrence in uses(path, text, &literals, &events, &headers) {
                let rank = |role| match role {
                    "declaration" => 3,
                    "consumer" => 2,
                    "producer" => 1,
                    _ => 0,
                };
                let files = grouped.entry(occurrence.key.clone()).or_default();
                if files
                    .get(path)
                    .is_none_or(|prior| rank(occurrence.role) > rank(prior.role))
                {
                    files.insert(path.into(), occurrence);
                }
            }
        })?;
    let dependencies: BTreeSet<_> = ix
        .traversal_links()
        .filter(|e| ["IMPORTS", "DEPENDS_ON", "REFERENCES"].contains(&e.predicate.as_str()))
        .flat_map(|e| {
            e.from_files.iter().flat_map(move |from| {
                e.to_files
                    .iter()
                    .map(move |to| (package(from), package(to)))
            })
        })
        .collect();
    // Only independently task-relevant origins participate. No recursive key
    // propagation and no authored paths, facts or witness locations enter here.
    let relevance = |c: &retrieval::Candidate| {
        c.signals
            .iter()
            .filter(|s| ["lexical", "anchors"].contains(&s.facet.as_str()))
            .map(|s| s.strength)
            .max()
            .unwrap_or(0)
    };
    let mut starters: Vec<_> = candidates.values().filter(|c| relevance(c) >= 50).collect();
    starters.sort_by_key(|c| (std::cmp::Reverse(relevance(c)), c.path.clone()));
    let omitted_origins = starters.len().saturating_sub(8);
    let origins: BTreeSet<_> = starters
        .into_iter()
        .take(8)
        .map(|c| c.path.clone())
        .collect();
    let mut associations = vec![];
    let (mut frequent, mut ambiguous) = (0, 0);
    for (key, occurrences) in grouped {
        let occurrences: Vec<_> = occurrences.into_values().collect();
        let holders: BTreeSet<_> = occurrences.iter().map(|u| u.path.clone()).collect();
        if holders.len() < 2 {
            continue;
        }
        if holders.len() > 8 {
            frequent += 1;
            continue;
        }
        let declarations: BTreeSet<_> = occurrences
            .iter()
            .filter(|u| u.role == "declaration" && snapshot.files[&u.path].role != "test")
            .map(|u| package(&u.path))
            .collect();
        if declarations.len() > 1 {
            ambiguous += 1;
            continue;
        }
        let canonical = declarations.iter().next();
        for origin in occurrences.iter().filter(|u| origins.contains(&u.path)) {
            let origin_scope = package(&origin.path);
            for target in occurrences.iter().filter(|u| u.path != origin.path) {
                let target_scope = package(&target.path);
                // Same-package literals share a declared repository scope.
                // Cross-package events require one canonical declaration scope
                // and a retained dependency on it from both endpoints.
                let accepts = |scope: &String, canonical: &String| {
                    scope == canonical || dependencies.contains(&(scope.clone(), canonical.clone()))
                };
                let scope = if origin_scope == target_scope {
                    origin_scope.clone()
                } else if target.family == "event"
                    && canonical
                        .is_some_and(|c| accepts(&origin_scope, c) && accepts(&target_scope, c))
                {
                    canonical.unwrap().clone()
                } else {
                    continue;
                };
                let witness = |u: &Use| {
                    json!({"path":u.path,"artifact_id":snapshot.files[&u.path].artifact_id,
                    "digest":snapshot.files[&u.path].digest,"byte_range":{"start":u.start,"end":u.end},"occurrence_role":u.role})
                };
                let row = json!({"kind":"exact_key_association","key":key,"family":target.family,
                    "scope":scope,"from_path":origin.path,"to_path":target.path,
                    "class":"candidate","basis":"shared exact literal within package/protocol scope",
                    "authority":"static_association","repository_files":holders.len(),
                    "witnesses":[witness(origin),witness(target)]});
                associations.push(row);
            }
        }
    }
    associations.sort_by_key(Value::to_string);
    associations.dedup();
    let report = crate::architecture::reports::retain_items(
        l,
        json!({"kind":"exact_key_associations",
        "view_id":snapshot.view.id,"index_id":ix.id,"items":associations}),
        0,
    )?;
    for (i, row) in associations.iter().enumerate() {
        let path = row["to_path"].as_str().unwrap();
        let provenance = json!({"key":row["key"],"scope":row["scope"],"authority":"static_association",
            "basis":row["basis"],"witnesses":row["witnesses"],"report_id":report["report_id"],"ref":format!("/items/{i}")});
        retrieval::contribute(
            candidates,
            path,
            retrieval::Signal {
                facet: "dependency".into(),
                kind: "exact_key_association".into(),
                strength: 60,
                confidence: "candidate".into(),
                depth: 1,
                via_link: None,
                provenance,
            },
        );
        let candidate = candidates.get_mut(path).unwrap();
        let span =
            json!({"byte_range":row["witnesses"][1]["byte_range"],"kind":"exact_key_association"});
        if !candidate.spans.contains(&span) {
            candidate.spans.push(span);
        }
    }
    Ok(
        json!({"state":"evaluated","associations":associations.len(),"report_id":report["report_id"],
        "max_key_files":8,"frequent_keys_skipped":frequent,"ambiguous_declaration_scopes_skipped":ambiguous,
        "origin_limit":8,"omitted_origins":omitted_origins,
        "interpretation":"Shared literals are candidate associations, not reads, writes, calls or behavioral guarantees."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_preserve_utf8_ranges_and_exclude_imports_comments_paths_and_generic_strings() {
        let text = "// type: \"a.b.c\"\nimport x from \"x-y-z-w\";\nconst λ = { type: 'thread.meta.update' };\nheaders.get(\"x-t3-relay-delivery-id\");\nconst path = './a.b.c'; const generic = 'accepted';\n";
        let got = uses(
            "src/a.ts",
            text,
            &Regex::new(r#"\"([^\"\\\s]{4,120})\"|'([^'\\\s]{4,120})'"#).unwrap(),
            &Regex::new(r"^[a-z][a-z0-9_-]*(?:\.[a-z][a-z0-9_-]*){2,}$").unwrap(),
            &Regex::new(r"^x-[a-z0-9]+(?:-[a-z0-9]+){2,}$").unwrap(),
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].role, "producer");
        assert_eq!(got[1].role, "consumer");
        for u in got {
            assert_eq!(&text[u.start..u.end], u.key);
        }
        assert_ne!(package("apps/a/src/file.ts"), package("apps/b/src/file.ts"));
    }
}
