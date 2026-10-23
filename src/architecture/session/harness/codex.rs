//! Codex history: header shape and store location.
//!
//! Codex records its working directory and session id in the first line's
//! `session_meta` payload. Transcripts are single JSONL files, so the
//! registry's shared materialization is the identity: the retained bytes are
//! the file bytes.

use crate::architecture::Result;

/// Leading lines read to find the recorded cwd.
pub(super) const HEADER_LINES: usize = 1;

/// The default history root: `$CODEX_HOME/sessions` or
/// `~/.codex/sessions`.
pub fn default_dir() -> Result<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        return Ok(std::path::PathBuf::from(dir).join("sessions"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("Cannot find the home directory; pass --sessions-dir")?;
    Ok(std::path::PathBuf::from(home).join(".codex/sessions"))
}

/// The recorded working directory from the leading lines: the first line's
/// `session_meta` payload. `None` is a malformed header.
pub fn cwd(lines: &[&str]) -> Option<Option<String>> {
    let first = lines.first()?;
    crate::architecture::identity::parse(first.as_bytes())
        .ok()
        .filter(|v| v["type"] == "session_meta")
        .map(|v| v["payload"]["cwd"].as_str().map(str::to_owned))
}
