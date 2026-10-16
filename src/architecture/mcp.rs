//! Read-only MCP tools expose the same named query service as the CLI.
//!
//! Two surfaces share `service::call`: the default gateway advertises
//! `briefcase_search`, `briefcase_query`, `briefcase_describe` and `briefcase_status`,
//! while the compact surface (`mcp --surface compact`, used by Muse)
//! advertises only the Pi pair `context` and `briefcase`. Legacy
//! `architecture_*` names stay callable on both surfaces without being
//! advertised.
use super::{identity, ledger::Ledger, response, service, Result};
use serde_json::{json, Value};
use std::{
    io::{BufRead, Write},
    path::Path,
};
const MAX_FRAME: usize = 1024 * 1024;

/// Which tool names `tools/list` advertises. Calling stays surface-scoped:
/// each surface accepts its own advertised names plus the legacy aliases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    #[default]
    Gateway,
    Compact,
}

impl Surface {
    pub fn name(self) -> &'static str {
        match self {
            Surface::Gateway => "gateway",
            Surface::Compact => "compact",
        }
    }
}

fn query_schema() -> Value {
    json!({"type":"object","properties":{"operation":{"type":"string"},
        "arguments":{"type":"object","additionalProperties":true}},
        "required":["operation"],"additionalProperties":false})
}

fn search_schema() -> Value {
    let mut schema = service::find("search").unwrap().schema;
    let text = schema["properties"]
        .as_object_mut()
        .unwrap()
        .remove("text")
        .unwrap();
    schema["properties"]["query"] = text;
    schema["required"] = json!(["query"]);
    schema
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({"name":name,"description":description,
    "inputSchema":schema,"annotations":{"readOnlyHint":true}})
}

/// The gateway `tools/list` body.
pub fn tools() -> Value {
    tools_for(Surface::Gateway)
}

/// A small gateway even for clients that eagerly load every tool schema.
/// The compact surface mirrors the Pi tools exactly: `context` searches
/// retained text and `briefcase` multiplexes the whole read catalog, so a
/// Muse session loads two schemas instead of the full catalog.
pub fn tools_for(surface: Surface) -> Value {
    match surface {
        Surface::Gateway => json!({"tools":[
            tool("briefcase_search", "Find retained code, documentation, configuration and Git history by keywords. Matches are text, not architecture proof; agent sessions join only when sessions is requested, as labelled hits.", search_schema()),
            tool("briefcase_query", "Run a captured-architecture read operation. Use briefcase_describe to choose an operation and inspect its arguments. Results retain coverage and provenance; unknown does not mean absent.", query_schema()),
            tool("briefcase_describe", "Discover operations by query, list compact summaries or names, or get one operation's full schema and examples. Works without a database.", service::discovery_schema()),
            tool("briefcase_status", "Inspect database publication and run diagnostics. Capture and sync are explicit CLI lifecycle actions.", service::find("status").unwrap().schema)
        ]}),
        Surface::Compact => json!({"tools":[
            tool("context", "Search the code, documentation and configuration text that Briefcase retained from this repository's latest capture, and the Git commits (messages and diffs) reachable from the synced HEAD. Results are file excerpts with paths and line ranges, or commit excerpts with hashes. Earlier agent sessions are searched only when sessions is true, as labelled session hits. For task context, use the briefcase tool with operation brief.", search_schema()),
            tool("briefcase", "Start coding duties with operation brief: anchors, implementation/tests/config/scripts/docs/plans, history, sessions/errors, external touchpoints and not_covered. Use occurrences for exhaustive literal search and neighbors for reverse callers/dependents/tests. page and explain expand retained reports; describe discovers schemas. Answers keep pins, coverage and provenance; unknown does not mean absent.", query_schema())
        ]}),
    }
}

/// Discovery needs no ledger on either surface: the gateway's
/// `briefcase_describe` (or a `describe` query) and the compact `briefcase` tool
/// running operation `describe`.
fn discovery_call_for(surface: Surface, request: &Value) -> bool {
    let name = request["params"]["name"].as_str().unwrap_or("");
    let describes = matches!(
        request["params"]["arguments"]["operation"].as_str(),
        Some("describe" | "architecture_describe")
    );
    match surface {
        Surface::Gateway => {
            matches!(name, "briefcase_describe" | "architecture_describe")
                || (name == "briefcase_query" && describes)
        }
        Surface::Compact => {
            matches!(name, "architecture_describe") || (name == "briefcase" && describes)
        }
    }
}

