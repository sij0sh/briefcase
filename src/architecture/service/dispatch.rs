//! Exhaustive typed dispatch: adding a catalog kind requires implementing it here.
use super::{catalog::{Kind,Operation}, discovery::discover, inventory};
use crate::architecture::{evolution, ledger::Ledger, query::{self,Options}, search, reports, session, task, Result};
use serde_json::{json,Value};
fn options(a:&Value) -> Options {
    Options {view:a["view"].as_str().unwrap_or("latest").into(),component:a["component"].as_str().map(str::to_owned),
        limit:a["limit"].as_u64().unwrap_or(100) as usize, allow_partial:a["allow_partial"].as_bool().unwrap_or(false),
        historical:a["historical"].as_bool().unwrap_or(false),include_structure:a["include_structure"].as_bool().unwrap_or(false)}
}
fn session_options(a:&Value) -> Options {Options{view:a["session"].as_str().unwrap_or("latest").into(),..options(a)}}
pub(super) fn dispatch(l:&Ledger,op:&Operation,a:&Value) -> Result<Value> {
    use Kind::*;
    let o=options(a);
    let s=|key:&str|a[key].as_str();
    let n=|key:&str,default:u64|a[key].as_u64().unwrap_or(default) as usize;
    let required=|key:&str|->Result<&str>{s(key).ok_or_else(||format!("ARGUMENTS: {key} is required").into())};
    match op.kind {
        Describe=>discover(a),
        Brief=>task::brief(l,a),
        AgentContext=>task::agent_context(l,a),
        Search=>search::search(l,a),
        Occurrences=>task::occurrences(l,a),
        Neighbors=>task::neighbors(l,a),
        Reachability=>query::reachability(l,&o,required("from")?,required("to")?),
        Page=>reports::page(l,a),
        Explain=>reports::explain(l,a),
        Compare=>query::compare(l,required("base")?,&o),
        HistoryLineage=>query::history_lineage(l,required("base")?,&o,s("mode").unwrap_or("combined")),
        Evolution=>{
            let report=evolution::evolution(l,&o,&evolution::Request{base:s("base").map(str::to_owned),since:s("since").map(str::to_owned),
                until:s("until").map(str::to_owned),min_shared:n("min_shared",evolution::DEFAULT_MIN_SHARED as u64)})?;
            match s("format") {None|Some("json")=>Ok(report),Some("llm")=>Ok(json!({"report_id":report["id"],"pinned_views":report["pinned_views"],"format":"llm","text":evolution::render_text(&report)})),
                _=>Err("ARGUMENTS: format must be json or llm".into())}
        },
        Policy=>query::policy(l,&o), Unresolved=>query::unresolved(l,&o), Domains=>query::domains(l,&o), Drift=>query::drift(l,&o),
        Sessions=>session::query::sessions_page(l,n("limit",20),n("offset",0)),
        Session=>session::query::session(l,&session_options(a)),
        SessionEvents=>session::query::events(l,&session_options(a),&s("kind").map(|k|vec![k.to_owned()]).unwrap_or_default()),
        SessionFiles=>session::query::files(l,&session_options(a)),
        SessionCommands=>session::query::commands(l,&session_options(a)),
        SessionChanges=>session::query::changes(l,&session_options(a),s("view").unwrap_or("latest"),s("base")),
        SessionActivity=>session::activity::activity(l,&o,&session::activity::Request{since:s("since").map(str::to_owned),until:s("until").map(str::to_owned),view:s("view").map(str::to_owned),base:s("base").map(str::to_owned)}),
        Status=>if a["summary"]==true {inventory::summary(l)} else {l.status()},
    }
}
