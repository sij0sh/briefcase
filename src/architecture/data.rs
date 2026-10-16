//! Data-lineage producer. Owns schema resources, ORM correspondence, supported
//! field dataflow, database reads/writes and transformation boundaries. The
//! TS/JS structure producer keeps symbols, imports and static calls.
// Occurrence tuples stay local to one analysis; naming each would add no meaning.
#![allow(clippy::type_complexity)]
use super::{
    capture::Capture,
    contracts::{self, add, rows, strings, text},
    identity, knowledge,
    ledger::{Ledger, Prepared},
    passes,
    query::{self, Options},
    require, rust, typescript, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const SCALARS: &[&str] = &[
    "String", "Int", "BigInt", "Float", "Decimal", "Boolean", "DateTime", "Json", "Bytes",
];

#[derive(Debug, Clone)]
pub(crate) struct Field {
    pub name: String,
    pub column: String,
    pub ty: String,
    pub optional: bool,
    pub list: bool,
    pub id: bool,
    pub unique: bool,
    pub relation: bool,
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone)]
pub(crate) struct Model {
    pub name: String,
    pub table: String,
    pub schema: Option<String>,
    pub start: usize,
    pub end: usize,
    pub table_span: (usize, usize),
    pub fields: Vec<Field>,
}
#[derive(Debug, Default)]
pub(crate) struct Schema {
    pub models: Vec<Model>,
    pub enums: BTreeSet<String>,
    pub issues: Vec<(usize, usize, String)>,
}
mod schema_parse;
pub(crate) use schema_parse::*;
mod produce;
pub(crate) use produce::*;
mod finish;
pub(crate) use finish::*;
mod analyze;
pub use analyze::*;
