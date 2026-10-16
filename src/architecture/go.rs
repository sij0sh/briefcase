//! Go AST/go-types analysis with retained-only package resolution.
use super::{
    capture::Capture,
    contracts::strings,
    execution, identity,
    ledger::{Ledger, Prepared},
    native_host, native_source, query, require, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct ScanOptions {
    pub go: PathBuf,
    pub source_roots: Vec<String>,
    pub tags: Vec<String>,
    pub goos: Option<String>,
    pub goarch: Option<String>,
    pub tests: bool,
    pub timeout: Duration,
    pub calls: bool,
    pub syntax_only: bool,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            go: std::env::var_os("BRIEFCASE_GO")
                .map(Into::into)
                .unwrap_or_else(|| "go".into()),
            source_roots: vec![],
            tags: vec![],
            goos: None,
            goarch: None,
            tests: false,
            timeout: Duration::from_secs(60),
            calls: false,
            syntax_only: false,
        }
    }
}
pub(crate) fn is_source(path: &str) -> bool {
    path.ends_with(".go")
}

fn go_command(executable: &Path, dir: &Path) -> std::process::Command {
    let mut cmd = native_host::command(executable, dir);
    cmd.env("GOENV", "off")
        .env("GOTOOLCHAIN", "local")
        .env("GOWORK", "off")
        .env("GO111MODULE", "off")
        .env("GOFLAGS", "")
        .env("CGO_ENABLED", "0")
        .env("GOPROXY", "off")
        .env("GOSUMDB", "off")
        .env("GOPATH", dir.join("gopath"))
        .env("GOCACHE", dir.join("gocache"));
    cmd
}
fn output(capture: Option<&Capture>, options: &ScanOptions) -> Result<Value> {
    let deadline = native_host::deadline(options.timeout)?;
    let executable = native_host::executable(&options.go, "Go")?;
    let scratch = native_host::scratch()?;
    let mut probe = go_command(&executable, scratch.path());
    probe.args(["env", "-json", "GOROOT", "GOOS", "GOARCH", "GOVERSION"]);
    let env = identity::parse(&native_host::run(probe, &[], deadline)?.stdout)?;
    let version = env["GOVERSION"].as_str().unwrap_or("");
    let minor = version
        .strip_prefix("go1.")
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u16>().ok());
    require(
        minor.is_some_and(|v| v >= 22),
        "ANALYZER",
        "Go analysis requires Go 1.22 or newer",
    )?;
    let root =
        PathBuf::from(env["GOROOT"].as_str().ok_or("ANALYZER: Missing GOROOT")?).canonicalize()?;
    let digest = native_host::fingerprint(
        &root,
        &[
            root.join("src"),
            root.join("pkg/tool"),
            root.join("VERSION"),
        ],
        deadline,
    )?;
    let toolchain = json!({"executable_digest":identity::bytes_digest(&std::fs::read(&executable)?),
        "compiler_digest":digest,"version":version,"host_goos":env["GOOS"],"host_goarch":env["GOARCH"]});
    let compiled = native_host::compiled(
        "go",
        &toolchain,
        include_bytes!("go_adapter.go"),
        deadline,
        |dir, output| {
            let source = dir.join("go_adapter.go");
            std::fs::write(&source, include_bytes!("go_adapter.go"))?;
            let mut cmd = go_command(&executable, dir);
            cmd.args([
                "build",
                "-p=1",
                "-trimpath",
                "-buildvcs=false",
                "-ldflags=-buildid=",
            ])
            .arg("-o")
            .arg(output.join("analyzer"))
            .arg(source);
            native_host::run(cmd, &[], deadline)?;
            Ok(())
        },
    )?;
    let files: BTreeMap<_, _> = capture
        .into_iter()
        .flat_map(|c| &c.files)
        .map(|(p, b)| Ok((p, std::str::from_utf8(b)?)))
        .collect::<Result<_>>()?;
    let input = serde_json::to_vec(
        &json!({"files":files,"source_roots":options.source_roots,"tags":options.tags,
        "goos":options.goos.as_deref().map(Value::from).unwrap_or_else(||env["GOOS"].clone()),
        "goarch":options.goarch.as_deref().map(Value::from).unwrap_or_else(||env["GOARCH"].clone()),
        "tests":options.tests,"syntax_only":options.syntax_only,"identity_only":capture.is_none()}),
    )?;
    let mut cmd = native_host::command(&compiled.directory.join("analyzer"), scratch.path());
    cmd.env("GOROOT", &root);
    let mut out = identity::parse(&native_host::run(cmd, &input, deadline)?.stdout)?;
    require(
        out["schema_version"] == "1.0"
            && out["provider"]["name"] == "go-ast-types"
            && out["provider"]["version"] == version
            && out["provider"]["executes_source"] == false
            && out["provider"]["syntax_only"] == options.syntax_only,
        "ANALYZER",
        "Go adapter/compiler identity mismatch",
    )?;
    out["provider"]["toolchain"] = toolchain;
    out["provider"]["compiled_adapter_digest"] = json!(compiled.digest);
    out["provider"]["adapter_digest"] =
        json!(identity::bytes_digest(include_bytes!("go_adapter.go")));
    out["provider"]["launcher_digest"] =
        json!(identity::bytes_digest(include_bytes!("native_host.rs")));
    Ok(out)
}
pub(crate) fn provider_identity(options: &ScanOptions) -> Result<Value> {
    Ok(output(None, options)?["provider"].clone())
}
pub fn produce(
    c: Capture,
    repository: &str,
    generation: u64,
    options: &ScanOptions,
) -> Result<Prepared> {
    require(c.complete, "CAPTURE", "Incomplete captured Go inputs")?;
    require(
        !options.syntax_only || !options.calls,
        "ANALYZER",
        "Go calls require native type resolution",
    )?;
    let out = output(Some(&c), options)?;
    native_source::Publisher {
        language: "go", name: "go-ast-types", source: is_source,
        implementation: include_bytes!("go.rs"), adapter: include_bytes!("go_adapter.go"),
        limitations: "Go analysis resolves retained packages and compiler-proven direct functions/concrete methods under pinned build selection. External dependencies, cgo, replacements, linkname, interface dispatch, function values and framework/data lineage are explicit boundaries. Static calls do not prove goroutine scheduling or runtime execution.",
    }.publish(c, repository, generation, options.calls, out)
}
pub fn analyze(l: &mut Ledger, o: &query::Options, options: &ScanOptions) -> Result<Value> {
    require(
        !o.historical && !options.syntax_only,
        "EXECUTION",
        "Go execution requires current retained semantic inputs",
    )?;
    let mut options = options.clone();
    options.calls = true;
    let provider = l.load(&o.view)?.diagnostics["provider"].clone();
    if options.source_roots.is_empty() {
        options.source_roots = strings(&provider["source_roots"]);
    }
    if options.tags.is_empty() {
        options.tags = strings(&provider["tags"]);
    }
    if options.goos.is_none() {
        options.goos = provider["goos"].as_str().map(str::to_owned);
    }
    if options.goarch.is_none() {
        options.goarch = provider["goarch"].as_str().map(str::to_owned);
    }
    options.tests |= provider["tests"] == true;
    execution::analyze_retained(l, o, &["go"], |c, repository, generation| {
        produce(c, repository, generation, &options)
    })
}
