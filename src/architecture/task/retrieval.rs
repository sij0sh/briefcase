//! Task-oriented facets over one evidence index. Scores select reading context;
//! they never create assertions, feature ownership or completeness guarantees.
use super::*;
use super::{request::Request,trace::{Reach,Seed,AnchorStatus}};

pub(super) const FACETS:&[&str]=&["structure","dependency","tests","domain","execution","data","runtime","history","sessions","lexical"];
const MIN_SCORE:u64=24;
const MAX_SIGNALS:usize=12;

#[derive(Clone,Debug,Serialize,Deserialize)]
pub(super) struct Signal {
    pub facet:String,
    pub kind:String,
    pub strength:u64,
    pub confidence:String,
    pub depth:usize,
    pub via_link:Option<String>,
    pub provenance:Value,
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub(super) struct Candidate {
    pub path:String,
    pub rank:usize,
    pub score:u64,
    pub selected:bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub delivery_rank:Option<usize>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub role:Option<String>,
    pub authority:String,
    pub signals:Vec<Signal>,
    pub spans:Vec<Value>,
    /// Source routing is valid for unselected candidates too. Omitted in old pins.
    #[serde(default,skip_serializing_if="String::is_empty")]
    pub source_ref:String,
}
impl Candidate {
    fn new(path:&str)->Self {Self {path:path.into(),rank:0,score:0,selected:false,
        delivery_rank:None,role:None,authority:"retrieval_hint; not implementation membership".into(),signals:vec![],spans:vec![],source_ref:String::new()}}
    pub fn reach(&self)->Reach {
        let signal=self.signals.iter().min_by_key(|s|(std::cmp::Reverse(s.strength),s.depth));
        Reach {depth:signal.map_or(0,|s|s.depth),via_link:signal.and_then(|s|s.via_link.clone())}
    }
}
pub(super) type Candidates=BTreeMap<String,Candidate>;
pub(super) fn contribute(candidates:&mut Candidates,path:&str,signal:Signal) {
    if source::is_evaluation_artifact(path) && !(signal.facet=="anchors" && signal.kind=="exact_anchor") { return; }
    let candidate=candidates.entry(path.into()).or_insert_with(||Candidate::new(path));
    // At most one contribution per facet/kind/link. Repeated occurrences and
    // repeated passes cannot inflate scores. Keep strongest explanations first.
    if let Some(prior)=candidate.signals.iter_mut().find(|s|s.facet==signal.facet && s.kind==signal.kind && s.via_link==signal.via_link) {
        if signal.strength>prior.strength {*prior=signal;}
    } else {candidate.signals.push(signal);}
    candidate.signals.sort_by_key(|s|(std::cmp::Reverse(s.strength),s.facet.clone(),s.via_link.clone()));
    // Reserve diversity before applying the explanation bound.
    let mut seen=BTreeSet::new();
    let diverse:Vec<_>=candidate.signals.iter().filter(|s|seen.insert(s.facet.clone())).cloned().collect();
    let mut kept=diverse;
    for s in &candidate.signals {if kept.len()<MAX_SIGNALS && !kept.iter().any(|k|k.facet==s.facet&&k.kind==s.kind&&k.via_link==s.via_link) {kept.push(s.clone());}}
    candidate.signals=kept;
    let mut strongest=BTreeMap::<&str,u64>::new();
    for s in &candidate.signals {strongest.entry(&s.facet).and_modify(|n|*n=(*n).max(s.strength)).or_insert(s.strength);}
    candidate.score=strongest.values().sum::<u64>()+strongest.len().saturating_sub(1) as u64*8;
}
fn signal(facet:&str,kind:&str,strength:u64,confidence:&str,depth:usize,link:Option<&str>,provenance:Value)->Signal {
    Signal {facet:facet.into(),kind:kind.into(),strength,confidence:confidence.into(),depth,
        via_link:link.map(str::to_owned),provenance}
}
pub(super) fn terms(task:&str)->Vec<String> {
    const STOP:&[&str]=&["the","and","for","with","from","this","that","how","what","which","does","into","through","about","can","please","would","should","could","use","using","make","change","fix","add","new","support","called","similar","correctly"];
    let mut terms=BTreeSet::new();
    for word in task.split(|c:char|!c.is_alphanumeric()) {
        let term=word.to_lowercase();
        if term.len()>2 && !STOP.contains(&term.as_str()) {terms.insert(term);}
    }
    let mut bounded=vec![];let mut used=0;
    for mut term in terms.into_iter().take(32) {
        let room=4096usize.saturating_sub(used+usize::from(!bounded.is_empty()));
        if room<3 {break;}
        if term.len()>room {let mut end=room;while !term.is_char_boundary(end) {end-=1;}term.truncate(end);}
        used+=term.len()+usize::from(!bounded.is_empty());bounded.push(term);
    }
    bounded
}
pub(super) fn matches(text:&str,terms:&[String])->usize {
    let text=text.to_lowercase();terms.iter().filter(|t|text.contains(t.as_str())).count()
}

pub(super) fn discover(l:&Ledger,s:&Snapshot,ix:&Index,r:&Request,seed:&Seed)->Result<(Candidates,Vec<Value>,Value)> {
    let mut candidates=Candidates::new();
    for path in &seed.paths {
        contribute(&mut candidates,path,signal("anchors",if seed.status==AnchorStatus::Resolved {"exact_anchor"} else {"ambiguous_location"},
            if seed.status==AnchorStatus::Resolved {10000} else {90},if seed.status==AnchorStatus::Resolved {"exact"} else {"candidate"},0,None,
            json!({"identities":seed.candidates,"status":seed.status})));
    }
    let mut lexical=vec![];
    let mut coverage=json!({"enabled":r.enabled("lexical"),"candidate_limit":1000,"excerpt_byte_limit":1048576,
        "candidate_limit_reached":false,"omitted_candidates":0,"state":"not_queried"});
    if r.enabled("lexical") {
        let terms=terms(&r.task);
        if !terms.is_empty() {
            let search=crate::architecture::search::search(l,&json!({"text":terms.join(" "),"view":s.view.id,
                "history":false,"sessions":false,"classes":"text","limit":1000}))?;
            coverage["state"]=json!("queried");coverage["report_id"]=search["report_id"].clone();
            coverage["candidate_limit_reached"]=search["coverage"]["candidate_limit_reached"].clone();
            coverage["omitted_candidates"]=search["omitted_candidate_count"].clone();
            for hit in rows(&search,"items") {
                let Some(path)=hit["path"].as_str().filter(|p|s.files.contains_key(*p)) else {continue};
                let rank=hit["rank"].as_u64().unwrap_or(1000);
                let matched=matches(s.contents.get(path).map(String::as_str).unwrap_or(""),&terms);
                let path_matches=matches(path,&terms);
                let strength=(70*8/(rank+7)+((matched*18)/terms.len().max(1)) as u64+(path_matches.min(3)*12) as u64).min(110);
                let span=json!({"byte_range":hit["byte_range"],"line_range":hit["line_range"]});
                contribute(&mut candidates,path,signal("lexical","retained_text",strength,"candidate",0,None,
                    json!({"report_id":search["report_id"],"rank":rank,"matched_terms":matched})));
                candidates.get_mut(path).unwrap().spans.push(span.clone());
                lexical.push(json!({"path":path,"rank":rank,"strength":strength,"span":span,
                    "search_report_id":search["report_id"],"basis":"retained text retrieval hint; no implementation membership established"}));
            }
        }
    }
    if r.enabled("domain") {claim_hints(s,ix,r,&mut candidates);}
    Ok((candidates,lexical,coverage))
}

/// Claims stay outside authoritative adjacency. Eligible explicit module →
/// domain/feature Claims may only suggest files when their domain is requested.
fn claim_hints(s:&Snapshot,ix:&Index,r:&Request,candidates:&mut Candidates) {
    let entities:BTreeMap<_,_>=rows(&s.view.bundle,"entity_versions").iter().map(|e|(text(e,"id"),e)).collect();
    let terms=terms(&r.task);
    for a in rows(&s.view.bundle,"assertions").iter().filter(|a|s.view.eligible.contains(&text(a,"id")) &&
        (a["class"]=="claim" || a["support_class"]=="includes_inference")) {
        let p=&a["proposition"];
        if !["BELONGS_TO_DOMAIN","IMPLEMENTS_FEATURE","PARTICIPATES_IN","ENTRYPOINT_FOR"].contains(&p["predicate"].as_str().unwrap_or("")) {continue;}
        let (Some(from),Some(to))=(entities.get(&text(p,"subject_id")),entities.get(&text(&p["object"],"entity_version_id"))) else {continue};
        let key=text(to,"canonical_key");
        if terms.is_empty() || matches(&key,&terms)==0 {continue;}
        if let Some(owner)=ix.entities.get(&text(from,"canonical_key")) {for path in &owner.files {
            contribute(candidates,path,signal("domain","claim_hint",28,"inferred_candidate",0,None,
                json!({"assertion_id":a["id"],"class":a["class"],"target":key,"membership_established":false})));
        }}
    }
}

/// Each predicate has a retrieval policy, not another persisted graph product.
fn facet(predicate:&str)->Option<(&'static str,u64)> {
    Some(match predicate {
        "CONTAINS"|"DECLARES"=>("structure",35),
        "TESTS"=>("tests",100),
        "IMPORTS"|"DEPENDS_ON"|"REFERENCES"=>("dependency",80),
        "REFERENCES_ARTIFACT"|"IMPLEMENTS_PLAN_SECTION"=>("dependency",65),
        "BELONGS_TO_DOMAIN"|"IMPLEMENTS_FEATURE"|"REALIZES_FEATURE"|"SUBFEATURE_OF"=>("domain",90),
        "ENTRYPOINT_FOR"|"PARTICIPATES_IN"|"ROUTES_TO"|"CALLS_STATIC"=>("execution",95),
        "READS"|"WRITES"|"DERIVES_FROM"|"MAPS_TO"|"TRANSFORMS"=>("data",85),
        "BUILDS"|"DEPLOYS_AS"|"CONNECTS_TO"|"CALLS_OBSERVED"|"CONNECTION_OBSERVED"|"MAPS_TO_SERVICE"|"DECLARES_CONNECTION"|"INCLUDES_SOURCE"=>("runtime",80),
        "READS_CONFIG"|"DECLARES_CONFIG_KEY"|"INSTALLS_TO"|"REFERENCES_TOUCHPOINT"=>("dependency",70),
        _=>return None,
    })
}

pub(super) fn expand(ix:&Index,r:&Request,seed:&Seed,candidates:&mut Candidates) {
    let mut neighbors=BTreeMap::<String,BTreeSet<String>>::new();
    let mut key_neighbors=BTreeMap::<&str,BTreeSet<&str>>::new();
    for e in &ix.links {
        if !link_allowed(e,seed) {continue;}
        let Some((facet,_))=facet(&e.predicate) else {continue};
        if !r.enabled(facet) || !r.duty.policy().trace.predicates.contains(&e.predicate.as_str()) {continue;}
        key_neighbors.entry(&e.from).or_default().insert(&e.to);
        key_neighbors.entry(&e.to).or_default().insert(&e.from);
        for a in &e.from_files {for b in &e.to_files {if a!=b {
            neighbors.entry(a.clone()).or_default().insert(b.clone());
            neighbors.entry(b.clone()).or_default().insert(a.clone());
        }}}
    }
    let mut starters:Vec<_>=candidates.values().filter(|c|c.score>=50).collect();
    starters.sort_by_key(|c|(std::cmp::Reverse(c.score),c.path.clone()));
    let mut frontier_paths:BTreeSet<_>=starters.iter().take(8).map(|c|c.path.clone()).collect();
    frontier_paths.extend(seed.paths.iter().filter(|path|path_allowed(path,seed)).cloned());
    let mut frontier_keys=seed.candidates.clone();
    let (mut visited_paths,mut visited_keys)=(frontier_paths.clone(),frontier_keys.clone());
    for depth in 1..=r.depth {
        let (mut next_paths,mut next_keys)=(BTreeSet::new(),BTreeSet::new());
        for e in &ix.links {
            if !link_allowed(e,seed) {continue;}
            let Some((facet,base))=facet(&e.predicate) else {continue};
            // TESTS is a direct association. A shared test suite must not
            // bridge from one implementation into every implementation it
            // mentions. Cargo ownership already supplies runner associations.
            if facet=="tests" && depth>1 {continue;}
            if !r.duty.policy().trace.predicates.contains(&e.predicate.as_str()) {continue;}
            if !r.enabled(facet) {continue;}
            // Structure localizes an explicit identity only. It cannot turn a
            // file into a traversal bridge through every declaration it holds.
            if facet=="structure" && (depth>1 || !seed.candidates.contains(&e.from)&&!seed.candidates.contains(&e.to)) {continue;}
            let incoming_only=matches!(r.duty,Duty::Impact)&&["dependency","execution"].contains(&facet);
            let outgoing_only=matches!(r.duty,Duty::Explain|Duty::Run)&&["dependency","execution"].contains(&facet);
            let outgoing=!incoming_only&&(frontier_keys.contains(&e.from)||!frontier_paths.is_disjoint(&e.from_files));
            let incoming=!outgoing_only&&(frontier_keys.contains(&e.to)||!frontier_paths.is_disjoint(&e.to_files));
            for (touch,origin,target,key,origin_key) in [(outgoing,&e.from_files,&e.to_files,&e.to,&e.from),(incoming,&e.to_files,&e.from_files,&e.from,&e.to)] {
                if !touch {continue;}
                let degree=origin.iter().filter(|p|frontier_paths.contains(*p)).map(|p|neighbors.get(p).map_or(0,BTreeSet::len)).max().unwrap_or(0)
                    .max(key_neighbors.get(origin_key.as_str()).map_or(0,BTreeSet::len));
                let hub_penalty=if degree>8 {(usize::BITS-(degree as usize).leading_zeros()) as u64*9} else {0};
                let fanout_penalty=target.len().saturating_sub(1).min(20) as u64*3;
                let strength=base.saturating_sub((depth-1) as u64*24+hub_penalty+fanout_penalty);
                if strength<12 {continue;}
                for path in target.iter().filter(|p|ix.files.contains_key(*p)) {
                    contribute(candidates,path,signal(facet,&e.predicate,strength,"retained_relationship",depth,Some(&e.id),
                        json!({"index_id":ix.id,"class":e.class,"basis":e.basis,"hub_degree":degree,"hub_penalty":hub_penalty})));
                    if facet!="structure"&&strength>=32&&!visited_paths.contains(path) {next_paths.insert(path.clone());}
                }
                if facet!="structure"&&strength>=32&&!visited_keys.contains(key) {next_keys.insert(key.clone());}
            }
        }
        if next_paths.is_empty()&&next_keys.is_empty() {break;}
        visited_paths.extend(next_paths.clone());visited_keys.extend(next_keys.clone());
        frontier_paths=next_paths;frontier_keys=next_keys;
    }
    // A narrow, explicit config-key join still works for depth-one duties.
    if r.enabled("dependency") {
        let readers:BTreeSet<_>=candidates.values().filter(|c|c.score>=50).map(|c|c.path.clone()).collect();
        let keys:BTreeSet<_>=ix.links.iter().filter(|e|e.predicate=="READS_CONFIG"&&!readers.is_disjoint(&e.from_files)).map(|e|e.to.clone()).collect();
        for e in ix.links.iter().filter(|e|link_allowed(e,seed)&&e.predicate=="DECLARES_CONFIG_KEY"&&keys.contains(&e.to)) {for path in &e.from_files {
            contribute(candidates,path,signal("dependency","config_key_join",65,"retained_relationship",2,Some(&e.id),json!({"key":e.to,"index_id":ix.id})));
        }}
    }
}

pub(super) fn path_allowed(path:&str,seed:&Seed)->bool {
    !source::is_evaluation_artifact(path) || seed.status==AnchorStatus::Resolved && seed.paths.contains(path)
}
pub(super) fn link_allowed(link:&Link,seed:&Seed)->bool {
    link.from_files.iter().chain(&link.to_files).all(|path|path_allowed(path,seed))
}

/// Keep evidence scores/ranks intact. A separate, deterministic reading queue
/// protects anchors and interleaves implementation, tests and supporting roles.
pub(super) fn selection_policy(duty:Duty)->Value {
    json!({"method":if duty.delivery_cycle().is_empty(){"evidence_rank"}else{"duty_roles_v1"},
        "cycle":duty.delivery_cycle(),"anchors":"exact anchors first",
        "rank_semantics":"candidates.rank is evidence rank; selected slice rank is delivery order"})
}

fn delivery_order(ix:&Index,r:&Request,ranked:&[Candidate])->Vec<usize> {
    let mut order=vec![];
    let mut queues=BTreeMap::<&str,std::collections::VecDeque<usize>>::new();
    let cycle=r.duty.delivery_cycle();
    for (index,candidate) in ranked.iter().enumerate().filter(|(_,c)|c.score>=MIN_SCORE) {
        if candidate.signals.iter().any(|s|s.facet=="anchors"&&s.kind=="exact_anchor") {order.push(index);continue;}
        let role=ix.files[&candidate.path].role.as_str();
        let band=if cycle.is_empty(){"evidence"}else if ["impl","test"].contains(&role){role}else{"support"};
        queues.entry(band).or_default().push_back(index);
    }
    if cycle.is_empty() {order.extend(queues.remove("evidence").unwrap_or_default());return order;}
    loop {
        let before=order.len();
        for band in cycle {
            if let Some(index)=queues.get_mut(*band).and_then(|queue|queue.pop_front()) {order.push(index);}
        }
        if order.len()==before {break;}
    }
    order
}

pub(super) fn rank(ix:&Index,r:&Request,candidates:Candidates)->Vec<Candidate> {
    let mut ranked:Vec<_>=candidates.into_values().filter(|c|ix.files.contains_key(&c.path)).collect();
    for candidate in &mut ranked {
        candidate.role=Some(ix.files[&candidate.path].role.clone());
        if r.duty.policy().case.required_roles.contains(&ix.files[&candidate.path].role.as_str()) {candidate.score+=6;}
    }
    ranked.sort_by_key(|c|(std::cmp::Reverse(c.score),c.path.clone()));
    for (i,candidate) in ranked.iter_mut().enumerate() {
        candidate.rank=i+1;
        candidate.source_ref=format!("/candidates/{i}");
        candidate.selected=false;candidate.delivery_rank=None;
    }
    let order=delivery_order(ix,r,&ranked);
    for (position,index) in order.into_iter().take(r.max_files).enumerate() {
        ranked[index].selected=true;ranked[index].delivery_rank=Some(position+1);
    }
    ranked
}

/// Retained declaration spans supplement lexical excerpts for graph-only
/// candidates. A declaration in a holding file does not relocate its target.
pub(super) fn localize(s:&Snapshot,r:&Request,candidates:&mut Candidates) {
    let artifacts:BTreeMap<_,_>=rows(&s.view.bundle,"artifacts").iter().map(|a|(text(a,"id"),text(a,"path"))).collect();
    let references:BTreeMap<_,_>=rows(&s.view.bundle,"evidence_refs").iter().map(|e|(text(e,"id"),e)).collect();
    let entities:BTreeMap<_,_>=rows(&s.view.bundle,"entity_versions").iter().map(|e|(text(e,"id"),e)).collect();
    let terms=terms(&r.task);
    let mut spans=BTreeMap::<String,Vec<(usize,Value)>>::new();
    for a in rows(&s.view.bundle,"assertions").iter().filter(|a|s.view.eligible.contains(&text(a,"id")) && a["class"]!="claim"
        && a["support_class"]!="includes_inference" && a["proposition"]["predicate"]=="DECLARES") {
        let Some(entity)=entities.get(&text(&a["proposition"]["object"],"entity_version_id")) else {continue};
        if !["symbol","function"].contains(&entity["kind"].as_str().unwrap_or("")) {continue;}
        for id in crate::architecture::contracts::strings(&a["evidence_ref_ids"]) {
            let Some(reference)=references.get(&id) else {continue};
            let Some(path)=artifacts.get(&text(reference,"artifact_id")).filter(|p|candidates.contains_key(*p)) else {continue};
            let Some(content)=s.contents.get(path) else {continue};
            let locator=&reference["locator"];
            let (Some(start),Some(end))=(locator["start"].as_u64(),locator["end"].as_u64()) else {continue};
            let (start,end)=(start as usize,end as usize);
            if locator["kind"]!="span"||start>end||end>content.len()||!content.is_char_boundary(start)||!content.is_char_boundary(end) {continue;}
            let symbol=text(entity,"canonical_key");
            let relevance=matches(&symbol,&terms);
            spans.entry(path.clone()).or_default().push((relevance,json!({"symbol":symbol,"evidence_ref_id":id,"assertion_id":a["id"],
                "byte_range":{"start":start,"end":end}})));
        }
    }
    for (path,mut located) in spans {
        located.sort_by_key(|(score,span)|(std::cmp::Reverse(*score),span["byte_range"]["start"].as_u64()));
        let candidate=candidates.get_mut(&path).unwrap();
        for (_,mut span) in located.into_iter().take(3) {
            let content=&s.contents[&path];
            let start=span["byte_range"]["start"].as_u64().unwrap() as usize;
            let end=span["byte_range"]["end"].as_u64().unwrap() as usize;
            span["line_range"]=json!({"start":1+content[..start].bytes().filter(|b|*b==b'\n').count(),
                "end":1+content[..end].bytes().filter(|b|*b==b'\n').count()});
            if !candidate.spans.contains(&span) {candidate.spans.push(span);}
        }
        candidate.spans.truncate(4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ix()->Index {
        let files=["src/a.rs","src/b.rs","src/proof.rs"].into_iter().map(|p|(p.into(),Artifact {path:p.into(),artifact_id:p.into(),
            digest:"sha256:fixture".into(),role:"impl".into(),language:"rust".into(),bytes:0})).collect();
        let mut link=Link::new("CALLS_STATIC","a","b","evidence","fixture",vec![],vec![],
            BTreeSet::from(["src/a.rs".into()]),BTreeSet::from(["src/b.rs".into()])).unwrap();
        link.support_files.insert("src/proof.rs".into());
        Index {id:"ix".into(),source_view:"v".into(),retraction_epoch:0,files,entities:BTreeMap::new(),links:vec![link],coverage:json!({})}
    }

    #[test]
    fn coding_selection_balances_roles_without_rewriting_evidence_ranks_or_scores() {
        let mut ix=ix();let mut candidates=Candidates::new();
        for (role,count,strength) in [("doc",30,200),("impl",20,50),("test",10,70)] {
            for index in 0..count {
                let path=format!("{role}/{index}.txt");
                ix.files.insert(path.clone(),Artifact {path:path.clone(),artifact_id:path.clone(),digest:"digest".into(),
                    role:role.into(),language:"text".into(),bytes:0});
                contribute(&mut candidates,&path,signal("lexical","text",strength,"candidate",0,None,json!({})));
            }
        }
        contribute(&mut candidates,"doc/0.txt",signal("anchors","exact_anchor",10000,"exact",0,None,json!({})));
        let mut previous=Vec::<String>::new();
        for bound in [1,4,12,32] {
            let request=super::super::request::parse(&json!({"task":"repair","duty":"debug","max_files":bound})).unwrap().0;
            let ranked=rank(&ix,&request,candidates.clone());
            assert!(ranked.windows(2).all(|pair|pair[0].score>=pair[1].score));
            let mut selected:Vec<_>=ranked.iter().filter(|candidate|candidate.selected).collect();
            selected.sort_by_key(|candidate|candidate.delivery_rank);
            assert_eq!(selected.len(),bound);assert_eq!(selected[0].path,"doc/0.txt");
            let paths:Vec<_>=selected.iter().map(|candidate|candidate.path.clone()).collect();
            assert!(paths.starts_with(&previous));previous=paths;
            if bound==12 {
                assert!(selected.iter().filter(|candidate|ix.files[&candidate.path].role=="impl").count()>=5);
                assert!(selected.iter().filter(|candidate|ix.files[&candidate.path].role=="test").count()>=3);
                assert!(ranked.iter().any(|candidate|!candidate.selected&&candidate.path.starts_with("doc/")));
                assert!(selected.iter().any(|candidate|candidate.rank>12));
            }
        }
        let request=super::super::request::parse(&json!({"task":"explain","duty":"explain","max_files":12})).unwrap().0;
        let ranked=rank(&ix,&request,candidates);
        assert!(ranked.iter().take(12).all(|candidate|candidate.selected));
    }

    #[test]
    fn evaluation_paths_cannot_enter_through_hints_or_bridge_task_adjacency() {
        let mut ix=ix();let path="EVALUATION_RERUN.md";
        ix.files.insert(path.into(),Artifact {path:path.into(),artifact_id:path.into(),digest:"digest".into(),role:"doc".into(),language:"text".into(),bytes:0});
        ix.links.push(Link::new("REFERENCES_ARTIFACT",path,"src/a.rs","derived","answer key",vec![],vec![],
            BTreeSet::from([path.into()]),BTreeSet::from(["src/a.rs".into()])).unwrap());
        let request=super::super::request::parse(&json!({"task":"repair","target":"src/a.rs"})).unwrap().0;
        let seed=super::super::trace::seed(&ix,"src/a.rs",true);
        let mut candidates=Candidates::new();
        for facet in ["lexical","history","sessions","dependency"] {
            contribute(&mut candidates,path,signal(facet,"hint",100,"candidate",0,None,json!({})));
        }
        contribute(&mut candidates,"src/a.rs",signal("anchors","exact_anchor",10000,"exact",0,None,json!({})));
        expand(&ix,&request,&seed,&mut candidates);assert!(!candidates.contains_key(path));
        let seed=super::super::trace::seed(&ix,path,true);
        contribute(&mut candidates,path,signal("anchors","exact_anchor",10000,"exact",0,None,json!({})));
        assert!(path_allowed(path,&seed));assert!(candidates.contains_key(path));
    }

    #[test]
    fn legacy_candidates_omit_the_new_optional_delivery_field_on_roundtrip() {
        let candidate=Candidate::new("src/a.rs");let original=serde_json::to_value(candidate).unwrap();
        assert!(original.get("delivery_rank").is_none());
        let decoded:Candidate=serde_json::from_value(original.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(),original);
    }
    #[test]
    fn shared_tests_are_direct_associations_not_transitive_implementation_bridges() {
        let mut ix=ix();let test="tests/shared.rs";
        ix.files.insert(test.into(),Artifact {path:test.into(),artifact_id:test.into(),digest:"digest".into(),
            role:"test".into(),language:"rust".into(),bytes:0});
        ix.links=["src/a.rs","src/b.rs"].into_iter().map(|implementation|Link::new("TESTS",test,implementation,
            "derived","shared suite",vec![],vec![],BTreeSet::from([test.into()]),BTreeSet::from([implementation.into()])).unwrap()).collect();
        for anchor in ["src/a.rs",test] {
            let request=super::super::request::parse(&json!({"task":"repair","target":anchor,"duty":"extend","depth":2})).unwrap().0;
            let seed=super::super::trace::seed(&ix,anchor,true);let mut candidates=Candidates::new();
            contribute(&mut candidates,anchor,signal("anchors","exact_anchor",10000,"exact",0,None,json!({})));
            expand(&ix,&request,&seed,&mut candidates);
            assert!(candidates.contains_key(test));assert!(candidates.contains_key("src/a.rs"));
            assert_eq!(candidates.contains_key("src/b.rs"),anchor==test,"{anchor}");
        }
    }
    #[test]
    fn disabled_facets_do_not_penalize_enabled_facets_through_hub_degrees() {
        let mut ix=ix();ix.links.clear();
        ix.links.push(Link::new("IMPORTS","src/a.rs","src/b.rs","derived","dependency",vec![],vec![],
            BTreeSet::from(["src/a.rs".into()]),BTreeSet::from(["src/b.rs".into()])).unwrap());
        for index in 0..20 {
            let path=format!("tests/{index}.rs");
            ix.files.insert(path.clone(),Artifact {path:path.clone(),artifact_id:path.clone(),digest:"digest".into(),
                role:"test".into(),language:"rust".into(),bytes:0});
            ix.links.push(Link::new("TESTS",&path,"src/a.rs","derived","test",vec![],vec![],
                BTreeSet::from([path.clone()]),BTreeSet::from(["src/a.rs".into()])).unwrap());
        }
        let request=super::super::request::parse(&json!({"task":"repair","target":"src/a.rs","duty":"debug",
            "exclude_facets":["tests"]})).unwrap().0;
        let seed=super::super::trace::seed(&ix,"src/a.rs",true);let mut candidates=Candidates::new();
        contribute(&mut candidates,"src/a.rs",signal("anchors","exact_anchor",10000,"exact",0,None,json!({})));
        expand(&ix,&request,&seed,&mut candidates);
        let dependency=candidates["src/b.rs"].signals.iter().find(|signal|signal.facet=="dependency").unwrap();
        assert_eq!(dependency.strength,80);assert_eq!(dependency.provenance["hub_degree"],1);
        assert!(candidates.keys().all(|path|!path.starts_with("tests/")));
    }
    #[test]
    fn provenance_files_are_never_traversal_endpoints_and_facets_can_be_ablated() {
        let ix=ix();
        for disabled in [false,true] {
            let mut args=json!({"task":"repair anchor","target":"src/a.rs","duty":"debug"});
            if disabled {args["exclude_facets"]=json!(["execution"]);}
            let request=super::super::request::parse(&args).unwrap().0;
            let seed=super::super::trace::seed(&ix,"src/a.rs",true);
            let mut candidates=Candidates::new();
            contribute(&mut candidates,"src/a.rs",signal("anchors","exact_anchor",10000,"exact",0,None,json!({})));
            expand(&ix,&request,&seed,&mut candidates);
            assert_eq!(candidates.contains_key("src/b.rs"),!disabled);
            assert!(!candidates.contains_key("src/proof.rs"));
        }
    }
    #[test]
    fn repeated_signals_do_not_outvote_independent_evidence_and_ties_are_stable() {
        let mut candidates=Candidates::new();
        for i in 0..100 {contribute(&mut candidates,"src/a.rs",signal("dependency","IMPORTS",30,"retained_relationship",1,Some(&format!("link:{i}")),json!({})));}
        assert_eq!(candidates["src/a.rs"].score,30);
        contribute(&mut candidates,"src/b.rs",signal("lexical","text",20,"candidate",0,None,json!({})));
        contribute(&mut candidates,"src/b.rs",signal("history","cochange",20,"observational_candidate",0,None,json!({})));
        let request=super::super::request::parse(&json!({"task":"repair anchor","max_files":1})).unwrap().0;
        let ranked=rank(&ix(),&request,candidates);
        assert_eq!(ranked[0].path,"src/b.rs");assert!(ranked[0].selected);assert!(!ranked[1].selected);
        assert!(ranked[0].score>ranked[1].score);
    }
}
