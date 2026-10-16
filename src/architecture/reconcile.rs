//! A shared, read-only definition of alignment. Events are only hints.
use super::{session::Harness, sync, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentState {
    Aligned,
    Dirty,
    Uninitialized,
    Unavailable,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alignment {
    pub source: String,
    pub state: AlignmentState,
    pub observed_revision: Option<String>,
    pub applied_revision: Option<String>,
    pub diagnostics: Vec<String>,
    pub detail: Value,
}
impl Alignment {
    pub fn new(source: &str, observed: Option<String>, applied: Option<String>) -> Self {
        let state = if observed == applied {
            AlignmentState::Aligned
        } else if applied.is_none() {
            AlignmentState::Uninitialized
        } else {
            AlignmentState::Dirty
        };
        Self {
            source: source.into(),
            state,
            observed_revision: observed,
            applied_revision: applied,
            diagnostics: Vec::new(),
            detail: json!({}),
        }
    }
    pub fn unavailable(source: &str, detail: Value) -> Self {
        Self {
            state: AlignmentState::Unavailable,
            diagnostics: [detail["message"]
                .as_str()
                .or(detail["error"].as_str())
                .unwrap_or("Local source unavailable")
                .to_owned()]
            .into(),
            detail,
            ..Self::new(source, None, None)
        }
    }
    pub fn dirty(&self) -> bool {
        matches!(
            self.state,
            AlignmentState::Dirty | AlignmentState::Uninitialized
        )
    }
}
/// Durable requests union hints; the worker rechecks them before applying anything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirtySet {
    pub project: bool,
    pub git: bool,
    pub sessions: BTreeSet<Harness>,
}
impl DirtySet {
    pub fn all() -> Self {
        Self {
            project: true,
            git: true,
            sessions: Harness::all().into(),
        }
    }
    pub fn union(&mut self, other: &Self) {
        self.project |= other.project;
        self.git |= other.git;
        self.sessions.extend(&other.sessions);
    }
    pub fn is_empty(&self) -> bool {
        !self.project && !self.git && self.sessions.is_empty()
    }
}
pub fn check(o: &sync::SyncOptions, domains: &DirtySet) -> Result<Vec<Alignment>> {
    if let Err(error) = sync::validate_database(&o.root, &o.db) {
        return Ok(vec![Alignment::unavailable(
            "project",
            json!({"sync":"failed","error":error.to_string()}),
        )]);
    }
    let mut alignments = Vec::new();
    if domains.project {
        alignments.push(sync::project::check(o).unwrap_or_else(|e| {
            Alignment::unavailable("project", json!({"sync":"failed","error":e.to_string()}))
        }));
    }
    if domains.git {
        alignments.push(
            sync::git::check(&o.root, &o.db)
                .unwrap_or_else(|e| Alignment::unavailable("git", json!({"error":e.to_string()}))),
        );
    }
    for harness in &domains.sessions {
        alignments.push(
            sync::sessions::check(&o.root, &o.db, *harness).unwrap_or_else(|e| {
                Alignment::unavailable(
                    &format!("sessions:{}", harness.name()),
                    json!({"error":e.to_string()}),
                )
            }),
        );
    }
    Ok(alignments)
}
pub fn dirty(alignments: &[Alignment]) -> DirtySet {
    let mut dirty = DirtySet::default();
    for a in alignments.iter().filter(|a| a.dirty()) {
        match a.source.as_str() {
            "project" => dirty.project = true,
            "git" => dirty.git = true,
            s => {
                if let Some(h) = s.strip_prefix("sessions:").and_then(|h| h.parse().ok()) {
                    dirty.sessions.insert(h);
                }
            }
        }
    }
    dirty
}

fn yes() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DomainPolicy {
    pub project: bool,
    pub git: bool,
    pub sessions: SessionPolicy,
}
impl Default for DomainPolicy {
    fn default() -> Self {
        Self {
            project: true,
            git: true,
            sessions: SessionPolicy::default(),
        }
    }
}
/// Per-harness session sync switches, keyed by registry name. Every
/// registered harness is enabled unless switched off; unknown names are
/// rejected. `muse` is reserved and must stay off until Muse is registered.
#[derive(Default, Deserialize)]
#[serde(try_from = "std::collections::BTreeMap<String, bool>")]
pub struct SessionPolicy {
    disabled: BTreeSet<Harness>,
    muse: bool,
}
impl TryFrom<std::collections::BTreeMap<String, bool>> for SessionPolicy {
    type Error = String;
    fn try_from(
        map: std::collections::BTreeMap<String, bool>,
    ) -> std::result::Result<Self, String> {
        let mut policy = Self::default();
        for (name, on) in map {
            if name == "muse" {
                policy.muse = on;
            } else if !on {
                policy.disabled.insert(name.parse()?);
            } else {
                name.parse::<Harness>()?;
            }
        }
        Ok(policy)
    }
}
impl SessionPolicy {
    pub fn enabled(&self, harness: Harness) -> bool {
        !self.disabled.contains(&harness)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchPolicy {
    #[serde(default = "yes")]
    pub enabled: bool,
}
impl Default for WatchPolicy {
    fn default() -> Self {
        Self { enabled: true }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct LocalConfig {
    sync: DomainPolicy,
    watch: WatchPolicy,
}
fn configuration(root: &std::path::Path) -> Result<LocalConfig> {
    let path = root.join(".briefcase/sync.json");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LocalConfig::default()),
        Err(e) => Err(e.into()),
    }
}
impl DomainPolicy {
    pub fn validate(&self) -> Result<()> {
        super::require(
            !self.sessions.muse,
            "NEEDS_CONFIGURATION",
            "Muse transcript synchronization is not supported",
        )
    }
}
pub fn configured_domains(root: &std::path::Path) -> Result<DirtySet> {
    let p = configuration(root)?.sync;
    p.validate()?;
    let mut domains = DirtySet {
        project: p.project,
        git: p.git,
        sessions: BTreeSet::new(),
    };
    for h in Harness::all() {
        if p.sessions.enabled(h) {
            domains.sessions.insert(h);
        }
    }
    Ok(domains)
}
pub fn watch_enabled(root: &std::path::Path) -> Result<bool> {
    Ok(configuration(root)?.watch.enabled)
}
impl DirtySet {
    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            project: self.project && other.project,
            git: self.git && other.git,
            sessions: self
                .sessions
                .intersection(&other.sessions)
                .copied()
                .collect(),
        }
    }
}

#[cfg(test)]
mod session_policy_tests {
    use super::*;

    fn parse(json: &str) -> std::result::Result<SessionPolicy, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn every_registered_harness_is_enabled_by_default_and_switchable_by_name() {
        let p = parse("{}").unwrap();
        assert!(Harness::all().into_iter().all(|h| p.enabled(h)));
        let p = parse(r#"{"claude-code":false,"pi":true}"#).unwrap();
        assert!(!p.enabled(Harness::ClaudeCode));
        assert!(p.enabled(Harness::Pi) && p.enabled(Harness::Codex));
    }

    #[test]
    fn unknown_names_are_rejected_and_muse_stays_reserved() {
        assert!(parse(r#"{"opencode":false}"#).is_err());
        assert!(parse(r#"{"muse":false}"#).is_ok());
        let policy = DomainPolicy {
            project: true,
            git: true,
            sessions: parse(r#"{"muse":true}"#).unwrap(),
        };
        assert!(policy.validate().is_err());
    }
}