fn search_call<'a>(get_ledger: &dyn Fn() -> Result<&'a Ledger>, a: &Value, root:Option<&Path>) -> Result<Value> {
    service::validate(&search_schema(), a)?;
    let mut args = a.clone();
    let query = args.as_object_mut().unwrap().remove("query").unwrap();
    args["text"] = query;
    service::call_with_context(get_ledger()?, "search", &args, root, root.is_some())
}

fn query_call<'a>(get_ledger: &dyn Fn() -> Result<&'a Ledger>, a: &Value, root:Option<&Path>) -> Result<Value> {
    service::validate(&query_schema(), a)?;
    let operation = a["operation"].as_str().unwrap();
    if matches!(operation, "describe" | "architecture_describe") {
        return service::discover(&a["arguments"]);
    }
    service::call_with_context(get_ledger()?, operation, &a["arguments"], root, root.is_some())
}

fn call_for(
    surface: Surface,
    ledger: std::result::Result<&Ledger, String>,
    name: &str,
    a: &Value,
    root:Option<&Path>,
) -> Result<Value> {
    let get_ledger = || -> Result<&Ledger> { ledger.clone().map_err(Into::into) };
    match (surface, name) {
        (Surface::Gateway, "briefcase_describe") => service::discover(a),
        (Surface::Gateway, "briefcase_query") => query_call(&get_ledger, a, root),
        (Surface::Gateway, "briefcase_search") => search_call(&get_ledger, a, root),
        (Surface::Gateway, "briefcase_status") => {
            service::validate(&service::find("status").unwrap().schema, a)?;
            service::call_with_context(get_ledger()?, "status", a, root, root.is_some())
        }
        (Surface::Compact, "context") => search_call(&get_ledger, a, root),
        (Surface::Compact, "briefcase") => query_call(&get_ledger, a, root),
        // Older clients can finish existing conversations. These aliases do
        // not appear in tools/list, so they add no eager-client context cost.
        (_, alias) if alias.starts_with("architecture_") && service::find(alias).is_some() => {
            if alias == "architecture_describe" {
                return service::discover(a);
            }
            service::call_with_context(get_ledger()?, alias, a, root, root.is_some())
        }
        (Surface::Gateway, _) => Err(
            "UNKNOWN_TOOL: Unknown Briefcase tool; use briefcase_describe or briefcase_query"
                .into(),
        ),
        (Surface::Compact, _) => {
            Err("UNKNOWN_TOOL: Unknown Briefcase tool; use context or briefcase".into())
        }
    }
}
pub fn dispatch(l: &Ledger, request: &Value) -> Value {
    dispatch_for(Surface::Gateway, l, request)
}

/// Dispatch one request against an opened ledger on `surface`.
pub fn dispatch_for(surface: Surface, l: &Ledger, request: &Value) -> Value {
    respond_for(surface, Ok(l), request, None)
}
/// `ledger` is an error while the selected database is not initialized; the
/// server still answers protocol requests and reports that readiness per call.
fn respond_for(
    surface: Surface,
    ledger: std::result::Result<&Ledger, String>,
    request: &Value,
    root:Option<&Path>,
) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let result: Result<Value> = (|| match request["method"].as_str().unwrap_or("") {
        "initialize" => Ok(
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":env!("CARGO_PKG_NAME"),"version":env!("CARGO_PKG_VERSION")}}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_for(surface)),
        "tools/call" => {
            let value = call_for(
                surface,
                ledger,
                request["params"]["name"].as_str().unwrap_or(""),
                &request["params"]["arguments"],
                root,
            );
            match value {
                Ok(v) => {
                    let wrapped = |v: &Value| -> Result<Value> {
                        Ok(
                            json!({"content":[{"type":"text","text":serde_json::to_string(v)?}],"structuredContent":v,"isError":false}),
                        )
                    };
                    let v = if let Some(max) = v["budget"]["max_response_bytes"].as_u64() {
                        response::bounded_with(v, max as usize, &|value| {
                            response::serialized_bytes(
                                &json!({"jsonrpc":"2.0","id":id,"result":wrapped(value)?}),
                            )
                        })?
                    } else {
                        v
                    };
                    wrapped(&v)
                }
                Err(e) => {
                    Ok(json!({"content":[{"type":"text","text":e.to_string()}],"isError":true}))
                }
            }
        }
        _ => Err("Method not found".into()),
    })();
    match result {
        Ok(v) => json!({"jsonrpc":"2.0","id":id,"result":v}),
        Err(e) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":e.to_string()}}),
    }
}
/// Serve one resolved database path on the gateway surface.
pub fn serve_path(db: &Path, input: impl BufRead, output: impl Write) -> Result<()> {
    serve_path_for(Surface::Gateway, db, input, output)
}

