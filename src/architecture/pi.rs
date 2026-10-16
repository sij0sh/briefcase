//! Previewed, idempotent installation of the native Pi extension.
//!
//! The extension is an adapter over this binary: it holds no resolver,
//! SQL or query logic of its own. Installation never edits Pi trust,
//! credentials, providers or MCP configuration, and it never deletes or
//! edits another extension: a replaced `briefcase-pi.ts` is moved to a backup.
use super::{require, workspace, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const TEMPLATE: &str = include_str!("../../extensions/briefcase-pi.ts");
pub const EXTENSION: &str = "briefcase-pi.ts";
const DIGEST_MARKER: &str = "// briefcase-pi-digest: ";

pub struct InstallOptions {
    /// Explicit agent directory; otherwise `PI_CODING_AGENT_DIR`, then `~/.pi/agent`.
    pub agent_dir: Option<PathBuf>,
    /// Install into `<project>/.pi/extensions` instead of the agent directory.
    pub project: Option<PathBuf>,
    pub binary: PathBuf,
    pub dry_run: bool,
    /// Approve replacing a locally edited `briefcase-pi.ts` (after backup).
    pub force: bool,
}

pub fn agent_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return Ok(dir.to_path_buf());
    }
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("Cannot find the home directory; pass --agent-dir")?;
    Ok(PathBuf::from(home).join(".pi/agent"))
}

fn digest(text: &str) -> String {
    super::identity::bytes_digest(text.as_bytes())
}

/// The extension with this binary substituted, followed by a digest line
/// that detects local edits. Catalog discovery stays in the running CLI.
pub fn render(binary: &Path) -> Result<String> {
    let body = TEMPLATE.replace(
        "\"__BRIEFCASE_BINARY__\"",
        &serde_json::to_string(&binary.to_string_lossy())?,
    );
    require(
        !body.contains("__BRIEFCASE_"),
        "INSTALL",
        "Extension template placeholders were not substituted",
    )?;
    let digest = digest(&body);
    Ok(format!("{body}{DIGEST_MARKER}{digest}\n"))
}

/// A file Briefcase wrote and nobody edited since: its digest line matches its body.
fn managed_unedited(text: &str) -> bool {
    let Some(at) = text.rfind(DIGEST_MARKER) else {
        return false;
    };
    let recorded = text[at + DIGEST_MARKER.len()..].trim();
    digest(&text[..at]) == recorded
}

fn registers_context(text: &str) -> bool {
    regex::Regex::new(r#"name:\s*["'`]context["'`]"#)
        .unwrap()
        .is_match(text)
}

/// Extension entry points Pi discovers in one directory.
fn entry_points(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_file() && (name.ends_with(".ts") || name.ends_with(".js")) {
            found.push((name, path));
        } else if path.is_dir() {
            for index in ["index.ts", "index.js"] {
                if path.join(index).is_file() {
                    found.push((format!("{name}/{index}"), path.join(index)));
                }
            }
        }
    }
    found.sort();
    found
}

