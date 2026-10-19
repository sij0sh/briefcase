//! Deterministic episode views and the three initial session analyses
//! (Phase 2, change set 05). Pure over normalized `session.v1` events:
//! identical pinned events and rules produce identical drafts.
//!
//! Episodes are bounded by recorded user turns. When harness correlation
//! (`external_message_id` / `external_parent_id`) is present, an event's
//! turn is its nearest user-message ancestor, and a recovery pair must lie
//! on one branch: the success must descend from the failure. Without
//! correlation, record order within the turn is the only claim made.
//!
//! Every claim is an observation about the recording. A success after an
//! intervention does not establish that the intervention caused or was
//! necessary for it; an attributed user span does not become a current
//! instruction; an environment value is never copied (keys only, values
//! redacted). Unsupported (`unknown`) events are coverage gaps, never
//! inferred side effects.
use super::paths;
use crate::architecture::contracts::{rows, text};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const PRODUCER: &str = "session-findings-v1";
pub const RULE_FAILURE_RECOVERY: &str = "session-failure-recovery-v1";
pub const RULE_REVIEW_REQUIREMENT: &str = "session-review-requirement-v1";
pub const RULE_ENVIRONMENT: &str = "session-environment-observation-v1";
pub const MAX_FINDINGS_PER_SESSION: usize = 8;
const MAX_SPAN_CHARS: usize = 800;

fn kind(e: &Value) -> &str {
    e["kind"].as_str().unwrap_or("")
}
fn id(e: &Value) -> String {
    text(e, "id")
}
fn ordinal(e: &Value) -> u64 {
    e["ordinal"].as_u64().unwrap_or(0)
}

/// One episode: the events under one recorded user turn (or the prelude
/// before the first user message), in record order.
struct Episode {
    #[allow(dead_code)]
    key: String,
    turn_event: Option<String>,
    branch_parent: Option<String>,
    events: Vec<usize>,
}

/// Message ancestry from harness correlation fields, when recorded.
struct Ancestry {
    parent: BTreeMap<String, String>,
    user_messages: BTreeSet<String>,
    message_of: Vec<Option<String>>,
}

impl Ancestry {
    fn build(events: &[Value]) -> Self {
        let mut parent = BTreeMap::new();
        let mut user_messages = BTreeSet::new();
        let mut message_of = Vec::with_capacity(events.len());
        for e in events {
            let message = e["external_message_id"].as_str().map(str::to_owned);
            if let (Some(m), Some(p)) = (&message, e["external_parent_id"].as_str()) {
                parent.entry(m.clone()).or_insert_with(|| p.to_owned());
            }
            if kind(e) == "user_message" {
                if let Some(m) = &message {
                    user_messages.insert(m.clone());
                }
            }
            message_of.push(message);
        }
        Self {
            parent,
            user_messages,
            message_of,
        }
    }
    /// Nearest user-message ancestor of a message, itself included.
    fn turn_of(&self, message: &str) -> Option<String> {
        let mut current = Some(message.to_owned());
        for _ in 0..64 {
            let m = current?;
            if self.user_messages.contains(&m) {
                return Some(m);
            }
            current = self.parent.get(&m).cloned();
        }
        None
    }
    /// True when `descendant`'s recorded ancestry passes through `ancestor`.
    fn descends_from(&self, descendant: &str, ancestor: &str) -> bool {
        let mut current = Some(descendant.to_owned());
        for _ in 0..64 {
            let Some(m) = current else { return false };
            if m == ancestor {
                return true;
            }
            current = self.parent.get(&m).cloned();
        }
        false
    }
    fn has_correlation(&self) -> bool {
        !self.parent.is_empty()
    }
}

