//! Retained Git history for recall.
//!
//! Commits are durable once stored: a rebase or branch switch changes the
//! current reachability membership, never the retained history. Each commit
//! commits in its own transaction, so a budget-limited run keeps its work and
//! the next sync continues. A commit message is narrative written by people;
//! it is retained text, never architecture Evidence.
//!
//! Git runs locally with external diff drivers, text conversion, fsmonitor
//! hooks, pagers and prompts disabled. Briefcase never fetches.
use super::{identity, ledger::Ledger, Result};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

/// Newest reachable commits that are retained and searchable.
pub const MAX_COMMITS: usize = 5000;
pub const MAX_FILES_PER_COMMIT: usize = 64;
pub const MAX_PATCH_BYTES: usize = 16 * 1024;
const MAX_UNIT_BYTES: usize = 1536;
const MAX_MESSAGE_BYTES: usize = 4096;

pub(crate) fn configuration() -> Value {
    json!({"representation":"briefcase-git-history-recall-v1","max_commits":MAX_COMMITS,
        "max_files_per_commit":MAX_FILES_PER_COMMIT,"max_patch_bytes":MAX_PATCH_BYTES,
        "max_unit_bytes":MAX_UNIT_BYTES,"max_message_bytes":MAX_MESSAGE_BYTES,"diff":"diff-tree -M -U3 --no-ext-diff --no-textconv",
        "implementation_digest":identity::bytes_digest(include_bytes!("git_history.rs"))})
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.quotepath=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.external=",
            "-c",
            "log.showSignature=false",
            "--no-pager",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_EXTERNAL_DIFF")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("GIT: git could not run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "GIT: git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub timestamp: i64,
    pub message: String,
}

pub struct Change {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub patch: String,
    pub truncated: bool,
}

fn commit_metadata(root: &Path, oid: &str) -> Result<Commit> {
    let out = git(
        root,
        &["show", "-s", "--format=%H%x1f%P%x1f%an%x1f%ct%x1f%B", oid],
    )?;
    let mut fields = out.splitn(5, '\x1f');
    let mut next = || fields.next().unwrap_or("").to_owned();
    let oid = next().trim().to_owned();
    let parents = next().split_whitespace().map(str::to_owned).collect();
    let author = next();
    let timestamp = next().trim().parse().unwrap_or(0);
    let message = next().trim_end().to_owned();
    Ok(Commit {
        oid,
        parents,
        author,
        timestamp,
        message,
    })
}

fn bounded(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_owned(), false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

/// Changed files against the first parent, renames detected, with bounded patches.
fn changes(root: &Path, commit: &Commit) -> Result<(Vec<Change>, usize)> {
    // Merges are compared with their first parent; a root commit with the empty tree.
    let trees: Vec<&str> = match commit.parents.first() {
        Some(parent) => vec![parent.as_str(), commit.oid.as_str()],
        None => vec!["--root", commit.oid.as_str()],
    };
    let run = |extra: &[&str]| {
        let mut args = vec!["diff-tree", "--no-commit-id", "-r", "-M"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&trees);
        git(root, &args)
    };
    let names = run(&["--name-status", "-z"])?;
    let patch = run(&["-p", "-U3", "--no-ext-diff", "--no-textconv"])?;
    // Patches and name-status rows share tree order.
    let mut patches: Vec<String> = Vec::new();
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            patches.push(String::new());
        }
        if let Some(current) = patches.last_mut() {
            current.push_str(line);
        }
    }
    let mut fields = names.split('\0').filter(|f| !f.is_empty());
    let mut rows = Vec::new();
    while let Some(status) = fields.next() {
        let (path, old_path) = if status.starts_with('R') || status.starts_with('C') {
            let old = fields.next().unwrap_or("").to_owned();
            (fields.next().unwrap_or("").to_owned(), Some(old))
        } else {
            (fields.next().unwrap_or("").to_owned(), None)
        };
        rows.push((
            status.chars().next().unwrap_or('M').to_string(),
            path,
            old_path,
        ));
    }
    let total = rows.len();
    let paired = rows.len() == patches.len();
    let mut out = Vec::new();
    for (index, (status, path, old_path)) in rows.into_iter().enumerate().take(MAX_FILES_PER_COMMIT)
    {
        let raw = if paired { patches[index].as_str() } else { "" };
        // Keep hunks and drop the git header lines the path already names.
        let body: String = raw
            .split_inclusive('\n')
            .skip_while(|l| !l.starts_with("@@") && !l.starts_with("Binary files"))
            .collect();
        let (patch, truncated) = bounded(&body, MAX_PATCH_BYTES);
        out.push(Change {
            path,
            old_path,
            status,
            patch,
            truncated,
        });
    }
    Ok((out, total))
}

/// Searchable units: one or more per changed file, each led by the commit's
/// subject and the file. The full message is kept in the first unit only so
/// one commit's narrative is not counted once per file.
pub fn units(commit: &Commit, changes: &[Change]) -> Vec<(String, String)> {
    let short = &commit.oid[..commit.oid.len().min(8)];
    let subject = commit.message.lines().next().unwrap_or("").trim();
    let body = commit
        .message
        .split_once('\n')
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    let mut out = Vec::new();
    let head = format!("commit {short} {subject}\n");
    let narrative = if body.is_empty() {
        String::new()
    } else {
        format!("\n{}\n", bounded(body, MAX_MESSAGE_BYTES).0)
    };
    if changes.is_empty() {
        out.push((String::new(), format!("{head}{narrative}")));
        return out;
    }
    for change in changes {
        let file = match &change.old_path {
            Some(old) => format!("\n{} (renamed from {old})\n\n", change.path),
            None => format!("\n{}\n\n", change.path),
        };
        let short_lead = format!("{head}{file}");
        let budget = MAX_UNIT_BYTES.saturating_sub(short_lead.len()).max(256);
        let mut part = String::new();
        let flush = |part: &mut String, out: &mut Vec<(String, String)>| {
            // Only the commit's first unit carries the message body.
            let lead = if out.is_empty() {
                format!("{head}{narrative}{file}")
            } else {
                short_lead.clone()
            };
            out.push((change.path.clone(), format!("{lead}{part}")));
            part.clear();
        };
        for line in change.patch.split_inclusive('\n') {
            if !part.is_empty() && part.len() + line.len() > budget {
                flush(&mut part, &mut out);
            }
            let (line, _) = bounded(line, budget);
            part.push_str(&line);
        }
        if !part.is_empty() || change.patch.is_empty() {
            flush(&mut part, &mut out);
        }
    }
    out
}

