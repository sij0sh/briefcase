use super::*;
use crate::architecture::retained_source;
use regex::Regex;

// Snapshot decoding and record definitions affect the derived index too.
const INDEX_SOURCES:&[&[u8]]=&[include_bytes!("mod.rs"),include_bytes!("evidence.rs"),
    include_bytes!("index.rs"),include_bytes!("producers.rs"),include_bytes!("../source.rs"),include_bytes!("../retained_source.rs")];
fn implementation_digest(sources:&[&[u8]]) -> String {identity::bytes_digest(&sources.concat())}

fn cache_key(s: &Snapshot) -> Result<String> {
    identity::id("task-index", &json!({"method":METHOD,"view":s.view.id,
        "profile_views":s.profiles.iter().chain(s.evidence_views.iter()).map(|v|v.id.clone()).collect::<Vec<_>>(),"profile_boundaries":s.profile_boundaries,
        "producer_links":s.producer_links,"bundle":identity::bytes_digest(&serde_json::to_vec(&s.view.bundle)?),
        "retraction_epoch":s.view.retraction_watermark,
        "implementation":implementation_digest(INDEX_SOURCES)}))
}

pub(super) fn load(l: &Ledger, s: &Snapshot) -> Result<Index> {
    let id = cache_key(s)?;
    if let Ok(value) = l.report(&id) {
        let ix: Index = serde_json::from_value(value)?;
        require(ix.id == id && ix.source_view == s.view.id,"TASK","Mismatched task index")?;
        return Ok(ix);
    }
    let mut ix = build(s,id)?;
    // Retain large occurrence inventories once, never in each task or response.
    for producer in ix.coverage["producer_boundaries"].as_array_mut().unwrap() {
        let unresolved=producer.as_object_mut().unwrap().remove("unresolved").unwrap_or(json!([]));
        let report=json!({"kind":"producer_boundaries","view_id":producer["view_id"],
            "scope":producer["scope"],"items":unresolved,"limitations":producer["limitations"]});
        let detail_id=identity::id("producer-boundaries",&report)?;
        l.save_report(&detail_id,&report,&[])?;
        producer["count"]=json!(rows(&report,"items").len());
        producer["detail_report_id"]=json!(detail_id);
        let limitations=rows(producer,"limitations").len();
        producer.as_object_mut().unwrap().remove("limitations");
        producer["limitation_count"]=json!(limitations);
    }
    l.save_report(&ix.id,&serde_json::to_value(&ix)?,&[])?;
    Ok(ix)
}

/// Called at sync time. Task reads reuse the same content-addressed index.
pub(crate) fn precompute(l: &Ledger, view: &str) -> Result<Value> {
    let s = Snapshot::load(l,view)?;
    let ix = load(l,&s)?;
    Ok(json!({"index_id":ix.id,"files":ix.files.len(),"relationships":ix.links.len(),"coverage":summary(&ix)}))
}

/// Bounded health metadata; the pinned index and producer reports own details.
pub(super) fn summary(ix:&Index) -> Value {
    json!({"source_view":ix.source_view,"files":ix.files.len(),"relationships":ix.links.len(),
        "profile_count":rows(&ix.coverage,"profile_views").len(),"languages":ix.coverage["languages"],
        "unresolved_occurrences":rows(&ix.coverage,"producer_boundaries").iter().map(|p|
            p["count"].as_u64().unwrap_or(rows(p,"unresolved").len() as u64)).sum::<u64>(),
        "unresolved_bindings":rows(&ix.coverage,"unresolved_bindings").len(),
        "omitted_relationship_occurrences":ix.coverage["omitted_relationship_occurrences"],
        "propagation":ix.coverage["propagation"],
        "details":{"operation":"page","arguments":{"report_id":ix.id,"section":"/coverage/producer_boundaries","offset":0,"limit":20}}})
}

