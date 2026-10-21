//! One typed operation catalog shared by CLI, MCP, discovery and dispatch.
use crate::architecture::task::Duty;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Describe,
    Brief,
    AgentContext,
    Impact,
    Inventory,
    ExpandCase,
    Search,
    Occurrences,
    Neighbors,
    Reachability,
    Page,
    Explain,
    Compare,
    HistoryLineage,
    Evolution,
    Policy,
    Unresolved,
    Domains,
    Drift,
    Sessions,
    Session,
    SessionEvents,
    SessionFiles,
    SessionCommands,
    SessionChanges,
    SessionActivity,
    Worktree,
    Status,
}
pub struct Operation {
    pub kind: Kind,
    pub name: &'static str,
    pub tool: String,
    pub description: &'static str,
    pub schema: Value,
}
pub fn discovery_schema() -> Value {
    schema(
        &["operation", "query"],
        &[],
        json!({"limit":integer(1,100),"names":{"type":"boolean"}}),
    )
}
fn integer(min: u64, max: u64) -> Value {
    json!({"type":"integer","minimum":min,"maximum":max})
}
fn schema(strings: &[&str], required: &[&str], extra: Value) -> Value {
    let mut properties = extra.as_object().unwrap().clone();
    for key in strings {
        properties.insert((*key).into(), json!({"type":"string"}));
    }
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn common(extra: &[&str], required: &[&str]) -> Value {
    let mut strings = vec!["view", "component"];
    strings.extend_from_slice(extra);
    schema(
        &strings,
        required,
        json!({"limit":integer(1,1000),"allow_partial":{"type":"boolean"},
        "historical":{"type":"boolean"},"include_structure":{"type":"boolean"}}),
    )
}
fn session(extra: &[&str]) -> Value {
    let mut strings = vec!["session"];
    strings.extend_from_slice(extra);
    schema(
        &strings,
        &[],
        json!({"limit":integer(1,1000),"allow_partial":{"type":"boolean"},"historical":{"type":"boolean"}}),
    )
}
pub fn catalog() -> Vec<Operation> {
    use Kind::*;
    let entries = [
        (Describe,"describe","Discover supported read operations and their arguments without a database."),
        (Brief,"brief","Trace retained evidence to build a case around a task, delivered as a brief: exact anchors, role-based files, relationships, history, sessions and typed coverage gaps. Feature and endpoint identities are optional targets."),
        (AgentContext,"agent_context","Compact coding-harness routing manifest: separate target anchor, ranked supplemental files, primary reasons, shared pinned source fetch and complete evidence continuations. Fills the final byte budget; shares the brief's case."),
        (Impact,"impact","Assess a proposed change or the full delta between two captured source snapshots. Retains affected components, dependency witnesses, declared constraints, suggested verification and typed unknowns. Does not execute tests, approve decisions or enforce policy."),
        (Inventory,"inventory","Read compact filtered candidates, lexical matches, or complete remaining work for an immutable case. Filters apply before paging; reading does not expand assembly."),
        (ExpandCase,"expand_case","Explicitly promote retained candidates or lexical-tail paths and assemble a new immutable case revision. Source inspection and full-file verification are independent."),
        (Search,"search","Rank retained text, evidence, Git history and optional session requests. A match is a candidate, never proof of ownership; use occurrences for exhaustive matching."),
        (Occurrences,"occurrences","Enumerate every retained literal match in path and byte order, including overlaps. Continue with the returned pinned view."),
        (Neighbors,"neighbors","Incoming/outgoing adjacency for any retained entity or file. Filter predicates for dependencies, callers, tests, data, deployment or runtime relationships."),
        (Reachability,"reachability","Transitive static call paths between two identities; dynamic behavior and observed calls remain separate."),
        (Page,"page","Read one retained report section using its report_id and cursor, without recomputing it."),
        (Explain,"explain","Expand a retained report reference, link, or explanation handle to exact source and provenance."),
        (Compare,"compare","Compare pinned source views using retained correspondence."),
        (HistoryLineage,"history_lineage","Inspect exact, likely or ambiguous historical correspondence with certainty labels."),
        (Evolution,"evolution","Inspect retained Git changes and co-change witnesses; co-change is not dependency."),
        (Policy,"policy","Inspect declared dependency rules, supported witnesses and unknown coverage."),
        (Unresolved,"unresolved","Inspect unsupported and unresolved producer occurrences."),
        (Domains,"domains","Inspect labelled domain Claims and Declarations without changing their authority."),
        (Drift,"drift","Inspect retained Claim-versus-intent disagreements and both origins."),
        (Sessions,"sessions","List imported session publications, newest first."),
        (Session,"session","Inspect one recorded agent session and its recording coverage."),
        (SessionEvents,"session_events","Inspect normalized session events in transcript order."),
        (SessionFiles,"session_files","Inspect recorded reads, edit attempts and confirmed edits."),
        (SessionCommands,"session_commands","Inspect recorded commands, results and exit codes."),
        (SessionChanges,"session_changes","Compare recorded edits with pinned source changes without causal attribution."),
        (SessionActivity,"session_activity","Inspect cross-session path activity, retained commits and net source changes."),
        (Status,"status","Inspect publication, coverage and readiness diagnostics."),
        (Worktree,"worktree","Compare all currently capturable source paths against a pinned view or case, including added/deleted paths outside its slice. Retains a paged digest inventory; never publishes source or executes repository code."),
    ];
    entries.into_iter().map(|(kind,name,description)| {
        let mut s=match kind {
            Describe=>discovery_schema(),
            Brief|AgentContext=>schema(&["task","target","view","duty","deployment_view","runtime_environment","runtime_from","runtime_to"],&["task"],
                json!({"depth":integer(1,3),"limit":integer(1,1000),"max_files":integer(1,1000),
                    "selection_variant":{"type":"string","enum":crate::architecture::task::selection_variants()},
                    "exclude_facets":{"type":"array","items":{"type":"string","enum":crate::architecture::task::retrieval_facets()},"uniqueItems":true}})),
            Search=>common(&["text","classes"],&["text"]),
            Impact=>schema(&["task","case_id","target","view","base"],&["mode"],json!({
                "mode":{"type":"string","enum":["pre","post"]},
                "targets":{"type":"array","items":{"type":"string","minLength":1,"maxLength":4096},"uniqueItems":true,"maxItems":1000},
                "depth":integer(1,3),"max_nodes":integer(1,100000),"limit":integer(1,1000)})),
            Occurrences=>schema(&["text","path","view"],&["text"],json!({"limit":integer(1,1000),"offset":integer(0,u64::MAX)})),
            Neighbors=>schema(&["target","direction","predicate","view","index_id","deployment_view","runtime_environment","runtime_from","runtime_to"],&["target"],
                json!({"relationship_scope":{"type":"string","enum":["hot","all","overflow"]},"depth":integer(1,3),"limit":integer(1,1000),"offset":integer(0,u64::MAX)})),
            Inventory=>schema(&["case_id","path","role","work_kind"],&["case_id"],json!({
                "kind":{"type":"string","enum":["candidates","lexical","files","work"]},
                "selected":{"type":"boolean"},"graph_only":{"type":"boolean"},"tail_only":{"type":"boolean"},
                "min_score":integer(0,u64::MAX),"max_score":integer(0,u64::MAX),
                "limit":integer(1,1000),"offset":integer(0,u64::MAX)})),
            ExpandCase=>schema(&["case_id","lexical_report_id"],&["case_id"],json!({
                "paths":{"type":"array","items":{"type":"string"},"maxItems":1000,"uniqueItems":true},
                "lexical_offset":integer(0,u64::MAX),"lexical_limit":integer(1,1000),"limit":integer(1,1000)})),
            Reachability=>common(&["from","to"],&["from","to"]),
            Page=>schema(&["report_id","cursor","section","token"],&[],json!({"offset":integer(0,u64::MAX),"limit":integer(1,1000)})),
            Explain=>schema(&["report_id","ref","handle","index_id","link_id"],&[],json!({"offset":integer(0,u64::MAX),"source_offset":integer(0,u64::MAX),"source_bytes":integer(4,65536),"limit":integer(1,1000)})),
            Compare=>common(&["base"],&["base"]),
            HistoryLineage=>common(&["base","mode"],&["base"]),
            Evolution=>common(&["base","since","until","format"],&[]),
            Policy|Unresolved|Domains|Drift=>common(&[],&[]),
            Sessions=>schema(&[],&[],json!({"limit":integer(1,1000),"offset":integer(0,u64::MAX)})),
            Session|SessionFiles|SessionCommands=>session(&[]),
            SessionEvents=>session(&["kind"]),
            SessionChanges=>session(&["view","base"]),
            SessionActivity=>common(&["base","since","until","format"],&[]),
            Status=>schema(&[],&[],json!({"summary":{"type":"boolean"}})),
            Worktree=>schema(&["view","case_id"],&[],json!({"limit":integer(1,1000)})),
        };
        if matches!(kind,Brief|AgentContext) {s["properties"]["duty"]["enum"]=json!(Duty::names());}
        if kind==AgentContext {s["properties"]["limit"]["description"]=json!("Deprecated compatibility argument; ignored. max_files bounds the retained case; max_bytes bounds delivery.");}
        if kind==Neighbors {s["properties"]["direction"]["enum"]=json!(["in","out","both"]);}
        if kind==Search {
            s["properties"]["include_evaluation"]=json!({"type":"boolean","description":"Explicitly search retained evaluation inputs/reports; excluded from normal retrieval."});
            s["properties"]["history"]=json!({"type":"boolean"});
            s["properties"]["sessions"]=json!({"type":"boolean"});
            // Legacy query flags must not silently change search authority.
            for field in ["historical","include_structure"] {
                s["properties"][field]["enum"]=json!([false]);
            }
            s["properties"]["component"]["description"]=json!("Retained file or directory path prefix; filtering follows bounded lexical candidate selection.");
        }
        let text_field=match kind {Brief|AgentContext|Impact=>Some(("task",16384)),Search|Occurrences=>Some(("text",4096)),_=>None};
        if let Some((field,max))=text_field {
            s["properties"][field]["minLength"]=json!(1);
            s["properties"][field]["maxLength"]=json!(max);
            if kind!=Occurrences {s["properties"][field]["pattern"]=json!("\\S");}
            s["properties"][field]["max_utf8_bytes"]=json!(max);
        }
        if kind==Evolution {s["properties"]["min_shared"]=integer(1,100000);}
        if kind!=Describe {s["properties"]["max_bytes"]=integer(256,1048576);}
        Operation{kind,name,tool:format!("architecture_{name}"),description,schema:s}
    }).collect()
}
/// Compatibility names are unadvertised and resolve to the same implementation.
/// Removed products intentionally have no compatibility entry.
pub fn find(name: &str) -> Option<Operation> {
    let short = name.strip_prefix("architecture_").unwrap_or(name);
    let canonical = match short {
        "ask" => "brief",
        "recall" | "retrieve" => "search",
        "task_page" | "continue" => "page",
        "task_explain" => "explain",
        "dependencies" | "callgraph" => "neighbors",
        _ => short,
    };
    catalog().into_iter().find(|op| op.name == canonical)
}
