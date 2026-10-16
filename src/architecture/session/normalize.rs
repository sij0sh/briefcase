//! Raw records + a profile → a deterministic normalized session. The event
//! vocabulary is deliberately smaller than any harness schema. A tool call
//! alone never proves success: only a result paired by exact call ID whose
//! outcome rules say it succeeded produces `file_edit_confirmed`, and an
//! assistant statement never produces an operation at all.
use super::{
    jsonl::{RawRecord, RawRecordStream},
    paths,
    profile::{self, Emit, Operation, Profile, Rule},
};
use crate::architecture::contracts::date_time;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const KINDS: &[&str] = &[
    "user_message",
    "assistant_message",
    "tool_call",
    "tool_result",
    "file_read",
    "file_edit_attempt",
    "file_edit_confirmed",
    "command_started",
    "command_finished",
    "unknown",
];

/// The normalized session: metadata, ordered events and coverage.
#[derive(Clone, Debug)]
pub struct Normalized {
    pub external_session_id: Option<String>,
    pub harness_version: Option<String>,
    pub working_directory: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub events: Vec<Value>,
    pub coverage: Value,
    /// `complete` or `partial`.
    pub capture_state: &'static str,
    pub limitations: Vec<String>,
}

/// One interpreted occurrence before pairing.
#[derive(Clone, Debug)]
struct Draft<'a> {
    record: &'a RawRecord,
    pointer: String,
    emit: Emit,
    item: Value,
    key: Option<String>,
    rule: &'a Rule,
    /// Working directory recorded on the enclosing record, if any.
    outer_cwd: Option<String>,
}

#[derive(Default)]
struct Counts(BTreeMap<&'static str, u64>);
impl Counts {
    fn add(&mut self, k: &'static str) {
        *self.0.entry(k).or_default() += 1;
    }
    fn get(&self, k: &str) -> u64 {
        self.0.get(k).copied().unwrap_or(0)
    }
}

fn outcome(rule: &Rule, item: &Value, record: &Value) -> &'static str {
    if !rule.failed_when.is_empty() && profile::all(&rule.failed_when, item, record) {
        "failed"
    } else if !rule.succeeded_when.is_empty() && profile::all(&rule.succeeded_when, item, record) {
        "succeeded"
    } else {
        "unknown"
    }
}

/// Paths a Codex-style `apply_patch` document names, in order.
pub fn patch_paths(patch: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for line in patch.lines() {
        let found = [
            ("*** Add File: ", "add"),
            ("*** Update File: ", "update"),
            ("*** Delete File: ", "delete"),
            ("*** Move to: ", "move"),
        ]
        .into_iter()
        .find_map(|(prefix, op)| line.strip_prefix(prefix).map(|p| (op, p.trim().to_owned())));
        if let Some((op, path)) = found.filter(|(_, p)| !p.is_empty()) {
            if !out.iter().any(|(_, p)| *p == path) {
                out.push((op, path));
            }
        }
    }
    out
}

fn timestamp(p: &Profile, v: &Value, counts: &mut Counts) -> Option<String> {
    let t = profile::first_text(&p.timestamp, v)?;
    if date_time(&t).is_ok() {
        counts.add("timestamps_present");
        Some(t)
    } else {
        counts.add("timestamps_invalid");
        None
    }
}