fn episodes(events: &[Value]) -> Vec<Episode> {
    let ancestry = Ancestry::build(events);
    let user_event_by_message: BTreeMap<String, String> = events
        .iter()
        .filter(|e| kind(e) == "user_message")
        .filter_map(|e| {
            e["external_message_id"]
                .as_str()
                .map(|m| (m.to_owned(), id(e)))
        })
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut by_key: BTreeMap<String, Episode> = BTreeMap::new();
    let mut current_key = "prelude".to_owned();
    for (index, e) in events.iter().enumerate() {
        if kind(e) == "user_message" {
            current_key = format!("turn:{}", id(e));
        } else if ancestry.has_correlation() {
            if let Some(message) = &ancestry.message_of[index] {
                if let Some(turn) = ancestry.turn_of(message) {
                    if let Some(event_id) = user_event_by_message.get(&turn) {
                        current_key = format!("turn:{event_id}");
                    }
                }
            }
        }
        let entry = by_key.entry(current_key.clone()).or_insert_with(|| {
            order.push(current_key.clone());
            let turn_event = current_key.strip_prefix("turn:").map(str::to_owned);
            let branch_parent = turn_event
                .as_ref()
                .and_then(|t| events.iter().find(|e| id(e) == *t))
                .and_then(|e| e["external_parent_id"].as_str().map(str::to_owned));
            Episode {
                key: current_key.clone(),
                turn_event,
                branch_parent,
                events: Vec::new(),
            }
        });
        entry.events.push(index);
    }
    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

/// Leading `KEY=value` assignments (optionally after `env`/`export`) and the
/// remaining core command. Values are dropped here and never reappear.
fn parse_command(raw: &str) -> Option<(Vec<String>, String)> {
    let words = paths::direct_command(raw)?;
    let mut keys = Vec::new();
    let mut i = 0;
    if words.get(i).map(String::as_str) == Some("env") {
        i += 1;
    }
    let exported = words.get(i).map(String::as_str) == Some("export");
    if exported {
        i += 1;
    }
    while let Some(word) = words.get(i) {
        let Some((key, _)) = word.split_once('=') else {
            break;
        };
        if key.is_empty()
            || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || key.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            break;
        }
        keys.push(key.to_owned());
        i += 1;
    }
    if exported && keys.is_empty() {
        return Some((keys, words.join(" ")));
    }
    let core: Vec<String> = words[i..]
        .iter()
        .map(|w| match w.split_once('=') {
            // Redact values of key-bearing arguments as well as assignments.
            Some((k, _))
                if k.len() > 1
                    && k.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)) =>
            {
                format!("{k}=<redacted>")
            }
            _ => w.clone(),
        })
        .collect();
    Some((keys, core.join(" ")))
}

