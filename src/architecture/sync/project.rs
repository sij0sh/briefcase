//! One bounded, idempotent source sync used by `sync`/`ensure`, the Pi
//! extension and any host-side wrapper.
//!
//! Unchanged inputs return `up-to-date` without allocating a generation.
//! Missing or stale projections are rebuilt from retained inputs. Writer
//! admission is process-shared, so concurrent launches and manual writers
//! never race one another. `up-to-date` means the selected inputs match the
//! publication; it does not mean every call target or policy is resolved.
use super::super::{
    capture::Capture,
    clang, csharp, go, identity, java,
    ledger::{self, Ledger},
    python, recall, rust, typescript,
    workspace::{self, WriterLock},
    Result,
};
use super::bounded;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const ERROR_LIMIT: usize = 4096;
const CONFIG: &str = ".briefcase/sync.json";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    Rust,
    C,
    Cpp,
    Csharp,
    Go,
    Java,
    Python,
    Typescript,
}

/// Repository-owned profile settings. Executables are never selected here:
/// a repository cannot choose which program Briefcase runs on launch.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryConfig {
    #[serde(rename = "sync")]
    _sync: Option<crate::architecture::reconcile::DomainPolicy>,
    #[serde(rename = "watch")]
    _watch: Option<crate::architecture::reconcile::WatchPolicy>,
    language: Option<String>,
    project: Option<String>,
    policy: Option<String>,
    calls: Option<bool>,
    #[serde(default)]
    profiles: Vec<AnalysisProfile>,
    python_source_roots: Option<Vec<String>>,
    go_source_roots: Option<Vec<String>>,
    go_tags: Option<Vec<String>>,
    goos: Option<String>,
    goarch: Option<String>,
    go_tests: Option<bool>,
    java_source_roots: Option<Vec<String>>,
    java_release: Option<u16>,
    csharp_source_roots: Option<Vec<String>>,
    csharp_defines: Option<Vec<String>>,
    csharp_language_version: Option<String>,
    cc_source_roots: Option<Vec<String>>,
    cc_include_roots: Option<Vec<String>>,
    cc_defines: Option<Vec<String>>,
    cc_standard: Option<String>,
    cc_target: Option<String>,
}

/// Independent language/project scopes share captured bytes, not fabricated
/// cross-language semantic identity. Executables remain operator-selected.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnalysisProfile {
    language: String,
    project: Option<String>,
    calls: Option<bool>,
}

#[derive(Clone)]
pub struct SyncOptions {
    pub root: PathBuf,
    pub db: PathBuf,
    /// Budget for the complete operation, including analysis.
    pub timeout: Duration,
    /// How long to wait for another writer before reporting `locked`.
    pub lock_wait: Duration,
    pub language: Option<Language>,
    pub project: Option<String>,
    pub policy: Option<PathBuf>,
    pub calls: Option<bool>,
    /// Operator-selected Rust analyzer. `None` uses `rust-analyzer` from PATH
    /// when it answers, otherwise a syntax-only capture.
    pub analyzer: Option<PathBuf>,
    pub node: PathBuf,
    pub compiler: Option<PathBuf>,
    pub python: Option<PathBuf>,
    pub python_source_roots: Vec<String>,
    pub go: Option<PathBuf>,
    pub go_source_roots: Vec<String>,
    pub go_tags: Vec<String>,
    pub goos: Option<String>,
    pub goarch: Option<String>,
    pub go_tests: bool,
    pub javac: Option<PathBuf>,
    pub java_source_roots: Vec<String>,
    pub java_release: Option<u16>,
    pub dotnet: Option<PathBuf>,
    pub csharp_source_roots: Vec<String>,
    pub csharp_defines: Vec<String>,
    pub csharp_language_version: Option<String>,
    pub clang: Option<PathBuf>,
    pub llvm_config: Option<PathBuf>,
    pub cc_source_roots: Vec<String>,
    pub cc_include_roots: Vec<String>,
    pub cc_defines: Vec<String>,
    pub cc_standard: Option<String>,
    pub cc_target: Option<String>,
}

