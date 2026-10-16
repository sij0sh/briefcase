//! One integration contract used by init, status, and the lower-level installer.
use super::{hooks, muse, pi, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
pub const AGENTS: [&str; 4] = ["pi", "codex", "claude-code", "muse"];

pub fn detected(agent: &str, root: &Path) -> bool {
    match agent {
        "pi" => root.join(".pi").exists() || pi::agent_dir(None).is_ok_and(|p| p.is_dir()),
        "codex" => {
            root.join(".codex").exists()
                || home_config("CODEX_HOME", ".codex").is_some_and(|p| p.is_dir())
        }
        "claude-code" => {
            root.join(".claude").exists()
                || home_config("CLAUDE_CONFIG_DIR", ".claude").is_some_and(|p| p.is_dir())
        }
        "muse" => {
            root.join(".muse").exists()
                || home_config("MUSE_HOME", ".muse").is_some_and(|p| p.is_dir())
        }
        _ => false,
    }
}
fn home_config(var: &str, sub: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(sub))
        })
}
pub fn select(agents: &str, root: &Path) -> Result<Vec<String>> {
    if agents == "detected" {
        return Ok(AGENTS
            .into_iter()
            .filter(|a| detected(a, root))
            .map(str::to_owned)
            .collect());
    }
    if agents == "all" {
        return Ok(AGENTS.into_iter().map(str::to_owned).collect());
    }
    let mut selected = Vec::new();
    for name in agents.split(',') {
        if !AGENTS.contains(&name) {
            return Err(format!(
                "INIT: Unknown agent {name}; use detected, all, or pi,codex,claude-code,muse"
            )
            .into());
        }
        if !selected.iter().any(|n| n == name) {
            selected.push(name.into());
        }
    }
    Ok(selected)
}
pub fn install(
    agent: &str,
    root: &Path,
    binary: &Path,
    dry_run: bool,
    force: bool,
) -> Result<Value> {
    match agent {
        "pi" => pi::install(&pi::InstallOptions {
            agent_dir: None,
            project: if !root.join(".pi/extensions").join(pi::EXTENSION).is_file()
                && pi::agent_dir(None)
                    .is_ok_and(|p| p.join("extensions").join(pi::EXTENSION).is_file())
            {
                None
            } else {
                Some(root.into())
            },
            binary: binary.into(),
            dry_run,
            force,
        }),
        "codex" | "claude-code" => hooks::install(agent, root, binary, dry_run, force),
        "muse" => muse::install(&muse::InstallOptions {
            project: Some(root.into()),
            binary: binary.into(),
            dry_run,
            force,
        }),
        _ => Err("INSTALL: Unknown integration".into()),
    }
}
pub fn inspect(agent: &str, root: &Path, binary: &Path) -> Result<Value> {
    let plan = install(agent, root, binary, true, false)?;
    let blocked = plan["blocked"].as_array().is_some_and(|b| !b.is_empty());
    let state = if plan["status"] == "already-configured" {
        "configured"
    } else if blocked {
        let reasons = plan["blocked"].to_string();
        if reasons.contains("invalid JSON")
            || reasons.contains("not an object")
            || reasons.contains("not a JSON object")
            || reasons.contains("not an array")
        {
            "invalid"
        } else if reasons.contains("differs") || reasons.contains("legacy") {
            "outdated"
        } else {
            "conflicting"
        }
    } else if plan["status"] == "updated" {
        "outdated"
    } else if detected(agent, root) {
        "missing"
    } else {
        "not_detected"
    };
    Ok(
        json!({"agent":agent,"state":state,"detected":detected(agent,root),"plan":plan,
        "context_delivery":if ["pi","claude-code"].contains(&agent) {"automatic_per_prompt_agent_context"} else {"explicit_agent_context_tool; no automatic per-prompt hook"},
        "session_sync":if agent == "muse" {"unsupported"} else {"supported"}}),
    )
}
pub fn status(root: &Path, binary: &Path) -> Result<Vec<Value>> {
    AGENTS
        .into_iter()
        .map(|a| inspect(a, root, binary))
        .collect()
}
