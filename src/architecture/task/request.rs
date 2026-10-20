//! Pure request validation. Presentation does not participate in case identity.
use super::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct EvidenceScope {
    pub deployment_view: Option<String>,
    pub runtime_environment: Option<String>,
    pub runtime_from: Option<String>,
    pub runtime_to: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Request {
    pub task: String,
    pub duty: Duty,
    pub target: Option<String>,
    pub depth: usize,
    pub view: String,
    pub scope: EvidenceScope,
    #[serde(
        default = "default_max_files",
        skip_serializing_if = "is_default_max_files"
    )]
    pub max_files: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_facets: Vec<String>,
    #[serde(
        default = "default_selection_variant",
        skip_serializing_if = "is_default_selection_variant"
    )]
    pub selection_variant: String,
}
pub(super) const SELECTION_VARIANTS: &[&str] = &[
    "baseline",
    "role_diverse",
    "reserved_floor",
    "multi_query",
    "linked_diversity",
    "combined",
    "docs_disabled",
    // Priority 2 experiments; each changes one stage and keeps baseline selection.
    "identifier_terms",
    "origin_fusion",
    "structural_reach",
    "exact_keys",
];
fn default_selection_variant() -> String {
    "baseline".into()
}
fn is_default_selection_variant(value: &str) -> bool {
    value == "baseline"
}
fn default_max_files() -> usize {
    48
}
fn is_default_max_files(value: &usize) -> bool {
    *value == default_max_files()
}
impl Request {
    pub fn enabled(&self, facet: &str) -> bool {
        !self.exclude_facets.iter().any(|f| f == facet)
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Presentation {
    pub limit: usize,
    pub max_bytes: Option<usize>,
}

pub(super) fn parse(a: &Value) -> Result<(Request, Presentation)> {
    let operation =
        crate::architecture::service::find("brief").ok_or("TASK: Missing brief schema")?;
    crate::architecture::service::validate(&operation.schema, a)?;
    let task = a["task"].as_str().ok_or("ARGUMENTS: task is required")?;
    require(
        !task.trim().is_empty() && task.len() <= 16384,
        "ARGUMENTS",
        "task must contain 1..16384 bytes",
    )?;
    let duty: Duty = if a["duty"].is_null() {
        Duty::infer(task)
    } else {
        serde_json::from_value(a["duty"].clone())?
    };
    let scope = EvidenceScope {
        deployment_view: a["deployment_view"].as_str().map(str::to_owned),
        runtime_environment: a["runtime_environment"].as_str().map(str::to_owned),
        runtime_from: a["runtime_from"].as_str().map(str::to_owned),
        runtime_to: a["runtime_to"].as_str().map(str::to_owned),
    };
    let mut exclude_facets = crate::architecture::contracts::strings(&a["exclude_facets"]);
    exclude_facets.sort();
    require(
        exclude_facets
            .iter()
            .all(|f| super::retrieval::FACETS.contains(&f.as_str())),
        "ARGUMENTS",
        "Unknown retrieval facet",
    )?;
    let selection_variant = a["selection_variant"]
        .as_str()
        .unwrap_or("baseline")
        .to_owned();
    require(
        SELECTION_VARIANTS.contains(&selection_variant.as_str()),
        "ARGUMENTS",
        "Unknown selection variant",
    )?;
    let from = scope
        .runtime_from
        .as_deref()
        .map(crate::architecture::runtime_observations::seconds)
        .transpose()?;
    let to = scope
        .runtime_to
        .as_deref()
        .map(crate::architecture::runtime_observations::seconds)
        .transpose()?;
    require(
        from.zip(to).is_none_or(|(f, t)| f < t),
        "ARGUMENTS",
        "runtime_from must precede runtime_to",
    )?;
    Ok((
        Request {
            task: task.into(),
            duty,
            target: a["target"].as_str().map(str::to_owned),
            depth: a["depth"]
                .as_u64()
                .unwrap_or(duty.policy().trace.depth as u64) as usize,
            view: a["view"].as_str().unwrap_or("latest").into(),
            scope,
            max_files: a["max_files"]
                .as_u64()
                .unwrap_or(default_max_files() as u64) as usize,
            exclude_facets,
            selection_variant,
        },
        Presentation {
            limit: a["limit"].as_u64().unwrap_or(10) as usize,
            max_bytes: a["max_bytes"].as_u64().map(|n| n as usize),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_types_are_checked_without_evidence() {
        for extra in [
            json!({"limit":0}),
            json!({"limit":1001}),
            json!({"limit":"2"}),
            json!({"depth":0}),
            json!({"depth":4}),
            json!({"depth":-1}),
            json!({"max_bytes":255}),
            json!({"max_bytes":1048577}),
            json!({"duty":true}),
            json!({"runtime_from":"invalid"}),
            json!({"runtime_from":"2026-01-02T00:00:00Z","runtime_to":"2026-01-01T00:00:00Z"}),
        ] {
            let mut args = json!({"task":"src/lib.rs"});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(parse(&args).is_err(), "{args}");
        }
        for (limit, max_bytes) in [(1, 256), (1000, 1048576)] {
            let (_, p) = parse(&json!({"task":"x","limit":limit,"max_bytes":max_bytes})).unwrap();
            assert_eq!((p.limit, p.max_bytes), (limit, Some(max_bytes)));
        }
        assert!(parse(&json!({"task":"   "})).is_err());
        assert!(parse(&json!({"task":"é".repeat(8193)})).is_err());
    }
    #[test]
    fn presentation_is_separate_and_duty_ties_follow_enum_order() {
        let (a, _) =
            parse(&json!({"task":"review and debug src/lib.rs","limit":1,"max_bytes":256}))
                .unwrap();
        let (b, _) =
            parse(&json!({"task":"review and debug src/lib.rs","limit":1000,"max_bytes":1048576}))
                .unwrap();
        assert_eq!(a.duty, Duty::Debug);
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap()
        );
        assert_eq!(
            parse(&json!({"task":"what is this?"})).unwrap().0.duty,
            Duty::Explain
        );
    }
}