fn configured_profiles(o:&SyncOptions) -> Result<Vec<(String,SyncOptions)>> {
    if o.language.is_some() {return Ok(vec![]);}
    let bytes=match std::fs::read(o.root.join(CONFIG)) {
        Ok(bytes)=>bytes,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(vec![]),Err(e)=>return Err(e.into())
    };
    let config:RepositoryConfig=serde_json::from_slice(&bytes)?;
    super::super::require(config.profiles.len()<=16,"CONFIGURATION","At most 16 analysis profiles are supported")?;
    let mut out=vec![];let mut seen=std::collections::BTreeSet::new();
    for profile in &config.profiles {
        let slot=identity::id("sync-profile",&serde_json::to_value(profile)?)?;
        super::super::require(seen.insert(slot.clone()),"CONFIGURATION","Duplicate analysis profile")?;
        let mut selected=o.clone();
        selected.language=Some(parse_language(&profile.language).map_err(|e|format!("CONFIGURATION: {e}"))?);
        selected.project=o.project.clone().or_else(||profile.project.clone());
        selected.calls=o.calls.or(profile.calls);
        out.push((slot,selected));
    }
    Ok(out)
}

fn profile_state_id(slot:&str,key:&str) -> Result<String> {
    identity::id("sync-profile-state",&json!([slot,key]))
}

pub(super) fn apply(o:&SyncOptions,lock:&WriterLock,started:Instant) -> Result<Value> {
    let profiles=configured_profiles(o)?;
    if profiles.is_empty() {return apply_one(o,lock,started,None);}
    let mut results=vec![];let mut pins=vec![];let mut keys=vec![];let mut good=true;let mut changed=false;
    for (slot,selected) in profiles {
        let value=apply_one(&selected,lock,started,Some(&slot)).unwrap_or_else(|e|json!({"sync":"failed","error":e.to_string()}));
        let state=value["sync"].as_str().unwrap_or("failed");
        good &= ["up-to-date","refreshed","rebuilt"].contains(&state);
        changed |= ["refreshed","rebuilt"].contains(&state);
        if let Some(view)=value["view_id"].as_str() {
            pins.push(json!({"slot":slot,"view_id":view,"profile":value["profile"]}));
            keys.push(json!([slot,value["input_key"]]));
        }
        results.push(json!({"slot":slot,"result":value}));
    }
    let ledger=Ledger::open(&o.db)?;
    if good {if let Some(view)=pins.last().and_then(|p|p["view_id"].as_str()) {ledger.select_source_profile(view)?;}}
    let current=ledger.resolve("latest").ok();
    let key=identity::id("profile-set",&json!(keys))?;
    if good {
        if let Some(view)=&current {ledger.record_sync(&key,view,&json!({"profile_views":pins,"method":"multi-profile-v1"}))?;}
    }
    let task_index=if good {current.as_deref().map(|view|crate::architecture::task::precompute(&ledger,view)).transpose()?} else {None};
    Ok(json!({"sync":if !good {"failed"} else if changed {"refreshed"} else {"up-to-date"},"ready":good,
        "view_id":current,"input_key":key,"profile_views":pins,"profiles":results,"task_index":task_index,
        "limitations":["Each profile retains its own semantic context. Task packets join profiles only when their source snapshot IDs match."]}))
}

fn cpp_family(path: &str) -> bool {
    clang::cpp_translation(path)
        || [
            ".hh", ".hpp", ".hxx", ".h++", ".ipp", ".tpp", ".inl", ".tcc", ".txx", ".cppm", ".ixx",
        ]
        .iter()
        .any(|ext| path.ends_with(ext))
}
fn detect(root: &Path, configured: Option<Language>) -> std::result::Result<Language, String> {
    if let Some(language) = configured {
        return Ok(language);
    }
    let rust = root.join("Cargo.toml").is_file();
    let typescript = root.join("tsconfig.json").is_file() || root.join("package.json").is_file();
    // Root project markers take precedence over helper source files. Only
    // marker-free trees use source-family detection; mixed families need scope.
    let csharp_marker = std::fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_type().is_ok_and(|t| t.is_file())
                && e.path()
                    .extension()
                    .is_some_and(|x| x == "csproj" || x == "sln" || x == "slnx")
        });
    let cc_marker = [
        "CMakeLists.txt",
        "Makefile",
        "GNUmakefile",
        "meson.build",
        "compile_commands.json",
    ]
    .iter()
    .any(|p| root.join(p).is_file());
    let cc_language = if cc_marker {
        let c = super::super::capture::contains_source(root, clang::c_translation)
            .map_err(|e| e.to_string())?;
        let cpp = super::super::capture::contains_source(root, clang::cpp_translation)
            .map_err(|e| e.to_string())?;
        if c == cpp {
            return Err("C/C++ build markers need one unambiguous source language; set language in .briefcase/sync.json".into());
        }
        Some(if cpp { Language::Cpp } else { Language::C })
    } else {
        None
    };
    let markers = [
        (Language::Csharp, csharp_marker),
        (Language::C, cc_language == Some(Language::C)),
        (Language::Cpp, cc_language == Some(Language::Cpp)),
        (Language::Rust, rust),
        (Language::Typescript, typescript),
        (
            Language::Python,
            [
                "pyproject.toml",
                "setup.py",
                "setup.cfg",
                "requirements.txt",
            ]
            .iter()
            .any(|p| root.join(p).is_file()),
        ),
        (
            Language::Go,
            ["go.mod", "go.work"].iter().any(|p| root.join(p).is_file()),
        ),
        (
            Language::Java,
            [
                "pom.xml",
                "build.gradle",
                "build.gradle.kts",
                "settings.gradle",
                "settings.gradle.kts",
            ]
            .iter()
            .any(|p| root.join(p).is_file()),
        ),
    ];
    let mut selected = markers
        .iter()
        .filter_map(|(l, found)| found.then_some(*l))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        for (language, source) in [
            (Language::Python, python::is_source as fn(&str) -> bool),
            (Language::Go, go::is_source),
            (Language::Java, java::is_source),
            (Language::Csharp, csharp::is_source),
            (Language::C, clang::c_translation),
            (Language::Cpp, cpp_family),
        ] {
            if super::super::capture::contains_source(root, source).map_err(|e| e.to_string())? {
                selected.push(language);
            }
        }
    }
    match selected.as_slice() {
        [language] => Ok(*language),
        [] => Err("No supported Rust, Go, Java, C#, C/C++, Python or TS/JS project at the repository root; set \"language\" in .briefcase/sync.json".into()),
        _ => Err("Multiple supported project markers or source families; set \"language\" in .briefcase/sync.json".into()),
    }
}

