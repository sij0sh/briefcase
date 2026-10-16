//! Deterministic session path resolution. Paths are interpreted lexically
//! against retained session metadata, never against today's filesystem:
//! no canonicalization, no symlink resolution, no existence checks.
use serde::Serialize;

/// Where a recorded path lies relative to the repository root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Resolved {
    /// `repository` (relative to the root), `external` (absolute, outside
    /// the root) or `unresolved` (relative with no recorded working
    /// directory, or escaping the filesystem root).
    pub location: &'static str,
    /// Repository-relative path, normalized absolute path, or the raw text.
    pub path: String,
}

/// Dialect follows retained root syntax, independent of the importing OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathDialect {
    Posix,
    Windows,
}
impl PathDialect {
    pub fn of(path: &str) -> Self {
        if path.starts_with("\\\\")
            || path.starts_with("//")
            || (path.as_bytes().get(1) == Some(&b':'))
        {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

/// Split an absolute path into its filesystem anchor and lexical components.
fn absolute(path: &str, dialect: PathDialect) -> Option<(String, Vec<String>)> {
    let path = match dialect {
        PathDialect::Posix => path.to_owned(),
        PathDialect::Windows => path.replace('\\', "/"),
    };
    let (anchor, tail) = match dialect {
        PathDialect::Posix => ("/".to_owned(), path.strip_prefix('/')?),
        PathDialect::Windows if path.starts_with("//") => {
            let mut pieces = path[2..].split('/');
            let server = pieces.next()?;
            let share = pieces.next()?;
            if [server, share]
                .iter()
                .any(|s| s.is_empty() || [".", "..", "?"].contains(s))
            {
                return None;
            }
            (
                format!("//{server}/{share}/"),
                &path[2 + server.len() + 1 + share.len()..],
            )
        }
        PathDialect::Windows => {
            let bytes = path.as_bytes();
            if bytes.len() < 3
                || !bytes[0].is_ascii_alphabetic()
                || bytes[1] != b':'
                || bytes[2] != b'/'
            {
                return None;
            }
            (
                format!("{}:/", (bytes[0] as char).to_ascii_uppercase()),
                &path[3..],
            )
        }
    };
    let mut parts = Vec::new();
    for part in tail.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p.to_owned()),
        }
    }
    Some((anchor, parts))
}

/// Lexically normalize an absolute POSIX, drive-qualified Windows or UNC path.
/// Escaping the filesystem anchor, drive-relative and device paths are unresolved.
pub fn normalize(path: &str) -> Option<String> {
    let (anchor, parts) = absolute(path, PathDialect::of(path))?;
    Some(format!("{anchor}{}", parts.join("/")))
}

fn join(base: &str, relative: &str, dialect: PathDialect) -> Option<String> {
    let (anchor, mut parts) = absolute(base, dialect)?;
    let relative = if dialect == PathDialect::Windows {
        if relative.starts_with(['\\', '/']) || relative.contains(':') {
            return None;
        }
        relative.replace('\\', "/")
    } else {
        relative.to_owned()
    };
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p.to_owned()),
        }
    }
    Some(format!("{anchor}{}", parts.join("/")))
}

