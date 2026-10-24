//! Hook-only adapters: source lifecycle stays separate from optional MCP reads.
use super::{workspace, Result};
use serde_json::{json, Value};
use std::path::Path;

/// Escape a path for a POSIX double-quoted shell string.
pub(crate) fn quoted(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for c in raw.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn is_briefcase_sync(command: &str) -> bool {
    command.contains("briefcase")
        && (command.contains("--json sync")
            || command.contains(" sync-request ")
            || command.contains(" monitor ensure")
            || command.contains(" context-hook "))
}

/// Load a JSON document an install may merge. Invalid JSON blocks the
/// install rather than being overwritten, even with `--force`.
pub(crate) fn load(path: &Path, blocked: &mut Vec<Value>) -> Option<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            blocked.push(json!({"path":path,"reason":format!("cannot read file: {e}")}));
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            blocked
                .push(json!({"path":path,"reason":format!("invalid JSON, left untouched: {e}")}));
            None
        }
    }
}

/// Ensure one owned handler for a lifecycle event in a harness configuration.
/// Existing groups, events and handlers are preserved; a stale Briefcase
/// handler is replaced only with `--force`, as for the MCP entry.
pub(crate) fn plan_hook_event(
    doc: &mut Value,
    event: &str,
    desired: &Value,
    force: bool,
    path: &Path,
    actions: &mut Vec<Value>,
    blocked: &mut Vec<Value>,
) -> bool {
    let hooks = doc
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        blocked.push(json!({"path":path,"reason":"hooks is not an object; left untouched"}));
        return false;
    }
    let groups = hooks
        .as_object_mut()
        .unwrap()
        .entry(event)
        .or_insert_with(|| json!([]));
    let Some(groups) = groups.as_array_mut() else {
        blocked.push(
            json!({"path":path,"reason":format!("hooks.{event} is not an array; left untouched")}),
        );
        return false;
    };
    let owned: Vec<_> = groups
        .iter()
        .flat_map(|group| {
            group["hooks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|h| is_briefcase_sync(h["command"].as_str().unwrap_or("")))
                .map(move |h| (h, group.get("matcher")))
        })
        .collect();
    if owned.len() == 1 && owned[0].0 == desired && owned[0].1.is_none() {
        return false;
    }
    let replacing = !owned.is_empty();
    if replacing && !force {
        blocked.push(json!({"path":path,"reason":format!("the {event} sync hook differs or is duplicated; rerun with --force to back it up and replace it")}));
        return false;
    }
    // Remove only our handlers, including duplicates; keep other handlers/matchers.
    if replacing {
        groups.retain_mut(|group| {
            if let Some(handlers) = group["hooks"].as_array_mut() {
                let before = handlers.len();
                handlers.retain(|h| !is_briefcase_sync(h["command"].as_str().unwrap_or("")));
                return before == handlers.len() || !handlers.is_empty();
            }
            true
        });
    }
    groups.push(json!({"hooks":[desired]}));
    actions.push(json!({"action":if replacing { "replace" } else { "update" },"path":path}));
    true
}

/// Retire only owned turn-boundary handlers. Preserve all unrelated handlers and matchers.
pub(crate) fn remove_hook_event(
    doc: &mut Value,
    event: &str,
    force: bool,
    path: &Path,
    actions: &mut Vec<Value>,
    blocked: &mut Vec<Value>,
) -> bool {
    let Some(groups) = doc.get_mut("hooks").and_then(|h| h.get_mut(event)) else {
        return false;
    };
    let Some(groups) = groups.as_array_mut() else {
        blocked.push(
            json!({"path":path,"reason":format!("hooks.{event} is not an array; left untouched")}),
        );
        return false;
    };
    let owned = groups.iter().any(|g| {
        g["hooks"].as_array().is_some_and(|hs| {
            hs.iter()
                .any(|h| is_briefcase_sync(h["command"].as_str().unwrap_or("")))
        })
    });
    if !owned {
        return false;
    }
    if !force {
        blocked.push(json!({"path":path,"reason":format!("legacy {event} sync hook requires --force (init --repair) to back it up and remove it")}));
        return false;
    }
    groups.retain_mut(|g| {
        if let Some(hs) = g["hooks"].as_array_mut() {
            let before = hs.len();
            hs.retain(|h| !is_briefcase_sync(h["command"].as_str().unwrap_or("")));
            return before == hs.len() || !hs.is_empty();
        }
        true
    });
    actions.push(json!({"action":"remove-owned-hook","path":path,"event":event}));
    true
}

