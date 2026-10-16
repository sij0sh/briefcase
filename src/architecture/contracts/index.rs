use super::*;
pub(crate) struct Index<'a>(BTreeMap<&'a str, BTreeMap<&'a str, &'a Value>>);
impl<'a> Index<'a> {
    pub(crate) fn new(b: &'a Value) -> Result<Self> {
        let mut all = BTreeSet::new();
        let mut index = BTreeMap::new();
        for table in TABLES {
            let mut t = BTreeMap::new();
            for row in rows(b, table) {
                let id = row["id"].as_str().ok_or("SCHEMA: Missing ID")?;
                require(
                    all.insert(id),
                    "DUPLICATE_ID",
                    "Record IDs must be globally unique",
                )?;
                t.insert(id, row);
            }
            index.insert(*table, t);
        }
        Ok(Self(index))
    }
    pub(crate) fn get(&self, t: &str, id: &str) -> Result<&'a Value> {
        self.0
            .get(t)
            .and_then(|m| m.get(id))
            .copied()
            .ok_or_else(|| format!("UNKNOWN_REFERENCE: Missing {t} {id}").into())
    }
    pub(crate) fn field(&self, t: &str, row: &Value, k: &str) -> Result<&'a Value> {
        self.get(t, &text(row, k))
    }
    pub(crate) fn context(&self, c: &Value) -> Result<()> {
        match c["kind"].as_str().unwrap_or("") {
            "source" => {
                self.field("snapshots", c, "snapshot_id")?;
                let b = self.field("build_contexts", c, "build_context_id")?;
                require(
                    b["snapshot_id"] == c["snapshot_id"],
                    "SCOPE",
                    "Build belongs to another snapshot",
                )?;
            }
            "runtime" => {
                self.field("runtime_contexts", c, "runtime_context_id")?;
            }
            "deployment" => {
                self.field("deployment_contexts", c, "deployment_context_id")?;
            }
            "session" => {
                self.field("session_contexts", c, "session_context_id")?;
            }
            "comparison" => {
                for sid in strings(&c["snapshot_ids"]) {
                    self.get("snapshots", &sid)?;
                }
            }
            _ => return Err("SCOPE: Unknown context".into()),
        }
        Ok(())
    }
}
/// A span may start at `start`: the prefix is valid UTF-8, or the byte before
/// it is a line terminator (which never sits inside a codepoint), so a
/// line-aligned span stays checkable even after an earlier malformed line.
pub(crate) fn char_boundary(data: &[u8], start: usize) -> bool {
    start == 0 || data.get(start - 1) == Some(&b'\n') || std::str::from_utf8(&data[..start]).is_ok()
}
pub fn contains(outer: &Value, inner: &Value) -> bool {
    outer == inner
        || (outer["kind"] == "comparison"
            && inner["kind"] == "source"
            && outer["snapshot_ids"]
                .as_array()
                .is_some_and(|a| a.contains(&inner["snapshot_id"])))
}
/// Identity of the exact validation logic. Any change to the contract code,
/// predicate registry, or owned schemas yields a new version, so cached
/// verdicts never outlive the validator that produced them.
pub fn validator_version() -> Result<String> {
    let parts: &[&[u8]] = &[
        include_bytes!("../contracts.rs"),
        include_bytes!("schema.rs"),
        include_bytes!("index.rs"),
        include_bytes!("validate.rs"),
        include_bytes!("batch.rs"),
        include_bytes!("../identity.rs"),
        include_bytes!("../../../contracts/v1/predicate-registry.json"),
        include_bytes!("../../../contracts/v1/knowledge.schema.json"),
        include_bytes!("../../../contracts/v1/batch.schema.json"),
        include_bytes!("../../../contracts/v1/query.schema.json"),
        include_bytes!("../../../contracts/v1/session-profile.schema.json"),
        include_bytes!("../../../contracts/v1/session.schema.json"),
        include_bytes!("../../../contracts/v1/architecture-declaration.schema.json"),
    ];
    Ok(identity::bytes_digest(&parts.concat()))
}

/// Inference-bearing support closure with the same cycle and support-class
/// verdicts as full validation. Extracted so cached query paths can reuse
/// the identical taint logic without re-running whole-bundle checks.
pub fn taint(b: &Value) -> Result<BTreeMap<String, bool>> {
    pub(crate) fn tainted(
        id: &str,
        ix: &Index,
        stack: &mut BTreeSet<String>,
        memo: &mut BTreeMap<String, bool>,
    ) -> Result<bool> {
        require(!stack.contains(id), "SUPPORT_CYCLE", "Circular provenance")?;
        if let Some(v) = memo.get(id) {
            return Ok(*v);
        }
        stack.insert(id.into());
        let r = ix.get("assertions", id)?;
        let mut result = r["class"] == "claim";
        for s in strings(&r["supports"]) {
            result |= tainted(&s, ix, stack, memo)?;
        }
        if r["class"] == "claim" {
            let inf = ix.field("inferences", &r["origin"], "inference_id")?;
            for s in strings(&ix.field("input_packets", inf, "packet_id")?["assertion_ids"]) {
                tainted(&s, ix, stack, memo)?;
            }
        }
        if r["class"] == "derived" {
            let d = ix.field("derivations", &r["origin"], "derivation_id")?;
            let mut input_taint = false;
            for s in strings(&d["input_assertion_ids"]) {
                input_taint |= tainted(&s, ix, stack, memo)?;
            }
            result |= input_taint;
            require(
                d["support_class"]
                    == if input_taint {
                        "includes_inference"
                    } else {
                        "evidence_only"
                    }
                    && r["support_class"]
                        == if result {
                            "includes_inference"
                        } else {
                            "evidence_only"
                        },
                "TAINT",
                "Derived class hides inferred inputs",
            )?;
        }
        stack.remove(id);
        memo.insert(id.into(), result);
        Ok(result)
    }
    let ix = Index::new(b)?;
    let mut memo = BTreeMap::new();
    for r in rows(b, "assertions") {
        tainted(&text(r, "id"), &ix, &mut BTreeSet::new(), &mut memo)?;
    }
    Ok(memo)
}
