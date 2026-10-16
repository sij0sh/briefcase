//! The single duty vocabulary, routing policy, and task coverage templates.
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Duty {
    Debug,
    Refactor,
    Impact,
    Extend,
    Continue,
    Explain,
    History,
    Run,
    Review,
    Test,
}

pub struct DutyPolicy {
    pub trace: TracePolicy,
    pub case: CasePolicy,
    pub cues: &'static [&'static str],
}
pub struct TracePolicy {
    pub predicates: &'static [&'static str],
    pub depth: usize,
}
pub struct CasePolicy {
    pub required_roles: &'static [&'static str],
    pub history_required: bool,
    pub sessions_required: bool,
    pub external_required: bool,
    pub test_execution_required: bool,
    pub plan_outcomes_required: bool,
}
const CODE: &[&str] = &["impl", "test", "config", "script", "build", "install", "doc", "adr", "plan", "fixture"];
const RELATIONSHIPS: &[&str] = &[
    "TESTS", "REFERENCES_ARTIFACT", "IMPLEMENTS_PLAN_SECTION", "DEPENDS_ON", "IMPORTS",
    "REFERENCES", "CALLS_STATIC", "ROUTES_TO", "READS", "WRITES", "DERIVES_FROM",
    "BUILDS", "DEPLOYS_AS", "CONNECTS_TO", "CALLS_OBSERVED", "CONNECTION_OBSERVED",
    "MAPS_TO_SERVICE", "MAPS_TO", "TRANSFORMS", "DECLARES_CONNECTION", "INCLUDES_SOURCE", "READS_CONFIG", "DECLARES_CONFIG_KEY", "INSTALLS_TO",
];
impl Duty {
    pub fn names() -> Vec<String> {
        Self::value_variants().iter().map(|d| serde_json::to_value(d).unwrap().as_str().unwrap().to_owned()).collect()
    }
    pub fn policy(self) -> DutyPolicy {
        let (roles, cues): (&'static [&'static str], &'static [&'static str]) = match self {
            Self::Debug => (&["impl", "test", "config"], &["debug", "error", "fails", "failure", "broken", "bug"]),
            Self::Refactor => (CODE, &["rename", "refactor", "move "]),
            Self::Impact => (CODE, &["impact", "blast radius", "dependents", "callers"]),
            Self::Extend => (CODE, &["add ", "extend", "new provider", "new plugin"]),
            Self::Continue => (&["plan", "impl", "test"], &["continue", "unfinished", "resume", "plan progress"]),
            Self::Explain => (&["impl", "doc"], &[]),
            Self::History => (&["impl", "doc", "adr"], &["history", "why was", "when did", "commit"]),
            Self::Run => (&["config", "script", "build", "install", "doc"], &["configure", "how to run", "how do i run", "install"]),
            Self::Review => (CODE, &["review", "patch complete", "missed", "completeness"]),
            Self::Test => (&["impl", "test", "fixture"], &["which tests", "test coverage", "tests for", "test this"]),
        };
        DutyPolicy {
            trace: TracePolicy { predicates: RELATIONSHIPS,
                depth: if matches!(self,Self::Explain|Self::History|Self::Test) {1}else{2} },
            case: CasePolicy {
            required_roles: roles,
            history_required: !matches!(self, Self::Run | Self::Test),
            sessions_required: matches!(self, Self::Debug | Self::Refactor | Self::Continue | Self::Review | Self::History | Self::Extend),
            external_required: matches!(self, Self::Refactor | Self::Impact | Self::Run | Self::Debug | Self::Review),
            test_execution_required: matches!(self, Self::Debug | Self::Refactor | Self::Extend | Self::Review | Self::Test),
            plan_outcomes_required: matches!(self, Self::Continue | Self::Review),
            },
            cues,
        }
    }
    pub fn infer(task: &str) -> Self {
        let lower = task.to_lowercase();
        // The enum order is the documented tie-breaker; all transports use it.
        Self::value_variants().iter().copied()
            .find(|d| d.policy().cues.iter().any(|cue| lower.contains(cue)))
            .unwrap_or(Self::Explain)
    }
}
