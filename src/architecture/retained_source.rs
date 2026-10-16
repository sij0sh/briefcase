//! Exact retained-source joins and module layout, independent of product or session reports.
use super::{
    contracts::{rows, text},
    ledger::View,
    query, require, source, Result,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Retained files of one pinned source view, joined by exact repository path.
pub(crate) struct SourceFiles {
    pub view: String,
    pub snapshot: String,
    pub digests: BTreeMap<String, String>,
    /// Path → canonical key of its file entity.
    pub keys: BTreeMap<String, String>,
    /// Path → canonical keys of modules it contains or declares, not holding-file inference.
    pub modules: BTreeMap<String, BTreeSet<String>>,
}
impl SourceFiles {
    /// Compare exact captured digests, without interpreting session activity.
    pub fn net_change(&self, after: &Self, path: &str) -> &'static str {
        match (self.digests.get(path), after.digests.get(path)) {
            (Some(a), Some(b)) if a == b => "unchanged",
            (Some(_), Some(_)) => "changed",
            (None, Some(_)) => "added",
            (Some(_), None) => "removed",
            (None, None) => "not_captured",
        }
    }

    pub fn new(v: &View) -> Result<Self> {
        require(
            v.scope == "source",
            "SCOPE",
            "A source view is required for joins",
        )?;
        let ctx = crate::architecture::knowledge::context(&v.bundle);
        let snapshot = text(&ctx, "snapshot_id");
        let digests = rows(&v.bundle, "snapshots")
            .iter()
            .filter(|s| s["id"] == snapshot)
            .flat_map(|s| rows(s, "files"))
            .map(|f| (text(f, "path"), text(f, "digest")))
            .collect();
        let mut ids = BTreeMap::new();
        let mut keys = BTreeMap::new();
        for e in rows(&v.bundle, "entity_versions")
            .iter()
            .filter(|e| e["kind"] == "file" && e["context"] == ctx)
        {
            let key = text(e, "canonical_key");
            let path = key.strip_prefix("file::").unwrap_or(&key).to_owned();
            ids.insert(text(e, "id"), path.clone());
            keys.insert(path, key);
        }
        let entities: BTreeMap<String, &Value> = rows(&v.bundle, "entity_versions")
            .iter()
            .map(|e| (text(e, "id"), e))
            .collect();
        let mut modules: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for a in rows(&v.bundle, "assertions").iter().filter(|a| {
            a["class"] == "evidence"
                && a["context"] == ctx
                && ["CONTAINS", "DECLARES"]
                    .contains(&a["proposition"]["predicate"].as_str().unwrap_or(""))
        }) {
            let (Some(path), Some(object)) = (
                ids.get(&text(&a["proposition"], "subject_id")),
                entities.get(&text(&a["proposition"]["object"], "entity_version_id")),
            ) else {
                continue;
            };
            if object["kind"] == "module" {
                modules
                    .entry(path.clone())
                    .or_default()
                    .insert(text(object, "canonical_key"));
            }
        }
        Ok(Self {
            view: v.id.clone(),
            snapshot,
            digests,
            keys,
            modules,
        })
    }
}

/// Symbols each module declares, with the DECLARES assertion.
pub(crate) fn declared_symbols_of(v: &View) -> BTreeMap<String, Vec<(String, String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for (symbol, owner) in symbol_owners(v) {
        out.entry(owner.module)
            .or_default()
            .push((symbol, owner.symbol_id, owner.declares));
    }
    for v in out.values_mut() {
        v.sort();
        v.dedup();
    }
    out
}

/// Module key → the retained files that hold it. A file that only declares
/// `mod x;` contains x without holding it: the holding files are those that
/// declare x's own symbols, else its single containing file, else the
/// containing files named like the module (`x.rs`, `x/mod.rs`, `x.ts`), else
/// every containing file (crates sharing one name).
pub(crate) fn module_files(v: &View) -> BTreeMap<String, BTreeSet<String>> {
    module_layout(v).0
}

