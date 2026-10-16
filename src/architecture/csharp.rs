//! Roslyn analysis of retained C# without MSBuild, NuGet or source execution.
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
    pub dotnet: PathBuf,
    pub source_roots: Vec<String>,
    pub defines: Vec<String>,
    /// Numeric, non-preview version. Empty uses the selected compiler's default.
    pub language_version: Option<String>,
    pub timeout: Duration,
    pub calls: bool,
    pub syntax_only: bool,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            dotnet: std::env::var_os("BRIEFCASE_DOTNET")
                .map(Into::into)
                .unwrap_or_else(|| "dotnet".into()),
            source_roots: vec![],
            defines: vec![],
            language_version: None,
            timeout: Duration::from_secs(60),
            calls: false,
            syntax_only: false,
        }
    }
}
pub(crate) fn is_source(path: &str) -> bool {
    path.ends_with(".cs")
}
fn command(executable: &Path, directory: &Path) -> std::process::Command {
    let mut cmd = native_host::command(executable, directory);
    cmd.env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .env("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1")
        .env("DOTNET_NOLOGO", "1")
        .env("DOTNET_MULTILEVEL_LOOKUP", "0")
        .env("DOTNET_EnableDiagnostics", "0")
        .env("DOTNET_GCHeapHardLimit", "20000000");
    cmd
}
fn versions(path: &Path, major: Option<u32>) -> Result<PathBuf> {
    let mut choices = Vec::new();
    for item in std::fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        let version = name
            .split('.')
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>();
        if let Ok(version) = version {
            if item.file_type()?.is_dir()
                && version.len() == 3
                && major.is_none_or(|m| version[0] == m)
            {
                choices.push((version, item.path()));
            }
        }
    }
    choices.sort();
    choices
        .pop()
        .map(|(_, path)| path)
        .ok_or_else(|| "ANALYZER: A stable .NET SDK/runtime/reference pack is required".into())
}
fn quote(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .ok_or("ANALYZER: Non-UTF-8 .NET toolchain path")?;
    require(
        !text.contains(['"', '\n', '\r']),
        "ANALYZER",
        "Unsupported .NET toolchain path",
    )?;
    Ok(format!("\"{text}\""))
}
fn output(capture: Option<&Capture>, options: &ScanOptions) -> Result<Value> {
    let deadline = native_host::deadline(options.timeout)?;
    let dotnet = native_host::executable(&options.dotnet, "C# .NET SDK")?;
    let root = dotnet
        .parent()
        .ok_or("ANALYZER: dotnet has no installation root")?;
    let scratch = native_host::scratch()?;
    let mut probe = command(&dotnet, scratch.path());
    probe.arg("--list-sdks");
    let sdk_listing = native_host::run(probe, &[], deadline)?;
    require(
        !sdk_listing.stdout.is_empty(),
        "ANALYZER",
        "C# analysis requires a full .NET SDK 8 or newer",
    )?;
    let sdk = versions(&root.join("sdk"), None)?;
    let sdk_version = sdk.file_name().unwrap().to_string_lossy().into_owned();
    let major = sdk_version.split('.').next().unwrap().parse::<u32>()?;
    require(major >= 8, "ANALYZER", "C# analysis requires .NET SDK 8+")?;
    let roslyn = sdk.join("Roslyn/bincore");
    let runtime = versions(&root.join("shared/Microsoft.NETCore.App"), Some(major))?;
    let runtime_version = runtime.file_name().unwrap().to_string_lossy().into_owned();
    let pack = versions(&root.join("packs/Microsoft.NETCore.App.Ref"), Some(major))?;
    let references = pack.join(format!("ref/net{major}.0"));
    let toolchain = json!({"sdk_version":sdk_version,"runtime_version":runtime_version,
        "reference_pack_version":pack.file_name().unwrap().to_string_lossy(),
        "executable_digest":identity::bytes_digest(&std::fs::read(&dotnet)?),
        "compiler_digest":native_host::fingerprint(root, &[roslyn.clone(), runtime.clone(), references.clone(), root.join("host/fxr")], deadline)?});
    let compiled = native_host::compiled(
        "csharp",
        &toolchain,
        include_bytes!("CSharpAdapter.cs"),
        deadline,
        |directory, output| {
            let source = directory.join("CSharpAdapter.cs");
            std::fs::write(&source, include_bytes!("CSharpAdapter.cs"))?;
            let mut assemblies = std::fs::read_dir(&references)?
                .map(|entry| Ok(entry?.path()))
                .collect::<Result<Vec<_>>>()?;
            assemblies.retain(|p| p.extension().is_some_and(|e| e == "dll"));
            assemblies.extend([
                roslyn.join("Microsoft.CodeAnalysis.dll"),
                roslyn.join("Microsoft.CodeAnalysis.CSharp.dll"),
            ]);
            assemblies.sort();
            let assembly = output.join("CSharpAdapter.dll");
            let mut response = format!(
                "/nostdlib+\n/target:exe\n/deterministic+\n/optimize+\n/langversion:12\n/out:{}\n",
                quote(&assembly)?
            );
            for path in assemblies {
                response.push_str(&format!("/reference:{}\n", quote(&path)?));
            }
            response.push_str(&format!("{}\n", quote(&source)?));
            let file = directory.join("compiler.rsp");
            std::fs::write(&file, response)?;
            let mut cmd = command(&dotnet, directory);
            cmd.arg(roslyn.join("csc.dll"))
                .arg("/noconfig")
                .arg(format!("@{}", file.display()));
            native_host::run(cmd, &[], deadline)?;
            for name in [
                "Microsoft.CodeAnalysis.dll",
                "Microsoft.CodeAnalysis.CSharp.dll",
            ] {
                std::fs::copy(roslyn.join(name), output.join(name))?;
            }
            std::fs::write(
                output.join("CSharpAdapter.runtimeconfig.json"),
                serde_json::to_vec(&json!({"runtimeOptions":{
                "tfm":format!("net{major}.0"),"rollForward":"Disable",
                "framework":{"name":"Microsoft.NETCore.App","version":runtime_version}}}))?,
            )?;
            Ok(())
        },
    )?;
    let files: BTreeMap<_, _> = capture
        .into_iter()
        .flat_map(|c| &c.files)
        .map(|(path, bytes)| Ok((path, std::str::from_utf8(bytes)?)))
        .collect::<Result<_>>()?;
    let input = serde_json::to_vec(&json!({"files":files,"source_roots":options.source_roots,
        "defines":options.defines,"language_version":options.language_version,
        "syntax_only":options.syntax_only,"identity_only":capture.is_none()}))?;
    let mut cmd = command(&dotnet, scratch.path());
    cmd.arg(compiled.directory.join("CSharpAdapter.dll"))
        .arg(references);
    let mut out = identity::parse(&native_host::run(cmd, &input, deadline)?.stdout)?;
    require(
        out["schema_version"] == "1.0"
            && out["provider"]["name"] == "roslyn"
            && out["provider"]["executes_source"] == false
            && out["provider"]["emit"] == false
            && out["provider"]["generators"] == false
            && out["provider"]["syntax_only"] == options.syntax_only,
        "ANALYZER",
        "C# adapter identity mismatch",
    )?;
    out["provider"]["toolchain"] = toolchain;
    out["provider"]["compiled_adapter_digest"] = json!(compiled.digest);
    out["provider"]["adapter_digest"] =
        json!(identity::bytes_digest(include_bytes!("CSharpAdapter.cs")));
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
    require(c.complete, "CAPTURE", "Incomplete captured C# inputs")?;
    require(
        !options.syntax_only || !options.calls,
        "ANALYZER",
        "C# calls require native semantic resolution",
    )?;
    let out = output(Some(&c), options)?;
    native_source::Publisher {
        language: "csharp", name: "roslyn", source: is_source,
        implementation: include_bytes!("csharp.rs"), adapter: include_bytes!("CSharpAdapter.cs"),
        limitations: "C# uses one retained Roslyn compilation with explicit roots, numeric language version and defines. MSBuild/project evaluation, implicit usings, repository binaries, NuGet, generators, virtual/interface/delegate dispatch, implicit runtime operations and framework/ORM joins remain boundaries. Trusted platform types are not retained executable targets.",
    }.publish(c, repository, generation, options.calls, out)
}
pub fn analyze(l: &mut Ledger, o: &query::Options, options: &ScanOptions) -> Result<Value> {
    require(
        !o.historical && !options.syntax_only,
        "EXECUTION",
        "C# execution requires current semantic inputs",
    )?;
    let mut options = options.clone();
    options.calls = true;
    let provider = l.load(&o.view)?.diagnostics["provider"].clone();
    if options.source_roots.is_empty() {
        options.source_roots = strings(&provider["source_roots"]);
    }
    if options.defines.is_empty() {
        options.defines = strings(&provider["defines"]);
    }
    if options.language_version.is_none() {
        options.language_version = provider["language_version"].as_str().map(str::to_owned);
    }
    execution::analyze_retained(l, o, &["csharp"], |c, repository, generation| {
        produce(c, repository, generation, &options)
    })
}
