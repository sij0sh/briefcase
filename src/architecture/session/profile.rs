//! Session profiles say what a harness record means. A profile is
//! declarative and deliberately small: JSON Pointer extraction, exact
//! condition matches, fixed tool-name → operation mappings and simple
//! outcome rules. There are no expressions and no embedded code; a harness
//! whose records cannot be described this way needs a narrowly scoped Rust
//! enricher, not a richer profile language
//! (`contracts/v1/session-profile.schema.json`).
use super::super::{contracts, identity, require, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Checked-in profiles. A profile change is a new version: it changes the
/// normalized projection and the session context, never the retained bytes.
pub const BUILTIN: &[(&str, &str)] = &[
    (
        "claude-code-v1",
        include_str!("../../../profiles/sessions/claude-code-v1.json"),
    ),
    (
        "codex-v1",
        include_str!("../../../profiles/sessions/codex-v1.json"),
    ),
    (
        "pi-v1",
        include_str!("../../../profiles/sessions/pi-v1.json"),
    ),
];

/// Guarantee levels a profile can declare for one semantic family.
pub const GUARANTEES: &[&str] = &[
    "exact",
    "supported_tools_only",
    "recorded_operations_only",
    "unsupported",
];
pub const GUARANTEE_KEYS: &[&str] = &[
    "user_messages",
    "assistant_messages",
    "tool_calls",
    "tool_results",
    "file_reads",
    "file_edits",
    "commands",
    "shell_side_effects",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: String,
    pub name: String,
    pub version: String,
    /// The harness whose records this profile describes (`codex`, `claude-code`, …).
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Record-level timestamp fields, tried in order.
    #[serde(default)]
    pub timestamp: Vec<Field>,
    /// Correlation identifiers supplied by the harness; never inferred.
    #[serde(default)]
    pub correlation: BTreeMap<String, Vec<Field>>,
    #[serde(default)]
    pub session: SessionFields,
    /// Only `call_id`: a result pairs with the one earlier call that has its
    /// exact ID. Adjacency is never assumed.
    pub pairing: String,
    /// Ordered: the first rule whose conditions all hold interprets a record.
    pub records: Vec<Rule>,
    /// Tool name → deterministic operation.
    #[serde(default)]
    pub tools: BTreeMap<String, Tool>,
    /// What this profile can guarantee per semantic family.
    pub guarantees: BTreeMap<String, String>,
}

/// Session metadata, taken from the first record in stream order that has it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFields {
    #[serde(default)]
    pub external_id: Vec<Field>,
    #[serde(default)]
    pub harness_version: Vec<Field>,
    #[serde(default)]
    pub cwd: Vec<Field>,
}

/// A JSON Pointer, `$key` (the member name while iterating an object), or a
/// pointer with fixed transforms.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Field {
    Pointer(String),
    Spec(FieldSpec),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    pub pointer: String,
    /// `json`: the value is a string holding one JSON document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip_prefix: Option<String>,
    /// For an array of strings: `last` element or `join` with spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub array: Option<String>,
}

/// Exactly one operator per condition.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub pointer: String,
    /// `record`: inside an each rule, read the enclosing record instead of
    /// the current element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    /// Holds when the value is absent or differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<Value>,
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    /// `string`, `array`, `object`, `number`, `boolean` or `null`.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub json_type: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emit {
    UserMessage,
    AssistantMessage,
    ToolCall,
    ToolResult,
    /// A self-contained recorded command with its own outcome.
    Command,
    /// A self-contained recorded file read with its own outcome.
    FileRead,
    /// A self-contained recorded file change with its own outcome.
    FileEdit,
    /// Recognized and deliberately not interpreted.
    Ignore,
    /// Apply `rules` to each element of `each` or each member of `each_key`.
    Each,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub when: Vec<Condition>,
    pub emit: Emit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<Field>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub succeeded_when: Vec<Condition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_when: Vec<Condition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    FileRead,
    FileEdit,
    Command,
    /// A patch document parsed deterministically for the paths it names.
    Patch,
}

/// What a named tool's call means. Fields resolve inside the call's input.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<Field>,
    /// Patch syntax; only `apply_patch` (`*** Update File: <path>` headers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// A loaded profile with its identity.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub profile: Profile,
    /// Digest of the canonical profile document.
    pub digest: String,
}

