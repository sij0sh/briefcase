//! JDK Compiler API analysis of retained .java source with processors/emit off.
use super::{
    capture::Capture,
    contracts::strings,
    execution, identity,
    ledger::{Ledger, Prepared},
    native_host, native_source, query, require, Result,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct ScanOptions {
    /// Operator-selected javac. Its sibling java must be the same JDK.
    pub javac: PathBuf,
    pub source_roots: Vec<String>,
    /// Empty uses the selected JDK's non-preview source release.
    pub release: Option<u16>,
    pub timeout: Duration,
    pub calls: bool,
    pub syntax_only: bool,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            javac: std::env::var_os("BRIEFCASE_JAVAC")
                .map(Into::into)
                .unwrap_or_else(|| "javac".into()),
            source_roots: vec![],
            release: None,
            timeout: Duration::from_secs(60),
            calls: false,
            syntax_only: false,
        }
    }
}
pub(crate) fn is_source(path: &str) -> bool {
    path.ends_with(".java")
}
fn java_command(executable: &Path, dir: &Path) -> std::process::Command {
    let mut cmd = native_host::command(executable, dir);
    cmd.arg("-Xmx512m")
        .arg(format!("-Djava.io.tmpdir={}", dir.display()))
        .arg(format!("-Duser.home={}", dir.display()))
        .arg("-Dfile.encoding=UTF-8");
    cmd
}
fn input(capture: Option<&Capture>, options: &ScanOptions) -> Result<Vec<u8>> {
    fn integer(out: &mut Vec<u8>, value: usize) -> Result<()> {
        out.extend_from_slice(&u32::try_from(value)?.to_be_bytes());
        Ok(())
    }
    fn string(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
        integer(out, value.len())?;
        out.extend_from_slice(value);
        Ok(())
    }
    let mut out = Vec::new();
    integer(&mut out, 1)?;
    integer(&mut out, capture.map_or(0, |c| c.files.len()))?;
    if let Some(capture) = capture {
        for (path, bytes) in &capture.files {
            string(&mut out, path.as_bytes())?;
            string(&mut out, bytes)?;
        }
    }
    integer(&mut out, options.source_roots.len())?;
    for root in &options.source_roots {
        string(&mut out, root.as_bytes())?;
    }
    integer(&mut out, options.release.unwrap_or(0) as usize)?;
    out.extend([u8::from(options.syntax_only), u8::from(capture.is_none())]);
    Ok(out)
}
fn output(capture: Option<&Capture>, options: &ScanOptions) -> Result<Value> {
    let deadline = native_host::deadline(options.timeout)?;
    let javac = native_host::executable(&options.javac, "Java javac")?;
    let bin = javac
        .parent()
        .ok_or("ANALYZER: javac has no JDK directory")?;
    let root = bin
        .parent()
        .ok_or("ANALYZER: javac is not in a JDK bin directory")?;
    let java = bin.join(if cfg!(windows) { "java.exe" } else { "java" });
    require(
        java.is_file(),
        "ANALYZER",
        "Selected javac requires its sibling JDK java executable",
    )?;
    let scratch = native_host::scratch()?;
    let mut probe = native_host::command(&javac, scratch.path());
    probe.arg("-version");
    let result = native_host::run(probe, &[], deadline)?;
    let version = String::from_utf8([result.stdout, result.stderr].concat())?
        .trim()
        .to_owned();
    let major = version
        .strip_prefix("javac ")
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u16>().ok());
    require(
        major.is_some_and(|v| v >= 17),
        "ANALYZER",
        "Java analysis requires a full JDK 17 or newer",
    )?;
    let mut inputs = vec![
        root.join("lib/modules"),
        root.join("lib/ct.sym"),
        root.join("lib/jrt-fs.jar"),
        root.join("release"),
    ];
    for p in ["lib/server/libjvm.so", "lib/libjava.so", "lib/libjli.so"] {
        let p = root.join(p);
        if p.is_file() {
            inputs.push(p);
        }
    }
    let toolchain = json!({"javac_version":version,
        "executable_digest":identity::bytes_digest(&std::fs::read(&javac)?),
        "java_executable_digest":identity::bytes_digest(&std::fs::read(&java)?),
        "compiler_digest":native_host::fingerprint(root, &inputs, deadline)?});
    let compiled = native_host::compiled(
        "java",
        &toolchain,
        include_bytes!("JavaAdapter.java"),
        deadline,
        |dir, output| {
            let source = dir.join("JavaAdapter.java");
            std::fs::write(&source, include_bytes!("JavaAdapter.java"))?;
            let mut cmd = native_host::command(&javac, dir);
            cmd.args([
                "-J-Xmx512m",
                "-proc:none",
                "-implicit:none",
                "-encoding",
                "UTF-8",
                "--release",
                "17",
                "-g:none",
            ])
            .arg("-classpath")
            .arg(dir.join("empty-classpath"))
            .arg("-sourcepath")
            .arg(dir.join("empty-sourcepath"))
            .arg("-d")
            .arg(output)
            .arg(source);
            native_host::run(cmd, &[], deadline)?;
            Ok(())
        },
    )?;
    let mut cmd = java_command(&java, scratch.path());
    cmd.arg("--limit-modules")
        .arg("java.base,java.compiler,jdk.compiler")
        .arg("--class-path")
        .arg(&compiled.directory)
        .arg("JavaAdapter");
    let mut out =
        identity::parse(&native_host::run(cmd, &input(capture, options)?, deadline)?.stdout)?;
    require(
        out["schema_version"] == "1.0"
            && out["provider"]["name"] == "jdk-compiler-api"
            && out["provider"]["executes_source"] == false
            && out["provider"]["emit"] == false
            && out["provider"]["annotation_processing"] == false
            && out["provider"]["syntax_only"] == options.syntax_only,
        "ANALYZER",
        "Java adapter/compiler identity mismatch",
    )?;
    out["provider"]["toolchain"] = toolchain;
    out["provider"]["compiled_adapter_digest"] = json!(compiled.digest);
    out["provider"]["adapter_digest"] =
        json!(identity::bytes_digest(include_bytes!("JavaAdapter.java")));
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
    require(c.complete, "CAPTURE", "Incomplete captured Java inputs")?;
    require(
        !options.syntax_only || !options.calls,
        "ANALYZER",
        "Java calls require native compiler resolution",
    )?;
    let out = output(Some(&c), options)?;
    native_source::Publisher {
        language: "java", name: "jdk-compiler-api", source: is_source,
        implementation: include_bytes!("java.rs"), adapter: include_bytes!("JavaAdapter.java"),
        limitations: "Java analysis covers retained source and compiler-selected static/private/final/super calls and explicit local constructors. Virtual/interface dispatch, lambdas, method references, external binaries, annotation processing, generated code, JPMS multi-module builds and framework/data lineage remain explicit boundaries. Trusted JDK platform types are not retained executable targets.",
    }.publish(c, repository, generation, options.calls, out)
}
pub fn analyze(l: &mut Ledger, o: &query::Options, options: &ScanOptions) -> Result<Value> {
    require(
        !o.historical && !options.syntax_only,
        "EXECUTION",
        "Java execution requires current retained semantic inputs",
    )?;
    let mut options = options.clone();
    options.calls = true;
    let provider = l.load(&o.view)?.diagnostics["provider"].clone();
    if options.source_roots.is_empty() {
        options.source_roots = strings(&provider["source_roots"]);
    }
    if options.release.is_none() {
        options.release = provider["release"]
            .as_u64()
            .and_then(|r| u16::try_from(r).ok());
    }
    execution::analyze_retained(l, o, &["java"], |c, repository, generation| {
        produce(c, repository, generation, &options)
    })
}