fn parse_language(name: &str) -> std::result::Result<Language, String> {
    match name {
        "rust" => Ok(Language::Rust),
        "c" => Ok(Language::C),
        "cpp" | "c++" => Ok(Language::Cpp),
        "csharp" | "c#" => Ok(Language::Csharp),
        "go" => Ok(Language::Go),
        "java" => Ok(Language::Java),
        "python" => Ok(Language::Python),
        "typescript" | "javascript" => Ok(Language::Typescript),
        other => Err(format!("Unsupported sync language {other}")),
    }
}

/// `rust-analyzer --version`, bounded, or `None` when it does not answer.
fn analyzer_identity(exe: &Path) -> Option<String> {
    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut out = String::new();
                std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
                return Some(out.trim().to_owned());
            }
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() > Duration::from_secs(10) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

struct Profile {
    language: Language,
    project: String,
    calls: bool,
    policy: Option<PathBuf>,
    analyzer: Option<(PathBuf, String)>,
    compiler: Option<PathBuf>,
    python: Option<python::ScanOptions>,
    go: Option<go::ScanOptions>,
    java: Option<java::ScanOptions>,
    csharp: Option<csharp::ScanOptions>,
    cc: Option<clang::ScanOptions>,
    summary: Value,
}

enum ProfileError {
    Configuration(String),
    Timeout(String),
}

impl From<String> for ProfileError {
    fn from(message: String) -> Self {
        Self::Configuration(message)
    }
}
impl From<&str> for ProfileError {
    fn from(message: &str) -> Self {
        Self::Configuration(message.into())
    }
}

pub(super) fn is_timeout(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
}

impl ProfileError {
    fn provider(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        if is_timeout(error.as_ref()) {
            Self::Timeout(error.to_string())
        } else {
            Self::Configuration(error.to_string())
        }
    }
}

