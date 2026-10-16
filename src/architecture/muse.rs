//! Previewed, idempotent installation of the Muse integration.
//!
//! The compact MCP pair shares the CLI service. SessionStart only bootstraps
//! the repository monitor; transcript import for Muse is not yet supported.
//! Installs preserve unrelated configuration and back up replaced owned entries.
use super::{
    hooks::{load, plan_hook_event, quoted, remove_hook_event},
    workspace, Result,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const SERVER: &str = "briefcase";

pub struct InstallOptions {
    /// Project root holding `.mcp.json` and `.muse/hooks.json`.
    /// `None` installs into the current directory.
    pub project: Option<PathBuf>,
    pub binary: PathBuf,
    pub dry_run: bool,
    /// Approve replacing a locally changed Briefcase entry (after backup).
    pub force: bool,
}

/// Muse starts hooks with a cleared environment; carry an absolute binary.
/// Durable requests survive writer contention and the end of the session.
pub fn sync_command(binary: &Path) -> String {
    format!("{} monitor ensure --hook --quiet", quoted(binary))
}

fn sync_handler(binary: &Path) -> Value {
    let command = sync_command(binary);
    // `commandWindows` carries the same line for `%COMSPEC%`; backslashes
    // in a Windows binary path are literal there, so it stays unescaped.
    let windows = format!(
        "\"{}\" monitor ensure --hook --quiet",
        binary.to_string_lossy()
    );
    json!({"type":"command","command":command,"commandWindows":windows,
        "timeout":10,"async":true,
        "statusMessage":"Briefcase: starting monitor"})
}

fn server_entry(binary: &Path) -> Value {
    json!({"type":"stdio","command":binary.to_string_lossy(),
        "args":["mcp","--surface","compact"],
        "framing":"line_delimited_json","required":false})
}

/// Ensure the `briefcase` server in `.mcp.json`; returns whether the file
/// must be written. Other servers and root keys are preserved.
fn plan_mcp(
    doc: &mut Value,
    binary: &Path,
    force: bool,
    path: &Path,
    actions: &mut Vec<Value>,
    blocked: &mut Vec<Value>,
) -> bool {
    let desired = server_entry(binary);
    if !doc.is_object() {
        blocked
            .push(json!({"path":path,"reason":".mcp.json is not a JSON object; left untouched"}));
        return false;
    }
    let servers = doc
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        blocked.push(json!({"path":path,"reason":"mcpServers is not an object; left untouched"}));
        return false;
    }
    match servers.get(SERVER) {
        Some(current) if *current == desired => false,
        Some(_) if !force => {
            blocked.push(json!({"path":path,"reason":"the briefcase MCP server differs (stale binary path or local edits); rerun with --force to back it up and replace it"}));
            false
        }
        Some(_) => {
            servers[SERVER] = desired;
            actions.push(json!({"action":"replace","path":path}));
            true
        }
        None => {
            servers[SERVER] = desired;
            actions.push(json!({"action":"update","path":path}));
            true
        }
    }
}

