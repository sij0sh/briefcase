//! Source classification, text limits, chunking and lexical query tokens.

/// Largest single source file a capture retains.
pub const MAX_SOURCE_BYTES: u64 = 10 * 1024 * 1024;

/// Extensions captured as code.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "pyi", "pyw", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "go", "java",
    "cs", "c", "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "h++", "ipp", "tpp", "inl",
    "inc", "def", "tcc", "txx", "ixx", "cppm", "rb", "rake", "gemspec", "php", "phtml", "sh",
    "bash", "gd", "tscn", "tres",
];

pub fn code_extension(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
    CODE_EXTENSIONS
        .iter()
        .copied()
        .find(|candidate| *candidate == extension)
}

/// Stable artifact roles for task composition, independent of language parsing.
pub const ARTIFACT_ROLES: &[&str] = &["impl", "test", "config", "script", "build", "install", "doc", "adr", "plan", "fixture", "generated"];
pub fn artifact_role(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let dirs: Vec<_> = lower.split('/').collect();
    if dirs.iter().any(|d| ["generated", "__generated__", "node_modules", "target", "dist"].contains(d))
        || name.contains(".generated.") || name.contains(".min.") { "generated" }
    else if dirs.iter().any(|d| ["fixture", "fixtures", "__fixtures__", "testdata", "golden", "goldens"].contains(d)) { "fixture" }
    else if dirs.iter().any(|d| ["adr", "adrs", "decisions"].contains(d)) || name.starts_with("adr-") { "adr" }
    else if dirs.iter().any(|d| ["plans", "plan", "proposals", "roadmap"].contains(d))
        || (dirs.contains(&"specs") && code_extension(path).is_none()) || name.contains("plan.") { "plan" }
    else if category(path) == "tests" || name.ends_with("_spec.rb") { "test" }
    else if name == "install" || name.starts_with("install.") || dirs.contains(&"install") || dirs.contains(&"installers") { "install" }
    else if ["build.rs", "makefile", "gnumakefile", "justfile", "cmakelists.txt", "pom.xml", "build.gradle", "build.gradle.kts", "package.json", "cargo.toml", "go.mod", "pyproject.toml", "setup.py"].contains(&name)
        || super::capture::is_dockerfile(path) || name.ends_with(".csproj") { "build" }
    else if dirs.iter().any(|d| ["scripts", "script", "bin", "hooks"].contains(d))
        || ["sh", "bash", "ps1", "bat", "cmd"].contains(&name.rsplit('.').next().unwrap_or("")) { "script" }
    else if dirs.iter().any(|d| ["docs", "doc", ".agents", ".codex"].contains(d))
        || ["md", "rst", "adoc"].contains(&name.rsplit('.').next().unwrap_or(""))
        || ["readme", "license", "copying", "agents.md"].contains(&name) { "doc" }
    else if code_extension(path).is_some() { "impl" }
    else { "config" }
}

/// Every captured language is inventoried even when its semantic profile did not run.
pub fn language(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "rs" => "rust", "py" | "pyi" | "pyw" => "python",
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => "typescript",
        "go" => "go", "java" => "java", "cs" => "csharp",
        "c" | "h" => "c", "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "sh" | "bash" => "shell", "ps1" => "powershell", "rb" => "ruby", "php" => "php",
        _ => "text",
    }
}

/// Bounded, quoted FTS5 terms joined with OR, without a column selector.
pub(crate) fn fts_expression(text: &str) -> Option<String> {
    let terms: Vec<_> = text
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|s| !s.is_empty())
        .take(64)
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| format!("({})", terms.join(" OR ")))
}