fn profile(o: &SyncOptions, deadline: Instant) -> std::result::Result<Profile, ProfileError> {
    let config: RepositoryConfig = match std::fs::read(o.root.join(CONFIG)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{CONFIG}: {e}"))?,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && std::fs::symlink_metadata(o.root.join(CONFIG)).is_err() =>
        {
            RepositoryConfig::default()
        }
        Err(e) => return Err(format!("{CONFIG}: {e}").into()),
    };
    if let Some(policy) = &config._sync {
        policy
            .validate()
            .map_err(|e| ProfileError::Configuration(e.to_string()))?;
    }
    let configured = match (o.language, &config.language) {
        (Some(l), _) => Some(l),
        (None, Some(name)) => Some(parse_language(name)?),
        (None, None) => None,
    };
    let language = detect(&o.root, configured)?;
    let policy = match (&o.policy, &config.policy) {
        (Some(p), _) => Some(p.clone()),
        (None, Some(p)) => {
            let path = o
                .root
                .join(p)
                .canonicalize()
                .map_err(|e| format!("policy {p}: {e}"))?;
            if !path.starts_with(&o.root) {
                return Err("Configured policy must be inside the repository".into());
            }
            Some(path)
        }
        (None, None) => None,
    };
    let project = o
        .project
        .clone()
        .or(config.project)
        .unwrap_or_else(|| "tsconfig.json".into());
    let calls = o.calls.or(config.calls).unwrap_or(true);
    let mut analyzer = None;
    let mut compiler = None;
    let mut python = None;
    let mut python_provider = None;
    let mut go = None;
    let mut go_provider = None;
    let mut java = None;
    let mut java_provider = None;
    let mut csharp = None;
    let mut csharp_provider = None;
    let mut cc = None;
    let mut cc_provider = None;
    match language {
        Language::Rust => {
            let exe = o
                .analyzer
                .clone()
                .unwrap_or_else(|| PathBuf::from("rust-analyzer"));
            match analyzer_identity(&exe) {
                Some(version) => analyzer = Some((exe, version)),
                None if o.analyzer.is_some() => {
                    return Err(
                        format!("Analyzer {} did not answer --version", exe.display()).into(),
                    )
                }
                None => {}
            }
        }
        Language::Csharp => {
            let mut options = csharp::ScanOptions {
                source_roots: if o.csharp_source_roots.is_empty() {
                    config.csharp_source_roots.unwrap_or_default()
                } else {
                    o.csharp_source_roots.clone()
                },
                defines: if o.csharp_defines.is_empty() {
                    config.csharp_defines.unwrap_or_default()
                } else {
                    o.csharp_defines.clone()
                },
                language_version: o
                    .csharp_language_version
                    .clone()
                    .or(config.csharp_language_version),
                timeout: deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(60)),
                calls,
                ..Default::default()
            };
            if let Some(executable) = &o.dotnet {
                options.dotnet = executable.clone();
            }
            csharp_provider =
                Some(csharp::provider_identity(&options).map_err(ProfileError::provider)?);
            csharp = Some(options);
        }
        Language::C | Language::Cpp => {
            let mut options = clang::ScanOptions {
                language: if language == Language::Cpp {
                    clang::Language::Cpp
                } else {
                    clang::Language::C
                },
                source_roots: if o.cc_source_roots.is_empty() {
                    config.cc_source_roots.unwrap_or_default()
                } else {
                    o.cc_source_roots.clone()
                },
                include_roots: if o.cc_include_roots.is_empty() {
                    config.cc_include_roots.unwrap_or_default()
                } else {
                    o.cc_include_roots.clone()
                },
                defines: if o.cc_defines.is_empty() {
                    config.cc_defines.unwrap_or_default()
                } else {
                    o.cc_defines.clone()
                },
                standard: o.cc_standard.clone().or(config.cc_standard),
                target: o.cc_target.clone().or(config.cc_target),
                timeout: deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(60)),
                calls,
                ..Default::default()
            };
            if let Some(executable) = &o.clang {
                options.clang = executable.clone();
            }
            if let Some(executable) = &o.llvm_config {
                options.llvm_config = executable.clone();
            }
            cc_provider = Some(clang::provider_identity(&options).map_err(ProfileError::provider)?);
            cc = Some(options);
        }
        Language::Go => {
            let mut options = go::ScanOptions {
                source_roots: if o.go_source_roots.is_empty() {
                    config.go_source_roots.unwrap_or_default()
                } else {
                    o.go_source_roots.clone()
                },
                tags: if o.go_tags.is_empty() {
                    config.go_tags.unwrap_or_default()
                } else {
                    o.go_tags.clone()
                },
                goos: o.goos.clone().or(config.goos),
                goarch: o.goarch.clone().or(config.goarch),
                tests: o.go_tests || config.go_tests.unwrap_or(false),
                timeout: deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(60)),
                calls,
                ..Default::default()
            };
            if let Some(executable) = &o.go {
                options.go = executable.clone();
            }
            go_provider = Some(go::provider_identity(&options).map_err(ProfileError::provider)?);
            go = Some(options);
        }
        Language::Java => {
            let mut options = java::ScanOptions {
                source_roots: if o.java_source_roots.is_empty() {
                    config.java_source_roots.unwrap_or_default()
                } else {
                    o.java_source_roots.clone()
                },
                release: o.java_release.or(config.java_release),
                timeout: deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(60)),
                calls,
                ..Default::default()
            };
            if let Some(executable) = &o.javac {
                options.javac = executable.clone();
            }
            java_provider =
                Some(java::provider_identity(&options).map_err(ProfileError::provider)?);
            java = Some(options);
        }
        Language::Python => {
            let mut options = python::ScanOptions {
                source_roots: if o.python_source_roots.is_empty() {
                    config.python_source_roots.unwrap_or_default()
                } else {
                    o.python_source_roots.clone()
                },
                timeout: Duration::from_secs(5),
                calls,
                ..Default::default()
            };
            if let Some(interpreter) = &o.python {
                options.python = interpreter.clone();
            }
            python_provider =
                Some(python::provider_identity(&options).map_err(ProfileError::provider)?);
            python = Some(options);
        }
        Language::Typescript => {
            let path = o
                .compiler
                .clone()
                .unwrap_or_else(|| typescript::ScanOptions::default().compiler);
            compiler = Some(path);
        }
    }
    let summary = json!({
        "language": match language { Language::Rust => "rust", Language::C => "c", Language::Cpp => "cpp", Language::Csharp => "csharp", Language::Go => "go", Language::Java => "java", Language::Python => "python", Language::Typescript => "typescript" },
        "project": (language == Language::Typescript).then_some(&project),
        "calls": calls,
        "policy": policy.as_ref().map(|p| p.strip_prefix(&o.root).unwrap_or(p).to_string_lossy().into_owned()),
        "analyzer": analyzer.as_ref().map(|(exe, version)| json!({"path":exe,"version":version})),
        "semantic": language != Language::Rust || analyzer.is_some(),
        "python_provider": python_provider,
        "go_provider": go_provider,
        "java_provider": java_provider,
        "csharp_provider": csharp_provider,
        "cc_provider": cc_provider,
        "python_source_roots": python.as_ref().map(|o| &o.source_roots),
        "compiler_digest": compiler.as_ref().and_then(|p| std::fs::read(p).ok()).map(|b| identity::bytes_digest(&b)),
    });
    Ok(Profile {
        language,
        project,
        calls,
        policy,
        analyzer,
        compiler,
        python,
        go,
        java,
        csharp,
        cc,
        summary,
    })
}

