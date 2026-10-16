use super::dispatch::{Ctx, Dispatch};
use super::*;
#[allow(unreachable_code)]
pub(crate) fn handle(command: Command, ledger: &mut Ledger, ctx: &Ctx) -> Result<Dispatch> {
    Ok(Dispatch::Value(match command {
        Command::Sessions { action } => match action {
            None => session_query::sessions(&*ledger)?,
            Some(SessionsAction::Import {
                path,
                profile,
                label,
                repository_root,
                repository_id,
                session_retention,
                dry_run: _,
            }) => {
                let root = match repository_root {
                    Some(r) => r,
                    None => (ctx.root)()?,
                };
                let raw = std::fs::read(&path)?;
                let value = session::import(
                    &mut *ledger,
                    &raw,
                    &session_options(
                        &path,
                        &profile,
                        &root,
                        label,
                        repository_id,
                        &session_retention,
                    ),
                )?;
                if ctx.json {
                    return Ok(Dispatch::Done(output(true, &value)));
                }
                let d = &value["diagnostics"];
                println!(
                    "{} {} ({} session, capture {})",
                    if value["status"] == "published" {
                        "Published"
                    } else {
                        "Already imported as"
                    },
                    value["view_id"].as_str().unwrap_or(""),
                    d["harness"].as_str().unwrap_or(""),
                    d["capture_state"].as_str().unwrap_or("")
                );
                for x in strings(&d["limitations"]) {
                    println!("Coverage: {x}");
                }
                println!(
                    "Next: briefcase session --session {0}",
                    value["view_id"].as_str().unwrap_or("")
                );
                return Ok(Dispatch::Done(Ok(())));
            }
            Some(SessionsAction::Profiles) => unreachable!("handled before opening the ledger"),
            Some(SessionsAction::ImportLocal {
                harness,
                sessions_dir,
                repository_root,
                session_retention,
                dry_run,
            }) => {
                let root = match repository_root {
                    Some(r) => r,
                    None => (ctx.root)()?,
                };
                let harness: session::Harness = harness.parse()?;
                let dir = local_sessions_dir(harness, sessions_dir)?;
                let value = session::import_local(
                    &mut *ledger,
                    harness,
                    &dir,
                    &root.to_string_lossy(),
                    session_retention.parse()?,
                    dry_run,
                )?;
                return Ok(Dispatch::Done(output(ctx.json, &value)));
            }
        },
        Command::Session(q) => {
            return Ok(Dispatch::Done(session_report(
                ctx.json,
                session_query::session(&*ledger, &q.into())?,
            )))
        }
        Command::SessionEvents { query: q, kind } => {
            return Ok(Dispatch::Done(session_report(
                ctx.json,
                session_query::events(&*ledger, &q.into(), &kind)?,
            )))
        }
        Command::SessionFiles(q) => {
            return Ok(Dispatch::Done(session_report(
                ctx.json,
                session_query::files(&*ledger, &q.into())?,
            )))
        }
        Command::SessionCommands(q) => {
            return Ok(Dispatch::Done(session_report(
                ctx.json,
                session_query::commands(&*ledger, &q.into())?,
            )))
        }
        Command::SessionChanges {
            query: q,
            view,
            base,
        } => {
            return Ok(Dispatch::Done(session_report(
                ctx.json,
                session_query::changes(&*ledger, &q.into(), &view, base.as_deref())?,
            )))
        }
        Command::SessionActivity {
            component,
            since,
            until,
            view,
            base,
            limit,
            allow_partial,
            historical,
        } => {
            let report = session::activity::activity(
                &*ledger,
                &Options {
                    component,
                    limit: limit as usize,
                    allow_partial,
                    historical,
                    ..Options::default()
                },
                &session::activity::Request {
                    since,
                    until,
                    view,
                    base,
                },
            )?;
            if ctx.json {
                return Ok(Dispatch::Done(output(true, &report)));
            }
            print!("{}", session::activity::render_text(&report));
            return Ok(Dispatch::Done(Ok(())));
        }
        other => return Ok(Dispatch::Unhandled(other)),
    }))
}
