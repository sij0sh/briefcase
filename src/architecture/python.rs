//! CPython AST/symbol-table producer over retained .py/.pyi/.pyw inputs.
//! Python reports occurrences; this boundary validates and publishes knowledge.
use super::{
    capture::Capture,
    contracts::strings,
    execution, identity,
    ledger::{Ledger, Prepared},
    native_host, query, require, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct ScanOptions {
    /// Operator-selected interpreter. Repository configuration cannot choose it.
    pub python: PathBuf,
    /// Retained import roots. Empty selects src plus the root, or just the root.
    pub source_roots: Vec<String>,
    pub timeout: Duration,
    pub calls: bool,
    pub syntax_only: bool,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            python: std::env::var_os("BRIEFCASE_PYTHON")
                .map(Into::into)
                .unwrap_or_else(|| "python3".into()),
            source_roots: vec![],
            timeout: Duration::from_secs(60),
            calls: false,
            syntax_only: false,
        }
    }
}

pub(crate) fn is_source(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|p| p.to_str())
        .is_some_and(|p| ["py", "pyi", "pyw"].contains(&p.to_ascii_lowercase().as_str()))
}
fn interpreter(path: &Path) -> Result<PathBuf> {
    let executable = if path.is_file() {
        path.to_owned()
    } else {
        std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join(path))
                    .find(|p| p.is_file())
            })
            .ok_or(
                "ANALYZER: Python interpreter was not found; use --python or BRIEFCASE_PYTHON",
            )?
    };
    Ok(executable.canonicalize()?)
}
fn output(capture: Option<&Capture>, options: &ScanOptions) -> Result<Value> {
    require(
        options.timeout >= Duration::from_secs(1) && options.timeout <= Duration::from_secs(600),
        "ANALYZER",
        "Python timeout must be 1..600 seconds",
    )?;
    let executable = interpreter(&options.python)?;
    let executable_digest = identity::bytes_digest(&std::fs::read(&executable)?);
    let temp = native_host::scratch()?;
    let script = temp.path().join("python.py");
    std::fs::write(&script, include_bytes!("python.py"))?;
    let files: BTreeMap<_, _> = capture
        .into_iter()
        .flat_map(|c| &c.files)
        .map(|(path, bytes)| Ok((path, std::str::from_utf8(bytes)?)))
        .collect::<Result<_>>()?;
    let input = serde_json::to_vec(&json!({"schema_version":"1.0","files":files,
        "source_roots":options.source_roots,"syntax_only":options.syntax_only,
        "identity_only":capture.is_none()}))?;
    // Preserve the interpreter's existing environment policy. Isolated mode
    // ignores PYTHONPATH/PYTHONHOME and user site packages; -S blocks startup
    // hooks and -B emits no pyc. The shared runner owns process groups and IO.
    let mut command = Command::new(&executable);
    command
        .args(["-I", "-S", "-B"])
        .arg(&script)
        .current_dir(temp.path());
    let output = identity::parse(
        &native_host::run(command, &input, Instant::now() + options.timeout)?.stdout,
    )?;
    let provider = &output["provider"];
    require(
        output["schema_version"] == "1.0"
            && provider["name"] == "cpython-ast-symtable"
            && provider["executable_digest"] == executable_digest
            && provider["adapter_digest"] == identity::bytes_digest(include_bytes!("python.py"))
            && provider["syntax_only"] == options.syntax_only
            && provider["isolated"] == true
            && provider["executes_source"] == false,
        "ANALYZER",
        "Python interpreter/adapter identity changed or isolated mode is unavailable",
    )?;
    Ok(output)
}

/// Bounded interpreter/compiler identity probe used in the automatic sync key.
pub(crate) fn provider_identity(options: &ScanOptions) -> Result<Value> {
    Ok(output(None, options)?["provider"].clone())
}
pub fn produce(
    c: Capture,
    repository: &str,
    generation: u64,
    options: &ScanOptions,
) -> Result<Prepared> {
    require(c.complete, "CAPTURE", "Incomplete captured Python inputs")?;
    require(
        !options.syntax_only || !options.calls,
        "ANALYZER",
        "Python calls require binding resolution, not syntax-only mode",
    )?;
    let out = output(Some(&c), options)?;
    super::native_source::Publisher {
        language: "python",
        name: "cpython-ast-symtable",
        source: is_source,
        implementation: include_bytes!("python.rs"),
        adapter: include_bytes!("python.py"),
        limitations: "Python analysis covers retained source roots and stable lexical/import bindings, not arbitrary runtime execution, installed packages, import hooks, decorators, instance dispatch, reflection or framework/data lineage. .pyi declarations are not executable targets; .pyw files are script-only.",
    }.publish(c, repository, generation, options.calls, out)
}

pub fn analyze(l: &mut Ledger, o: &query::Options, options: &ScanOptions) -> Result<Value> {
    let mut options = options.clone();
    options.calls = true;
    require(
        !o.historical,
        "EXECUTION",
        "Execution needs current eligible source inputs",
    )?;
    require(
        !options.syntax_only,
        "EXECUTION",
        "Python execution requires binding resolution",
    )?;
    if options.source_roots.is_empty() {
        options.source_roots = strings(&l.load(&o.view)?.diagnostics["provider"]["source_roots"]);
    }
    execution::analyze_retained(l, o, &["python"], |capture, repository, generation| {
        produce(capture, repository, generation, &options)
    })
}
