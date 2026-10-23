//! The harness registry: the single entry point for harness differences.
//!
//! The session core (`jsonl`, `profile`, `normalize`, `paths`, `import`,
//! `inspect`) is harness-agnostic. Everything that differs per harness lives
//! here: the name, the built-in profile, the default history directory,
//! candidate enumeration, the recorded cwd of a candidate, and the
//! materialization from a history candidate to canonical JSONL bytes.
//!
//! Every indexing path — explicit `import_local`, sync `check`/`apply`, the
//! monitor's watch hints and the scheduler's dirty domains — resolves
//! harnesses through this registry. Adding a harness means adding one
//! converter module, one profile and one `Harness` variant; no caller
//! branches on harness names directly.
//!
//! A candidate is whatever path names one session in the store: a file for
//! single-JSONL stores, a directory or database key path otherwise. Each
//! harness enumerates its own candidates and reads each candidate's
//! recorded cwd without materializing it, so discovery stays cheap.
//!
//! Materialization converts one candidate to the JSONL bytes the core
//! ingests, and is the only byte source for import digests and sync
//! freshness. Pi, Codex and Claude Code transcripts are already single
//! JSONL files, so their materializers are the identity. Stores that are
//! not single JSONL files (a directory of JSON parts, a database plus log)
//! materialize to canonical JSONL first, so ordinals, byte ranges, digests
//! and pairing rules stay exactly the transport's. A materializer rejects
//! a candidate over `MAX_BYTES` as early as its store allows; the registry
//! also checks the materialized length.
pub mod claude_code;
pub mod codex;
pub mod pi;

use crate::architecture::{session::MAX_BYTES, Result};
use std::path::{Path, PathBuf};

/// A harness's recorded-cwd parser over its leading lines.
type CwdParser = fn(&[&str]) -> Option<Option<String>>;

/// A history candidate converted to the canonical JSONL bytes the core ingests.
pub struct Materialized {
    /// The JSONL bytes fed to `jsonl::read` and retained by `import`.
    pub bytes: Vec<u8>,
    /// The converter name and version (for example `identity-v1`), reported
    /// per session and never mixed into the retained bytes.
    pub materializer: &'static str,
    /// The history file the bytes came from.
    pub source: std::path::PathBuf,
}

/// A harness whose local history store `import_local` can discover.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    Pi,
    Codex,
    ClaudeCode,
}

impl std::str::FromStr for Harness {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "pi" => Ok(Self::Pi),
            "codex" => Ok(Self::Codex),
            "claude-code" => Ok(Self::ClaudeCode),
            _ => Err(format!(
                "ARGUMENTS: Unknown harness {s}; use {}",
                Self::names().join(", ")
            )),
        }
    }
}

impl Harness {
    /// Every registered harness, in CLI help order.
    pub fn all() -> [Self; 3] {
        [Self::Pi, Self::Codex, Self::ClaudeCode]
    }

    /// Every registered harness name, for help text and error messages.
    pub fn names() -> [&'static str; 3] {
        ["pi", "codex", "claude-code"]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }

    pub fn profile(self) -> &'static str {
        match self {
            Self::Pi => "pi-v1",
            Self::Codex => "codex-v1",
            Self::ClaudeCode => "claude-code-v1",
        }
    }

    /// The default history directory for this harness.
    pub fn default_dir(self) -> Result<std::path::PathBuf> {
        match self {
            Self::Pi => pi::default_dir(),
            Self::Codex => codex::default_dir(),
            Self::ClaudeCode => claude_code::default_dir(),
        }
    }

    /// Every `.jsonl` session candidate in this harness's store under `dir`,
    /// sorted. Symlinks are never followed.
    pub fn candidates(self, dir: &Path) -> Result<Vec<PathBuf>> {
        files_with_extension(dir, "jsonl")
    }

    /// The recorded working directory of one candidate, read without
    /// materializing it. `None` is a malformed header; `Some(None)` is a
    /// well-formed header without a cwd.
    pub fn recorded_cwd(self, candidate: &Path) -> Result<Option<Option<String>>> {
        let (count, parse): (usize, CwdParser) = match self {
            Self::Pi => (pi::HEADER_LINES, pi::cwd),
            Self::Codex => (codex::HEADER_LINES, codex::cwd),
            Self::ClaudeCode => (claude_code::HEADER_LINES, claude_code::cwd),
        };
        let lines = leading_lines(candidate, count)?;
        Ok(parse(&lines.iter().map(String::as_str).collect::<Vec<_>>()))
    }

    /// Convert one candidate to canonical JSONL bytes, bounded by `MAX_BYTES`.
    pub fn materialize(self, candidate: &Path) -> Result<Materialized> {
        let m = identity(candidate)?;
        crate::architecture::require(
            m.bytes.len() <= MAX_BYTES,
            "SESSION",
            "Session transcript byte limit exceeded",
        )?;
        Ok(m)
    }
}

