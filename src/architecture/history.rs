//! Supported correspondence between immutable module versions. Equal retained
//! whole-module bytes plus unique reciprocal candidates establish exact content
//! movement, not eternal conceptual identity. Copies remain ambiguous.
use super::{
    contracts::{self, add, rows, text},
    identity, knowledge,
    ledger::{Ledger, Prepared, View},
    require, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Parser;

pub struct Correspondence {
    pub bundle: Value,
    pub watermark: u64,
    pub exact: Vec<Value>,
    pub ambiguous: Vec<Value>,
}
fn key(before: &View, after: &View) -> Result<String> {
    identity::id(
        "comparison-overlay",
        &json!({"before":knowledge::context(&before.bundle),"after":knowledge::context(&after.bundle),
        "method":"unique-retained-module-content-v1","implementation_digest":identity::bytes_digest(include_bytes!("history.rs"))}),
    )
}
fn describe(b: &Value) -> (Vec<Value>, Vec<Value>) {
    let exact = rows(b, "assertions")
        .iter()
        .filter(|a| {
            a["context"]["kind"] == "comparison" && a["proposition"]["predicate"] == "RENAMED_FROM"
        })
        .cloned()
        .collect();
    let ambiguous=rows(b,"derivations").iter().filter(|d|d["pass_name"] == "ambiguous-retained-module-content-v1" || d["pass_name"] == "uncertain-retained-module-declarations-v1")
        .filter_map(|d|d.get("input_artifact_ids").map(|ids|json!({"derivation_id":d["id"],"input_artifact_ids":ids,"input_assertion_ids":d["input_assertion_ids"]}))).collect();
    (exact, ambiguous)
}
pub(crate) fn load(l: &Ledger, before: &View, after: &View) -> Result<Correspondence> {
    let overlay_key = key(before, after)?;
    let bundle = l
        .comparison_overlay(&overlay_key)?
        .unwrap_or_else(contracts::empty_bundle);
    let (exact, ambiguous) = describe(&bundle);
    Ok(Correspondence {
        bundle,
        watermark: l.overlay_watermark(&overlay_key)?.unwrap_or(0),
        exact,
        ambiguous,
    })
}
/// Whether `prepare` has retained correspondence for this pair of views.
pub(crate) fn prepared(l: &Ledger, before: &View, after: &View) -> Result<bool> {
    Ok(l.comparison_overlay(&key(before, after)?)?.is_some())
}
pub fn prepare(l: &mut Ledger, before: &View, after: &View) -> Result<()> {
    let key = key(before, after)?;
    if l.comparison_overlay(&key)?.is_some() {
        return Ok(());
    }
    let ctx = json!({"kind":"comparison","snapshot_ids":[knowledge::context(&before.bundle)["snapshot_id"],knowledge::context(&after.bundle)["snapshot_id"]]});
    require(
        ctx["snapshot_ids"][0] != ctx["snapshot_ids"][1],
        "COMPARE",
        "Comparison needs distinct snapshots",
    )?;
    let mut b = contracts::empty_bundle();
    knowledge::merge(&mut b, &before.bundle)?;
    knowledge::merge(&mut b, &after.bundle)?;
    let mut payloads = l.payloads(&before.bundle)?;
    payloads.extend(l.payloads(&after.bundle)?);
    let watermark = l.watermark()?;
    let removed = l.retracted()?;
    let mut old = BTreeMap::new();
    let mut new = BTreeMap::new();
    for (view, map) in [(before, &mut old), (after, &mut new)] {
        for e in rows(&view.bundle, "entity_versions")
            .iter()
            .filter(|e| e["kind"] == "module")
        {
            if let Some(s) = knowledge::module_source(&view.bundle, &payloads, &text(e, "id"))? {
                if !removed.contains(&s.assertion_id) {
                    map.insert(text(e, "id"), (e.clone(), s));
                }
            }
        }
    }
    let old_keys: BTreeSet<_> = old
        .values()
        .map(|(e, _)| text(e, "canonical_key"))
        .collect();
    let new_keys: BTreeSet<_> = new
        .values()
        .map(|(e, _)| text(e, "canonical_key"))
        .collect();
    let gone: Vec<_> = old
        .iter()
        .filter(|(_, (e, _))| !new_keys.contains(&text(e, "canonical_key")))
        .collect();
    let added: Vec<_> = new
        .iter()
        .filter(|(_, (e, _))| !old_keys.contains(&text(e, "canonical_key")))
        .collect();
    let implementation = identity::bytes_digest(include_bytes!("history.rs"));
    let mut selected = BTreeSet::new();
    let mut matched = BTreeSet::new();
    for (oid, (oe, os)) in &gone {
        let candidates: Vec<_> = added
            .iter()
            .filter(|(_, (ne, ns))| ne["language"] == oe["language"] && ns.text == os.text)
            .collect();
        if candidates.is_empty() {
            continue;
        }
        matched.insert((*oid).clone());
        matched.extend(candidates.iter().map(|(id, _)| (*id).clone()));
        let supports = std::iter::once(os.assertion_id.clone())
            .chain(candidates.iter().map(|(_, (_, s))| s.assertion_id.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let unique = candidates.len() == 1 && gone.iter().filter(|(_,(_,s))|s.text == os.text).count()==1
            // Do not call a duplicate-copy move exact even if only one locator disappears.
            && old.values().filter(|(_,s)|s.text == os.text).count()==1 && new.values().filter(|(_,s)|s.text == os.text).count()==1;
        let method = if unique {
            "unique-retained-module-content-v1"
        } else {
            "ambiguous-retained-module-content-v1"
        };
        let mapping = json!({"schema_version":"1.0","method":method,"context":ctx,"from_entity_version_id":oid,
            "from_key":oe["canonical_key"],"candidate_entity_version_ids":candidates.iter().map(|(id,_)|*id).collect::<Vec<_>>(),
            "source_evidence_ref_ids":std::iter::once(os.evidence_id.clone()).chain(candidates.iter().map(|(_,(_,s))|s.evidence_id.clone())).collect::<Vec<_>>(),
            "status":if unique {"exact_content_move"} else {"ambiguous"},"exact_match":unique,
            "limitation":"Retained whole-module byte equality and reciprocal uniqueness, not conceptual identity or Git similarity."});
        let artifact = knowledge::artifact(
            &mut b,
            &mut payloads,
            "correspondence",
            identity::canonical(&mapping)?,
        )?;
        let mut d = json!({"pass_name":method,"implementation_digest":implementation,"configuration_digest":identity::digest(&json!({"match":"whole-module exact bytes","tie_breaking":"none","reciprocal_unique":true}))?,
            "input_assertion_ids":supports,"input_artifact_ids":[artifact],"deterministic":true,"support_class":"evidence_only"});
        let did = identity::id("derivation", &d)?;
        d["id"] = json!(did);
        add(&mut b, "derivations", d);
        if unique {
            let nid = candidates[0].0;
            selected.insert(knowledge::derived(
                &mut b,
                &ctx,
                nid,
                "RENAMED_FROM",
                knowledge::target(oid),
                &did,
                supports,
            )?);
        }
    }
    let unmatched_old = gone
        .iter()
        .filter(|(id, _)| !matched.contains(*id))
        .map(|(id, (e, s))| ((*id).clone(), e.clone(), s.clone()))
        .collect::<Vec<_>>();
    let unmatched_new = added
        .iter()
        .filter(|(id, _)| !matched.contains(*id))
        .map(|(id, (e, s))| ((*id).clone(), e.clone(), s.clone()))
        .collect::<Vec<_>>();
    uncertain(&mut b, &mut payloads, &ctx, &unmatched_old, &unmatched_new)?;
    // Retain candidate calculations/support even when no lineage assertion is admissible.
    let p = Prepared::extension(
        b,
        payloads,
        after.generation,
        &selected,
        &[contracts::Capability::Derived],
        json!({"method":"retained-module-content-v1"}),
    )?;
    l.commit_extension(
        &key,
        "comparison",
        &[before.id.clone(), after.id.clone()],
        &p,
        &selected,
        watermark,
    )
}
pub(crate) fn declarations(source: &str) -> Result<BTreeSet<String>> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .ok_or("COMPARE: Cannot parse retained module")?;
    let mut cursor = tree.root_node().walk();
    Ok(tree
        .root_node()
        .named_children(&mut cursor)
        .filter(|n| {
            matches!(
                n.kind(),
                "function_item"
                    | "struct_item"
                    | "enum_item"
                    | "trait_item"
                    | "type_item"
                    | "const_item"
                    | "static_item"
            )
        })
        .filter_map(|n| {
            n.child_by_field_name("name").map(|name| {
                format!(
                    "{}:{}",
                    n.kind(),
                    &source[name.start_byte()..name.end_byte()]
                )
            })
        })
        .collect())
}
/// Non-Rust comparison signals come from retained canonical declarations, not a
/// Rust parser applied to another language. They remain advisory name overlap.
pub(crate) fn module_declarations(
    b: &Value,
    module: &str,
    source: &str,
) -> Result<BTreeSet<String>> {
    let entity = knowledge::row(b, "entity_versions", module)?;
    if entity["language"] == "rust" {
        return declarations(source);
    }
    let prefix = format!("{}::", text(entity, "canonical_key"));
    let symbols: BTreeSet<_> = rows(b, "assertions")
        .iter()
        .filter(|a| {
            a["class"] == "evidence"
                && a["proposition"]["predicate"] == "DECLARES"
                && a["proposition"]["subject_id"] == module
        })
        .map(|a| text(&a["proposition"]["object"], "entity_version_id"))
        .collect();
    Ok(rows(b, "entity_versions")
        .iter()
        .filter(|e| symbols.contains(&text(e, "id")))
        .filter_map(|e| e["canonical_key"].as_str()?.strip_prefix(&prefix))
        .filter_map(|key| key.split('#').next())
        .filter(|name| !name.contains("::"))
        .map(|name| format!("symbol:{name}"))
        .collect())
}
type Module = (String, Value, knowledge::Source);
fn uncertain(
    b: &mut Value,
    payloads: &mut contracts::Payloads,
    ctx: &Value,
    old: &[Module],
    new: &[Module],
) -> Result<()> {
    const LIMIT: usize = 64;
    let coverage = json!({"before_total":old.len(),"after_total":new.len(),"before_selected":old.len().min(LIMIT),
        "after_selected":new.len().min(LIMIT),"before_omitted":old.len().saturating_sub(LIMIT),"after_omitted":new.len().saturating_sub(LIMIT)});
    let old = &old[..old.len().min(LIMIT)];
    let new = &new[..new.len().min(LIMIT)];
    if old.is_empty() || new.is_empty() {
        return Ok(());
    }
    let signals: BTreeMap<_, _> = old
        .iter()
        .chain(new)
        .map(|(id, _, source)| {
            module_declarations(b, id, &source.text).map(|names| (id.clone(), names))
        })
        .collect::<Result<_>>()?;
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (from, _, _) in old {
        for (to, _, _) in new {
            if !signals[from].is_disjoint(&signals[to]) {
                edges.entry(from.clone()).or_default().insert(to.clone());
                edges.entry(to.clone()).or_default().insert(from.clone());
            }
        }
    }
    let mut remaining: BTreeSet<_> = edges.keys().cloned().collect();
    let mut groups = Vec::new();
    while let Some(start) = remaining.first().cloned() {
        let mut group = BTreeSet::new();
        let mut todo = vec![start];
        while let Some(id) = todo.pop() {
            if group.insert(id.clone()) {
                remaining.remove(&id);
                todo.extend(edges[&id].iter().cloned());
            }
        }
        groups.push(group);
    }
    let linked: BTreeSet<_> = groups.iter().flatten().cloned().collect();
    let unsupported: BTreeSet<_> = old
        .iter()
        .chain(new)
        .map(|(id, _, _)| id.clone())
        .filter(|id| !linked.contains(id))
        .collect();
    if old.iter().any(|(id, _, _)| unsupported.contains(id))
        && new.iter().any(|(id, _, _)| unsupported.contains(id))
    {
        groups.push(unsupported);
    }
    for group in groups {
        let before: Vec<_> = old.iter().filter(|(id, _, _)| group.contains(id)).collect();
        let after: Vec<_> = new.iter().filter(|(id, _, _)| group.contains(id)).collect();
        let mut overlap = Vec::new();
        for (from, _, _) in &before {
            for (to, _, _) in &after {
                let common: Vec<_> = signals[from].intersection(&signals[to]).cloned().collect();
                if !common.is_empty() {
                    overlap.push(json!({"from_entity_version_id":from,"to_entity_version_id":to,"shared_declarations":common}));
                }
            }
        }
        let status = if overlap.is_empty() {
            "unsupported_mapping"
        } else {
            match (before.len(), after.len()) {
                (1, 1) => "possible_edited_move",
                (1, _) => "possible_split",
                (_, 1) => "possible_merge",
                _ => "ambiguous_continuity",
            }
        };
        let members: Vec<_> = before.iter().chain(after.iter()).copied().collect();
        let supports: Vec<_> = members
            .iter()
            .map(|(_, _, source)| source.assertion_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mapping = json!({"schema_version":"1.0","method":"uncertain-retained-module-declarations-v1","context":ctx,
            "from_entity_version_ids":before.iter().map(|(id,_,_)|id).collect::<Vec<_>>(),
            "candidate_entity_version_ids":after.iter().map(|(id,_,_)|id).collect::<Vec<_>>(),
            "from_keys":before.iter().map(|(_,e,_)|&e["canonical_key"]).collect::<Vec<_>>(),
            "candidate_keys":after.iter().map(|(_,e,_)|&e["canonical_key"]).collect::<Vec<_>>(),
            "source_evidence_ref_ids":members.iter().map(|(_,_,s)|&s.evidence_id).collect::<Vec<_>>(),
            "status":status,"exact_match":false,"signals":overlap,"selection_limit_per_side":LIMIT,"candidate_coverage":coverage,
            "limitation":"Uncertain candidate groups from retained declaration-name overlap, not exact identity or proven conceptual continuity. Unsupported mappings only juxtapose unmatched versions; they assert no lineage. No candidate is selected or merged."});
        let artifact = knowledge::artifact(
            b,
            payloads,
            "correspondence",
            identity::canonical(&mapping)?,
        )?;
        let mut d = json!({"pass_name":"uncertain-retained-module-declarations-v1",
            "implementation_digest":identity::bytes_digest(include_bytes!("history.rs")),
            "configuration_digest":identity::digest(&json!({"signal":"top-level declaration kind and name","limit_per_side":LIMIT,"tie_breaking":"none","identity_promotion":false}))?,
            "input_assertion_ids":supports,"input_artifact_ids":[artifact],"deterministic":true,"support_class":"evidence_only"});
        d["id"] = json!(identity::id("derivation", &d)?);
        add(b, "derivations", d);
    }
    Ok(())
}

pub(crate) fn ambiguous_details(l: &Ledger, c: &Correspondence) -> Result<Vec<Value>> {
    let payloads = l.payloads(&c.bundle)?;
    let mut out = Vec::new();
    for d in &c.ambiguous {
        for aid in super::contracts::strings(&d["input_artifact_ids"]) {
            let raw = payloads
                .get(&aid)
                .ok_or("EXPLAIN: Missing correspondence artifact")?;
            let mut mapping = identity::parse(raw)?;
            mapping["input_assertion_ids"] = d["input_assertion_ids"].clone();
            mapping["derivation_id"] = d["derivation_id"].clone();
            out.push(mapping);
        }
    }
    Ok(out)
}