/// Child module → the modules that declare it (Rust `mod x;`): the modules
/// held by a file that contains the child without holding it.
pub(crate) fn module_parents(v: &View) -> BTreeMap<String, BTreeSet<String>> {
    let (held, containing) = module_layout(v);
    let mut holders: BTreeMap<&String, BTreeSet<&String>> = BTreeMap::new();
    for (m, files) in &held {
        for f in files {
            holders.entry(f).or_default().insert(m);
        }
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (child, files) in &containing {
        for f in files
            .iter()
            .filter(|f| !held.get(child).is_some_and(|h| h.contains(*f)))
        {
            for parent in holders.get(f).into_iter().flatten() {
                if *parent != child {
                    out.entry(child.clone())
                        .or_default()
                        .insert((*parent).clone());
                }
            }
        }
    }
    out
}

type Layout = (
    BTreeMap<String, BTreeSet<String>>,
    BTreeMap<String, BTreeSet<String>>,
);
/// Held files and containing files of every module.
fn module_layout(v: &View) -> Layout {
    let ctx = query::context(v);
    let es: BTreeMap<String, &Value> = rows(&v.bundle, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), e))
        .collect();
    let mut containing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for a in rows(&v.bundle, "assertions").iter().filter(|a| {
        a["proposition"]["predicate"] == "CONTAINS"
            && a["class"] == "evidence"
            && a["context"] == ctx
    }) {
        let (Some(file), Some(module)) = (
            es.get(&text(&a["proposition"], "subject_id")),
            es.get(&text(&a["proposition"]["object"], "entity_version_id")),
        ) else {
            continue;
        };
        if file["kind"] == "file" && module["kind"] == "module" {
            let key = text(file, "canonical_key");
            containing
                .entry(text(module, "canonical_key"))
                .or_default()
                .insert(key.strip_prefix("file::").unwrap_or(&key).to_owned());
        }
    }
    let mut defining: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for owner in symbol_owners(v).into_values() {
        if let Some(file) = owner.file {
            defining.entry(owner.module).or_default().insert(file);
        }
    }
    let mut out = BTreeMap::new();
    for e in rows(&v.bundle, "entity_versions")
        .iter()
        .filter(|e| e["kind"] == "module" && e["context"] == ctx)
    {
        let key = text(e, "canonical_key");
        let contains = containing.get(&key).cloned().unwrap_or_default();
        let held: BTreeSet<String> = match defining.get(&key) {
            Some(files) if !files.is_empty() => files.clone(),
            _ if contains.len() <= 1 => contains,
            _ => {
                let last = key.rsplit("::").next().unwrap_or(&key);
                let named: BTreeSet<String> = contains
                    .iter()
                    .filter(|p| {
                        let mut parts = p.rsplit('/');
                        let name = parts.next().unwrap_or(p);
                        name.split('.').next() == Some(last)
                            || (name.starts_with("mod.") && parts.next() == Some(last))
                    })
                    .cloned()
                    .collect();
                if named.is_empty() {
                    contains
                } else {
                    named
                }
            }
        };
        out.insert(key, held);
    }
    (out, containing)
}

/// The category of a module by the file that holds it (a crate-named fixture
/// module is not application code), or by its key without one.
pub(crate) fn module_category(files: Option<&BTreeSet<String>>, key: &str) -> &'static str {
    match files.and_then(|f| f.iter().next()) {
        Some(path) => source::category(path),
        None => source::category(key),
    }
}

/// The module a symbol belongs to, the file that declares it (when DECLARES
/// names a file) and the DECLARES assertion.
pub(crate) struct SymbolOwner {
    pub module: String,
    pub file: Option<String>,
    pub symbol_id: String,
    pub declares: String,
}