fn rev_list(root: &Path, range: &str) -> Result<Vec<String>> {
    let max = format!("--max-count={MAX_COMMITS}");
    Ok(git(root, &["rev-list", &max, range])?
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Resolve HEAD locally. Distinguish an unborn repository from unavailable Git.
pub fn head(root: &Path) -> Result<Option<String>> {
    if !root.join(".git").exists() {
        return Ok(None);
    }
    git(root, &["--version"])?;
    if let Ok(head) = git(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]) {
        return Ok(Some(head.trim().to_owned()));
    }
    // A missing HEAD in an empty repository is expected; other failures are diagnostics.
    if git(root, &["rev-parse", "--is-inside-work-tree"])?.trim() == "true" {
        return Ok(None);
    }
    Err("GIT: Could not resolve repository HEAD".into())
}

/// Bring retained history and reachability up to HEAD, stopping at `deadline`.
pub fn update(l: &Ledger, root: &Path, deadline: Instant) -> Result<Value> {
    if !root.join(".git").exists() {
        l.history_replace_reach(&[], &identity::digest(&configuration())?)?;
        l.clear_history_tip()?;
        return Ok(json!({"status":"not_a_repository"}));
    }
    git(root, &["--version"]).map_err(|e| format!("GIT_UNAVAILABLE: {e}"))?;
    let Ok(head) = git(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]) else {
        l.history_replace_reach(&[], &identity::digest(&configuration())?)?;
        l.clear_history_tip()?;
        return Ok(json!({"status":"no_commits"}));
    };
    let head = head.trim().to_owned();
    let config = identity::digest(&configuration())?;
    let previous = l.history_tip()?;
    if previous.as_ref() == Some(&(head.clone(), config.clone())) {
        return Ok(
            json!({"status":"up-to-date","tip":head,"commits":l.history_reachable_count()?}),
        );
    }
    // A valid previous tip under the same configuration allows an
    // incremental walk; otherwise reachability is recomputed in full.
    let incremental = previous.as_ref().is_some_and(|(tip, c)| {
        *c == config && git(root, &["merge-base", "--is-ancestor", tip, &head]).is_ok()
    });
    let reachable = if incremental {
        let tip = &previous.as_ref().unwrap().0;
        let new = rev_list(root, &format!("{tip}..{head}"))?;
        l.history_extend_reach(&new)?;
        new
    } else {
        let all = rev_list(root, &head)?;
        l.history_replace_reach(&all, &config)?;
        all
    };
    let stored: HashSet<String> = l.history_stored(&config)?;
    let missing: Vec<&String> = reachable.iter().filter(|o| !stored.contains(*o)).collect();
    let mut added = 0;
    let mut truncated_commits = 0;
    for oid in &missing {
        if Instant::now() >= deadline {
            return Ok(json!({"status":"partial","tip":head,"added":added,
                "remaining":missing.len()-added,"message":"Budget exhausted; the next sync continues"}));
        }
        let commit = commit_metadata(root, oid)?;
        let (changes, total) = changes(root, &commit)?;
        if total > changes.len() || changes.iter().any(|c| c.truncated) {
            truncated_commits += 1;
        }
        let units = units(&commit, &changes);
        l.store_commit(&commit, &changes, total, &units, &config)?;
        added += 1;
    }
    l.set_history_tip(&head, &config)?;
    Ok(
        json!({"status":if added > 0 || !incremental {"refreshed"} else {"up-to-date"},
        "tip":head,"added":added,"commits":l.history_reachable_count()?,
        "bounded_commits":truncated_commits,
        "limits":{"max_commits":MAX_COMMITS,"max_files_per_commit":MAX_FILES_PER_COMMIT,"max_patch_bytes":MAX_PATCH_BYTES}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_lead_with_subject_and_keep_the_message_once() {
        let commit = Commit {
            oid: "0123456789abcdef".into(),
            parents: vec![],
            author: "a".into(),
            timestamp: 0,
            message: "Add backoff\n\nWhy: uploads flapped.".into(),
        };
        let change = |path: &str| Change {
            path: path.into(),
            old_path: None,
            status: "M".into(),
            patch: format!("@@ -1 +1 @@\n{}", "+x\n".repeat(1000)),
            truncated: false,
        };
        let units = units(&commit, &[change("a.rs"), change("b.rs")]);
        assert!(units.len() > 2);
        assert!(units
            .iter()
            .all(|(_, t)| t.starts_with("commit 01234567 Add backoff\n")));
        assert_eq!(
            units
                .iter()
                .filter(|(_, t)| t.contains("uploads flapped"))
                .count(),
            1
        );
        // The message body may extend the first unit; later units stay bounded.
        assert!(units
            .iter()
            .skip(1)
            .all(|(_, t)| t.len() <= MAX_UNIT_BYTES + 64));
    }
}
