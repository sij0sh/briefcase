use super::*;
pub(crate) struct Ctx<'a> {
    pub(crate) json: bool,
    pub(crate) root: &'a (dyn Fn() -> Result<PathBuf> + 'a),
}
#[allow(clippy::large_enum_variant)]
pub(crate) enum Dispatch {
    Value(Value),
    Done(Result<()>),
    Unhandled(Command),
}
/// Core reads use the path gateway, including validation before first capture.
pub(crate) fn context_request(command:&Command)->Option<(&'static str,Value)> {
    Some(match command {
        Command::Brief {task,target,duty,view,depth,limit,max_bytes,max_files,agent_context}=>(if *agent_context {"agent_context"} else {"brief"},
            json!({"task":task,"target":target,"duty":duty,"view":view,"depth":depth,"limit":limit,"max_bytes":max_bytes,"max_files":max_files})),
        Command::Occurrences {text,path,view,offset,limit,max_bytes}=>("occurrences",
            json!({"text":text,"path":path,"view":view,"offset":offset,"limit":limit,"max_bytes":max_bytes})),
        Command::Neighbors {target,direction,predicate,view,depth,offset,limit,max_bytes}=>("neighbors",
            json!({"target":target,"direction":direction,"predicate":predicate,"view":view,"depth":depth,"offset":offset,"limit":limit,"max_bytes":max_bytes})),
        Command::Search {text,query,max_bytes,no_history,sessions}=>{
            let mut args=query.arguments();args["text"]=json!(text);args["max_bytes"]=json!(max_bytes);
            args["history"]=json!(!*no_history);args["sessions"]=json!(sessions);("search",args)
        },
        _=>return None,
    })
}
pub(crate) fn dispatch(command: Command, ledger: &mut Ledger, ctx: &Ctx) -> Result<Option<Value>> {
    let command = match dispatch_capture::handle(command, ledger, ctx)? {
        Dispatch::Value(v) => return Ok(Some(v)),
        Dispatch::Done(r) => return r.map(|_| None),
        Dispatch::Unhandled(c) => c,
    };
    let command = match dispatch_history::handle(command, ledger, ctx)? {
        Dispatch::Value(v) => return Ok(Some(v)),
        Dispatch::Done(r) => return r.map(|_| None),
        Dispatch::Unhandled(c) => c,
    };
    let command = match dispatch_sessions::handle(command, ledger, ctx)? {
        Dispatch::Value(v) => return Ok(Some(v)),
        Dispatch::Done(r) => return r.map(|_| None),
        Dispatch::Unhandled(c) => c,
    };
    let value: Value = match command {
        Command::InspectExternal {paths,view}=>briefcase::architecture::external::inspect(ledger,&view,&paths)?,
        Command::Policy(q) => q.call(&*ledger, "policy", json!({}))?,
        Command::Unresolved(q) => q.call(&*ledger, "unresolved", json!({}))?,
        Command::Explain {report_id,reference,limit,offset,source_offset} => service::call(ledger,"explain",
            &json!({"report_id":report_id,"ref":reference,"limit":limit,"offset":offset,"source_offset":source_offset}))?,
        Command::Page {report_id,cursor,section,offset,limit} => service::call(ledger,"page",
            &json!({"report_id":report_id,"cursor":cursor,"section":section,"offset":offset,"limit":limit}))?,
        Command::Callgraph(q) => q.call(ledger,"callgraph",json!({}))?,
        Command::Reachability {query:q,from,to} => q.call(ledger,"reachability",json!({"from":from,"to":to}))?,
        Command::ContextHook {..}
        | Command::Brief {..} | Command::Search {..} | Command::Occurrences {..} | Command::Neighbors {..}
        | Command::Init { .. }
        | Command::Monitor { .. }
        | Command::Claims { .. }
        | Command::Status
        | Command::SyncRequest { .. }
        | Command::SyncWorker
        | Command::Watch { .. }
        | Command::Mcp { .. }
        | Command::Sync { .. }
        | Command::Call { .. }
        | Command::Serve { .. }
        | Command::Operations { .. }
        | Command::Install { .. } => unreachable!("handled before opening the ledger"),
        Command::Recover => ledger.recover()?,
        Command::Rebuild { view } => json!({"projections":ledger.rebuild(&view)?}),
        Command::Retract { assertion, reason } => ledger.retract(&assertion, &reason)?,
        _ => unreachable!("all other commands are handled by a dispatch group"),
    };
    Ok(Some(value))
}