/// Load a checked-in profile by name or a profile document from a path.
pub fn load(selector: &str) -> Result<Loaded> {
    let path = std::path::Path::new(selector);
    if path.is_file() {
        return parse(&std::fs::read(path)?);
    }
    let (_, raw) = BUILTIN
        .iter()
        .find(|(name, _)| *name == selector || name.strip_suffix("-v1") == Some(selector))
        .ok_or_else(|| {
            format!(
                "PROFILE: {selector} is neither a profile file nor a built-in profile ({})",
                BUILTIN
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    parse(raw.as_bytes())
}

pub fn parse(bytes: &[u8]) -> Result<Loaded> {
    let value = identity::parse(bytes)?;
    contracts::schema(&value, "session-profile", None)?;
    let profile: Profile = serde_json::from_value(value.clone())?;
    validate(&profile)?;
    Ok(Loaded {
        digest: identity::digest(&value)?,
        profile,
    })
}

fn pointer_ok(p: &str) -> bool {
    p.is_empty() || p.starts_with('/')
}
fn field_ok(f: &Field, in_object: bool) -> bool {
    match f {
        Field::Pointer(p) => pointer_ok(p) || (in_object && p == "$key"),
        Field::Spec(s) => {
            pointer_ok(&s.pointer)
                && s.decode.as_deref().is_none_or(|d| d == "json")
                && s.array
                    .as_deref()
                    .is_none_or(|a| a == "last" || a == "join")
        }
    }
}
fn condition_ok(c: &Condition) -> bool {
    let operators = [
        c.equals.is_some(),
        c.not_equals.is_some(),
        c.one_of.is_some(),
        c.exists.is_some(),
        c.json_type.is_some(),
    ];
    pointer_ok(&c.pointer)
        && c.scope.as_deref().is_none_or(|s| s == "record")
        && operators.iter().filter(|o| **o).count() == 1
        && c.json_type
            .as_deref()
            .is_none_or(|t| ["string", "array", "object", "number", "boolean", "null"].contains(&t))
}

fn validate_rule(r: &Rule, depth: usize, in_object: bool) -> Result<()> {
    let fail = |m: &str| -> Result<()> { Err(format!("PROFILE: {m}").into()) };
    for c in r.when.iter().chain(&r.succeeded_when).chain(&r.failed_when) {
        if !condition_ok(c) {
            return fail("A condition needs a JSON Pointer and exactly one operator");
        }
    }
    let fields = [
        &r.text,
        &r.call_id,
        &r.tool,
        &r.input,
        &r.path,
        &r.command,
        &r.cwd,
        &r.exit_code,
    ];
    if fields
        .iter()
        .flat_map(|f| f.iter())
        .any(|f| !field_ok(f, in_object))
    {
        return fail(
            "Invalid field (JSON Pointer, $key inside each_key, decode json, array last|join)",
        );
    }
    let need = |f: &Option<Field>, name: &str| -> Result<()> {
        if f.is_none() {
            return fail(&format!("{:?} rules need {name}", r.emit));
        }
        Ok(())
    };
    match r.emit {
        Emit::UserMessage | Emit::AssistantMessage => need(&r.text, "text")?,
        Emit::ToolCall => {
            need(&r.call_id, "call_id")?;
            need(&r.tool, "tool")?;
        }
        Emit::ToolResult => need(&r.call_id, "call_id")?,
        Emit::Command => need(&r.command, "command")?,
        Emit::FileRead | Emit::FileEdit => need(&r.path, "path")?,
        Emit::Ignore => {}
        Emit::Each => {
            require(depth == 0, "PROFILE", "each rules cannot nest")?;
            require(
                r.each.is_some() != r.each_key.is_some()
                    && r.each.iter().chain(&r.each_key).all(|p| pointer_ok(p)),
                "PROFILE",
                "An each rule names exactly one of each (array) or each_key (object)",
            )?;
            require(!r.rules.is_empty(), "PROFILE", "An each rule needs rules")?;
            for inner in &r.rules {
                validate_rule(inner, depth + 1, r.each_key.is_some())?;
            }
        }
    }
    require(
        r.emit == Emit::Each || (r.rules.is_empty() && r.each.is_none() && r.each_key.is_none()),
        "PROFILE",
        "Only each rules carry each, each_key or rules",
    )?;
    require(
        (r.succeeded_when.is_empty() && r.failed_when.is_empty())
            || matches!(
                r.emit,
                Emit::ToolResult | Emit::Command | Emit::FileRead | Emit::FileEdit
            ),
        "PROFILE",
        "Outcome rules belong to results and self-contained operations",
    )?;
    Ok(())
}

pub fn validate(p: &Profile) -> Result<()> {
    require(
        p.schema_version == "1",
        "PROFILE",
        "schema_version must be 1",
    )?;
    for (key, fields) in &p.correlation {
        require(
            [
                "external_turn_id",
                "external_message_id",
                "external_parent_id",
            ]
            .contains(&key.as_str())
                && fields.iter().all(|f| field_ok(f, false)),
            "PROFILE",
            "Invalid correlation field",
        )?;
    }
    require(
        !p.name.is_empty() && !p.version.is_empty() && !p.harness.is_empty(),
        "PROFILE",
        "name, version and harness are required",
    )?;
    require(
        p.pairing == "call_id",
        "PROFILE",
        "pairing must be call_id: results pair only by exact call ID",
    )?;
    for f in p
        .timestamp
        .iter()
        .chain(&p.session.external_id)
        .chain(&p.session.harness_version)
        .chain(&p.session.cwd)
    {
        require(field_ok(f, false), "PROFILE", "Invalid record-level field")?;
    }
    for r in &p.records {
        validate_rule(r, 0, false)?;
    }
    for (name, t) in &p.tools {
        let ok = match t.operation {
            Operation::FileRead | Operation::FileEdit => t.path.is_some(),
            Operation::Command => t.command.is_some(),
            Operation::Patch => t.patch.is_some() && t.format.as_deref() == Some("apply_patch"),
        };
        require(
            ok && [&t.path, &t.command, &t.cwd, &t.patch]
                .iter()
                .flat_map(|f| f.iter())
                .all(|f| field_ok(f, false)),
            "PROFILE",
            &format!("Tool {name}: file operations need path, command needs command, patch needs patch with format apply_patch"),
        )?;
    }
    for (k, v) in &p.guarantees {
        require(
            GUARANTEE_KEYS.contains(&k.as_str()) && GUARANTEES.contains(&v.as_str()),
            "PROFILE",
            &format!("Unknown guarantee {k}: {v}"),
        )?;
    }
    require(
        GUARANTEE_KEYS.iter().all(|k| p.guarantees.contains_key(*k)),
        "PROFILE",
        &format!(
            "Every guarantee family must be declared: {}",
            GUARANTEE_KEYS.join(", ")
        ),
    )?;
    Ok(())
}

// ---- Evaluation -------------------------------------------------------------------

/// Read a field from `item` (or the member name for `$key`).
pub fn eval(f: &Field, item: &Value, key: Option<&str>) -> Option<Value> {
    let (pointer, spec) = match f {
        Field::Pointer(p) if p == "$key" => return key.map(|k| Value::String(k.to_owned())),
        Field::Pointer(p) => (p.as_str(), None),
        Field::Spec(s) => (s.pointer.as_str(), Some(s)),
    };
    let mut v = item.pointer(pointer)?.clone();
    let Some(s) = spec else { return Some(v) };
    if s.decode.is_some() {
        v = identity::parse(v.as_str()?.as_bytes()).ok()?;
    }
    if let Some(mode) = &s.array {
        let parts: Vec<&str> = v.as_array()?.iter().filter_map(Value::as_str).collect();
        v = Value::String(match mode.as_str() {
            "last" => parts.last()?.to_string(),
            _ => parts.join(" "),
        });
    }
    if let Some(prefix) = &s.strip_prefix {
        if let Some(text) = v.as_str() {
            v = Value::String(
                text.strip_prefix(prefix.as_str())
                    .unwrap_or(text)
                    .to_owned(),
            );
        }
    }
    Some(v)
}
/// A field as text: strings verbatim, numbers and booleans spelled out.
pub fn eval_text(f: &Option<Field>, item: &Value, key: Option<&str>) -> Option<String> {
    match eval(f.as_ref()?, item, key)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
pub fn first_text(fields: &[Field], item: &Value) -> Option<String> {
    fields
        .iter()
        .find_map(|f| eval_text(&Some(f.clone()), item, None))
        .filter(|s| !s.is_empty())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
    }
}
pub fn holds(c: &Condition, item: &Value, record: &Value) -> bool {
    let target = if c.scope.is_some() { record } else { item };
    let v = target.pointer(&c.pointer);
    if let Some(e) = &c.equals {
        return v == Some(e);
    }
    if let Some(e) = &c.not_equals {
        return v != Some(e);
    }
    if let Some(xs) = &c.one_of {
        return v.is_some_and(|v| xs.contains(v));
    }
    if let Some(e) = c.exists {
        return v.is_some() == e;
    }
    if let Some(t) = &c.json_type {
        return v.is_some_and(|v| type_name(v) == t);
    }
    false
}
pub fn all(cs: &[Condition], item: &Value, record: &Value) -> bool {
    cs.iter().all(|c| holds(c, item, record))
}