/// Everything that can change the publication: retained bytes, source
/// selection, profile and producer implementation.
fn key(repository: &str, capture: &Capture, profile: &Profile) -> Result<String> {
    let files: Vec<_> = capture
        .files
        .iter()
        .map(|(p, b)| json!([p, identity::bytes_digest(b)]))
        .collect();
    identity::digest(&json!({
        "repository": repository,
        "files": files,
        "capture_diagnostics": capture.diagnostics,
        "profile": profile.summary,
        "briefcase_version": env!("CARGO_PKG_VERSION"),
        "implementation": [
            identity::bytes_digest(include_bytes!("../capture.rs")),
            identity::bytes_digest(include_bytes!("../task/mod.rs")),
            identity::bytes_digest(include_bytes!("../task/index.rs")),
            identity::bytes_digest(include_bytes!("../source.rs")),
            identity::bytes_digest(include_bytes!("../rust.rs")),
            identity::bytes_digest(include_bytes!("../analyzer.rs")),
            identity::bytes_digest(include_bytes!("../typescript.rs")),
            identity::bytes_digest(include_bytes!("../typescript.cjs")),
            identity::bytes_digest(include_bytes!("../python.rs")),
            identity::bytes_digest(include_bytes!("../python.py")),
            identity::bytes_digest(include_bytes!("../native_source.rs")),
            identity::bytes_digest(include_bytes!("../native_host.rs")),
            identity::bytes_digest(include_bytes!("../go.rs")),
            identity::bytes_digest(include_bytes!("../go_adapter.go")),
            identity::bytes_digest(include_bytes!("../java.rs")),
            identity::bytes_digest(include_bytes!("../JavaAdapter.java")),
            identity::bytes_digest(include_bytes!("../csharp.rs")),
            identity::bytes_digest(include_bytes!("../CSharpAdapter.cs")),
            identity::bytes_digest(include_bytes!("../clang.rs")),
            identity::bytes_digest(include_bytes!("../clang_adapter.cpp")),
            identity::bytes_digest(include_bytes!("../execution.rs")),
            identity::bytes_digest(include_bytes!("../passes.rs")),
            identity::bytes_digest(include_bytes!("project.rs")),
        ],
        "recall": recall::configuration(),
    }))
}