fn interpret<'a>(
    p: &'a Profile,
    stream: &'a RawRecordStream,
    counts: &mut Counts,
    unknown: &mut Vec<(&'a RawRecord, String, String)>,
) -> Vec<Draft<'a>> {
    let mut drafts = Vec::new();
    for r in &stream.records {
        let Some(v) = &r.value else {
            counts.add("records_malformed");
            unknown.push((
                r,
                String::new(),
                format!("malformed: {}", r.error.clone().unwrap_or_default()),
            ));
            continue;
        };
        let Some(rule) = p.records.iter().find(|rule| profile::all(&rule.when, v, v)) else {
            counts.add("records_unknown");
            unknown.push((r, String::new(), "unrecognized_record".into()));
            continue;
        };
        match rule.emit {
            Emit::Ignore => counts.add("records_ignored"),
            Emit::Each => {
                counts.add("records_recognized");
                let outer_cwd = profile::eval_text(&rule.cwd, v, None);
                type Members<'a> = Vec<(String, Option<String>, &'a Value)>;
                let (base, members): (&str, Members) = match (&rule.each, &rule.each_key) {
                    (Some(ptr), _) => (
                        ptr,
                        v.pointer(ptr)
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .enumerate()
                                    .map(|(i, x)| (i.to_string(), None, x))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ),
                    (None, Some(ptr)) => (
                        ptr,
                        v.pointer(ptr)
                            .and_then(Value::as_object)
                            .map(|o| {
                                o.iter()
                                    .map(|(k, x)| (escape(k), Some(k.clone()), x))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ),
                    _ => unreachable!("validated profile"),
                };
                for (segment, key, item) in members {
                    let pointer = format!("{base}/{segment}");
                    match rule
                        .rules
                        .iter()
                        .find(|inner| profile::all(&inner.when, item, v))
                    {
                        None => {
                            counts.add("items_unknown");
                            unknown.push((r, pointer, "unrecognized_item".into()));
                        }
                        Some(inner) if inner.emit == Emit::Ignore => counts.add("items_ignored"),
                        Some(inner) => drafts.push(Draft {
                            record: r,
                            pointer,
                            emit: inner.emit,
                            item: item.clone(),
                            key,
                            rule: inner,
                            outer_cwd: outer_cwd.clone(),
                        }),
                    }
                }
            }
            emit => {
                counts.add("records_recognized");
                drafts.push(Draft {
                    record: r,
                    pointer: String::new(),
                    emit,
                    item: v.clone(),
                    key: None,
                    rule,
                    outer_cwd: None,
                });
            }
        }
    }
    drafts
}

/// RFC 6901 member escaping.
fn escape(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

struct Pending {
    call: usize,
    derived: Vec<usize>,
    operation: Option<Operation>,
    result: Option<usize>,
}

struct Builder<'a> {
    p: &'a Profile,
    root: &'a str,
    session_cwd: Option<String>,
    events: Vec<Value>,
    counts: Counts,
    /// Next event index per record ordinal.
    next: BTreeMap<usize, usize>,
    timestamps: BTreeMap<usize, Option<String>>,
}
impl<'a> Builder<'a> {
    fn push(
        &mut self,
        r: &RawRecord,
        pointer: &str,
        kind: &str,
        mut fields: Map<String, Value>,
    ) -> usize {
        let index = self.next.entry(r.ordinal).or_default();
        let id = format!("{}.{}", r.ordinal, index);
        *index += 1;
        let mut e = Map::new();
        e.insert("id".into(), json!(id));
        e.insert("ordinal".into(), json!(r.ordinal));
        e.insert("byte_start".into(), json!(r.byte_start));
        e.insert("byte_end".into(), json!(r.byte_end));
        e.insert("pointer".into(), json!(pointer));
        e.insert("kind".into(), json!(kind));
        if let Some(Some(t)) = self.timestamps.get(&r.ordinal) {
            e.insert("timestamp".into(), json!(t));
        }
        if let Some(record) = &r.value {
            for (key, fields) in &self.p.correlation {
                if let Some(value) = profile::first_text(fields, record) {
                    e.insert(key.clone(), json!(value));
                }
            }
        }
        e.append(&mut fields);
        self.events.push(Value::Object(e));
        self.events.len() - 1
    }
    fn cwd(&self, raw: Option<String>) -> Option<String> {
        raw.and_then(|c| paths::directory(&c, self.session_cwd.as_deref()))
    }
    fn path(&mut self, raw: &str, cwd: Option<&str>) -> Value {
        let r = paths::resolve(raw, cwd, self.session_cwd.as_deref(), self.root);
        self.counts.add(match r.location {
            "repository" => "paths_repository",
            "external" => "paths_external",
            _ => "paths_unresolved",
        });
        json!({"location":r.location,"path":r.path,"raw":raw})
    }
    fn fields(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
        pairs
            .into_iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| (k.to_owned(), v))
            .collect()
    }
    fn set(&mut self, i: usize, k: &str, v: Value) {
        self.events[i][k] = v;
    }
}

