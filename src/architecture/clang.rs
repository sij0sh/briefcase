//! Clang AST/preprocessor analysis of retained C/C++ on an in-memory filesystem.
//! Repository build commands, external headers, modules and plugins are not used.
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
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Language {
    C,
    Cpp,
}
impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Cpp => "cpp",
        }
    }
    fn source(self) -> fn(&str) -> bool {
        match self {
            Self::C => c_source,
            Self::Cpp => cpp_source,
        }
    }
}
#[derive(Clone)]
pub struct ScanOptions {
    pub clang: PathBuf,
    pub llvm_config: PathBuf,
    pub language: Language,
    pub source_roots: Vec<String>,
    pub include_roots: Vec<String>,
    pub defines: Vec<String>,
    pub standard: Option<String>,
    pub target: Option<String>,
    pub timeout: Duration,
    pub calls: bool,
    pub syntax_only: bool,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            clang: std::env::var_os("BRIEFCASE_CLANG")
                .map(Into::into)
                .unwrap_or_else(|| "clang".into()),
            llvm_config: std::env::var_os("BRIEFCASE_LLVM_CONFIG")
                .map(Into::into)
                .unwrap_or_else(|| "llvm-config".into()),
            language: Language::C,
            source_roots: vec![],
            include_roots: vec![],
            defines: vec![],
            standard: None,
            target: None,
            timeout: Duration::from_secs(60),
            calls: false,
            syntax_only: false,
        }
    }
}
pub(crate) fn c_translation(path: &str) -> bool {
    path.ends_with(".c")
}
pub(crate) fn cpp_translation(path: &str) -> bool {
    [".cpp", ".cc", ".cxx", ".c++", ".C"]
        .iter()
        .any(|ext| path.ends_with(ext))
}
pub(crate) fn c_source(path: &str) -> bool {
    c_translation(path)
        || [".h", ".H", ".inc", ".def"]
            .iter()
            .any(|ext| path.ends_with(ext))
}
pub(crate) fn cpp_source(path: &str) -> bool {
    cpp_translation(path)
        || [
            ".h", ".H", ".inc", ".def", ".tcc", ".txx", ".hh", ".hpp", ".hxx", ".h++", ".ipp",
            ".tpp", ".inl", ".ixx", ".cppm",
        ]
        .iter()
        .any(|ext| path.ends_with(ext))
}
fn probe(executable: &Path, directory: &Path, args: &[&str], deadline: Instant) -> Result<String> {
    let mut cmd = native_host::command(executable, directory);
    cmd.args(args);
    Ok(
        String::from_utf8(native_host::run(cmd, &[], deadline)?.stdout)?
            .trim()
            .to_owned(),
    )
}
fn trusted_digest(path: &Path, deadline: Instant) -> Result<String> {
    let path = path.canonicalize()?;
    native_host::fingerprint_aliases(
        path.parent()
            .ok_or("ANALYZER: Toolchain input has no parent")?,
        std::slice::from_ref(&path),
        deadline,
    )
}
fn output(capture: Option<&Capture>, options: &ScanOptions) -> Result<Value> {
    let deadline = native_host::deadline(options.timeout)?;
    let clang = native_host::executable(&options.clang, "Clang")?;
    let config = native_host::executable(&options.llvm_config, "LLVM configuration")?;
    let scratch = native_host::scratch()?;
    let version = probe(&config, scratch.path(), &["--version"], deadline)?;
    let major = version.split('.').next().unwrap_or("").parse::<u32>()?;
    require(
        major >= 22,
        "ANALYZER",
        "C/C++ analysis requires matching Clang/LLVM 22+ development tools",
    )?;
    let clang_version = probe(
        &clang,
        scratch.path(),
        &["--no-default-config", "--version"],
        deadline,
    )?;
    require(
        clang_version.contains(&format!("clang version {major}.")),
        "ANALYZER",
        "Clang and LLVM development versions must match",
    )?;
    let include = PathBuf::from(probe(&config, scratch.path(), &["--includedir"], deadline)?);
    let libraries = PathBuf::from(probe(&config, scratch.path(), &["--libdir"], deadline)?);
    require(
        include.is_absolute() && libraries.is_absolute(),
        "ANALYZER",
        "LLVM must report absolute development paths",
    )?;
    let mut cpp_libraries = std::fs::read_dir(&libraries)?
        .map(|entry| Ok(entry?.path()))
        .collect::<Result<Vec<_>>>()?;
    cpp_libraries.retain(|p| {
        p.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n.starts_with("libclang-cpp") && (n.contains(".so") || n.ends_with(".dylib"))
        })
    });
    cpp_libraries.sort();
    let cpp_library = cpp_libraries
        .first()
        .ok_or("ANALYZER: LLVM development installation needs the shared clang-cpp library")?
        .canonicalize()?;
    let llvm_files = probe(
        &config,
        scratch.path(),
        &["--libfiles", "--link-shared"],
        deadline,
    )?;
    let llvm_library = PathBuf::from(&llvm_files);
    require(
        llvm_library.is_absolute() && llvm_library.is_file(),
        "ANALYZER",
        "LLVM needs one shared library reported by llvm-config",
    )?;
    let llvm_library = llvm_library.canonicalize()?;
    let toolchain = json!({"llvm_version":version,"clang_version":clang_version,
        "executable_digest":identity::bytes_digest(&std::fs::read(&clang)?),
        "llvm_config_digest":identity::bytes_digest(&std::fs::read(&config)?),
        "clang_library_digest":trusted_digest(&cpp_library, deadline)?,
        "llvm_library_digest":trusted_digest(&llvm_library, deadline)?,
        "clang_headers_digest":trusted_digest(&include.join("clang"), deadline)?,
        "llvm_headers_digest":trusted_digest(&include.join("llvm"), deadline)?,
        "llvm_c_headers_digest":trusted_digest(&include.join("llvm-c"), deadline)?});
    let compiled = native_host::compiled(
        "clang",
        &toolchain,
        include_bytes!("clang_adapter.cpp"),
        deadline,
        |directory, output| {
            let source = directory.join("clang_adapter.cpp");
            std::fs::write(&source, include_bytes!("clang_adapter.cpp"))?;
            let mut cmd = native_host::command(&clang, directory);
            cmd.args([
                "--no-default-config",
                "--driver-mode=g++",
                "-x",
                "c++",
                "-std=c++20",
                "-O0",
                "-g0",
                "-fno-rtti",
            ])
            .arg("-I")
            .arg(&include)
            .arg(source)
            .arg("-x")
            .arg("none")
            .arg(&cpp_library)
            .arg(&llvm_library)
            .arg(format!(
                "-Wl,-rpath,{}",
                cpp_library.parent().unwrap().display()
            ))
            .arg(format!(
                "-Wl,-rpath,{}",
                llvm_library.parent().unwrap().display()
            ))
            .arg("-o")
            .arg(output.join("analyzer"));
            native_host::run(cmd, &[], deadline)?;
            Ok(())
        },
    )?;
    let target = match &options.target {
        Some(target) => target.clone(),
        None => probe(
            &clang,
            scratch.path(),
            &["--no-default-config", "-dumpmachine"],
            deadline,
        )?,
    };
    let files: BTreeMap<_, _> = capture
        .into_iter()
        .flat_map(|c| &c.files)
        .map(|(path, bytes)| Ok((path, std::str::from_utf8(bytes)?)))
        .collect::<Result<_>>()?;
    let input = serde_json::to_vec(&json!({"files":files,"language":options.language.name(),
        "source_roots":options.source_roots,"include_roots":options.include_roots,"defines":options.defines,
        "standard":options.standard,"target":target,"syntax_only":options.syntax_only,"identity_only":capture.is_none()}))?;
    let cmd = native_host::command(&compiled.directory.join("analyzer"), scratch.path());
    let mut out = identity::parse(&native_host::run(cmd, &input, deadline)?.stdout)?;
    require(
        out["schema_version"] == "1.0"
            && out["provider"]["name"] == "clang-ast"
            && out["provider"]["version"] == version
            && out["provider"]["language"] == options.language.name()
            && out["provider"]["syntax_only"] == options.syntax_only
            && out["provider"]["executes_source"] == false
            && out["provider"]["emit"] == false
            && out["provider"]["plugins"] == false
            && out["provider"]["filesystem"] == "retained-memory-only",
        "ANALYZER",
        "Clang adapter/toolchain identity mismatch",
    )?;
    out["provider"]["toolchain"] = toolchain;
    out["provider"]["compiled_adapter_digest"] = json!(compiled.digest);
    out["provider"]["adapter_digest"] =
        json!(identity::bytes_digest(include_bytes!("clang_adapter.cpp")));
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
    require(c.complete, "CAPTURE", "Incomplete captured C/C++ inputs")?;
    require(
        !options.syntax_only || !options.calls,
        "ANALYZER",
        "C/C++ calls require native semantic resolution",
    )?;
    let out = output(Some(&c), options)?;
    native_source::Publisher {
        language: options.language.name(), name: "clang-ast", source: options.language.source(),
        implementation: include_bytes!("clang.rs"), adapter: include_bytes!("clang_adapter.cpp"),
        limitations: "C/C++ use retained Clang translation units, explicit include roots, defines, standard and target. Includes resolve only in retained memory. Build commands, compilation database flags, external/system headers, PCH/modules, plugins, macro-generated code, templates, virtual/function-pointer dispatch, implicit runtime operations, linkage ambiguity and framework/data joins remain boundaries. Static paths do not prove runtime execution.",
    }.publish(c, repository, generation, options.calls, out)
}
pub fn analyze(l: &mut Ledger, o: &query::Options, options: &ScanOptions) -> Result<Value> {
    require(
        !o.historical && !options.syntax_only,
        "EXECUTION",
        "C/C++ execution requires current semantic inputs",
    )?;
    let mut options = options.clone();
    options.calls = true;
    let provider = l.load(&o.view)?.diagnostics["provider"].clone();
    if options.source_roots.is_empty() {
        options.source_roots = strings(&provider["source_roots"]);
    }
    if options.include_roots.is_empty() {
        options.include_roots = strings(&provider["include_roots"]);
    }
    if options.defines.is_empty() {
        options.defines = strings(&provider["defines"]);
    }
    if options.standard.is_none() {
        options.standard = provider["standard"].as_str().map(str::to_owned);
    }
    if options.target.is_none() {
        options.target = provider["target"].as_str().map(str::to_owned);
    }
    execution::analyze_retained(
        l,
        o,
        &[options.language.name()],
        |c, repository, generation| produce(c, repository, generation, &options),
    )
}