pub fn install(o: &InstallOptions) -> Result<Value> {
    let root = match &o.project {
        Some(project) => std::fs::canonicalize(project)?,
        None => std::env::current_dir()?.canonicalize()?,
    };
    let mcp_path = root.join(".mcp.json");
    let hooks_path = root.join(".muse").join("hooks.json");
    let mut blocked: Vec<Value> = Vec::new();
    let mut actions: Vec<Value> = Vec::new();

    let mcp_existed = mcp_path.is_file();
    let mut mcp_doc = load(&mcp_path, &mut blocked).unwrap_or_else(|| json!({}));
    let mcp_dirty = blocked.is_empty()
        && plan_mcp(
            &mut mcp_doc,
            &o.binary,
            o.force,
            &mcp_path,
            &mut actions,
            &mut blocked,
        );
    if !mcp_existed && mcp_dirty {
        // A fresh file is a write, not a merge into an existing one.
        for action in actions.iter_mut().filter(|a| a["path"] == json!(mcp_path)) {
            action["action"] = json!("write");
        }
    }

    let hooks_existed = hooks_path.is_file();
    let mut hooks_doc = load(&hooks_path, &mut blocked).unwrap_or_else(|| json!({"hooks":{}}));
    let mut hooks_dirty = false;
    if blocked.is_empty() {
        if !hooks_doc.is_object() {
            blocked.push(json!({"path":hooks_path,"reason":"hooks.json is not a JSON object; left untouched"}));
        } else {
            hooks_dirty |= plan_hook_event(
                &mut hooks_doc,
                "SessionStart",
                &sync_handler(&o.binary),
                o.force,
                &hooks_path,
                &mut actions,
                &mut blocked,
            );
            hooks_dirty |= remove_hook_event(
                &mut hooks_doc,
                "Stop",
                o.force,
                &hooks_path,
                &mut actions,
                &mut blocked,
            );
        }
    }
    if !hooks_existed && hooks_dirty {
        for action in actions
            .iter_mut()
            .filter(|a| a["path"] == json!(hooks_path))
        {
            if action["action"] == "update" {
                action["action"] = json!("write");
            }
        }
    }

    let replaced = actions.iter().any(|a| a["action"] == "replace");
    let touched_existing = (mcp_dirty && mcp_existed) || (hooks_dirty && hooks_existed);
    let status = if !blocked.is_empty() {
        "blocked"
    } else if actions.is_empty() {
        "already-configured"
    } else if replaced || touched_existing {
        "updated"
    } else {
        "wired"
    };
    let mut report = json!({"agent":"muse","status":status,"dry_run":o.dry_run,
    "project":root,"mcp_config":mcp_path,"hooks":hooks_path,"binary":o.binary,
    "surface":"compact","tools":["context","briefcase"],
    "actions":actions,"blocked":blocked,
    "notes":[
        "Muse sees mcp__briefcase__context and mcp__briefcase__briefcase; both share service::call with the CLI and Pi.",
        "SessionStart ensures one repository monitor; the monitor reconciles local state.",
        "No Muse trust, provider or credential setting was changed. Delete the briefcase server and the monitor hook to uninstall."
    ]});
    if o.dry_run || !blocked.is_empty() || status == "already-configured" {
        return Ok(report);
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_dir = root
        .join(".muse")
        .join("briefcase-backups")
        .join(stamp.to_string());
    let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (path, existed, dirty) in [
        (&mcp_path, mcp_existed, mcp_dirty),
        (&hooks_path, hooks_existed, hooks_dirty),
    ] {
        if existed && dirty {
            backups.push((path.clone(), backup_dir.join(path.file_name().unwrap())));
        }
    }
    for (from, to) in &backups {
        workspace::private_dir(to.parent().unwrap())?;
        if to.exists() {
            return Err(format!(
                "INSTALL: Backup destination already exists: {}",
                to.display()
            )
            .into());
        }
        std::fs::copy(from, to)?;
    }
    if mcp_dirty {
        workspace::write_atomic(
            &mcp_path,
            format!("{}\n", serde_json::to_string_pretty(&mcp_doc)?).as_bytes(),
        )?;
    }
    if hooks_dirty {
        workspace::write_atomic(
            &hooks_path,
            format!("{}\n", serde_json::to_string_pretty(&hooks_doc)?).as_bytes(),
        )?;
    }
    let written_mcp: Value = serde_json::from_str(&std::fs::read_to_string(&mcp_path)?)?;
    let written_hooks: Value = serde_json::from_str(&std::fs::read_to_string(&hooks_path)?)?;
    let desired = sync_handler(&o.binary);
    let hooks_ok = ["SessionStart"].iter().all(|event| {
        written_hooks["hooks"][event]
            .as_array()
            .is_some_and(|groups| {
                groups.iter().any(|g| {
                    g["hooks"]
                        .as_array()
                        .is_some_and(|hs| hs.contains(&desired))
                })
            })
    });
    if written_mcp["mcpServers"][SERVER] != server_entry(&o.binary) || !hooks_ok {
        return Err("INSTALL: written Muse configuration did not verify".into());
    }
    if !backups.is_empty() {
        report["backups"] = json!(backups
            .iter()
            .map(|(from, to)| json!({"original":from,"backup":to}))
            .collect::<Vec<_>>());
        report["restore"] =
            json!("Move a backup file back to its original path to undo that file's change.");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(dir: &Path) -> InstallOptions {
        InstallOptions {
            project: Some(dir.to_path_buf()),
            binary: PathBuf::from("/opt/briefcase/bin/briefcase"),
            dry_run: false,
            force: false,
        }
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn install_is_idempotent_and_detects_edits() {
        let dir = tempfile::tempdir().unwrap();
        let o = options(dir.path());
        assert_eq!(install(&o).unwrap()["status"], "wired");
        assert_eq!(install(&o).unwrap()["status"], "already-configured");
        let mcp = read(&dir.path().join(".mcp.json"));
        let server = &mcp["mcpServers"][SERVER];
        assert_eq!(server["command"], "/opt/briefcase/bin/briefcase");
        assert_eq!(server["args"], json!(["mcp", "--surface", "compact"]));
        assert_eq!(server["framing"], "line_delimited_json");
        let hooks = read(&dir.path().join(".muse/hooks.json"));
        {
            let event = "SessionStart";
            let handler = &hooks["hooks"][event][0]["hooks"][0];
            assert_eq!(handler["type"], "command");
            assert_eq!(handler["async"], true);
            assert!(handler["command"]
                .as_str()
                .unwrap()
                .contains("/opt/briefcase/bin/briefcase"));
            assert!(handler["command"]
                .as_str()
                .unwrap()
                .contains("monitor ensure"));
        }
        // A locally changed Briefcase entry blocks until forced, then backs up.
        let mut edited = mcp.clone();
        edited["mcpServers"][SERVER]["command"] = json!("/elsewhere/briefcase");
        std::fs::write(
            dir.path().join(".mcp.json"),
            serde_json::to_string_pretty(&edited).unwrap(),
        )
        .unwrap();
        assert_eq!(install(&o).unwrap()["status"], "blocked");
        let forced = install(&InstallOptions {
            force: true,
            ..options(dir.path())
        })
        .unwrap();
        assert_eq!(forced["status"], "updated");
        assert!(PathBuf::from(forced["backups"][0]["backup"].as_str().unwrap()).is_file());
        assert_eq!(
            read(&dir.path().join(".mcp.json"))["mcpServers"][SERVER]["command"],
            "/opt/briefcase/bin/briefcase"
        );
    }

    #[test]
    fn install_merges_without_touching_other_configuration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            json!({"mcpServers":{"docs":{"type":"stdio","command":"docs-mcp"}}}).to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".muse")).unwrap();
        std::fs::write(
            dir.path().join(".muse/hooks.json"),
            json!({"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"cat >> log"}]}]}})
                .to_string(),
        )
        .unwrap();
        let report = install(&options(dir.path())).unwrap();
        assert_eq!(report["status"], "updated");
        let mcp = read(&dir.path().join(".mcp.json"));
        assert_eq!(mcp["mcpServers"]["docs"]["command"], "docs-mcp");
        assert!(mcp["mcpServers"].get(SERVER).is_some());
        let hooks = read(&dir.path().join(".muse/hooks.json"));
        assert_eq!(
            hooks["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "cat >> log"
        );
        assert!(hooks["hooks"].get("SessionStart").is_some());
        assert!(hooks["hooks"].get("Stop").is_none());
        // A dry run reports the plan and writes nothing new.
        let fresh = tempfile::tempdir().unwrap();
        let planned = install(&InstallOptions {
            dry_run: true,
            ..options(fresh.path())
        })
        .unwrap();
        assert_eq!(planned["status"], "wired");
        assert!(!fresh.path().join(".mcp.json").exists());
        assert!(!fresh.path().join(".muse/hooks.json").exists());
    }
}
