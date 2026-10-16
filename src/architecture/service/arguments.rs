//! Shared argument aliases and validation before any operation runs.
use crate::architecture::Result;
use serde_json::Value;

/// Argument spellings callers reach for first, mapped to the canonical
/// name. An alias applies only when the operation's schema defines the
/// canonical name and does not define the alias itself, so operations with
/// their own `query` argument (`describe`) are never rewritten.
const ARGUMENT_ALIASES: &[(&str, &str)] = &[
    ("query", "task"),
    ("text", "task"),
    ("query", "text"),
    ("query", "question"),
    ("text", "question"),
    ("explanation_handle", "handle"),
];

/// Rewrite known alias keys to their canonical names before validation.
/// The canonical spelling wins when both are present; nulls stay absent.
pub(crate) fn normalize_aliases(schema: &Value, args: &Value) -> Value {
    let mut normalized = args.clone();
    let Some(properties) = schema["properties"].as_object() else {
        return normalized;
    };
    let Some(object) = normalized.as_object_mut() else {
        return normalized;
    };
    for (alias, canonical) in ARGUMENT_ALIASES {
        if !properties.contains_key(*canonical) || properties.contains_key(*alias) {
            continue;
        }
        if let Some(value) = object.remove(*alias) {
            if !value.is_null() && object.get(*canonical).is_none_or(Value::is_null) {
                object.insert((*canonical).into(), value);
            }
        }
    }
    normalized
}

/// Reject unknown arguments, wrong types and missing requirements before
/// any query runs. Nulls are treated as absent.
pub(crate) fn validate(schema: &Value, args: &Value) -> Result<()> {
    let empty = serde_json::Map::new();
    let object = match args {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err("ARGUMENTS: Arguments must be a JSON object".into()),
    };
    for (key, value) in object.iter().filter(|(_, v)| !v.is_null()) {
        let property = schema["properties"].get(key).ok_or_else(|| {
            let mut valid: Vec<_> = schema["properties"]
                .as_object()
                .map(|p| p.keys().map(String::as_str).collect())
                .unwrap_or_default();
            valid.sort_unstable();
            format!(
                "ARGUMENTS: Unknown argument {key}; valid arguments are: {}",
                valid.join(", ")
            )
        })?;
        let ok = match property["type"].as_str() {
            Some("string") => value.as_str().is_some_and(|s| {
                let chars=s.chars().count() as u64;
                property["minLength"].as_u64().is_none_or(|min| chars>=min)
                    && property["maxLength"].as_u64().is_none_or(|max| chars<=max)
                    && property["max_utf8_bytes"].as_u64().is_none_or(|max| s.len() as u64<=max)
                    && property["pattern"].as_str().is_none_or(|pattern| {
                        regex::Regex::new(pattern).is_ok_and(|re| re.is_match(s))
                    })
            }),
            Some("boolean") => value.is_boolean(),
            Some("object") => value.is_object(),
            Some("integer") => value.as_u64().is_some_and(|n| {
                property["minimum"].as_u64().is_none_or(|m| n >= m)
                    && property["maximum"].as_u64().is_none_or(|m| n <= m)
            }),
            _ => false,
        };
        if !ok || property["enum"].as_array().is_some_and(|choices| !choices.contains(value)) {
            return Err(format!("ARGUMENTS: Invalid value for {key}").into());
        }
    }
    for key in schema["required"].as_array().into_iter().flatten() {
        let key = key.as_str().unwrap_or("");
        if object.get(key).is_none_or(Value::is_null) {
            return Err(format!("ARGUMENTS: {key} is required").into());
        }
    }
    Ok(())
}