fn redacted_display(keys: &[String], core: &str) -> String {
    let assignments: Vec<String> = keys.iter().map(|k| format!("{k}=<redacted>")).collect();
    assignments
        .into_iter()
        .chain((!core.is_empty()).then(|| core.to_owned()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn path_subjects(events: &[Value], indices: &[usize]) -> BTreeSet<String> {
    indices
        .iter()
        .map(|&i| &events[i])
        .filter(|e| {
            ["file_read", "file_edit_attempt", "file_edit_confirmed"].contains(&kind(e))
                && e["path"]["location"] == "repository"
        })
        .map(|e| text(&e["path"], "path"))
        .filter(|p| !p.is_empty())
        .collect()
}

fn base_limitations(capture_state: &str, unknown_events: usize) -> Vec<String> {
    let mut out = vec![
        "Historical recording only: present applicability is unverified and must be checked against current evidence before acting on it.".to_owned(),
        "Recorded events, interpreted lessons, present applicability and current instructions are distinct; this finding is not a current instruction.".to_owned(),
    ];
    if capture_state != "complete" {
        out.push(format!(
            "Session capture is {capture_state}; the recording may be incomplete."
        ));
    }
    if unknown_events > 0 {
        out.push(format!(
            "{unknown_events} unsupported or unpaired event(s) are coverage gaps; their side effects were not inferred."
        ));
    }
    out
}

fn draft(
    finding_kind: &str,
    rule: &str,
    authority: &str,
    claim: String,
    relevance: &str,
    subjects: BTreeSet<String>,
    payload: Value,
    evidence: &[&Value],
    session_view: &str,
    session_context_id: &str,
    capture_state: &str,
    unknown_events: usize,
) -> Value {
    let mut limitations = base_limitations(capture_state, unknown_events);
    if let Some(extra) = payload["extra_limitations"].as_array() {
        limitations.extend(extra.iter().filter_map(|l| l.as_str().map(str::to_owned)));
    }
    let mut payload = payload;
    if let Some(object) = payload.as_object_mut() {
        object.remove("extra_limitations");
    }
    json!({
        "kind": finding_kind,
        "rule": rule,
        "authority": authority,
        "claim": claim,
        "relevance": relevance,
        "subjects": subjects.into_iter().collect::<Vec<_>>(),
        "payload": payload,
        "evidence_event_ids": evidence.iter().map(|e| id(e)).collect::<Vec<_>>(),
        "session_view": session_view,
        "session_context_id": session_context_id,
        "capture_state": capture_state,
        "limitations": limitations,
    })
}

/// Failure → intervention → scoped success of the same normalized command
/// within one episode and, when correlation exists, one branch.
fn failure_recovery(
    events: &[Value],
    episode: &Episode,
    ancestry: &Ancestry,
    session_view: &str,
    session_context_id: &str,
    capture_state: &str,
    unknown_events: usize,
    out: &mut Vec<Value>,
) {
    let finished: Vec<usize> = episode
        .events
        .iter()
        .copied()
        .filter(|&i| kind(&events[i]) == "command_finished")
        .collect();
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    for (pos, &failure_index) in finished.iter().enumerate() {
        let failure = &events[failure_index];
        if failure["outcome"] != "failed" {
            continue;
        }
        let Some((_, failure_core)) = parse_command(failure["command"].as_str().unwrap_or(""))
        else {
            continue;
        };
        if failure_core.is_empty() || !emitted.insert(failure_core.clone()) {
            continue;
        }
        let failure_cwd = failure["cwd"].as_str().unwrap_or("");
        for &success_index in finished.iter().skip(pos + 1) {
            let success = &events[success_index];
            if success["outcome"] != "succeeded"
                || success["cwd"].as_str().unwrap_or("") != failure_cwd
            {
                continue;
            }
            let Some((success_keys, success_core)) =
                parse_command(success["command"].as_str().unwrap_or(""))
            else {
                continue;
            };
            if success_core != failure_core {
                continue;
            }
            // Branch-aware pairing: with correlation recorded, the success
            // must descend from the failure, not sit on a sibling branch.
            if ancestry.has_correlation() {
                let (Some(fm), Some(sm)) = (
                    &ancestry.message_of[failure_index],
                    &ancestry.message_of[success_index],
                ) else {
                    continue;
                };
                if !ancestry.descends_from(sm, fm) {
                    continue;
                }
            }
            let between: Vec<usize> = episode
                .events
                .iter()
                .copied()
                .filter(|&i| i > failure_index && i < success_index)
                .collect();
            let mut interventions: Vec<Value> = between
                .iter()
                .map(|&i| &events[i])
                .filter(|e| kind(e) == "file_edit_confirmed")
                .map(|e| {
                    json!({"kind":"confirmed_edit","event_id":id(e),
                        "path":e["path"]["path"],"ordinal":ordinal(e)})
                })
                .collect();
            for key in &success_keys {
                interventions.push(
                    json!({"kind":"environment_assignment","event_id":id(success),
                    "setting_key":key,"value":"redacted","ordinal":ordinal(success)}),
                );
            }
            interventions.extend(
                between
                    .iter()
                    .map(|&i| &events[i])
                    .filter(|e| kind(e) == "command_finished" && id(e) != id(failure))
                    .map(|e| {
                        let (keys, core) =
                            parse_command(e["command"].as_str().unwrap_or("")).unwrap_or_default();
                        json!({"kind":"intervening_command","event_id":id(e),
                            "command":redacted_display(&keys, &core),"outcome":e["outcome"],"ordinal":ordinal(e)})
                    }),
            );
            if interventions.is_empty() {
                continue; // A bare retry records no intervention to learn from.
            }
            let mut subjects = path_subjects(events, &between);
            subjects.insert(format!("command:{success_core}"));
            let later_failure = finished.iter().skip(pos + 1).any(|&i| {
                let e = &events[i];
                i > success_index
                    && e["outcome"] == "failed"
                    && parse_command(e["command"].as_str().unwrap_or(""))
                        .is_some_and(|(_, core)| core == failure_core)
            });
            let command = redacted_display(&success_keys, &success_core);
            let mut payload = json!({"type":"session_experience","rule":rule_name(RULE_FAILURE_RECOVERY),
                "episode":{"turn_event":episode.turn_event,"branch_parent":episode.branch_parent},
                "failure":{"event_id":id(failure),"command":command,"cwd":failure["cwd"],
                    "exit_code":failure["exit_code"],"ordinal":ordinal(failure)},
                "interventions":interventions,
                "success":{"event_id":id(success),"ordinal":ordinal(success),
                    "scope":"same normalized command, same recorded working directory, same episode and branch"},
                "causality":"unverified","necessity":"unverified",
                "guidance":"advisory_verify_before_use"});
            if later_failure {
                payload["extra_limitations"] = json!([
                    "The same normalized command failed again later in this episode; the recorded success was scoped, not durable."]);
            }
            out.push(draft(
                "session_failure_recovery",
                RULE_FAILURE_RECOVERY,
                "observed_execution",
                format!("A recorded command (`{command}`) failed, {} intervention(s) were recorded, and the same normalized command then succeeded in the same recorded scope. Which intervention mattered, and whether any was necessary, is unverified.",
                    interventions_len(&payload)),
                "Prior recorded recovery sequence joined to this task through its subjects; advisory history, not current guidance.",
                subjects,
                payload,
                &[failure, success],
                session_view,
                session_context_id,
                capture_state,
                unknown_events,
            ));
            break;
        }
    }
}

fn rule_name(rule: &str) -> &str {
    rule.trim_end_matches("-v1")
}
fn interventions_len(payload: &Value) -> usize {
    rows(payload, "interventions").len()
}

const REQUIREMENT_CUES: &[&str] = &[
    "must",
    "never",
    "always",
    "do not",
    "don't",
    "make sure",
    "ensure",
    "require",
    "requires",
    "required",
];
const FUTURE_SCOPE: &[&str] = &[
    "always",
    "every time",
    "for all",
    "in the future",
    "going forward",
    "from now on",
    "whenever",
];

fn contains_cue(text: &str, cues: &[&str]) -> Option<String> {
    let lower = format!(" {} ", text.to_lowercase());
    cues.iter()
        .find(|cue| {
            lower.contains(&format!(" {cue} "))
                || lower.contains(&format!(" {cue},"))
                || lower.contains(&format!(" {cue}."))
                || lower.contains(&format!(" {cue}:"))
        })
        .map(|cue| cue.to_string())
}

/// Explicit requirement stated by the recorded user. Assistant text never
/// qualifies; the verbatim span is retained, never promoted to a global
/// preference or a current instruction.
fn review_requirements(
    events: &[Value],
    episode: &Episode,
    session_view: &str,
    session_context_id: &str,
    capture_state: &str,
    unknown_events: usize,
    out: &mut Vec<Value>,
) {
    for &index in &episode.events {
        let e = &events[index];
        if kind(e) != "user_message" {
            continue;
        }
        let Some(span) = e["text"].as_str().filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        let Some(cue) = contains_cue(span, REQUIREMENT_CUES) else {
            continue;
        };
        let scope = if contains_cue(span, FUTURE_SCOPE).is_some() {
            "stated_future"
        } else {
            "task_local"
        };
        let mut subjects = path_subjects(events, &episode.events);
        for token in span.split_whitespace() {
            let token = token.trim_matches(|c: char| ",.;:()[]\"'".contains(c));
            if token.contains('/')
                && token.rsplit('.').next().is_some_and(|ext| {
                    ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric())
                })
            {
                subjects.insert(token.to_owned());
            }
        }
        let kept: String = span.chars().take(MAX_SPAN_CHARS).collect();
        out.push(draft(
            "session_review_requirement",
            RULE_REVIEW_REQUIREMENT,
            "direct_user_statement_recorded",
            format!("A recorded user explicitly stated a requirement ({scope} scope) in a prior session: “{kept}” It is evidence of that recorded statement, not a current instruction for this task."),
            "Attributed prior user requirement joined through this episode's subjects; scope and authority are preserved.",
            subjects,
            json!({"type":"session_experience","rule":rule_name(RULE_REVIEW_REQUIREMENT),
                "episode":{"turn_event":episode.turn_event,"branch_parent":episode.branch_parent},
                "actor":"user","cue":cue,"scope":scope,"current_instruction":false,
                "attributed_span":kept,"omitted_characters":span.chars().count().saturating_sub(MAX_SPAN_CHARS),
                "guidance":if scope == "stated_future" {"advisory_verify_before_use"} else {"task_local_record_only"}}),
            &[e],
            session_view,
            session_context_id,
            capture_state,
            unknown_events,
        ));
    }
}

/// Environment keys set on recorded commands. Values are redacted at
/// extraction; an observed setting is not a universal prerequisite.
fn environment_observations(
    events: &[Value],
    episode: &Episode,
    session_view: &str,
    session_context_id: &str,
    capture_state: &str,
    unknown_events: usize,
    out: &mut Vec<Value>,
) {
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for &index in &episode.events {
        let e = &events[index];
        if kind(e) != "command_finished" {
            continue;
        }
        let Some((keys, core)) = parse_command(e["command"].as_str().unwrap_or("")) else {
            continue;
        };
        for key in keys {
            if !seen.insert((key.clone(), core.clone())) {
                continue;
            }
            let mut subjects = path_subjects(events, &episode.events);
            subjects.insert(format!("env:{key}"));
            out.push(draft(
                "session_environment_observation",
                RULE_ENVIRONMENT,
                "observed_execution",
                format!("Environment key `{key}` was set (value redacted) on a recorded command (`{}`) in this session's context. Whether it was necessary, caused any outcome, or remains a safe present-day value is unverified.",
                    redacted_display(&[key.clone()], &core)),
                "Observed recorded environment joined to this task through its configuration or path subjects.",
                subjects,
                json!({"type":"session_experience","rule":rule_name(RULE_ENVIRONMENT),
                    "episode":{"turn_event":episode.turn_event,"branch_parent":episode.branch_parent},
                    "setting_key":key,"value":"redacted","value_redacted":true,
                    "command":redacted_display(&[key.clone()], &core),"cwd":e["cwd"],
                    "recorded_outcome":e["outcome"],
                    "guidance":"advisory_verify_before_use"}),
                &[e],
                session_view,
                session_context_id,
                capture_state,
                unknown_events,
            ));
        }
    }
}

/// Analyze one retained session into finding drafts plus diagnostics.
/// Drafts carry no applicability decision: the task join layer owns that.
pub fn analyze(
    events: &[Value],
    session_view: &str,
    session_context_id: &str,
    capture_state: &str,
) -> (Vec<Value>, Value) {
    let ancestry = Ancestry::build(events);
    let unknown_events = events.iter().filter(|e| kind(e) == "unknown").count();
    let mut out = Vec::new();
    let episodes = episodes(events);
    for episode in &episodes {
        failure_recovery(
            events,
            episode,
            &ancestry,
            session_view,
            session_context_id,
            capture_state,
            unknown_events,
            &mut out,
        );
        review_requirements(
            events,
            episode,
            session_view,
            session_context_id,
            capture_state,
            unknown_events,
            &mut out,
        );
        environment_observations(
            events,
            episode,
            session_view,
            session_context_id,
            capture_state,
            unknown_events,
            &mut out,
        );
    }
    let truncated = out.len() > MAX_FINDINGS_PER_SESSION;
    out.truncate(MAX_FINDINGS_PER_SESSION);
    let diag = json!({"producer":PRODUCER,"session_view":session_view,
        "episodes":episodes.len(),"drafts":out.len(),"truncated":truncated,
        "unknown_events":unknown_events,"branch_correlation":ancestry.has_correlation(),
        "bounds":{"max_findings_per_session":MAX_FINDINGS_PER_SESSION}});
    (out, diag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(
        id: &str,
        raw: &str,
        outcome: &str,
        message: Option<&str>,
        parent: Option<&str>,
    ) -> Value {
        let mut e = json!({"id":id,"ordinal":id.parse::<u64>().unwrap_or(0),"kind":"command_finished",
            "command":raw,"cwd":"/work/shop","outcome":outcome,"exit_code":if outcome=="failed" {1} else {0}});
        if let Some(m) = message {
            e["external_message_id"] = json!(m);
        }
        if let Some(p) = parent {
            e["external_parent_id"] = json!(p);
        }
        e
    }
    fn edit(id: &str, path: &str) -> Value {
        json!({"id":id,"ordinal":id.parse::<u64>().unwrap_or(0),"kind":"file_edit_confirmed",
            "path":{"location":"repository","path":path},"outcome":"succeeded"})
    }
    fn user(id: &str, body: &str) -> Value {
        json!({"id":id,"ordinal":id.parse::<u64>().unwrap_or(0),"kind":"user_message","text":body})
    }

    #[test]
    fn recovery_requires_intervention_and_a_scoped_success() {
        // Failure, intervention, success of the same command.
        let events = vec![
            user("1", "fix the build"),
            command("2", "cargo test", "failed", None, None),
            edit("3", "src/lib.rs"),
            command("4", "cargo test", "succeeded", None, None),
        ];
        let (drafts, diag) = analyze(&events, "view:s", "ctx", "complete");
        assert_eq!(diag["episodes"], 1);
        let recovery: Vec<_> = drafts
            .iter()
            .filter(|d| d["kind"] == "session_failure_recovery")
            .collect();
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0]["payload"]["causality"], "unverified");
        assert!(recovery[0]["subjects"]
            .as_array()
            .unwrap()
            .contains(&json!("src/lib.rs")));

        // Unresolved outcome: failure with no recorded success is no finding.
        let unresolved = vec![
            user("1", "fix the build"),
            command("2", "cargo test", "failed", None, None),
            edit("3", "src/lib.rs"),
        ];
        let (drafts, _) = analyze(&unresolved, "view:s", "ctx", "complete");
        assert!(drafts
            .iter()
            .all(|d| d["kind"] != "session_failure_recovery"));

        // Bare retry without an intervention records nothing to learn.
        let retry = vec![
            command("1", "cargo test", "failed", None, None),
            command("2", "cargo test", "succeeded", None, None),
        ];
        let (drafts, _) = analyze(&retry, "view:s", "ctx", "complete");
        assert!(drafts.is_empty());
    }

    #[test]
    fn recovery_does_not_cross_branches_or_turns() {
        // Same turn, but the success sits on a sibling branch.
        let events = vec![
            user("1", "fix it"),
            command("2", "cargo test", "failed", Some("m-fail"), Some("m-user")),
            edit("3", "src/lib.rs"),
            command(
                "4",
                "cargo test",
                "succeeded",
                Some("m-other"),
                Some("m-user"),
            ),
        ];
        let mut events = events;
        // Give the user message its correlation identity.
        events[0]["external_message_id"] = json!("m-user");
        let (drafts, _) = analyze(&events, "view:s", "ctx", "complete");
        assert!(drafts
            .iter()
            .all(|d| d["kind"] != "session_failure_recovery"));
        // A later turn never completes an earlier turn's failure.
        let events = vec![
            user("1", "fix it"),
            command("2", "cargo test", "failed", None, None),
            user("3", "something else"),
            edit("4", "src/lib.rs"),
            command("5", "cargo test", "succeeded", None, None),
        ];
        let (drafts, _) = analyze(&events, "view:s", "ctx", "complete");
        assert!(drafts
            .iter()
            .all(|d| d["kind"] != "session_failure_recovery"));
    }

    #[test]
    fn requirements_come_only_from_the_user_and_keep_their_scope() {
        let events = vec![
            user("1", "You must run cargo test before finishing."),
            json!({"id":"2","ordinal":2,"kind":"assistant_message","text":"I must always refactor everything."}),
        ];
        let (drafts, _) = analyze(&events, "view:s", "ctx", "complete");
        let requirements: Vec<_> = drafts
            .iter()
            .filter(|d| d["kind"] == "session_review_requirement")
            .collect();
        assert_eq!(requirements.len(), 1, "{drafts:?}");
        assert_eq!(requirements[0]["payload"]["scope"], "task_local");
        assert_eq!(requirements[0]["payload"]["actor"], "user");
        assert_eq!(requirements[0]["payload"]["current_instruction"], false);

        let future = vec![user("1", "Always run cargo test for this project.")];
        let (drafts, _) = analyze(&future, "view:s", "ctx", "complete");
        assert_eq!(drafts[0]["payload"]["scope"], "stated_future");
    }

    #[test]
    fn environment_values_are_redacted_and_analysis_is_deterministic() {
        let events = vec![
            user("1", "run it"),
            command(
                "2",
                "API_TOKEN=s3cr3t-value cargo test",
                "succeeded",
                None,
                None,
            ),
        ];
        let (a, _) = analyze(&events, "view:s", "ctx", "complete");
        let (b, _) = analyze(&events, "view:s", "ctx", "complete");
        assert_eq!(a, b, "identical pinned inputs give identical drafts");
        let env: Vec<_> = a
            .iter()
            .filter(|d| d["kind"] == "session_environment_observation")
            .collect();
        assert_eq!(env.len(), 1);
        assert_eq!(env[0]["payload"]["setting_key"], "API_TOKEN");
        assert!(env[0]["subjects"]
            .as_array()
            .unwrap()
            .contains(&json!("env:API_TOKEN")));
        assert!(
            !serde_json::to_string(&a).unwrap().contains("s3cr3t-value"),
            "secret values must never reach a finding"
        );
    }
}