pub fn install(
    agent: &str,
    root: &Path,
    binary: &Path,
    dry_run: bool,
    force: bool,
) -> Result<Value> {
    let directory = match agent {
        "codex" => ".codex",
        "claude-code" => ".claude",
        _ => return Err("INSTALL: Unknown hook adapter".into()),
    };
    let path = root.join(directory).join(if agent == "codex" {
        "hooks.json"
    } else {
        "settings.json"
    });
    let mut blocked = Vec::new();
    let mut actions = Vec::new();
    let mut doc = load(&path, &mut blocked).unwrap_or_else(|| json!({}));
    let mut dirty = false;
    if !doc.is_object() {
        blocked.push(json!({"path":path,"reason":"Hook configuration must be a JSON object"}));
    }
    if blocked.is_empty() {
        let handler = json!({"type":"command","command":format!(
            "{} monitor ensure --hook --quiet", quoted(binary)), "timeout":10,"async":true});
        dirty |= plan_hook_event(
            &mut doc,
            "SessionStart",
            &handler,
            force,
            &path,
            &mut actions,
            &mut blocked,
        );
        if agent=="claude-code" {
            for (event,phase) in [("UserPromptSubmit","prompt"),("Stop","complete")] {
                let handler=json!({"type":"command","command":format!("{} context-hook --phase {phase}",quoted(binary)),"timeout":10});
                dirty |= plan_hook_event(&mut doc,event,&handler,force,&path,&mut actions,&mut blocked);
            }
        } else {
            dirty |= remove_hook_event(&mut doc, "Stop", force, &path, &mut actions, &mut blocked);
        }
    }
    let existed = path.exists();
    let status = if !blocked.is_empty() {
        "blocked"
    } else if !dirty {
        "already-configured"
    } else if existed {
        "updated"
    } else {
        "wired"
    };
    let mut report = json!({"agent":agent,"status":status,"project":root,"hooks":path,
        "dry_run":dry_run,"actions":actions,"blocked":blocked,
        "notes":["SessionStart maintains the monitor. Claude Code receives prompt context and a live worktree diagnostic against its original Case. BRIEFCASE_REVIEW_CONTEXT=1 also builds a full review Case.",
        "Requires a client supporting SessionStart command hooks. Inspect /hooks and review/trust the definitions in that client.",
        "Hooks ensure the repository monitor exists without continuing the conversation. Remove only these Briefcase handlers to uninstall."]});
    if dry_run || !blocked.is_empty() || !dirty {
        return Ok(report);
    }
    if existed {
        let dir = root.join(directory).join("briefcase-backups");
        workspace::private_dir(&dir)?;
        let backup = tempfile::Builder::new()
            .prefix("hooks-")
            .suffix(".json")
            .tempfile_in(&dir)?;
        std::fs::copy(&path, backup.path())?;
        let (_, saved) = backup.keep()?;
        report["backups"] = json!([{"original":path,"backup":saved}]);
    }
    workspace::write_atomic(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&doc)?).as_bytes(),
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapters_merge_preview_preserve_and_back_up_changes() {
        for agent in ["codex", "claude-code"] {
            let d = tempfile::tempdir().unwrap();
            let binary = Path::new("/opt/a $tool/briefcase");
            let preview = install(agent, d.path(), binary, true, false).unwrap();
            let path = std::path::PathBuf::from(preview["hooks"].as_str().unwrap());
            assert!(!path.exists());
            workspace::write_atomic(&path, br#"{"theme":"keep","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]}}"#).unwrap();
            install(agent, d.path(), binary, false, false).unwrap();
            assert_eq!(
                install(agent, d.path(), binary, false, false).unwrap()["status"],
                "already-configured"
            );
            let doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(doc["theme"], "keep");
            assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["command"], "echo keep");
            let mut duplicates = doc.clone();
            let duplicate = duplicates["hooks"]["SessionStart"][0].clone();
            duplicates["hooks"]["SessionStart"]
                .as_array_mut()
                .unwrap()
                .push(duplicate);
            std::fs::write(&path, duplicates.to_string()).unwrap();
            assert_eq!(
                install(agent, d.path(), binary, false, false).unwrap()["status"],
                "blocked"
            );
            install(agent, d.path(), binary, false, true).unwrap();
            let repaired: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(repaired["hooks"]["Stop"].as_array().unwrap().len(), if agent=="claude-code" {2}else{1});
            assert_eq!(
                install(
                    agent,
                    d.path(),
                    Path::new("/other/briefcase"),
                    false,
                    false
                )
                .unwrap()["status"],
                "blocked"
            );
            let forced =
                install(agent, d.path(), Path::new("/other/briefcase"), false, true).unwrap();
            assert!(Path::new(forced["backups"][0]["backup"].as_str().unwrap()).exists());
            std::fs::write(&path, "invalid").unwrap();
            assert_eq!(
                install(agent, d.path(), binary, false, true).unwrap()["status"],
                "blocked"
            );
            assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid");
        }
    }
}
