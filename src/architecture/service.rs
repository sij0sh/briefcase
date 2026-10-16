//! One transport-neutral gateway for CLI, MCP and Pi reads. Catalog/discovery,
//! argument handling and operation execution have separate owners; this module
//! owns validation order, pinned fallback, freshness and response budgets.
use crate::architecture::{ledger::Ledger, response, Result};
use serde_json::{json, Value};

mod arguments;
mod catalog;
mod discovery;
mod dispatch;
mod inventory;

pub(crate) use arguments::{normalize_aliases, validate};
pub use catalog::{catalog, discovery_schema, find, Kind, Operation};
pub use discovery::{describe, discover};
use dispatch::dispatch;

/// Validate and run one read operation. Unknown names, including every
/// canonical write command, are rejected.
pub fn call(l: &Ledger, name: &str, a: &Value) -> Result<Value> {
    let operation = find(name).ok_or(
        "UNKNOWN_OPERATION: Unknown architecture operation; canonical writes are not exposed here",
    )?;
    let normalized = normalize(name, &operation, a);
    let a = &normalized;
    validate(&operation.schema, a)?;
    call_validated(l, &operation, a, None, false)
}

/// Read with an explicitly selected live workspace and optional pending-capture
/// fallback. Only the last published source pin is used; stale/corrupt lexical
/// projections and unrelated failures are never relaxed.
pub fn call_with_context(
    l: &Ledger,
    name: &str,
    a: &Value,
    root: Option<&std::path::Path>,
    fallback_published: bool,
) -> Result<Value> {
    let operation = find(name).ok_or("UNKNOWN_OPERATION: Unknown architecture operation")?;
    let args = normalize(name, &operation, a);
    validate(&operation.schema, &args)?;
    call_validated(l, &operation, &args, root, fallback_published)
}

pub(crate) fn normalize(name:&str,operation:&Operation,a:&Value) -> Value {
    let mut args=normalize_aliases(&operation.schema,a);
    if args.is_null() {args=json!({});}
    if !args.is_object() {return args;}
    let short=name.strip_prefix("architecture_").unwrap_or(name);
    if short=="ask" {
        if args["task"].is_null() {args["task"]=args["question"].clone();}
        args.as_object_mut().unwrap().remove("question");
        if args["target"].is_null() {args["target"]=args["component"].clone();}
        args.as_object_mut().unwrap().remove("component");
    }
    if short=="dependencies"||short=="callgraph" {
        args["target"]=args["component"].as_str().unwrap_or(".").into();
        args["predicate"]=json!(if short=="dependencies" {"IMPORTS,DEPENDS_ON"} else {"CALLS_STATIC"});
        args["direction"]=json!("out");
        for field in ["component","include_structure","allow_partial","historical"] {args.as_object_mut().unwrap().remove(field);}
    }
    if short=="retrieve" {args.as_object_mut().unwrap().remove("graph_depth");}
    args
}
fn fallback_argument(operation:&Operation)->Option<&'static str> {
    operation.schema["properties"].get("view").map(|_|"view")
}

/// First context read creates only a text inventory, never executes repository
/// code or waits for semantic providers. Existing pins are never refreshed here.
pub fn open_context(db:&std::path::Path,root:&std::path::Path) -> Result<Ledger> {
    if db.is_file() {
        let l=Ledger::open_existing(db)?;
        if l.resolve("latest").is_ok()||l.pending_source()? {return Ok(l);}
    }
    let _lock=super::workspace::writer_lock(db)?;
    let mut l=Ledger::open(db)?;
    if l.resolve("latest").is_ok()||l.pending_source()? {return Ok(l);}
    let root=root.canonicalize()?;
    let repository=super::identity::id("repository",&json!({"root":root.to_string_lossy()}))?;
    let capture=super::capture::Capture::read_excluding(&root,None,&super::workspace::database_files(db))?;
    let generation=l.begin(&repository,None)?;
    let outcome=(||->Result<()> {
        let prepared=capture.text_publication(&repository,generation)?;
        let view=l.commit(generation,&prepared)?;l.publish(&view,true)?;
        Ok(())
    })();
    if let Err(error)=outcome {l.fail(generation,&error.to_string())?;return Err(error);}
    Ok(l)
}
/// Path-based gateway used by CLI adapters so a fresh repository is useful.
pub fn call_path(db:&std::path::Path,root:Option<&std::path::Path>,name:&str,args:&Value,fallback:bool)->Result<Value> {
    let operation=find(name).ok_or("UNKNOWN_OPERATION: Unknown architecture operation")?;
    let normalized=normalize(name,&operation,args);
    validate(&operation.schema,&normalized)?;
    if operation.kind==Kind::Describe {return discover(&normalized);}
    if operation.kind==Kind::Brief {super::task::validate_request(&normalized)?;}
    let l=match (operation.kind,root) {
        (Kind::Brief|Kind::Search|Kind::Occurrences|Kind::Neighbors,Some(root))=>open_context(db,root)?,
        _=>Ledger::open_existing(db)?,
    };
    call_validated(&l,&operation,&normalized,root,fallback)
}

fn call_validated(
    l: &Ledger,
    operation: &Operation,
    args: &Value,
    root: Option<&std::path::Path>,
    fallback_published: bool,
) -> Result<Value> {
    let mut fallback = None;
    let result = dispatch(l, operation, args);
    let mut result = match result {
        Ok(result) => result,
        Err(error)
            if fallback_published
                && error
                    .to_string()
                    .contains("A newer scan is pending/interrupted")
                && args["historical"] != true =>
        {
            let Some(argument) = fallback_argument(operation) else {
                return Err(error);
            };
            if args[argument].as_str().is_some_and(|v| v != "latest") {
                return Err(error);
            }
            let pin = l.resolve("latest")?;
            let mut pinned = args.clone();
            if pinned.is_null() {
                pinned = json!({});
            }
            pinned[argument] = json!(pin);
            let result = dispatch(l, operation, &pinned)?;
            fallback = Some(
                json!({"used":true,"reason":"pending_source_capture","argument":argument,"requested":"latest","pinned_view":pin}),
            );
            result
        }
        Err(error) => return Err(error),
    };
    if operation.name == "describe" {
        return Ok(result);
    }
    if fallback.is_none() && fallback_published
        && matches!(operation.kind,Kind::Brief|Kind::Search|Kind::Occurrences|Kind::Neighbors)
        && args["view"].as_str().is_none_or(|view|view=="latest") && l.pending_source()? {
        fallback=Some(json!({"used":true,"reason":"pending_source_capture","argument":"view",
            "requested":"latest","pinned_view":result["view_id"],"mode":"retained_context_read"}));
    }
    super::freshness::attach(l, &mut result, root)?;
    if let Some(fallback) = fallback {
        result["freshness"]["fallback"] = fallback;
    }
    response::bounded(result,args["max_bytes"].as_u64().map(|n|n as usize).unwrap_or(response::DEFAULT_MAX_BYTES))
}

#[cfg(test)]
mod tests;