fn build(s: &Snapshot, id: String) -> Result<Index> {
    let v = &s.view;
    let mut entities = BTreeMap::<String,Entity>::new();
    let mut by_id = BTreeMap::<String,String>::new();
    let mut modules=BTreeMap::<String,BTreeSet<String>>::new();
    let mut symbols=BTreeMap::new();
    for profile in &s.profiles {
        let mut active=profile.clone();
        let eligible=active.eligible.clone();
        active.bundle["assertions"].as_array_mut().unwrap().retain(|a|eligible.contains(&text(a,"id")));
        for (key,files) in retained_source::module_files(&active) {modules.entry(key).or_default().extend(files);}
        symbols.extend(retained_source::symbol_owners(&active));
    }
    for e in rows(&v.bundle,"entity_versions") {
        if ["feature","workflow"].contains(&e["kind"].as_str().unwrap_or(""))
            && !rows(&v.bundle,"assertions").iter().any(|a|a["class"]=="declaration" && v.eligible.contains(&text(a,"id"))
                && (a["proposition"]["subject_id"]==e["id"] || a["proposition"]["object"]["entity_version_id"]==e["id"])) {
            continue;
        }
        let raw = text(e,"canonical_key");
        let key = if e["kind"] == "file" { raw.strip_prefix("file::").unwrap_or(&raw).to_owned() } else { raw };
        let mut files = BTreeSet::new();
        if e["kind"] == "file" && s.files.contains_key(&key) { files.insert(key.clone()); }
        if let Some(held) = modules.get(&key) { files.extend(held.iter().filter(|p|s.files.contains_key(*p)).cloned()); }
        if let Some(owner) = symbols.get(&key) {
            if let Some(file) = &owner.file { if s.files.contains_key(file) { files.insert(file.clone()); } }
            if files.is_empty() { files.extend(modules.get(&owner.module).into_iter().flatten().cloned()); }
        }
        let mut names = BTreeSet::from([key.clone()]);
        if e["kind"] == "symbol" {names.insert(key.split('#').next().unwrap_or(&key).to_owned());}
        if ["module","symbol","function","domain","feature","workflow"].contains(&e["kind"].as_str().unwrap_or("")) {
            let terminal = key.rsplit("::").next().unwrap_or(&key).split('#').next().unwrap_or(&key);
            names.insert(terminal.trim_start_matches("feature:").trim_start_matches("workflow:").trim_start_matches("domain:").into());
        }
        by_id.insert(text(e,"id"),key.clone());
        let entry=entities.entry(key.clone()).or_insert_with(||Entity {key,kind:text(e,"kind"),names:BTreeSet::new(),files:BTreeSet::new()});
        entry.names.extend(names);entry.files.extend(files);
    }
    let assertions: Vec<_> = rows(&v.bundle,"assertions").iter().filter(|a|
        v.eligible.contains(&text(a,"id"))
            && a["class"] != "claim" && a["support_class"] != "includes_inference").collect();
    // Literal wording is eligible only as Declaration, never model prose.
    for a in &assertions {
        if a["class"] != "declaration" { continue; }
        let p = &a["proposition"];
        if ["DISPLAY_NAME","ALIAS"].contains(&p["predicate"].as_str().unwrap_or("")) {
            if let (Some(key),Some(name)) = (by_id.get(&text(p,"subject_id")),p["object"]["value"].as_str()) {
                if let Some(e) = entities.get_mut(key) {e.names.insert(name.into());}
            }
        }
    }
    let artifact_paths:BTreeMap<_,_>=rows(&v.bundle,"artifacts").iter().map(|a|(text(a,"id"),text(a,"path"))).collect();
    let evidence_paths:BTreeMap<_,_>=rows(&v.bundle,"evidence_refs").iter().filter_map(|r|artifact_paths.get(&text(r,"artifact_id")).map(|p|(text(r,"id"),p.clone()))).collect();
    let mut links = BTreeMap::new();
    let mut omitted=0usize;
    for a in &assertions {
        let p = &a["proposition"];
        let Some(from)=by_id.get(&text(p,"subject_id")) else {continue};
        let predicate=text(p,"predicate");
        let (to,value)=if let Some(to)=by_id.get(&text(&p["object"],"entity_version_id")) {(to.clone(),None)}
            else if let Some(value)=p["object"].get("value") {(identity::id("context-value",&json!([predicate,value]))?,Some(value.clone()))}
            else {continue};
        let mut support_files=BTreeSet::new();
        for reference in crate::architecture::contracts::strings(&a["evidence_ref_ids"]) {
            if let Some(path)=evidence_paths.get(&reference).filter(|p|s.files.contains_key(*p)) {support_files.insert(path.clone());}
        }
        // A source occurrence localizes a module-level import/reference. A
        // declaration file never becomes another owner of the target symbol.
        let from_files=if entities[from].kind=="module" && !support_files.is_empty()
            && ["IMPORTS","REFERENCES","CALLS_STATIC","DEPENDS_ON"].contains(&predicate.as_str()) {
            support_files.clone()
        } else {entities[from].files.clone()};
        let to_files=entities.get(&to).map(|e|e.files.clone()).unwrap_or_default();
        let mut link=Link::new(&predicate,from,&to,&text(a,"class"),"retained_assertion",
            vec![text(a,"id")],vec![],from_files.clone(),to_files.clone())?;
        link.value=value;
        link.support_files=support_files;
        insert_link(&mut links,link,&mut omitted);
        if ["IMPORTS","REFERENCES","CALLS_STATIC","DEPENDS_ON"].contains(&predicate.as_str()) {
            for test in from_files.iter().filter(|p|s.files.get(*p).is_some_and(|f|f.role == "test")) {
                for implementation in to_files.iter().filter(|p|s.files.get(*p).is_some_and(|f|f.role == "impl")) {
                    let link = Link::new("TESTS",test,implementation,"derived",&format!("test_{predicate}; static association, not executed coverage"),
                        vec![text(a,"id")],vec![],BTreeSet::from([test.clone()]),BTreeSet::from([implementation.clone()]))?;
                    insert_link(&mut links,link,&mut omitted);
                }
            }
        }
    }
    for link in &s.producer_links {insert_link(&mut links,link.clone(),&mut omitted);}
    // A path token is joined only when it resolves to a captured exact path.
    // Relative references are resolved against the referring artifact directory.
    let path_token = Regex::new(r#"[\p{L}\p{N}_@.+-]+(?:[/\\][\p{L}\p{N}_@.+-]+)+"#)?;
    let qualified = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+")?;
    let quoted_path=Regex::new(r#"[\"']([^\"'\r\n]+\.(?:rs|py|ts|tsx|js|mjs|cjs|json|toml|yaml|yml))[\"']"#)?;
    let mut qualified_files=BTreeMap::<String,BTreeSet<String>>::new();
    for entity in entities.values() {for name in &entity.names {
        qualified_files.entry(name.clone()).or_default().extend(entity.files.clone());
    }}
    for (path,content) in &s.contents {
        for found in path_token.find_iter(content) {
            let literal=found.as_str().trim_end_matches(['.', ',', ';', ':']);
            let token = literal.replace('\\',"/");
            let relative = normalize_relative(path,&token);
            let target = if s.files.contains_key(&token) {Some(token)} else {relative.filter(|p|s.files.contains_key(p))};
            let Some(target) = target.filter(|t|t != path) else {continue};
            let predicate = if s.files[path].role == "test" && s.files[&target].role == "impl" {"TESTS"} else {"REFERENCES_ARTIFACT"};
            let link = Link::new(predicate,path,&target,"derived","exact retained path reference; not execution or implementation proof",
                vec![],vec![s.witness(path,found.start(),found.start()+literal.len())],BTreeSet::from([path.clone()]),BTreeSet::from([target.clone()]))?;
            insert_link(&mut links,link,&mut omitted);
        }
        // Includes such as #[path="briefcase_cases.rs"] name a local file
        // without a slash. The retained relative path is an exact association.
        for capture in quoted_path.captures_iter(content) {
            let literal=capture.get(1).unwrap();
            let Some(target)=normalize_relative(path,literal.as_str()).filter(|p|p!=path&&s.files.contains_key(p)) else {continue};
            let predicate=if s.files[path].role=="test"&&s.files[&target].role=="impl" {"TESTS"} else {"REFERENCES_ARTIFACT"};
            let link=Link::new(predicate,path,&target,"derived","exact quoted relative path; static association, not executed coverage",vec![],
                vec![s.witness(path,literal.start(),literal.end())],BTreeSet::from([path.clone()]),BTreeSet::from([target.clone()]))?;
            insert_link(&mut links,link,&mut omitted);
        }
        if s.files[path].role == "test" {
            for found in qualified.find_iter(content) {
                let Some(target) = qualified_files.get(found.as_str()) else {continue};
                for implementation in target.iter().filter(|p|s.files.get(*p).is_some_and(|f|f.role == "impl")) {
                    let link = Link::new("TESTS",path,implementation,"derived","test mentions exact qualified entity; static association, not executed coverage",
                        vec![],vec![s.witness(path,found.start(),found.end())],BTreeSet::from([path.clone()]),BTreeSet::from([implementation.clone()]))?;
                    insert_link(&mut links,link,&mut omitted);
                }
            }
        }
    }
    config_links(s,&mut links,&mut omitted)?;
    // Feature membership uses only declared implementation/workflow bindings.
    let mut memberships: BTreeMap<String,BTreeSet<String>> = entities.iter().filter(|(_,e)|e.kind == "feature")
        .map(|(key,_)|(key.clone(),BTreeSet::new())).collect();
    let mut membership_complete=memberships.is_empty();
    for _ in 0..entities.len().min(64) {
        let before = memberships.clone();
        for link in links.values() {
            if link.class != "declaration" {continue;}
            match link.predicate.as_str() {
                "IMPLEMENTS_FEATURE" => {memberships.entry(link.to.clone()).or_default().extend(link.from_files.clone());},
                "REALIZES_FEATURE" => {
                    let participants: BTreeSet<_> = links.values().filter(|l|l.class == "declaration" && l.to == link.from && ["ENTRYPOINT_FOR","PARTICIPATES_IN"].contains(&l.predicate.as_str()))
                        .flat_map(|l|l.from_files.iter().cloned()).collect();
                    memberships.entry(link.to.clone()).or_default().extend(participants);
                },
                "SUBFEATURE_OF" => {
                    let children = before.get(&link.from).cloned().unwrap_or_default();
                    memberships.entry(link.to.clone()).or_default().extend(children);
                },
                _=>{}
            }
        }
        if before == memberships {membership_complete=true;break;}
    }
    for (feature,files) in memberships {
        if let Some(e)=entities.get_mut(&feature) {e.files.extend(files);}
    }
    // Ownership propagation is restricted to explicit structural/binding links.
    // Calls and references do not make the caller's file own the callee.
    let mut ownership_complete=entities.is_empty();
    for _ in 0..entities.len().min(64) {
        let before: BTreeMap<_,_>=entities.iter().map(|(key,e)|(key.clone(),e.files.clone())).collect();
        for edge in links.values() {
            match edge.predicate.as_str() {
                "DECLARES"|"CONTAINS" if entities.get(&edge.from).is_some_and(|e|e.kind=="file")
                    || (entities.get(&edge.from).is_some_and(|e|e.kind!="repository"&&e.kind!="package")
                        && entities.get(&edge.to).is_some_and(|e|!["module","file","package","repository"].contains(&e.kind.as_str()))) => {
                    if let Some(files)=before.get(&edge.from) {if let Some(e)=entities.get_mut(&edge.to) {
                        if e.files.is_empty() {e.files.extend(files.clone());}
                    }}
                },
                "ROUTES_TO" => if let Some(files)=before.get(&edge.to) {if let Some(e)=entities.get_mut(&edge.from) {e.files.extend(files.clone());}},
                "ENTRYPOINT_FOR"|"PARTICIPATES_IN"|"INCLUDES_SOURCE"|"DEPLOYS_AS"|"MAPS_TO" => {
                    let files=before.get(&edge.from).cloned().unwrap_or_else(||edge.from_files.clone());
                    if let Some(e)=entities.get_mut(&edge.to) {e.files.extend(files);}
                },
                "MAPS_TO_SERVICE" => if let Some(files)=before.get(&edge.to) {if let Some(e)=entities.get_mut(&edge.from) {e.files.extend(files.clone());}},
                _=>{},
            }
        }
        if entities.iter().all(|(key,e)|before.get(key)==Some(&e.files)) {ownership_complete=true;break;}
    }
    for edge in links.values_mut() {
        if edge.from_files.is_empty() {if let Some(e)=entities.get(&edge.from) {edge.from_files=e.files.clone();}}
        if let Some(e)=entities.get(&edge.to) {edge.to_files=e.files.clone();}
    }
    let captured_languages: BTreeSet<_> = s.files.values().map(|f|f.language.clone()).filter(|l|l != "text").collect();
    let analyzed_languages: BTreeSet<_> = rows(&v.bundle,"entity_versions").iter().filter(|e|e["kind"] == "module")
        .map(|e|text(e,"language")).filter(|l|!l.is_empty() && l != "none").collect();
    let languages: Vec<_> = captured_languages.iter().map(|lang|json!({"language":lang,"retained_files":s.files.values().filter(|f|&f.language == lang).count(),
        "semantic_profile":if analyzed_languages.contains(lang) {"present; consult producer coverage"} else {"not_run"}})).collect();
    let unresolved_bindings:Vec<_>=crate::architecture::declarations::current_resolutions(&v.bundle).into_values()
        .filter(|r|r["status"]!="resolved" || !v.eligible.contains(&text(r,"assertion_id")))
        .map(|r|json!({"resolution_id":r["id"],"context":r["context"],"owner":by_id.get(&text(&r,"owner_entity_id")),
            "status":if r["status"]=="resolved" {json!("withdrawn")} else {r["status"].clone()},"predicate":r["predicate"],
            "requested_key":r["requested_key"],"requested_kind":r["requested_kind"],"reason":r["reason"]})).collect();
    let coverage = json!({"source_view":v.id,"profile_views":s.profiles.iter().chain(s.evidence_views.iter()).map(|p|p.id.clone()).collect::<Vec<_>>(),
        "evidence_producers":s.evidence_views.iter().map(|p|json!({"view_id":p.id,"scope":p.scope,"diagnostics":p.diagnostics})).collect::<Vec<_>>(),
        "analysis_profiles":s.profiles.iter().map(|p|json!({"view_id":p.id,"coverage":p.diagnostics["coverage"],
            "call_coverage":p.diagnostics["call_coverage"],
            "static_call_runs":rows(&p.bundle,"runs").iter().filter(|r|rows(&r["coverage"],"guarantees").iter().any(|g|g["predicate"]=="CALLS_STATIC")).map(|r|json!({"id":r["id"],"coverage":r["coverage"]})).collect::<Vec<_>>(),"provider_name":p.diagnostics["provider"]["name"]})).collect::<Vec<_>>(),
        "capture":inventory_summary(&v.diagnostics["capture"]["inventory"]),"profile_boundaries":s.profile_boundaries,"languages":languages,"unresolved_bindings":unresolved_bindings,
        "semantic_coverage":v.diagnostics["coverage"],
        "producer_boundaries":s.profiles.iter().chain(s.evidence_views.iter()).map(|p|json!({"view_id":p.id,"scope":p.scope,
            "unresolved":rows(&p.bundle,"unresolved_references"),"limitations":p.diagnostics["limitations"]})).collect::<Vec<_>>(),"relationship_limit":100000,"omitted_relationship_occurrences":omitted,"relationships":"retained eligible assertions and labelled exact-text derivations; no inferred feature ownership",
        "projection_check":"not_requested; durable source records and digest-verified bytes only",
        "propagation":{"iteration_limit":64,"feature_membership_complete":membership_complete,"file_associations_complete":ownership_complete},
        "uncovered":["dynamic dispatch and generated references", "unrecorded sessions", "runtime selection of configuration", "live installed files"]});
    Ok(Index {id,source_view:v.id.clone(),retraction_epoch:v.retraction_watermark,files:s.files.clone(),entities,links:links.into_values().collect(),coverage})
}

fn normalize_relative(path: &str, token: &str) -> Option<String> {
    let mut parts: Vec<_> = path.split('/').collect(); parts.pop();
    for part in token.split('/') {
        match part {"."=>{},".."=>{parts.pop()?;},""=>return None,_=>parts.push(part)}
    }
    Some(parts.join("/"))
}

fn config_links(s: &Snapshot, links: &mut BTreeMap<String,Link>, omitted: &mut usize) -> Result<()> {
    let access = Regex::new(r#"(?:env::var(?:_os)?|std::env::var(?:_os)?|os\.getenv|getenv)\s*\(\s*["']([A-Za-z_][A-Za-z0-9_]*)["']|process\.env\.([A-Za-z_][A-Za-z0-9_]*)|os\.environ\s*\[\s*["']([A-Za-z_][A-Za-z0-9_]*)["']"#)?;
    let declaration = Regex::new(r#"(?m)^\s*["']?([A-Z][A-Z0-9_]+)["']?\s*[:=]"#)?;
    let executable = Regex::new(r#"(?:Command::new|spawn|execFile)\s*\(\s*["']([^"'\r\n]+)["']"#)?;
    let external = Regex::new(r#"(?:\$\{?HOME\}?|~|[A-Z]:|/etc|/usr|/opt)/[A-Za-z0-9_./{}$-]+"#)?;
    let plan = Regex::new(r"briefcase:\s*implements-plan\s+([\p{L}\p{N}_./-]+)(?:#([\p{L}\p{N}_-]+))?")?;
    let install = Regex::new(r#"(?m)^\s*(?:cp|install)\s+([A-Za-z0-9_./-]+)\s+["']?((?:\$\{?HOME\}?|~|/etc|/usr|/opt)/[A-Za-z0-9_./{}$-]+)["']?\s*$"#)?;
    for (path,content) in &s.contents {
        let role = &s.files[path].role;
        if ["impl","script","install","test"].contains(&role.as_str()) {
            for found in access.captures_iter(content) {
                let key = (1..=3).find_map(|i|found.get(i)).expect("config access capture");
                let target = format!("env:{}",key.as_str());
                let link = Link::new("READS_CONFIG",path,&target,"derived","literal environment access; runtime value/source unknown",vec![],
                    vec![s.witness(path,key.start(),key.end())],BTreeSet::from([path.clone()]),BTreeSet::new())?;
                insert_link(links,link,omitted);
            }
            for found in executable.captures_iter(content) {
                let value = found.get(1).unwrap(); let target = format!("executable:{}",value.as_str());
                let link = Link::new("REFERENCES_TOUCHPOINT",path,&target,"derived","literal spawned executable; installed binary identity unknown",vec![],
                    vec![s.witness(path,value.start(),value.end())],BTreeSet::from([path.clone()]),BTreeSet::new())?;
                insert_link(links,link,omitted);
            }
        }
        if ["config","build"].contains(&role.as_str()) {
            for found in declaration.captures_iter(content) {
                let key = found.get(1).unwrap(); let target = format!("env:{}",key.as_str());
                let link = Link::new("DECLARES_CONFIG_KEY",path,&target,"derived","literal configuration key; active runtime configuration unknown",vec![],
                    vec![s.witness(path,key.start(),key.end())],BTreeSet::from([path.clone()]),BTreeSet::new())?;
                insert_link(links,link,omitted);
            }
        }
        for value in external.find_iter(content) {
            let target=format!("path:{}",value.as_str());
            let link=Link::new("REFERENCES_TOUCHPOINT",path,&target,"derived","literal external path; external filesystem was not read",vec![],
                vec![s.witness(path,value.start(),value.end())],BTreeSet::from([path.clone()]),BTreeSet::new())?;
            insert_link(links,link,omitted);
        }
        for annotation in plan.captures_iter(content) {
            let target=annotation.get(1).unwrap();
            if !s.files.get(target.as_str()).is_some_and(|a|a.role == "plan" || a.role == "adr") {continue;}
            let section=annotation.get(2).map(|m|m.as_str()).unwrap_or("");
            let target_key=if section.is_empty() {target.as_str().to_owned()} else {format!("{}#{section}",target.as_str())};
            let found=annotation.get(0).unwrap();
            let link=Link::new("IMPLEMENTS_PLAN_SECTION",path,&target_key,"derived","literal implements-plan annotation; declared intent, not verified completion",vec![],
                vec![s.witness(path,found.start(),found.end())],BTreeSet::from([path.clone()]),BTreeSet::from([target.as_str().to_owned()]))?;
            insert_link(links,link,omitted);
        }
        if s.files[path].role == "install" || s.files[path].role == "script" {
            for declaration in install.captures_iter(content) {
                let from=declaration.get(1).unwrap();let to=declaration.get(2).unwrap();
                if !s.files.contains_key(from.as_str()) {continue;}
                let target=format!("path:{}",to.as_str());let found=declaration.get(0).unwrap();
                let mut link=Link::new("INSTALLS_TO",from.as_str(),&target,"derived","retained copy/install command; desired installation only, actual installed bytes unknown",vec![],
                    vec![s.witness(path,found.start(),found.end())],BTreeSet::from([from.as_str().to_owned()]),BTreeSet::new())?;
                link.support_files.insert(path.clone());
                insert_link(links,link,omitted);
            }
        }
    }
    Ok(())
}

fn insert_link(links:&mut BTreeMap<String,Link>,link:Link,omitted:&mut usize) {
    if links.len()<100000 || links.contains_key(&link.id) {links.insert(link.id.clone(),link);}
    else {*omitted+=1;}
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn every_index_source_participates_in_cache_invalidation() {
        let original=implementation_digest(INDEX_SOURCES);
        for i in 0..INDEX_SOURCES.len() {
            let mut changed=INDEX_SOURCES[i].to_vec();changed.push(b' ');
            let mut sources=INDEX_SOURCES.to_vec();sources[i]=&changed;
            assert_ne!(original,implementation_digest(&sources),"source {i}");
        }
    }
}