/// Symbol key → its owning module, for every language. DECLARES Evidence
/// names the declaring entity: a module, or (Rust) the file holding the
/// definition. The owner is the longest module key that is a `::` prefix of
/// the symbol key without its `#offset`; without one it is the declaring
/// entity itself (a TS/JS module is keyed by its file path).
pub(crate) fn symbol_owners(v: &View) -> BTreeMap<String, SymbolOwner> {
    let ctx = query::context(v);
    let es: BTreeMap<String, &Value> = rows(&v.bundle, "entity_versions")
        .iter()
        .map(|e| (text(e, "id"), e))
        .collect();
    let modules: BTreeSet<String> = rows(&v.bundle, "entity_versions")
        .iter()
        .filter(|e| e["kind"] == "module" && e["context"] == ctx)
        .map(|e| text(e, "canonical_key"))
        .collect();
    let mut out = BTreeMap::new();
    for a in rows(&v.bundle, "assertions").iter().filter(|a| {
        a["proposition"]["predicate"] == "DECLARES" && v.eligible.contains(&text(a, "id"))
    }) {
        let p = &a["proposition"];
        let (Some(declarer), Some(symbol)) = (
            es.get(&text(p, "subject_id")),
            es.get(&text(&p["object"], "entity_version_id")),
        ) else {
            continue;
        };
        if symbol["kind"] != "symbol" {
            continue;
        }
        let key = text(symbol, "canonical_key");
        let declared_by = text(declarer, "canonical_key");
        let path = key.split('#').next().unwrap_or(&key);
        let parts: Vec<&str> = path.split("::").collect();
        let module = (1..parts.len())
            .rev()
            .map(|n| parts[..n].join("::"))
            .find(|prefix| modules.contains(prefix))
            .unwrap_or_else(|| declared_by.clone());
        let file = (declarer["kind"] == "file").then(|| declared_by.clone());
        out.insert(
            key,
            SymbolOwner {
                module,
                file,
                symbol_id: text(symbol, "id"),
                declares: text(a, "id"),
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> View {
        let ctx = json!({"kind":"source","snapshot_id":"snapshot:layout","build_context_id":"build:layout"});
        let entities: Vec<Value> = [
            ("lib", "file", "src/lib.rs"),
            ("child_file", "file", "src/child.rs"),
            ("root", "module", "demo"),
            ("child", "module", "demo::child"),
            ("root_symbol", "symbol", "demo::run#1"),
            ("child_symbol", "symbol", "demo::child::run#2"),
            ("other_file", "file", "crates/other/lib.rs"),
            ("same_root", "module", "demo"),
            ("ts", "module", "src/app.ts"),
            ("ts_symbol", "symbol", "src/app.ts::run#3"),
            ("bin_file", "file", "src/bin/worker.rs"),
            ("bin", "module", "worker"),
            ("missing", "symbol", "demo::child::missing#4"),
        ]
        .into_iter()
        .map(|(id, kind, key)| json!({"id":id,"kind":kind,"canonical_key":key,"context":ctx}))
        .collect();
        let assertions: Vec<Value> = [
            ("root_contains", "lib", "CONTAINS", "root"),
            ("child_declared_in_parent", "lib", "CONTAINS", "child"),
            ("child_contains", "child_file", "CONTAINS", "child"),
            ("root_declares", "lib", "DECLARES", "root_symbol"),
            ("child_declares", "child_file", "DECLARES", "child_symbol"),
            ("other_contains", "other_file", "CONTAINS", "same_root"),
            ("ts_declares", "ts", "DECLARES", "ts_symbol"),
            ("bin_contains", "bin_file", "CONTAINS", "bin"),
            ("not_eligible", "lib", "DECLARES", "missing"),
        ].into_iter().map(|(id, subject, predicate, object)| json!({"id":id,"class":"evidence","context":ctx,
            "proposition":{"subject_id":subject,"predicate":predicate,"object":{"entity_version_id":object}}})).collect();
        View {
            id: "view:layout".into(),
            generation: 1,
            bundle: json!({"runs":[{"context":ctx}],"entity_versions":entities,"assertions":assertions,
                "snapshots":[{"id":"snapshot:layout","files":[{"path":"src/lib.rs","digest":"a"},
                    {"path":"src/child.rs","digest":"b"},{"path":"src/bin/worker.rs","digest":"c"}]}]}),
            diagnostics: json!({}),
            projections: vec![],
            eligible: ["root_declares", "child_declares", "ts_declares"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            stale: false,
            invalidated: false,
            retraction_watermark: 0,
            overlay_watermark: 0,
            scope: "source".into(),
        }
    }

    #[test]
    fn containing_files_are_not_implementation_holding_files() {
        let v = fixture();
        let files = SourceFiles::new(&v).unwrap();
        assert_eq!(files.digests["src/child.rs"], "b");
        assert_eq!(files.keys["src/lib.rs"], "src/lib.rs");
        assert!(files.modules["src/lib.rs"].contains("demo::child"));
        let held = module_files(&v);
        assert_eq!(held["demo::child"], BTreeSet::from(["src/child.rs".into()]));
        assert_eq!(held["demo"], BTreeSet::from(["src/lib.rs".into()]));
        assert_eq!(held["worker"], BTreeSet::from(["src/bin/worker.rs".into()]));
        assert_eq!(
            module_parents(&v)["demo::child"],
            BTreeSet::from(["demo".into()])
        );
        let owners = symbol_owners(&v);
        assert_eq!(owners["demo::child::run#2"].module, "demo::child");
        assert_eq!(owners["src/app.ts::run#3"].module, "src/app.ts");
        assert_eq!(owners["src/app.ts::run#3"].file, None);
        assert!(!owners.contains_key("demo::child::missing#4"));
        let mut runtime = v;
        runtime.scope = "runtime".into();
        assert!(SourceFiles::new(&runtime).is_err());
    }

    #[test]
    fn empty_inline_modules_and_named_fallbacks_keep_existing_choices() {
        let mut v = fixture();
        v.eligible.remove("child_declares");
        assert_eq!(
            module_files(&v)["demo::child"],
            BTreeSet::from(["src/child.rs".into()])
        );
        let a = v.bundle["assertions"].as_array_mut().unwrap();
        a.retain(|a| a["id"] != "child_contains");
        assert_eq!(
            module_files(&v)["demo::child"],
            BTreeSet::from(["src/lib.rs".into()])
        );
        v.eligible.clear();
        assert_eq!(
            module_files(&v)["demo"],
            BTreeSet::from(["src/lib.rs".into(), "crates/other/lib.rs".into()])
        );
    }
}
