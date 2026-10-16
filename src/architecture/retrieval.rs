//! Unadvertised Rust compatibility API using the canonical search operation.
use super::{ledger::Ledger,query::Options,require,service,Result};
use serde_json::{json,Value};
pub fn retrieve(l:&Ledger,text:&str,o:&Options,max_bytes:usize,depth:usize)->Result<Value>{
    // Preserve strict projection validation for existing direct Rust consumers.
    let view=super::query::loaded(l,o)?;
    require(!view.stale,"FRESHNESS","Retained projections require rebuild")?;
    require(depth<=3,"ARGUMENTS","graph depth must be 0..3")?;
    require((super::response::MIN_MAX_BYTES..=super::response::MAX_MAX_BYTES).contains(&max_bytes),
        "ARGUMENTS","Response budget must be 256..1048576 bytes")?;
    service::call(l,"search",&json!({"text":text,"view":o.view,"component":o.component,
        "limit":o.limit,"max_bytes":max_bytes,"history":true}))
}