/// Serve one resolved database path. Fresh task reads can retain a text
/// inventory when an invocation root is available; discovery stays database-free.
pub fn serve_path_for(
    surface: Surface,
    db: &Path,
    input: impl BufRead,
    output: impl Write,
) -> Result<()> {
    serve_path_for_root(surface,db,None,input,output)
}
/// Explicit invocation root lets fresh MCP task reads retain the text layer.
pub fn serve_path_for_root(surface:Surface,db:&Path,root:Option<&Path>,input:impl BufRead,output:impl Write)->Result<()> {
    let mut ledger:Option<Ledger>=None;
    frames(input,output,|request| {
        let name=request["params"]["name"].as_str().unwrap_or("");
        let arguments=&request["params"]["arguments"];
        let (operation,args)=if ["context","briefcase_search"].contains(&name) {
            let mut a=arguments.clone();if let Some(object)=a.as_object_mut(){if let Some(query)=object.remove("query"){object.insert("text".into(),query);}}
            ("search",a)
        }else if ["briefcase","briefcase_query"].contains(&name){
            (arguments["operation"].as_str().unwrap_or(""),arguments["arguments"].clone())
        }else{(name,arguments.clone())};
        let accepted=match surface {
            Surface::Gateway=>["briefcase_search","briefcase_query"].contains(&name),
            Surface::Compact=>["context","briefcase"].contains(&name),
        } || (name.starts_with("architecture_")&&service::find(name).is_some());
        let wrapper_valid=if ["context","briefcase_search"].contains(&name) {service::validate(&search_schema(),arguments).is_ok()}
            else if ["briefcase","briefcase_query"].contains(&name) {service::validate(&query_schema(),arguments).is_ok()}else{true};
        let context_read=accepted&&wrapper_valid&&service::find(operation).is_some_and(|op| {
            let normalized=service::normalize(operation,&op,&args);
            service::validate(&op.schema,&normalized).is_ok() && matches!(op.kind,service::Kind::Brief|service::Kind::Search|service::Kind::Occurrences|service::Kind::Neighbors)
        });
        if request["method"]=="tools/call" && context_read && ledger.as_ref().is_none_or(|l|l.resolve("latest").is_err()) {
            if let Some(root)=root {
                match service::open_context(db,root){Ok(l)=>ledger=Some(l),Err(e)=>return respond_for(surface,Err(e.to_string()),request,Some(root))}
            }
        }
        if ledger.is_none()
            && request["method"] == "tools/call"
            && !discovery_call_for(surface, request)
        {
            match Ledger::open_existing(db) {
                Ok(l) => ledger = Some(l),
                Err(e) => return respond_for(surface, Err(e.to_string()), request, root),
            }
        }
        match &ledger {
            Some(l) => respond_for(surface, Ok(l), request, root),
            None => respond_for(surface, Err("NOT_INITIALIZED".into()), request, root),
        }
    })
}
pub fn serve(l: &Ledger, input: impl BufRead, output: impl Write) -> Result<()> {
    serve_for(Surface::Gateway, l, input, output)
}

/// Serve an opened ledger on `surface` until the input closes.
pub fn serve_for(
    surface: Surface,
    l: &Ledger,
    input: impl BufRead,
    output: impl Write,
) -> Result<()> {
    frames(input, output, |request| dispatch_for(surface, l, request))
}
fn frames(
    mut input: impl BufRead,
    mut output: impl Write,
    mut handle: impl FnMut(&Value) -> Value,
) -> Result<()> {
    loop {
        let mut bytes = Vec::new();
        let count = std::io::Read::take(&mut input, (MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut bytes)?;
        if count == 0 {
            return Ok(());
        }
        if count > MAX_FRAME {
            return Err("MCP frame limit exceeded".into());
        }
        let request = match identity::parse(&bytes) {
            Ok(v) => v,
            Err(e) => {
                writeln!(
                    output,
                    "{}",
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}})
                )?;
                output.flush()?;
                continue;
            }
        };
        if request.get("id").is_none() {
            continue;
        }
        writeln!(output, "{}", handle(&request))?;
        output.flush()?;
    }
}