// ---- Shared helpers for single-JSONL stores -----------------------------------------

/// Regular files with `extension` anywhere under `dir`, sorted. Symlinked
/// files and directories are skipped.
pub(crate) fn files_with_extension(dir: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let mut pending = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                pending.push(entry.path());
            } else if ty.is_file() && entry.path().extension().is_some_and(|e| e == extension) {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Up to `count` leading lines of a JSONL file, reading at most 256 KiB.
pub(crate) fn leading_lines(path: &Path, count: usize) -> Result<Vec<String>> {
    use std::io::{BufRead, BufReader, Read};
    Ok(BufReader::new(std::fs::File::open(path)?)
        .take(256 * 1024)
        .lines()
        .take(count)
        .map_while(std::result::Result::ok)
        .collect())
}

/// Identity materialization of a single JSONL file, rejected by size before
/// it is read.
pub(crate) fn identity(path: &Path) -> Result<Materialized> {
    crate::architecture::require(
        std::fs::metadata(path)?.len() <= MAX_BYTES as u64,
        "SESSION",
        "Session transcript byte limit exceeded",
    )?;
    Ok(Materialized {
        bytes: std::fs::read(path)?,
        materializer: "identity-v1",
        source: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_names_profiles_and_cli_help_agree() {
        assert_eq!(Harness::all().map(|h| h.name()), Harness::names());
        assert_eq!(
            Harness::all().map(|h| h.profile()),
            ["pi-v1", "codex-v1", "claude-code-v1"]
        );
        for name in Harness::names() {
            let h: Harness = name.parse().expect("registry name parses");
            assert_eq!(h.name(), name);
        }
        assert!("muse".parse::<Harness>().is_err());
    }

    #[test]
    fn pi_header_carries_cwd_and_rejects_other_types() {
        let line = r#"{"type":"session","cwd":"/work/shop","id":"abc"}"#;
        assert_eq!(pi::cwd(&[line]), Some(Some("/work/shop".to_owned())));
        assert!(pi::cwd(&[]).is_none());
        assert!(pi::cwd(&[r#"{"type":"session_meta","payload":{"cwd":"/work/shop"}}"#]).is_none());
        // A header without cwd is well-formed but matches nothing.
        assert_eq!(pi::cwd(&[r#"{"type":"session"}"#]), Some(None));
    }

    #[test]
    fn codex_header_reads_the_session_meta_payload() {
        let line = r#"{"type":"session_meta","payload":{"cwd":"/work/shop","id":"abc"}}"#;
        assert_eq!(codex::cwd(&[line]), Some(Some("/work/shop".to_owned())));
        assert!(codex::cwd(&[]).is_none());
        assert!(codex::cwd(&[r#"{"type":"session","cwd":"/work/shop"}"#]).is_none());
    }

    #[test]
    fn claude_code_header_uses_the_first_record_with_cwd() {
        let lines = [
            r#"{"type":"summary","leafUuid":"x"}"#,
            r#"{"type":"user","cwd":"/work/shop","message":{"role":"user"}}"#,
        ];
        assert_eq!(
            claude_code::cwd(&lines),
            Some(Some("/work/shop".to_owned()))
        );
        assert!(claude_code::cwd(&[]).is_none());
        // A parseable header without cwd is well-formed but matches nothing.
        assert_eq!(
            claude_code::cwd(&[r#"{"type":"summary","leafUuid":"x"}"#]),
            Some(None)
        );
    }

    #[test]
    fn registry_reads_each_harness_recorded_cwd() {
        for (harness, body) in [
            (
                Harness::Pi,
                "{\"type\":\"session\",\"cwd\":\"/work/shop\"}\n",
            ),
            (
                Harness::Codex,
                "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/work/shop\"}}\n",
            ),
            (
                Harness::ClaudeCode,
                "{\"type\":\"summary\"}\n{\"type\":\"user\",\"cwd\":\"/work/shop\"}\n",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.jsonl");
            std::fs::write(&path, body).unwrap();
            assert_eq!(
                harness.recorded_cwd(&path).unwrap(),
                Some(Some("/work/shop".to_owned())),
                "{harness:?}"
            );
            std::fs::write(&path, "not JSON\n").unwrap();
            assert_eq!(harness.recorded_cwd(&path).unwrap(), None, "{harness:?}");
        }
    }

    #[test]
    fn identity_materializers_return_exact_file_bytes() {
        for harness in Harness::all() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.jsonl");
            let bytes = b"{\"type\":\"session\",\"cwd\":\"/work/shop\"}\n";
            std::fs::write(&path, bytes).unwrap();
            let m = harness.materialize(&path).unwrap();
            assert_eq!(m.bytes, bytes);
            assert_eq!(m.materializer, "identity-v1");
            assert_eq!(m.source, path);
        }
    }
}