pub fn install(o: &InstallOptions) -> Result<Value> {
    let (root, extensions) = match &o.project {
        Some(project) => {
            let root = project.canonicalize()?.join(".pi");
            (root.clone(), root.join("extensions"))
        }
        None => {
            let root = agent_dir(o.agent_dir.as_deref())?;
            (root.clone(), root.join("extensions"))
        }
    };
    let target = extensions.join(EXTENSION);
    let rendered = render(&o.binary)?;
    let mut actions = Vec::new();
    let mut blocked = Vec::new();
    let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_dir = root.join("briefcase-backups").join(stamp.to_string());

    let existing = std::fs::read_to_string(&target).ok();
    let status = match &existing {
        Some(text) if *text == rendered => "already-configured",
        Some(text) if managed_unedited(text) => {
            backups.push((target.clone(), backup_dir.join(EXTENSION)));
            actions.push(json!({"action":"update","path":target}));
            "updated"
        }
        Some(_) if o.force => {
            backups.push((target.clone(), backup_dir.join(EXTENSION)));
            actions.push(json!({"action":"replace-edited","path":target}));
            "updated"
        }
        Some(_) => {
            blocked.push(json!({"path":target,"reason":"briefcase-pi.ts was edited locally; rerun with --force to back it up and replace it"}));
            "blocked"
        }
        None => {
            actions.push(json!({"action":"write","path":target}));
            "wired"
        }
    };
    for (name, path) in entry_points(&extensions) {
        if name == EXTENSION {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if !registers_context(&text) {
            continue;
        }
        blocked.push(json!({"path":path,"reason":"Another extension registers `context`; Briefcase will not modify it. Remove it (for example a retired snoop-pi.ts), then rerun"}));
    }
    let status = if blocked.is_empty() {
        status
    } else {
        "blocked"
    };
    let mut report = json!({"agent":"pi","status":status,"dry_run":o.dry_run,
    "extensions_dir":extensions,"extension":target,"binary":o.binary,
    "actions":actions,"blocked":blocked,
    "notes":[
        "Plain `pi` ensures the shared repository monitor exists on session start.",
        "No Pi trust, provider, credential or MCP setting was changed.",
        "MCP stays optional: configure `briefcase mcp` with an absolute BRIEFCASE_DB if a client should use it."
    ]});
    if o.dry_run || !blocked.is_empty() || status == "already-configured" {
        return Ok(report);
    }
    for (from, to) in &backups {
        workspace::private_dir(to.parent().unwrap())?;
        require(!to.exists(), "INSTALL", "Backup destination already exists")?;
        std::fs::rename(from, to)?;
    }
    workspace::private_dir(&extensions)?;
    workspace::write_atomic(&target, rendered.as_bytes())?;
    let contexts: Vec<String> = entry_points(&extensions)
        .into_iter()
        .filter(|(_, p)| registers_context(&std::fs::read_to_string(p).unwrap_or_default()))
        .map(|(n, _)| n)
        .collect();
    require(
        contexts == [EXTENSION],
        "INSTALL",
        &format!("Expected exactly one `context` extension, found {contexts:?}"),
    )?;
    if !backups.is_empty() {
        report["backups"] = json!(backups
            .iter()
            .map(|(from, to)| json!({"original":from,"backup":to}))
            .collect::<Vec<_>>());
        report["restore"] = json!(
            "Move a backup file back to its original path and remove briefcase-pi.ts to undo."
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(dir: &Path) -> InstallOptions {
        InstallOptions {
            agent_dir: Some(dir.to_path_buf()),
            project: None,
            binary: PathBuf::from("/opt/briefcase/bin/briefcase"),
            dry_run: false,
            force: false,
        }
    }

    #[test]
    fn install_is_idempotent_and_detects_edits() {
        let dir = tempfile::tempdir().unwrap();
        let o = options(dir.path());
        assert_eq!(install(&o).unwrap()["status"], "wired");
        assert_eq!(install(&o).unwrap()["status"], "already-configured");
        let target = dir.path().join("extensions").join(EXTENSION);
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(managed_unedited(&text));
        assert!(text.contains("/opt/briefcase/bin/briefcase"));
        assert!(text.contains("operation describe"));
        std::fs::write(
            &target,
            text.replace("Briefcase: monitoring", "Briefcase: ok"),
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
    }

    #[test]
    fn other_context_extensions_block_installation_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("extensions");
        std::fs::create_dir_all(&ext).unwrap();
        let other = "pi.registerTool({ name: \"context\" })\n";
        std::fs::write(ext.join("snoop-pi.ts"), other).unwrap();
        std::fs::write(ext.join("unrelated.ts"), "export default () => {}\n").unwrap();
        let report = install(&InstallOptions {
            force: true,
            ..options(dir.path())
        })
        .unwrap();
        assert_eq!(report["status"], "blocked");
        assert_eq!(
            std::fs::read_to_string(ext.join("snoop-pi.ts")).unwrap(),
            other
        );
        assert!(!ext.join(EXTENSION).exists());
        std::fs::remove_file(ext.join("snoop-pi.ts")).unwrap();
        assert_eq!(install(&options(dir.path())).unwrap()["status"], "wired");
        assert!(ext.join("unrelated.ts").is_file());
    }
}
