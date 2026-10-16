use super::dispatch::{Ctx, Dispatch};
use super::*;
pub(crate) fn handle(command: Command, ledger: &mut Ledger, ctx: &Ctx) -> Result<Dispatch> {
    Ok(Dispatch::Value(match command {
        Command::Compare { base, query: q } => {
            let o = q.into();
            query::prepare_compare(&mut *ledger, &base, &o)?;
            query::compare(&*ledger, &base, &o)?
        }
        Command::HistoryLineage {
            base,
            query: q,
            mode,
        } => {
            let o = q.into();
            query::prepare_compare(&mut *ledger, &base, &o)?;
            query::history_lineage(&*ledger, &base, &o, &mode)?
        }
        Command::Evolution {
            query: q,
            base,
            since,
            until,
            min_shared,
        } => {
            let o = q.into();
            if let Some(base) = &base {
                query::prepare_compare(&mut *ledger, base, &o)?;
            }
            let report = evolution::evolution(
                &*ledger,
                &o,
                &evolution::Request {
                    base,
                    since,
                    until,
                    min_shared,
                },
            )?;
            if ctx.json {
                return Ok(Dispatch::Done(output(true, &report)));
            }
            print!("{}", evolution::render_text(&report));
            return Ok(Dispatch::Done(Ok(())));
        }
        Command::ClassifyHistoryLineage {
            base,
            query: q,
            provider,
            max_bytes,
        } => lineage::classify(&mut *ledger, &base, &q.into(), &provider, max_bytes)?,
        Command::Passes {
            view,
            plan,
            require_complete,
        } => {
            let plan = plan
                .map(|path| {
                    std::fs::read(path)
                        .map_err(Into::into)
                        .and_then(|raw| identity::parse(&raw))
                })
                .transpose()?;
            passes::inspect(&mut *ledger, &view, plan.as_ref(), require_complete)?
        }
        Command::Classify {
            query: q,
            provider,
            taxonomy,
            max_assertions,
            max_bytes,
        } => inference::classify(
            &mut *ledger,
            &q.into(),
            &provider,
            &taxonomy,
            max_assertions,
            max_bytes,
        )?,
        Command::Domains(q) => q.call(&*ledger, "domains", json!({}))?,
        Command::Drift(q) => q.call(&*ledger, "drift", json!({}))?,
        other => return Ok(Dispatch::Unhandled(other)),
    }))
}
