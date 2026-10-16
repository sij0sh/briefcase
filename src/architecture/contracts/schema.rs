use super::*;
pub fn schema(value: &Value, name: &str, definition: Option<&str>) -> Result<()> {
    static KNOWLEDGE: OnceLock<Value> = OnceLock::new();
    static BATCH: OnceLock<Value> = OnceLock::new();
    static QUERY: OnceLock<Value> = OnceLock::new();
    static SESSION_PROFILE: OnceLock<Value> = OnceLock::new();
    static SESSION: OnceLock<Value> = OnceLock::new();
    static DECLARATION: OnceLock<Value> = OnceLock::new();
    static SESSION_ACTIVITY: OnceLock<Value> = OnceLock::new();
    static TASK_CASE: OnceLock<Value> = OnceLock::new();
    static TASK_BRIEF: OnceLock<Value> = OnceLock::new();
    let (cell, raw) = match name {
        "task-case" => (&TASK_CASE, include_str!("../../../contracts/v1/task-case.schema.json")),
        "task-brief" => (&TASK_BRIEF, include_str!("../../../contracts/v1/task-brief.schema.json")),
        "knowledge" => (&KNOWLEDGE, include_str!("../../../contracts/v1/knowledge.schema.json")),
        "batch" => (&BATCH, include_str!("../../../contracts/v1/batch.schema.json")),
        "query" => (&QUERY, include_str!("../../../contracts/v1/query.schema.json")),
        "session-profile" => (&SESSION_PROFILE, include_str!("../../../contracts/v1/session-profile.schema.json")),
        "session" => (&SESSION, include_str!("../../../contracts/v1/session.schema.json")),
        "architecture-declaration" => (&DECLARATION, include_str!("../../../contracts/v1/architecture-declaration.schema.json")),
        "session-activity" => (&SESSION_ACTIVITY, include_str!("../../../contracts/v1/session-activity.schema.json")),
        _ => return Err("Unknown owned schema".into()),
    };
    let root = cell.get_or_init(|| serde_json::from_str(raw).expect("owned schema is valid JSON"));
    let node = definition.map(|n| &root["$defs"][n]).unwrap_or(root);
    structural(value, node, root, 0)
}
pub(crate) fn pattern_matches(pattern: &str, value: &str) -> Result<bool> {
    static PATTERNS: OnceLock<Mutex<BTreeMap<String, Regex>>> = OnceLock::new();
    let mut patterns = PATTERNS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| "SCHEMA: Pattern cache poisoned")?;
    if !patterns.contains_key(pattern) {
        patterns.insert(pattern.to_owned(), Regex::new(pattern)?);
    }
    Ok(patterns[pattern].is_match(value))
}
pub(crate) fn structural(v: &Value, s: &Value, root: &Value, depth: usize) -> Result<()> {
    require(depth <= 128, "SCHEMA", "Nesting limit exceeded")?;
    if let Some(r) = s["$ref"].as_str() {
        let name = r
            .strip_prefix("#/$defs/")
            .ok_or("SCHEMA: Nonlocal reference")?;
        return structural(v, &root["$defs"][name], root, depth + 1);
    }
    if !s["type"].is_null() {
        let ts = if s["type"].is_array() {
            strings(&s["type"])
        } else {
            vec![text(s, "type")]
        };
        let matches = ts.iter().any(|t| match t.as_str() {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => false,
        });
        require(matches, "SCHEMA", "Wrong JSON type")?;
    }
    if s.get("const").is_some() {
        require(v == &s["const"], "SCHEMA", "Constant mismatch")?;
    }
    if let Some(es) = s["enum"].as_array() {
        require(es.contains(v), "SCHEMA", "Enum mismatch")?;
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(choices) = s[key].as_array() {
            let count = choices
                .iter()
                .filter(|c| structural(v, c, root, depth + 1).is_ok())
                .count();
            require(
                count > 0 && (key != "oneOf" || count == 1),
                "SCHEMA",
                "Union mismatch",
            )?;
        }
    }
    if let Some(o) = v.as_object() {
        for k in strings(&s["required"]) {
            require(o.contains_key(&k), "SCHEMA", &format!("Missing {k}"))?;
        }
        for (k, item) in o {
            if let Some(child) = s["properties"].get(k) {
                structural(item, child, root, depth + 1)?;
            } else {
                require(
                    s["additionalProperties"] != false,
                    "SCHEMA",
                    &format!("Extra field {k}"),
                )?;
            }
        }
    }
    if let Some(a) = v.as_array() {
        require(
            a.len() >= s["minItems"].as_u64().unwrap_or(0) as usize,
            "SCHEMA",
            "Too few items",
        )?;
        require(
            a.len() <= s["maxItems"].as_u64().unwrap_or(u64::MAX) as usize,
            "SCHEMA",
            "Too many items",
        )?;
        for (i, item) in a.iter().enumerate() {
            if s["uniqueItems"] == true {
                require(!a[..i].contains(item), "SCHEMA", "Duplicate array item")?;
            }
            if !s["items"].is_null() {
                structural(item, &s["items"], root, depth + 1)?;
            }
        }
    }
    if let Some(t) = v.as_str() {
        require(
            t.chars().count() >= s["minLength"].as_u64().unwrap_or(0) as usize,
            "SCHEMA",
            "String too short",
        )?;
        require(
            t.chars().count() <= s["maxLength"].as_u64().unwrap_or(u64::MAX) as usize,
            "SCHEMA",
            "String too long",
        )?;
        if let Some(pattern) = s["pattern"].as_str() {
            require(pattern_matches(pattern, t)?, "SCHEMA", "Pattern mismatch")?;
        }
        if s["format"] == "date-time" {
            date_time(t)?;
        }
    }
    if let Some(n) = v.as_f64() {
        if let Some(min) = s["minimum"].as_f64() {
            require(n >= min, "SCHEMA", "Below minimum")?;
        }
        if let Some(max) = s["maximum"].as_f64() {
            require(n <= max, "SCHEMA", "Above maximum")?;
        }
    }
    Ok(())
}
pub(crate) fn date_time(value: &str) -> Result<(i64, String)> {
    let re =
        Regex::new(r"^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)$")?;
    let c = re.captures(value).ok_or("SCHEMA: Invalid date-time")?;
    let n = |i: usize| c[i].parse::<i64>().unwrap();
    let (year, month, day, hour, minute, second) = (n(1), n(2), n(3), n(4), n(5), n(6));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    require(
        year >= 1
            && (1..=12).contains(&month)
            && day >= 1
            && day <= months.get((month - 1) as usize).copied().unwrap_or(0)
            && hour < 24
            && minute < 60
            && second < 60,
        "SCHEMA",
        "Invalid calendar date/time",
    )?;
    let zone = &c[8];
    let offset = if zone == "Z" {
        0
    } else {
        let h = zone[1..3].parse::<i64>()?;
        let m = zone[4..6].parse::<i64>()?;
        require(h < 24 && m < 60, "SCHEMA", "Invalid UTC offset")?;
        (h * 60 + m) * 60 * if zone.starts_with('-') { -1 } else { 1 }
    };
    let prior = year - 1;
    let days = prior * 365 + prior / 4 - prior / 100
        + prior / 400
        + months[..(month - 1) as usize].iter().sum::<i64>()
        + day
        - 1;
    Ok((
        days * 86400 + hour * 3600 + minute * 60 + second - offset,
        c.get(7)
            .map(|s| s.as_str().trim_end_matches('0').to_owned())
            .unwrap_or_default(),
    ))
}