/// Normalize one retained record stream with one profile. Pure: equal
/// bytes, profile and root give equal output.
pub fn normalize(stream: &RawRecordStream, p: &Profile, root: &str) -> Normalized {
    let mut counts = Counts::default();
    let mut unknown = Vec::new();
    let drafts = interpret(p, stream, &mut counts, &mut unknown);
    let valid: Vec<&Value> = stream
        .records
        .iter()
        .filter_map(|r| r.value.as_ref())
        .collect();
    let meta =
        |fields: &[profile::Field]| valid.iter().find_map(|v| profile::first_text(fields, v));
    let session_cwd = meta(&p.session.cwd).and_then(|c| paths::directory(&c, None));
    let mut b = Builder {
        p,
        root,
        session_cwd: session_cwd.clone(),
        events: Vec::new(),
        counts,
        next: BTreeMap::new(),
        timestamps: BTreeMap::new(),
    };
    let mut times = Vec::new();
    for r in &stream.records {
        let t = r
            .value
            .as_ref()
            .and_then(|v| timestamp(p, v, &mut b.counts));
        if let Some(t) = &t {
            times.push(t.clone());
        }
        b.timestamps.insert(r.ordinal, t);
    }
    // Duplicate call IDs make every result for that ID unpairable.
    let mut seen = BTreeSet::new();
    let mut duplicate = BTreeSet::new();
    for d in drafts.iter().filter(|d| d.emit == Emit::ToolCall) {
        if let Some(id) = profile::eval_text(&d.rule.call_id, &d.item, d.key.as_deref()) {
            if !seen.insert(id.clone()) {
                duplicate.insert(id);
            }
        }
    }
    let mut pending: BTreeMap<String, Pending> = BTreeMap::new();
    // Interleave unknown records with interpreted ones in stream order.
    let mut unknown_at: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    for (r, pointer, reason) in unknown {
        unknown_at
            .entry(r.ordinal)
            .or_default()
            .push((pointer, reason));
    }
    let records: BTreeMap<usize, &RawRecord> =
        stream.records.iter().map(|r| (r.ordinal, r)).collect();
    let mut by_record: BTreeMap<usize, Vec<&Draft>> = BTreeMap::new();
    for d in &drafts {
        by_record.entry(d.record.ordinal).or_default().push(d);
    }
    let ordinals: BTreeSet<usize> = unknown_at.keys().chain(by_record.keys()).copied().collect();
    for ordinal in ordinals {
        let r = records[&ordinal];
        for (pointer, reason) in unknown_at.remove(&ordinal).unwrap_or_default() {
            b.push(
                r,
                &pointer,
                "unknown",
                Builder::fields(vec![("reason", json!(reason))]),
            );
        }
        for d in by_record.remove(&ordinal).unwrap_or_default() {
            draft(&mut b, d, &duplicate, &mut pending);
        }
    }
    for _ in pending.values().filter(|c| c.result.is_none()) {
        b.counts.add("tool_calls_without_result");
    }
    finish(
        b,
        stream,
        p,
        session_cwd,
        meta(&p.session.external_id),
        meta(&p.session.harness_version),
        times,
    )
}