pub fn resolve(
    raw: &str,
    event_cwd: Option<&str>,
    session_cwd: Option<&str>,
    root: &str,
) -> Resolved {
    let unresolved = || Resolved {
        location: "unresolved",
        path: raw.to_owned(),
    };
    if raw.is_empty() {
        return unresolved();
    }
    let dialect = PathDialect::of(root);
    let Some(root) = normalize(root) else {
        return unresolved();
    };
    let absolute = if let Some((anchor, parts)) = absolute(raw, dialect) {
        Some(format!("{anchor}{}", parts.join("/")))
    } else if raw.starts_with('/')
        || (dialect == PathDialect::Windows && (raw.starts_with('\\') || raw.contains(':')))
    {
        None
    } else {
        [event_cwd, session_cwd]
            .into_iter()
            .flatten()
            .find_map(|base| join(base, raw, dialect))
    };
    let Some(absolute) = absolute else {
        return unresolved();
    };
    let root_prefix = format!("{}/", root.trim_end_matches('/'));
    let equal = |a: &str, b: &str| {
        if dialect == PathDialect::Windows {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let relative = if equal(&absolute, &root) {
        Some(".".to_owned())
    } else if absolute
        .get(..root_prefix.len())
        .is_some_and(|p| equal(p, &root_prefix))
    {
        Some(absolute[root_prefix.len()..].to_owned())
    } else {
        None
    };
    match relative {
        Some(path) => Resolved {
            location: "repository",
            path,
        },
        None => Resolved {
            location: "external",
            path: absolute,
        },
    }
}

pub fn directory(raw: &str, session_cwd: Option<&str>) -> Option<String> {
    normalize(raw).or_else(|| session_cwd.and_then(|base| join(base, raw, PathDialect::of(base))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_is_lexical_and_scoped_to_the_root() {
        let r = |raw, cwd| resolve(raw, cwd, Some("/repo"), "/repo");
        assert_eq!(r("/repo/src/a.rs", None).path, "src/a.rs");
        assert_eq!(r("src/./b/../a.rs", None).path, "src/a.rs");
        assert_eq!(r("a.rs", Some("/repo/src")).path, "src/a.rs");
        assert_eq!(r("/repo", None).path, ".");
        assert_eq!(r("/repository/x", None).location, "external");
        assert_eq!(r("/etc/passwd", None).location, "external");
        assert_eq!(r("../other/x", None).path, "/other/x");
        assert_eq!(resolve("a.rs", None, None, "/repo").location, "unresolved");
        assert_eq!(
            resolve("/../../x", None, None, "/repo").location,
            "unresolved"
        );
    }

    #[test]
    fn windows_drives_unc_and_ambiguous_paths_are_lexical() {
        let root = r"C:\work\repo";
        let r = |raw, cwd| resolve(raw, cwd, Some(root), root);
        assert_eq!(r(r"c:\WORK\repo\src\a.cs", None).path, "src/a.cs");
        assert_eq!(r(r"src\folder\..\a.cs", None).path, "src/a.cs");
        assert_eq!(r("a.cs", Some(r"C:\work\repo\src")).path, "src/a.cs");
        assert_eq!(r(r"D:\work\repo\a.cs", None).location, "external");
        assert_eq!(r(r"C:src\a.cs", None).location, "unresolved");
        assert_eq!(r(r"\src\a.cs", None).location, "unresolved");
        assert_eq!(r(r"C:\..\a.cs", None).location, "unresolved");
        assert_eq!(r(r"C:\work\repository\a.cs", None).location, "external");
        let unc = r"\\server\share\repo";
        assert_eq!(resolve(r"src\a.cs", None, Some(unc), unc).path, "src/a.cs");
        assert_eq!(
            resolve(r"\\other\share\a.cs", None, Some(unc), unc).location,
            "external"
        );
        assert!(normalize(r"\\server\share\..\a.cs").is_none());
        assert!(normalize(r"\\?\C:\repo").is_none());
        assert_eq!(
            resolve(r"src\a.rs", None, Some("/repo"), "/repo").path,
            r"src\a.rs",
            "POSIX backslash is literal"
        );
        assert_eq!(
            resolve(r"\a.rs", None, Some("/repo"), "/repo").path,
            r"\a.rs"
        );
    }
}

/// Parse only a direct POSIX command. Shell compositions, substitutions and
/// redirections have no interpreted executable identity.
pub(crate) fn direct_command(command:&str)->Option<Vec<String>> {
    let mut words=vec![];let mut word=String::new();let mut quote=None;let mut escaped=false;
    for ch in command.trim().chars() {
        if escaped {word.push(ch);escaped=false;continue;}
        if ch=='\\' && quote!=Some('\'') {escaped=true;continue;}
        if ch=='`'||(ch=='$'&&quote!=Some('\'')) {return None;}
        if let Some(q)=quote {if ch==q {quote=None;}else{word.push(ch);}}
        else if ch=='\''||ch=='"' {quote=Some(ch);}
        else if [';','&','|','<','>','(',')','\n','\r'].contains(&ch) {return None;}
        else if ch.is_whitespace() {if !word.is_empty(){words.push(std::mem::take(&mut word));}}
        else {word.push(ch);}
    }
    if quote.is_some()||escaped{return None;}
    if !word.is_empty(){words.push(word);}
    (!words.is_empty()).then_some(words)
}