fn ensure_rust_calls(l:&mut Ledger,view:&str,profile:&Profile,deadline:Instant)->Result<Value>{
    if profile.language!=Language::Rust||!profile.calls {return Ok(json!({"state":"not_requested"}));}
    let retained=l.retained(view)?;
    if super::super::contracts::rows(&retained.bundle,"runs").iter().any(|r|
        super::super::contracts::rows(&r["coverage"],"guarantees").iter().any(|g|g["predicate"]=="CALLS_STATIC")) {
        return Ok(json!({"state":"retained"}));
    }
    let Some((analyzer,_))=&profile.analyzer else {return Ok(json!({"state":"unavailable","reason":"Rust static calls require rust-analyzer; retained syntax/text remains available."}));};
    let remaining=deadline.saturating_duration_since(Instant::now());
    if remaining<Duration::from_secs(1) {return Ok(json!({"state":"budget_exhausted"}));}
    Ok(super::super::execution::analyze(l,&super::super::query::Options{view:view.into(),..Default::default()},
        &rust::ScanOptions{analyzer:Some(analyzer.clone()),timeout:remaining,features:vec![]})
        .unwrap_or_else(|error|json!({"state":"failed","reason":error.to_string()})))
}

fn normalize_project(project: &str) -> Option<String> {
    let mut parts = Vec::new();
    for component in Path::new(project).components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str()?),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn configuration_result(capture: &Capture, project: &str, reason: &str, detail: &str) -> Value {
    let candidates: Vec<_> = capture
        .files
        .keys()
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or(path);
            (name.starts_with("tsconfig") || name.starts_with("jsconfig"))
                && name.ends_with(".json")
        })
        .collect();
    // Keep structured suggestions bounded, including unusually long paths.
    let mut bytes = 0;
    let shown: Vec<_> = candidates
        .iter()
        .take(20)
        .take_while(|path| {
            bytes += path.len();
            bytes <= ERROR_LIMIT
        })
        .collect();
    let message = format!(
        "TS/JS project {project}: {detail}. Select a retained config explicitly with sync --project <path> --require-ready or set {{\"language\":\"typescript\",\"project\":\"<path>\"}} in {CONFIG}. Selection defines analysis coverage; whole-workspace analysis requires a config covering the intended sources without project references. Config candidates (coverage not verified): {}",
        shown.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")
    );
    json!({"sync":"needs_configuration", "message":bounded(&message),
        "configuration":{"project":bounded(project), "reason":reason,
            "candidates":shown,
            "candidates_truncated":candidates.len() > shown.len()}})
}