/// Classify a retained path or technical key without dropping any source.
pub fn category(id: &str) -> &'static str {
    if id.contains("://") || id.starts_with("http:") || (!id.contains('/') && !id.contains('.')) {
        // Data stores, endpoints, domains and services are not repository paths.
        return "application";
    }
    // A Rust file outside the Cargo module roots is keyed `unconfigured::<path>`.
    let id = id.strip_prefix("unconfigured::").unwrap_or(id);
    let path = id.split("::").next().unwrap_or(id).to_lowercase();
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let name = parts.pop().unwrap_or_default();
    let dirs = |set: &[&str]| parts.iter().any(|p| set.contains(p));
    if dirs(&[
        "fixtures",
        "fixture",
        "__fixtures__",
        "testdata",
        "test-data",
        "evaluation",
        "examples",
        "example",
        "samples",
        "sample",
        "__mocks__",
        "mocks",
        "golden",
        "goldens",
    ]) {
        "fixtures"
    } else if dirs(&[
        "test",
        "tests",
        "__tests__",
        "spec",
        "specs",
        "e2e",
        "testing",
    ]) || [".test.", ".spec.", "_test.", "_spec."]
        .iter()
        .any(|s| name.contains(s))
        || name.starts_with("test_")
        || name.ends_with("test.java")
        || name.ends_with("tests.cs")
    {
        "tests"
    } else if parts.first().is_some_and(|p| {
        [
            "tools", "tool", "scripts", "script", ".github", "ci", "hack",
        ]
        .contains(p)
    }) || ["build.rs", "setup.py", "makefile", "dockerfile", "justfile"].contains(&name)
    {
        "tooling"
    } else if dirs(&["docs", "doc", "generated", "__generated__"])
        || name.ends_with(".md")
        || name.contains(".generated.")
        || name.contains(".min.")
    {
        "other"
    } else {
        "application"
    }
}

/// The first `::` key component, not a containing-file or holding-file join.
pub(crate) fn module_path(key: &str) -> &str {
    key.split("::").next().unwrap_or(key)
}

/// Approximate model tokens: four characters each, at least one.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4).max(1)
}

/// Split text into pieces of at most `max_chars` characters, preferring to
/// end a piece after a full stop or newline. Returns pieces with byte ranges.
pub fn split_oversized(text: &str, max_chars: usize) -> Vec<(String, usize, usize)> {
    let max_chars = max_chars.max(1);
    let mut output = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let candidate_end = text[start..]
            .char_indices()
            .nth(max_chars)
            .map(|(offset, _)| start + offset)
            .unwrap_or(text.len());
        let mut end = candidate_end;
        if end < text.len() {
            if let Some((offset, character)) = text[start..end]
                .char_indices()
                .rev()
                .find(|(_, character)| matches!(character, '.' | '\n'))
            {
                end = start + offset + character.len_utf8();
            }
        }
        if end <= start {
            end = candidate_end.max(start + 1).min(text.len());
        }
        output.push((text[start..end].to_string(), start, end));
        start = end;
    }
    output
}

/// Reciprocal-rank fusion constant.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_categories_and_key_prefixes_preserve_existing_rules() {
        for (key, expected) in [
            ("src/app.rs", "application"),
            ("demo::child", "application"),
            ("http:POST /orders", "application"),
            ("postgresql://app/orders", "application"),
            ("evaluation/sources/app.ts", "fixtures"),
            ("examples/demo.rs", "fixtures"),
            ("tests/app.rs", "tests"),
            ("src/app.test.ts", "tests"),
            ("unconfigured::tests/app.rs", "tests"),
            ("scripts/run.py", "tooling"),
            ("build.rs", "tooling"),
            ("docs/guide.md", "other"),
            ("src/app.generated.ts", "other"),
        ] {
            assert_eq!(category(key), expected, "{key}");
        }
        assert_eq!(module_path("src/app.ts::run#12"), "src/app.ts");
        assert_eq!(module_path("demo::child::run"), "demo");
        assert_eq!(module_path("plain"), "plain");
    }

    #[test]
    fn fts_terms_preserve_boundaries_quotes_and_the_term_limit() {
        for (input, expected) in [
            ("", None),
            (" . ! \" ", None),
            ("hello-world", Some("(\"hello\" OR \"world\")")),
            ("café_雪 'quoted'", Some("(\"café_雪\" OR \"quoted\")")),
            ("a a", Some("(\"a\" OR \"a\")")),
        ] {
            assert_eq!(fts_expression(input).as_deref(), expected);
        }
        let many = (0..70)
            .map(|n| format!("t{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let expression = fts_expression(&many).unwrap();
        assert_eq!(expression.matches(" OR ").count(), 63);
        assert!(expression.ends_with("\"t63\")"));
        assert!(!expression.contains("t64"));
    }

    #[test]
    fn split_prefers_sentence_ends_and_keeps_utf8() {
        let pieces = split_oversized("é one. two three. four", 8);
        assert!(pieces
            .iter()
            .all(|(p, s, e)| p.chars().count() <= 8 && &"é one. two three. four"[*s..*e] == p));
        assert_eq!(pieces[0].0, "é one.");
        assert_eq!(code_extension("src/A.TSX"), Some("tsx"));
        assert_eq!(code_extension("README.md"), None);
    }
}