fn draft(
    b: &mut Builder,
    d: &Draft,
    duplicate: &BTreeSet<String>,
    pending: &mut BTreeMap<String, Pending>,
) {
    let r = d.record;
    let text = |f: &Option<profile::Field>| profile::eval_text(f, &d.item, d.key.as_deref());
    let cwd = b.cwd(text(&d.rule.cwd).or_else(|| d.outer_cwd.clone()));
    let exit_code = d
        .rule
        .exit_code
        .as_ref()
        .and_then(|f| profile::eval(f, &d.item, d.key.as_deref()))
        .and_then(|v| v.as_i64());
    match d.emit {
        Emit::UserMessage | Emit::AssistantMessage => {
            let Some(t) = text(&d.rule.text) else {
                b.counts.add("semantic_fields_missing");
                return;
            };
            let kind = if d.emit == Emit::UserMessage {
                "user_message"
            } else {
                "assistant_message"
            };
            b.counts.add(if d.emit == Emit::UserMessage {
                "user_messages"
            } else {
                "assistant_messages"
            });
            b.push(
                r,
                &d.pointer,
                kind,
                Builder::fields(vec![("text", json!(t))]),
            );
        }
        Emit::ToolCall => {
            b.counts.add("tool_calls");
            let call_id = text(&d.rule.call_id);
            let tool = text(&d.rule.tool).unwrap_or_default();
            let input = d
                .rule
                .input
                .as_ref()
                .and_then(|f| profile::eval(f, &d.item, d.key.as_deref()))
                .unwrap_or(Value::Null);
            let semantics = b.p.tools.get(&tool).cloned();
            let call = b.push(
                r,
                &d.pointer,
                "tool_call",
                Builder::fields(vec![
                    ("call_id", json!(call_id)),
                    ("tool", json!(tool)),
                    ("operation", json!(semantics.as_ref().map(|s| s.operation))),
                    ("cwd", json!(cwd)),
                    ("outcome", json!("unknown")),
                ]),
            );
            let mut derived = Vec::new();
            match &semantics {
                None => b.counts.add("tool_calls_without_semantics"),
                Some(s) => {
                    let get = |f: &Option<profile::Field>| profile::eval_text(f, &input, None);
                    let op_cwd = get(&s.cwd).and_then(|c| b.cwd(Some(c))).or(cwd.clone());
                    let link = json!(b.events[call]["id"]);
                    match s.operation {
                        Operation::FileRead | Operation::FileEdit => match get(&s.path) {
                            None => b.counts.add("semantic_fields_missing"),
                            Some(raw) => {
                                let path = b.path(&raw, op_cwd.as_deref());
                                let kind = if s.operation == Operation::FileRead {
                                    b.counts.add("file_reads");
                                    "file_read"
                                } else {
                                    b.counts.add("file_edit_attempts");
                                    "file_edit_attempt"
                                };
                                derived.push(b.push(
                                    r,
                                    &d.pointer,
                                    kind,
                                    Builder::fields(vec![
                                        ("call_event", link.clone()),
                                        ("tool", json!(tool)),
                                        ("path", path),
                                        ("outcome", json!("unknown")),
                                    ]),
                                ));
                            }
                        },
                        Operation::Command => match get(&s.command) {
                            None => b.counts.add("semantic_fields_missing"),
                            Some(command) => {
                                b.counts.add("commands");
                                derived.push(b.push(
                                    r,
                                    &d.pointer,
                                    "command_started",
                                    Builder::fields(vec![
                                        ("call_event", link.clone()),
                                        ("tool", json!(tool)),
                                        ("command", json!(command)),
                                        ("cwd", json!(op_cwd)),
                                        ("outcome", json!("unknown")),
                                    ]),
                                ));
                            }
                        },
                        Operation::Patch => {
                            let found = get(&s.patch).map(|t| patch_paths(&t)).unwrap_or_default();
                            if found.is_empty() {
                                b.counts.add("patches_unparsed");
                            }
                            for (op, raw) in found {
                                b.counts.add("file_edit_attempts");
                                let path = b.path(&raw, op_cwd.as_deref());
                                derived.push(b.push(
                                    r,
                                    &d.pointer,
                                    "file_edit_attempt",
                                    Builder::fields(vec![
                                        ("call_event", link.clone()),
                                        ("tool", json!(tool)),
                                        ("path", path),
                                        ("patch_operation", json!(op)),
                                        ("outcome", json!("unknown")),
                                    ]),
                                ));
                            }
                        }
                    }
                }
            }
            match call_id {
                None => b.counts.add("missing_call_ids"),
                Some(id) if duplicate.contains(&id) => b.counts.add("duplicate_call_ids"),
                Some(id) => {
                    pending.insert(
                        id,
                        Pending {
                            call,
                            derived,
                            operation: semantics.map(|s| s.operation),
                            result: None,
                        },
                    );
                }
            }
        }
        Emit::ToolResult => {
            b.counts.add("tool_results");
            let call_id = text(&d.rule.call_id);
            let o = outcome(d.rule, &d.item, d.record.value.as_ref().unwrap());
            let result = b.push(
                r,
                &d.pointer,
                "tool_result",
                Builder::fields(vec![
                    ("call_id", json!(call_id)),
                    ("outcome", json!(o)),
                    ("exit_code", json!(exit_code)),
                    ("paired", json!(false)),
                ]),
            );
            let Some(p) = call_id.as_ref().and_then(|id| pending.get_mut(id)) else {
                b.counts.add(
                    if call_id.as_ref().is_some_and(|id| duplicate.contains(id)) {
                        "tool_results_ambiguous"
                    } else {
                        "tool_results_unpaired"
                    },
                );
                return;
            };
            if p.result.is_some() {
                b.counts.add("tool_results_duplicate");
                return;
            }
            b.counts.add("tool_results_paired");
            p.result = Some(result);
            let call_id_value = b.events[p.call]["id"].clone();
            let result_id = b.events[result]["id"].clone();
            b.set(result, "paired", json!(true));
            b.set(result, "call_event", call_id_value.clone());
            b.set(p.call, "outcome", json!(o));
            b.set(p.call, "result_event", result_id.clone());
            let derived = p.derived.clone();
            let operation = p.operation;
            for &i in &derived {
                b.set(i, "outcome", json!(o));
                b.set(i, "result_event", result_id.clone());
            }
            match operation {
                Some(Operation::FileEdit | Operation::Patch) => {
                    if o == "succeeded" {
                        for &i in &derived {
                            b.counts.add("file_edits_confirmed");
                            let mut fields = Builder::fields(vec![
                                ("call_event", call_id_value.clone()),
                                ("attempt_event", b.events[i]["id"].clone()),
                                ("tool", b.events[i]["tool"].clone()),
                                ("path", b.events[i]["path"].clone()),
                                ("outcome", json!("succeeded")),
                            ]);
                            if let Some(op) = b.events[i].get("patch_operation").cloned() {
                                fields.insert("patch_operation".into(), op);
                            }
                            b.push(r, &d.pointer, "file_edit_confirmed", fields);
                        }
                    } else if o == "failed" {
                        for _ in &derived {
                            b.counts.add("file_edits_failed");
                        }
                    }
                }
                Some(Operation::Command) => {
                    for &i in &derived {
                        if o == "failed" {
                            b.counts.add("commands_failed");
                        }
                        b.push(
                            r,
                            &d.pointer,
                            "command_finished",
                            Builder::fields(vec![
                                ("call_event", call_id_value.clone()),
                                ("started_event", b.events[i]["id"].clone()),
                                ("command", b.events[i]["command"].clone()),
                                (
                                    "cwd",
                                    b.events[i].get("cwd").cloned().unwrap_or(Value::Null),
                                ),
                                ("exit_code", json!(exit_code)),
                                ("outcome", json!(o)),
                            ]),
                        );
                    }
                }
                Some(Operation::FileRead) if o == "failed" => {
                    b.counts.add("file_reads_failed");
                }
                Some(Operation::FileRead) | None => {}
            }
        }
        Emit::Command => {
            let Some(command) = text(&d.rule.command) else {
                b.counts.add("semantic_fields_missing");
                return;
            };
            b.counts.add("commands");
            b.counts.add("recorded_operations");
            let o = outcome(d.rule, &d.item, d.record.value.as_ref().unwrap());
            if o == "failed" {
                b.counts.add("commands_failed");
            }
            let started = b.push(
                r,
                &d.pointer,
                "command_started",
                Builder::fields(vec![
                    ("command", json!(command)),
                    ("cwd", json!(cwd)),
                    ("outcome", json!(o)),
                ]),
            );
            let started_id = b.events[started]["id"].clone();
            b.push(
                r,
                &d.pointer,
                "command_finished",
                Builder::fields(vec![
                    ("started_event", started_id),
                    ("command", json!(command)),
                    ("cwd", json!(cwd)),
                    ("exit_code", json!(exit_code)),
                    ("outcome", json!(o)),
                ]),
            );
        }
        Emit::FileRead | Emit::FileEdit => {
            let Some(raw) = text(&d.rule.path) else {
                b.counts.add("semantic_fields_missing");
                return;
            };
            b.counts.add("recorded_operations");
            let o = outcome(d.rule, &d.item, d.record.value.as_ref().unwrap());
            let path = b.path(&raw, cwd.as_deref());
            if d.emit == Emit::FileRead {
                b.counts.add("file_reads");
                if o == "failed" {
                    b.counts.add("file_reads_failed");
                }
                b.push(
                    r,
                    &d.pointer,
                    "file_read",
                    Builder::fields(vec![("path", path), ("outcome", json!(o))]),
                );
                return;
            }
            b.counts.add("file_edit_attempts");
            let attempt = b.push(
                r,
                &d.pointer,
                "file_edit_attempt",
                Builder::fields(vec![("path", path.clone()), ("outcome", json!(o))]),
            );
            match o {
                "succeeded" => {
                    b.counts.add("file_edits_confirmed");
                    let attempt_id = b.events[attempt]["id"].clone();
                    b.push(
                        r,
                        &d.pointer,
                        "file_edit_confirmed",
                        Builder::fields(vec![
                            ("attempt_event", attempt_id),
                            ("path", path),
                            ("outcome", json!("succeeded")),
                        ]),
                    );
                }
                "failed" => b.counts.add("file_edits_failed"),
                _ => {}
            }
        }
        Emit::Ignore | Emit::Each => unreachable!("expanded during interpretation"),
    }
}