fn apply_one(o: &SyncOptions, _lock: &WriterLock, started: Instant, slot:Option<&str>) -> Result<Value> {
    let Input {
        profile,
        capture,
        repository,
        key,
    } = match prepare(o, started)? {
        Ok(input) => input,
        Err(value) => return Ok(value),
    };
    let mut ledger = Ledger::open(&o.db)?;
    let deadline = started + o.timeout;
    ledger::recovery::check_deadline(deadline)?;
    let prior=if let Some(slot)=slot {
        ledger.report(&profile_state_id(slot,&key)?).ok().and_then(|r|r["view_id"].as_str().map(|v|(key.clone(),v.to_owned(),r.clone())))
    } else {ledger.sync_state()?};
    if let Some((previous, view_id, _)) = prior {
        if previous == key
            && (slot.is_some() || ledger.resolve("latest").ok().as_deref() == Some(view_id.as_str()))
            && !ledger.pending_source()?
        {
            // These inputs already passed validation at their publication.
            let recovered = ledger.recover_pending(deadline)?;
            let mut view = ledger.load(&view_id)?;
            ledger::recovery::check_deadline(deadline)?;
            let rebuilt_now = view.stale || view.projections.len() != 2;
            let rebuilt = recovered.contains(&view_id) || rebuilt_now;
            if rebuilt_now {
                ledger.rebuild(&view_id)?;
                view = ledger.load(&view_id)?;
            }
            ledger::recovery::check_deadline(deadline)?;
            let static_calls=ensure_rust_calls(&mut ledger,&view_id,&profile,deadline)?;
            let (_, units) = recall::ensure_index(&ledger, &view)?;
            let task_index = crate::architecture::task::precompute(&ledger,&view_id)?;
            ledger::recovery::check_deadline(deadline)?;
            return Ok(json!({"sync": if rebuilt {"rebuilt"} else {"up-to-date"},
                "view_id":view_id,"input_key":key,"profile":profile.summary,"recall_units":units,"static_calls":static_calls,"task_index":task_index}));
        }
    }
    if profile.language == Language::Typescript {
        let remaining = o.timeout.saturating_sub(started.elapsed());
        if remaining < Duration::from_secs(1) {
            return Ok(
                json!({"sync":"timeout","message":"Budget exhausted before TypeScript configuration preflight"}),
            );
        }
        if let Some(error) = typescript::validate_project(
            &capture,
            &typescript::ScanOptions {
                node: o.node.clone(),
                compiler: profile.compiler.clone().unwrap(),
                project: profile.project.clone(),
                timeout: remaining.min(Duration::from_secs(600)),
                calls: profile.calls,
            },
        )? {
            return Ok(configuration_result(
                &capture,
                &profile.project,
                error["reason"].as_str().unwrap(),
                error["message"].as_str().unwrap(),
            ));
        }
    }
    // Recover only after validation succeeds: even recovery may publish
    // committed inputs or change an interrupted attempt's status.
    ledger.recover_pending(deadline)?;
    let remaining = o.timeout.saturating_sub(started.elapsed());
    if remaining < Duration::from_secs(1) {
        return Ok(json!({"sync":"timeout","profile":profile.summary,
            "message":"Budget exhausted before analysis; the last publication is unchanged"}));
    }
    let analysis_timeout = remaining.min(Duration::from_secs(600));
    let generation = ledger.begin(&repository, None)?;
    let produced = match profile.language {
        Language::Rust => rust::produce(
            capture,
            &repository,
            generation,
            &rust::ScanOptions {
                analyzer: profile.analyzer.as_ref().map(|(exe, _)| exe.clone()),
                timeout: analysis_timeout,
                features: vec![],
            },
        ),
        Language::Csharp => {
            let mut options = profile.csharp.clone().unwrap();
            options.timeout = analysis_timeout;
            csharp::produce(capture, &repository, generation, &options)
        }
        Language::C | Language::Cpp => {
            let mut options = profile.cc.clone().unwrap();
            options.timeout = analysis_timeout;
            clang::produce(capture, &repository, generation, &options)
        }
        Language::Go => {
            let mut options = profile.go.clone().unwrap();
            options.timeout = analysis_timeout;
            go::produce(capture, &repository, generation, &options)
        }
        Language::Java => {
            let mut options = profile.java.clone().unwrap();
            options.timeout = analysis_timeout;
            java::produce(capture, &repository, generation, &options)
        }
        Language::Python => {
            let mut options = profile.python.clone().unwrap();
            options.timeout = analysis_timeout;
            python::produce(capture, &repository, generation, &options)
        }
        Language::Typescript => typescript::produce(
            capture,
            &repository,
            generation,
            &typescript::ScanOptions {
                node: o.node.clone(),
                compiler: profile.compiler.clone().unwrap(),
                project: profile.project.clone(),
                timeout: analysis_timeout,
                calls: profile.calls,
            },
        ),
    };
    let prepared = match produced {
        Ok(p) => p,
        Err(e) => {
            ledger.fail(generation, &e.to_string())?;
            return Err(e);
        }
    };
    if let Err(error) = ledger::recovery::check_deadline(deadline) {
        ledger.fail(generation, &error.to_string())?;
        return Err(error);
    }
    let view = match ledger.commit(generation, &prepared) {
        Ok(view) => view,
        Err(e) => {
            ledger.fail(generation, &e.to_string())?;
            return Err(e);
        }
    };
    ledger::recovery::check_deadline(deadline)?;
    ledger.publish(&view, true)?;
    let static_calls=ensure_rust_calls(&mut ledger,&view,&profile,deadline)?;
    // A completed publication is durable even if the deadline expires in an
    // atomic phase. Record its input key so a retry can reuse and finish it.
    ledger.record_sync(&key, &view, &profile.summary)?;
    if let Some(slot)=slot {ledger.save_report(&profile_state_id(slot,&key)?,&json!({"view_id":view,"input_key":key,"profile":profile.summary}),&[])?;}
    ledger::recovery::check_deadline(deadline)?;
    let (_, units) = recall::ensure_index(&ledger, &ledger.load(&view)?)?;
    let task_index = crate::architecture::task::precompute(&ledger,&view)?;
    ledger::recovery::check_deadline(deadline)?;
    Ok(
        json!({"sync":"refreshed","view_id":view,"input_key":key,"generation":generation,
        "profile":profile.summary,"recall_units":units,"static_calls":static_calls,"task_index":task_index,
        "limitations":prepared.diagnostics["limitations"]}),
    )
}

