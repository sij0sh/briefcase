use super::*;
use std::time::Instant;

pub(crate) fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Sync deadline expired").into(),
        );
    }
    Ok(())
}

impl Ledger {
    pub fn recover(&mut self) -> Result<Value> {
        // An interrupted preparation has no canonical records. Committed inputs can always be retried.
        self.db.execute(
            "UPDATE scans SET status='interrupted',diagnostics=? WHERE status='preparing'",
            [json!({"error":"process ended before commit"}).to_string()],
        )?;
        let intended: Option<u64> = self
            .db
            .query_row("SELECT intended FROM namespace", [], |r| r.get(0))
            .optional()?;
        let mut s = self
            .db
            .prepare("SELECT id,generation FROM views ORDER BY generation")?;
        let pending = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(s);
        let mut recovered = Vec::new();
        for (id, generation) in pending {
            if self.load(&id)?.stale {
                self.publish(&id, Some(generation) == intended)?;
                recovered.push(id);
            }
        }
        Ok(json!({"recovered":recovered,"status":self.status()?}))
    }

    /// Resume interrupted publications, without auditing every historical
    /// projection. The selected publication is verified by the sync caller;
    /// reads still verify their own projection, and explicit recover audits all.
    pub(crate) fn recover_pending(&mut self, deadline: Instant) -> Result<Vec<String>> {
        check_deadline(deadline)?;
        let mut statement = self.db.prepare(
            "SELECT v.id,v.generation FROM views v JOIN scans s ON s.generation=v.generation
             WHERE v.projection_generation=0 OR s.status!='published' ORDER BY v.generation",
        )?;
        let pending = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        check_deadline(deadline)?;
        self.db.execute(
            "UPDATE scans SET status='interrupted',diagnostics=? WHERE status='preparing'",
            [json!({"error":"process ended before commit"}).to_string()],
        )?;
        let intended: Option<u64> = self
            .db
            .query_row("SELECT intended FROM namespace", [], |r| r.get(0))
            .optional()?;
        let mut recovered = Vec::new();
        for (id, generation) in pending {
            check_deadline(deadline)?;
            if self.load(&id)?.stale {
                check_deadline(deadline)?;
                self.publish(&id, Some(generation) == intended)?;
                recovered.push(id);
            }
            check_deadline(deadline)?;
        }
        Ok(recovered)
    }
}