fn finish(
    b: Builder,
    stream: &RawRecordStream,
    p: &Profile,
    working_directory: Option<String>,
    external_session_id: Option<String>,
    harness_version: Option<String>,
    times: Vec<String>,
) -> Normalized {
    let c = &b.counts;
    let mut tools_without: BTreeMap<String, u64> = BTreeMap::new();
    for e in b
        .events
        .iter()
        .filter(|e| e["kind"] == "tool_call" && e["operation"].is_null())
    {
        *tools_without
            .entry(e["tool"].as_str().unwrap_or_default().to_owned())
            .or_default() += 1;
    }
    let mut coverage = json!({
        "records_total": stream.records.len(),
        "records_valid_json": stream.valid(),
        "blank_lines": stream.blank_lines,
        "truncated_final_record": stream.truncated_final_record,
        "tools_without_semantics": tools_without,
        "pairing": p.pairing,
        "guarantees": p.guarantees,
    });
    for k in [
        "records_malformed",
        "records_recognized",
        "records_ignored",
        "records_unknown",
        "items_unknown",
        "items_ignored",
        "timestamps_present",
        "timestamps_invalid",
        "user_messages",
        "assistant_messages",
        "tool_calls",
        "tool_results",
        "tool_results_paired",
        "tool_results_unpaired",
        "tool_results_ambiguous",
        "tool_results_duplicate",
        "tool_calls_without_result",
        "tool_calls_without_semantics",
        "duplicate_call_ids",
        "missing_call_ids",
        "semantic_fields_missing",
        "patches_unparsed",
        "recorded_operations",
        "file_reads",
        "file_reads_failed",
        "file_edit_attempts",
        "file_edits_confirmed",
        "file_edits_failed",
        "commands",
        "commands_failed",
        "paths_repository",
        "paths_external",
        "paths_unresolved",
    ] {
        coverage[k] = json!(c.get(k));
    }
    let n = |k: &str| c.get(k);
    let mut limitations = Vec::new();
    let mut gap = |count: u64, one: &str, many: &str| {
        if count > 0 {
            limitations.push(if count == 1 {
                one.to_owned()
            } else {
                many.replace("{n}", &count.to_string())
            });
        }
    };
    gap(
        n("records_malformed"),
        "1 record is not valid JSON.",
        "{n} records are not valid JSON.",
    );
    gap(
        n("records_unknown") + n("items_unknown"),
        "1 record could not be interpreted by the profile.",
        "{n} records could not be interpreted by the profile.",
    );
    gap(
        n("tool_calls_without_result"),
        "1 tool call has no matching result; its outcome is unknown.",
        "{n} tool calls have no matching result; their outcomes are unknown.",
    );
    gap(
        n("tool_results_unpaired") + n("tool_results_ambiguous") + n("tool_results_duplicate"),
        "1 tool result could not be paired to exactly one earlier call.",
        "{n} tool results could not be paired to exactly one earlier call.",
    );
    gap(
        n("duplicate_call_ids"),
        "1 call ID is reused; its results stay unpaired.",
        "{n} calls reuse an ID; their results stay unpaired.",
    );
    gap(
        n("missing_call_ids"),
        "1 tool call has no call ID.",
        "{n} tool calls have no call ID.",
    );
    gap(
        n("semantic_fields_missing"),
        "1 recognized record lacks a field its rule needs.",
        "{n} recognized records lack a field their rule needs.",
    );
    gap(
        n("patches_unparsed"),
        "1 patch names no file in a supported form.",
        "{n} patches name no file in a supported form.",
    );
    gap(
        n("paths_external"),
        "1 referenced path is outside the repository.",
        "{n} referenced paths are outside the repository.",
    );
    gap(
        n("paths_unresolved"),
        "1 relative path has no recorded working directory.",
        "{n} relative paths have no recorded working directory.",
    );
    if stream.truncated_final_record {
        limitations.push("The final record is truncated; the session may have continued after the retained bytes.".into());
    }
    if !tools_without.is_empty() {
        limitations.push(format!(
            "Tools without deterministic semantics in this profile ({}) may have read or changed files; their effects are not reconstructed.",
            tools_without.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if p.guarantees
        .get("shell_side_effects")
        .is_some_and(|g| g == "unsupported")
        && n("commands") > 0
    {
        limitations.push(
            "Shell commands are recorded as commands; files they read or changed are not inferred."
                .into(),
        );
    }
    if n("timestamps_present") == 0 && !stream.records.is_empty() {
        limitations.push(
            "No record carries a usable timestamp; events are ordered by record ordinal only."
                .into(),
        );
    }
    let partial = n("records_malformed")
        + n("records_unknown")
        + n("items_unknown")
        + n("tool_calls_without_result")
        + n("tool_results_unpaired")
        + n("tool_results_ambiguous")
        + n("tool_results_duplicate")
        + n("duplicate_call_ids")
        + n("missing_call_ids")
        + n("semantic_fields_missing")
        + n("patches_unparsed")
        > 0
        || stream.truncated_final_record
        || stream.records.is_empty();
    let mut sorted = times;
    sorted.sort_by_key(|t| date_time(t).map(|d| (d.0, d.1)).unwrap_or_default());
    Normalized {
        external_session_id,
        harness_version,
        working_directory,
        started_at: sorted.first().cloned(),
        ended_at: sorted.last().cloned(),
        events: b.events,
        coverage,
        capture_state: if partial { "partial" } else { "complete" },
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_patch_headers_name_paths_once() {
        let p = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: b.md\n+hi\n*** Update File: src/a.rs\n*** End Patch";
        assert_eq!(
            patch_paths(p),
            vec![
                ("update", "src/a.rs".to_owned()),
                ("add", "b.md".to_owned())
            ]
        );
    }
}
