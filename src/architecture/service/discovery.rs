//! Database-free operation lookup, examples and ranked vocabulary search.
use super::{
    arguments::validate,
    catalog::{catalog, discovery_schema, find, Operation},
};
use crate::architecture::{require, Result};
use serde_json::{json, Value};

/// Operation names, descriptions and argument schemas for adapters.
pub fn describe() -> Value {
    json!(catalog()
        .into_iter()
        .map(|o| description(&o))
        .collect::<Vec<_>>())
}

pub(super) fn examples(name:&str)->Vec<Value>{
    let args=match name {
        "brief"|"agent_context"=>json!({"task":"rename the session importer safely","target":"src/session.rs","duty":"refactor"}),
        "search"=>json!({"text":"payment authorization retry"}),
        "occurrences"=>json!({"text":"old-binary-name","limit":100}),
        "neighbors"=>json!({"target":"src/session.rs","direction":"in","predicate":"TESTS"}),
        "page"=>json!({"report_id":"<retained report id>","section":"/slice/test"}),
        "inventory"=>json!({"case_id":"<retained task-case id>","kind":"candidates","selected":false}),
        "expand_case"=>json!({"case_id":"<retained task-case id>","paths":["src/consumer.rs"]}),
        "explain"=>json!({"report_id":"<retained report id>","ref":"<task-link id>"}),
        "worktree"=>json!({"case_id":"<retained task-case id>","limit":100}),
        "impact"=>json!({"mode":"pre","task":"change the authentication token format","view":"<captured source view id>","targets":["src/auth.rs"]}),
        "reachability"=>json!({"from":"orders::create","to":"payments::authorize"}),
        "compare"|"history_lineage"=>json!({"base":"before","view":"latest"}),
        "describe"=>json!({"query":"what calls this function","limit":5}),
        _=>json!({}),
    };vec![json!({"operation":name,"arguments":args})]
}

fn description(o: &Operation) -> Value {
    json!({"name":o.name,"description":o.description,"arguments":o.schema,"examples":examples(o.name)})
}

/// Routing vocabulary supplements literal names/descriptions, without a
/// model, repository state or a second argument-schema catalog.
fn search_terms(name:&str)->&'static str{
    match name {
        "brief"=>"coding duty debug refactor rename impact extend resume plan review patch complete tests context packet uncovered feature endpoint workflow",
        "agent_context"=>"coding harness automatic prompt manifest budget ranked files selection spans fetch",
        "occurrences"=>"exhaustive literal occurrence rename every match all files completion search",
        "neighbors"=>"what calls this function caller callee reverse dependents tests configuration references blast radius imports uses dependency data lineage deployment runtime",
        "reachability"=>"can reach path between functions transitive static execution",
        "search"=>"find text keywords code docs documentation git history evidence provenance",
        "page"=>"pagination omitted sections continuation",
        "inventory"=>"compact candidates remaining work frontier lexical tail filter package role unselected low score",
        "expand_case"=>"promote lexical tail expand assemble immutable case revision additional evidence scope",
        "explain"=>"proof provenance support evidence source references byte witness retraction",
        "session"|"session_events"|"session_activity"=>"previous agent activity session forensic history",
        "evolution"|"compare"|"history_lineage"=>"history changes revisions snapshots rename correspondence",
        "status"=>"ready readiness initialized diagnostics database",
        "worktree"=>"current dirty untracked added deleted modified freshness working tree diff outside initial slice",
        "impact"=>"architectural change impact planned targets base candidate snapshots consumers declared constraints suggested checks blast radius pre post",
        _=>"",
    }
}

fn words(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Compact, deterministic lexical discovery. An exact operation request
/// returns the full executable schema; searches never include full schemas.
pub fn discover(a: &Value) -> Result<Value> {
    validate(&discovery_schema(), a)?;
    let query = a["query"].as_str();
    let names = a["names"].as_bool().unwrap_or(false);
    if let Some(name) = a["operation"].as_str() {
        require(
            query.is_none() && !names && a["limit"].is_null(),
            "ARGUMENTS",
            "operation cannot be combined with query, names or limit",
        )?;
        let o = find(name).ok_or("UNKNOWN_OPERATION: Unknown architecture operation")?;
        return Ok(description(&o));
    }
    let terms = words(query.unwrap_or(""));
    let terms: Vec<_> = terms
        .iter()
        .filter(|s| {
            !matches!(
                s.as_str(),
                "a" | "an"
                    | "the"
                    | "this"
                    | "that"
                    | "to"
                    | "of"
                    | "in"
                    | "is"
                    | "it"
                    | "what"
                    | "show"
                    | "me"
                    | "with"
            )
        })
        .collect();
    let mut matches: Vec<_> = catalog()
        .into_iter()
        .filter_map(|o| {
            let name = words(o.name);
            let routing = words(search_terms(o.name));
            let text = words(o.description);
            let matched = terms
                .iter()
                .filter(|t| name.contains(**t) || routing.contains(**t) || text.contains(**t))
                .count();
            let weighted: usize = terms
                .iter()
                .map(|t| {
                    if name.contains(*t) {
                        8
                    } else if routing.contains(*t) {
                        4
                    } else if text.contains(*t) {
                        1
                    } else {
                        0
                    }
                })
                .sum();
            (query.is_none() || matched > 0).then_some(((matched, weighted), o))
        })
        .collect();
    matches.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then(a.name.cmp(b.name)));
    matches.truncate(
        a["limit"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(if query.is_some() { 5 } else { 100 }),
    );
    if names {
        return Ok(json!(matches
            .iter()
            .map(|(_, o)| o.name)
            .collect::<Vec<_>>()));
    }
    Ok(json!(matches.iter().map(|(_, o)| {
        let first = o.description.split(". ").next().unwrap_or(o.description);
        let summary = if first.chars().count() > 200 { format!("{}…", first.chars().take(199).collect::<String>()) } else { first.to_owned() };
        json!({"name":o.name,"description":summary,
            "arguments":o.schema["properties"].as_object().map(|p| p.keys().collect::<Vec<_>>()).unwrap_or_default(),
            "required":o.schema.get("required").cloned().unwrap_or(json!([]))})
    }).collect::<Vec<_>>()))
}
