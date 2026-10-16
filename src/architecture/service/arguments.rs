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

/// Validate the owned argument-schema vocabulary, including recursive arrays.
fn value_matches(property: &Value, value: &Value, depth: usize) -> bool {
    if depth > 32 { return false; }
    let valid = match property["type"].as_str() {
        Some("string") => value.as_str().is_some_and(|s| {
            let chars = s.chars().count() as u64;
            property["minLength"].as_u64().is_none_or(|min| chars >= min)
                && property["maxLength"].as_u64().is_none_or(|max| chars <= max)
                && property["max_utf8_bytes"].as_u64().is_none_or(|max| s.len() as u64 <= max)
                && property["pattern"].as_str().is_none_or(|pattern| {
                    regex::Regex::new(pattern).is_ok_and(|re| re.is_match(s))
                })
        }),
        Some("boolean") => value.is_boolean(),
        Some("object") => value.is_object(),
        Some("integer") => value.as_u64().is_some_and(|n| {
            property["minimum"].as_u64().is_none_or(|min| n >= min)
                && property["maximum"].as_u64().is_none_or(|max| n <= max)
        }),
        Some("array") => value.as_array().is_some_and(|items| {
            property["minItems"].as_u64().is_none_or(|min| items.len() as u64 >= min)
                && property["maxItems"].as_u64().is_none_or(|max| items.len() as u64 <= max)
                && items.iter().enumerate().all(|(index, item)| {
                    (property["uniqueItems"] != true || !items[..index].contains(item))
                        && property.get("items").is_none_or(|schema| value_matches(schema, item, depth + 1))
                })
        }),
        None => property.is_object() && property.get("type").is_none(),
        _ => false,
    };
    valid && property["enum"].as_array().is_none_or(|choices| choices.contains(value))
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
        if !value_matches(property, value, 0) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn facet_arrays_validate_items_enums_and_uniqueness_before_any_read() {
        let schema=crate::architecture::service::find("brief").unwrap().schema;
        for facets in [json!([]),json!(["lexical"]),json!(["tests","history","runtime"])] {
            validate(&schema,&json!({"task":"repair storage","exclude_facets":facets})).unwrap();
        }
        for facets in [json!("tests"),json!(["unknown"]),json!([1]),json!([true]),json!([null]),
            json!([{}]),json!([["tests"]]),json!(["tests","tests"])] {
            assert!(validate(&schema,&json!({"task":"repair storage","exclude_facets":facets})).is_err(),"{facets}");
        }
    }
    #[test]
    fn recursive_array_item_constraints_are_not_silently_ignored() {
        let schema=json!({"properties":{"values":{"type":"array","minItems":1,"maxItems":2,"uniqueItems":true,
            "items":{"type":"array","items":{"enum":["a","b"]}}}}});
        validate(&schema,&json!({"values":[["a"],["b"]]})).unwrap();
        for values in [json!([]),json!([["a"],["b"],["a","b"]]),json!([["c"]]),json!([["a"],["a"]])] {
            assert!(validate(&schema,&json!({"values":values})).is_err());
        }
    }
}