struct Input {
    profile: Profile,
    capture: Capture,
    repository: String,
    key: String,
}
fn prepare(o: &SyncOptions, started: Instant) -> Result<std::result::Result<Input, Value>> {
    let mut profile = match profile(o, started + o.timeout) {
        Ok(p) => p,
        Err(ProfileError::Configuration(message)) => {
            return Ok(Err(
                json!({"sync":"needs_configuration","message":bounded(&message)}),
            ))
        }
        Err(ProfileError::Timeout(message)) => {
            return Ok(Err(json!({"sync":"timeout","message":bounded(&message)})))
        }
    };
    let capture = Capture::read_excluding(
        &o.root,
        profile.policy.as_deref(),
        &workspace::database_files(&o.db),
    )?;
    if profile.language == Language::Typescript {
        let Some(project) = normalize_project(&profile.project) else {
            return Ok(Err(configuration_result(
                &capture,
                &profile.project,
                "invalid_project",
                "configuration path must be relative and stay inside the repository",
            )));
        };
        profile.project = project;
        profile.summary["project"] = json!(profile.project);
        if !capture.files.contains_key(&profile.project) {
            let exists = std::fs::symlink_metadata(o.root.join(&profile.project)).is_ok();
            return Ok(Err(configuration_result(
                &capture,
                &profile.project,
                if exists {
                    "unavailable_project"
                } else {
                    "missing_project"
                },
                if exists {
                    "configuration exists but is unavailable to capture (ignored, excluded, symlinked or unreadable)"
                } else {
                    "configuration is missing"
                },
            )));
        }
        let compiler = profile.compiler.as_ref().unwrap();
        if !compiler.is_file() {
            return Ok(Err(
                json!({"sync":"needs_configuration", "message":bounded(&format!(
                "TS/JS sync needs the pinned TypeScript compiler; set BRIEFCASE_TYPESCRIPT_COMPILER (missing {})", compiler.display()))}),
            ));
        }
    }
    let repository = identity::id("repository", &json!({"root":o.root.to_string_lossy()}))?;
    let key = key(&repository, &capture, &profile)?;
    Ok(Ok(Input {
        profile,
        capture,
        repository,
        key,
    }))
}

/// Calculate the same publication key used by apply, without opening a generation.
pub fn check(o: &SyncOptions) -> Result<crate::architecture::reconcile::Alignment> {
    let profiles=configured_profiles(o)?;
    if profiles.is_empty() {return check_one(o,None);}
    use crate::architecture::reconcile::{Alignment,AlignmentState};
    let states=profiles.iter().map(|(slot,selected)|check_one(selected,Some(slot))).collect::<Result<Vec<_>>>()?;
    if states.iter().all(|s|s.state==AlignmentState::Unavailable) {
        return Ok(Alignment::unavailable("project",json!({"sync":"needs_configuration","profiles":states,
            "message":"One or more configured analysis profiles are unavailable; each profile keeps its own diagnostics."})));
    }
    let keys:Vec<_>=profiles.iter().zip(states.iter()).map(|((slot,_),s)|json!([slot,s.observed_revision])).collect();
    let observed=identity::id("profile-set",&json!(keys))?;
    let active=if o.db.is_file() {Ledger::read_only(&o.db)?.sync_state()?.map(|s|s.0)} else {None};
    let aligned=states.iter().all(|s|s.state==AlignmentState::Aligned) && active.as_deref()==Some(observed.as_str());
    let mut out=Alignment::new("project",Some(observed.clone()),aligned.then_some(observed));
    out.detail=json!({"profiles":states,"profile_count":profiles.len()});
    Ok(out)
}

fn check_one(o: &SyncOptions, slot:Option<&str>) -> Result<crate::architecture::reconcile::Alignment> {
    use crate::architecture::reconcile::{Alignment, AlignmentState};
    let input = match prepare(o, Instant::now())? {
        Ok(input) => input,
        Err(value) => return Ok(Alignment::unavailable("project", value)),
    };
    let mut alignment = Alignment::new("project", Some(input.key.clone()), None);
    alignment.detail = json!({"profile":input.profile.summary});
    if o.db.is_file() {
        let ledger = Ledger::read_only(&o.db)?;
        let prior=if let Some(slot)=slot {
            ledger.report(&profile_state_id(slot,&input.key)?).ok().and_then(|r|r["view_id"].as_str().map(|v|(input.key.clone(),v.to_owned(),r.clone())))
        } else {ledger.sync_state()?};
        if let Some((key, view_id, _)) = prior {
            alignment.applied_revision = Some(key.clone());
            alignment.state = AlignmentState::Dirty;
            if key == input.key
                && (slot.is_some() || ledger.resolve("latest").ok().as_deref() == Some(&view_id))
                && !ledger.pending_source()?
            {
                let view = ledger.load(&view_id)?;
                let recall =
                    ledger.recall_ready(&view_id, &identity::digest(&recall::configuration())?)?;
                if !view.stale && view.projections.len() == 2 && recall.is_some() {
                    alignment.state = AlignmentState::Aligned;
                }
                alignment.detail["view_id"] = json!(view_id);
                alignment.detail["recall_units"] = json!(recall);
            }
        }
    }
    Ok(alignment)
}
