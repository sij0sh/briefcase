//! Claude Code history: header shape, store location and materialization.
//!
//! Claude Code records its working directory in the first record that
//! carries `cwd`, so discovery reads more than the first line. Transcripts
//! are single JSONL files, so materialization is the identity: the retained
//! bytes are the file bytes.

use super::Materialized;
use crate::architecture::Result;

/// Leading lines read to find the recorded cwd.
const HEADER_LINES: usize = 64;

/// The default history root: `$CLAUDE_CONFIG_DIR/projects` or
/// `~/.claude/projects`.
pub fn default_dir() -> Result<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(std::path::PathBuf::from(dir).join("projects"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("Cannot find the home directory; pass --sessions-dir")?;
    Ok(std::path::PathBuf::from(home).join(".claude/projects"))
}

/// The recorded working directory from the leading lines: the first record
/// carrying `cwd`. `None` is a malformed header.
pub fn cwd(lines: &[&str]) -> Option<Option<String>> {
    let records: Vec<serde_json::Value> = lines
        .iter()
        .filter_map(|l| crate::architecture::identity::parse(l.as_bytes()).ok())
        .collect();
    if records.is_empty() {
        return None;
    }
    Some(
        records
            .iter()
            .find_map(|v| v["cwd"].as_str().map(str::to_owned)),
    )
}

/// Every `.jsonl` transcript under `dir`.
pub fn candidates(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    super::files_with_extension(dir, "jsonl")
}

/// The recorded cwd from the transcript's leading lines.
pub fn recorded_cwd(path: &std::path::Path) -> Result<Option<Option<String>>> {
    let lines = super::leading_lines(path, HEADER_LINES)?;
    Ok(cwd(&lines.iter().map(String::as_str).collect::<Vec<_>>()))
}

/// Identity materialization: the transcript is already canonical JSONL.
pub fn materialize(path: &std::path::Path) -> Result<Materialized> {
    super::identity(path)
}
