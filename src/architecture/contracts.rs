//! Closed interchange shapes plus cross-record checks. No producer can bypass this boundary.
use super::{identity, require, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

pub type Payloads = BTreeMap<String, Vec<u8>>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Evidence,
    Derived,
    Claim,
    Declaration,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Context {
    Source {
        snapshot_id: String,
        build_context_id: String,
    },
    Runtime {
        runtime_context_id: String,
    },
    Comparison {
        snapshot_ids: Vec<String>,
    },
    /// Desired-state knowledge from retained build and deployment inputs.
    /// It is neither a source snapshot nor an observation interval.
    Deployment {
        deployment_context_id: String,
    },
    /// One retained agent session transcript under one profile (ADR-0027).
    /// It scopes what the harness recorded, not architectural truth.
    Session {
        session_context_id: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Object {
    Entity { entity_version_id: String },
    Literal { value: Value },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposition {
    pub subject_id: String,
    pub predicate: String,
    pub object: Object,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceOrigin {
    pub run_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedOrigin {
    pub derivation_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimOrigin {
    pub inference_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationOrigin {
    pub declaration_record_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportClass {
    EvidenceOnly,
    IncludesInference,
}
/// Mandatory origins are distinct Rust types as well as closed serialized contracts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assertion {
    Evidence {
        id: String,
        proposition: Proposition,
        context: Context,
        origin: EvidenceOrigin,
        evidence_ref_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolution: Option<Value>,
        /// Typed transformation class; admissible only on TRANSFORMS Evidence.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transformation: Option<Value>,
    },
    Derived {
        id: String,
        proposition: Proposition,
        context: Context,
        origin: DerivedOrigin,
        supports: Vec<String>,
        support_class: SupportClass,
    },
    Claim {
        id: String,
        proposition: Proposition,
        context: Context,
        origin: ClaimOrigin,
        supports: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_score: Option<Value>,
    },
    Declaration {
        id: String,
        proposition: Proposition,
        context: Context,
        origin: DeclarationOrigin,
    },
}
impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Self::Evidence => "evidence",
            Self::Derived => "derived",
            Self::Claim => "claim",
            Self::Declaration => "declaration",
        }
    }
}
// Granted by the local application boundary, never parsed from a batch/MCP argument.
#[derive(Clone, Copy)]
pub enum Capability {
    Evidence,
    Derived,
    Claim,
    Declaration,
}
impl Capability {
    pub fn class(self) -> Class {
        match self {
            Self::Evidence => Class::Evidence,
            Self::Derived => Class::Derived,
            Self::Claim => Class::Claim,
            Self::Declaration => Class::Declaration,
        }
    }
}
pub const TABLES: &[&str] = &[
    "snapshots",
    "build_contexts",
    "analysis_configs",
    "runtime_contexts",
    "deployment_contexts",
    "session_contexts",
    "entity_versions",
    "artifacts",
    "evidence_refs",
    "runs",
    "derivations",
    "pass_manifests",
    "input_packets",
    "inferences",
    "authorities",
    "declaration_records",
    "assertions",
    "unresolved_references",
    "semantic_binding_resolutions",
    "projections",
];
pub fn empty_bundle() -> Value {
    let mut b = json!({"schema_version":"1.0"});
    for t in TABLES {
        b[t] = json!([]);
    }
    b
}
pub fn rows<'a>(b: &'a Value, table: &str) -> &'a [Value] {
    b[table].as_array().map(Vec::as_slice).unwrap_or(&[])
}
/// Below this size a linear id scan is cheaper than maintaining an index.
const ADD_INDEX_MIN: usize = 256;
thread_local! {
    /// Id index for large tables, keyed by table name and validated against the
    /// array's buffer, length and last id so any outside mutation forces a rebuild.
    static ADD_INDEX: std::cell::RefCell<std::collections::HashMap<String, (usize, usize, Value, std::collections::HashSet<String>)>> = Default::default();
}
pub fn add(b: &mut Value, table: &str, row: Value) {
    let existing = rows(b, table);
    if existing.len() < ADD_INDEX_MIN {
        if !existing.iter().any(|r| r["id"] == row["id"]) {
            b[table]
                .as_array_mut()
                .expect("owned bundle table")
                .push(row);
        }
        return;
    }
    let Some(id) = row["id"].as_str().map(str::to_owned) else {
        if !existing.iter().any(|r| r["id"] == row["id"]) {
            b[table]
                .as_array_mut()
                .expect("owned bundle table")
                .push(row);
        }
        return;
    };
    ADD_INDEX.with(|cache| {
        let mut cache = cache.borrow_mut();
        let rows_ = b[table].as_array_mut().expect("owned bundle table");
        let key = (rows_.as_ptr() as usize, rows_.len(), rows_.last().map(|r| r["id"].clone()).unwrap_or(Value::Null));
        let fresh = matches!(cache.get(table), Some((p, n, last, _)) if (*p, *n, last) == (key.0, key.1, &key.2));
        if !fresh {
            let ids = rows_.iter().filter_map(|r| r["id"].as_str().map(str::to_owned)).collect();
            cache.insert(table.to_owned(), (key.0, key.1, key.2, ids));
        }
        let entry = cache.get_mut(table).unwrap();
        if !entry.3.contains(&id) {
            rows_.push(row);
            entry.3.insert(id);
        }
        entry.0 = rows_.as_ptr() as usize;
        entry.1 = rows_.len();
        entry.2 = rows_.last().map(|r| r["id"].clone()).unwrap_or(Value::Null);
    });
}
pub fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_owned()
}
pub fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

mod schema;
pub use schema::*;
mod index;
pub use index::*;
mod validate;
pub use validate::*;
mod batch;
pub use batch::*;
