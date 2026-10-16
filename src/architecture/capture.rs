//! Bounded, immutable capture. Analysis consumes these bytes, never a mutable working tree.
use super::{
    contracts::{add, Payloads},
    identity, require, Result,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(crate) const EXCLUDED: &[&str] = &[
    ".git",
    ".aws",
    ".briefcase",
    ".briefcase-scheduler",
    ".briefcase-input",
    "target",
    "node_modules",
    "dist",
    "build",
    "BUILD",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".nox",
    ".gradle",
    ".mvn-cache",
];
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;
/// `Dockerfile`, `Dockerfile.<name>` and `<name>.Dockerfile` are retained build inputs.
pub(crate) fn is_dockerfile(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "Dockerfile" || name.starts_with("Dockerfile.") || name.ends_with(".Dockerfile")
}
#[derive(Debug)]
pub struct Capture {
    pub root: PathBuf,
    pub files: BTreeMap<String, Vec<u8>>,
    pub diagnostics: Vec<String>,
    pub exclusions: Vec<String>,
    pub complete: bool,
    /// Selection coverage is separate from semantic producer completeness.
    pub inventory: Value,
    /// Advisory source-read time; never part of canonical snapshot identity.
    pub captured_at_unix_ms: Option<u64>,
}
type Selection = (Vec<PathBuf>, Vec<String>, Vec<String>, Option<BTreeSet<String>>);

/// Never follow a tracked symlink, including a symlink in a parent directory.
fn safe_path(root: &Path, relative: &Path) -> bool {
    let mut current = root.to_path_buf();
    for part in relative.components() {
        let std::path::Component::Normal(part) = part else { return false };
        current.push(part);
        if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            return false;
        }
    }
    true
}

fn tracked_paths(root: &Path) -> Result<Option<BTreeSet<String>>> {
    if !root.join(".git").exists() { return Ok(None); }
    let git = super::native_host::executable(Path::new("git"), "Git")?;
    let mut command = super::native_host::command(&git, root);
    command.env("GIT_OPTIONAL_LOCKS", "0").env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["ls-files", "--cached", "-z"]);
    let output = super::native_host::run(command, &[],
        std::time::Instant::now() + std::time::Duration::from_secs(5))?;
    output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty())
        .map(|p| Ok(std::str::from_utf8(p)?.to_owned())).collect::<Result<BTreeSet<_>>>().map(Some)
}

fn paths(root: &Path) -> Result<Selection> {
    let mut diagnostics = Vec::new();
    let mut exclusions = Vec::new();
    let mut paths = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .parents(false)
        .require_git(false)
        .filter_entry(|entry| {
            !entry.file_type().is_some_and(|t| t.is_dir())
                || !EXCLUDED.iter().any(|n| entry.file_name() == *n)
        })
        .build();
    for entry in walker {
        match entry {
            Ok(entry) => {
                if entry.path() == root {
                    continue;
                }
                let label = entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .to_string();
                if entry.file_type().is_some_and(|t| t.is_symlink()) {
                    exclusions.push(format!("symlink: {label}"));
                    continue;
                }
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                // File names/extensions are not a completeness boundary. Retain
                // extensionless scripts, instructions, plans and new formats too.
                paths.push(entry.into_path());
            }
            Err(e) => diagnostics.push(format!("unreadable scan entry: {e}")),
        }
    }
    let tracked = match tracked_paths(root) {
        Ok(tracked) => tracked,
        Err(error) => {
            diagnostics.push(format!("tracked inventory unavailable: {error}"));
            None
        }
    };
    for label in tracked.iter().flatten() {
        let relative = Path::new(label);
        if !safe_path(root, relative) {
            exclusions.push(format!("unsafe or symlinked tracked path: {label}"));
        } else if relative.components().any(|p| matches!(p,
            std::path::Component::Normal(n) if [".git", ".briefcase", ".briefcase-scheduler", ".briefcase-input"].iter().any(|x| n == std::ffi::OsStr::new(x)))) {
            exclusions.push(format!("internal tracked state: {label}"));
        } else if root.join(relative).is_file() {
            paths.push(root.join(relative));
        } else {
            exclusions.push(format!("tracked path unavailable: {label}"));
        }
    }
    paths.sort();
    paths.dedup();
    exclusions.sort();
    exclusions.dedup();
    Ok((paths, diagnostics, exclusions, tracked))
}
/// Probe source selection without reading source bytes or executing a project.
pub(crate) fn contains_source(root: &Path, selected: impl Fn(&str) -> bool) -> Result<bool> {
    let (paths, _, _, _) = paths(root)?;
    Ok(paths
        .iter()
        .any(|path| path.to_str().is_some_and(&selected)))
}
impl Capture {
    /// Publish a useful lowest layer even for an unconfigured/polyglot project
    /// or a host without compiler adapters. No semantic graph is invented.
    pub fn text_publication(self,repository:&str,generation:u64) -> Result<super::ledger::Prepared> {
        use super::{contracts::add,rust};
        require(!self.files.is_empty(),"CAPTURE","No retained text inputs")?;
        let settings=json!({"name":"retained-text-inventory","method":"file-inventory-v1"});
        let (mut bundle,payloads,ctx)=self.seed(repository,&settings)?;
        let implementation=identity::bytes_digest(include_bytes!("capture.rs"));
        let config_id=identity::id("analysis",&json!({"producer_name":"retained-text-inventory","implementation_digest":implementation,"settings_digest":identity::digest(&settings)?}))?;
        add(&mut bundle,"analysis_configs",json!({"id":config_id,"producer_name":"retained-text-inventory","implementation_digest":implementation,"settings_digest":identity::digest(&settings)?}));
        let coverage=json!({"attempted":self.files.len(),"completed":self.files.len(),"unsupported_count":0,"unresolved_count":0,
            "resolution_complete":false,"guarantees":[{"predicate":"CONTAINS","kind":"syntax_only"}]});
        let run=identity::id("run",&json!({"context":ctx,"analysis_config_id":config_id,"coverage":coverage}))?;
        add(&mut bundle,"runs",json!({"id":run,"producer_kind":"syntax","analysis_config_id":config_id,"context":ctx,"status":if self.complete {"complete"} else {"partial"},"coverage":coverage}));
        let provider=identity::id("identity-provider",&settings)?;
        let repo=rust::entity_for_language(&mut bundle,&ctx,"repository",repository,&provider,"syntax","none")?;
        for (path,bytes) in &self.files {
            let file=rust::entity_for_language(&mut bundle,&ctx,"file",path,&provider,"syntax","none")?;
            if bytes.is_empty() {continue;}
            let evidence=rust::evidence(&mut bundle,path,0,bytes.len())?;
            rust::assertion(&mut bundle,&ctx,"evidence",&repo,"CONTAINS",json!({"kind":"entity","entity_version_id":file}),
                json!({"run_id":run}),vec![evidence],None)?;
        }
        let diagnostics=json!({"capture":self.metadata(),"coverage":coverage,"exclusions":self.exclusions,"capture_diagnostics":self.diagnostics,
            "requested_analyses":{"dependencies":false,"static_calls":false,"data":false},
            "limitations":["Text/file inventory only. Language parsing, dependencies, calls, data and runtime analysis have not run."]});
        super::ledger::Prepared::seal(bundle,payloads,generation,diagnostics)
    }

    pub fn read(root: &Path, policy: Option<&Path>) -> Result<Self> {
        Self::read_excluding(root, policy, &[])
    }
    /// A live database may be outside `.briefcase`; its sidecars are never source inputs.
    pub(crate) fn read_excluding(
        root: &Path,
        policy: Option<&Path>,
        excluded: &[PathBuf],
    ) -> Result<Self> {
        let root = root.canonicalize()?;
        require(root.is_dir(), "CAPTURE", "Repository must be a directory")?;
        let capture_head_before=super::freshness::head(&root).ok();
        let (mut discovered, mut diagnostics, mut exclusions, tracked) = paths(&root)?;
        discovered.retain(|p| !excluded.contains(p));
        let mut files = BTreeMap::new();
        let mut total = 0;
        for path in &discovered {
            let name = path
                .strip_prefix(&root)?
                .to_str()
                .ok_or("CAPTURE: Non-UTF-8 source path")?
                .replace('\\', "/");
            match std::fs::metadata(path).and_then(|m| {
                if m.len() > super::source::MAX_SOURCE_BYTES {
                    Err(std::io::Error::other("per-file byte limit"))
                } else {
                    std::fs::read(path)
                }
            }) {
                Ok(bytes) => {
                    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
                        exclusions.push(format!("binary input: {name}"));
                        continue;
                    }
                    total += bytes.len();
                    if total > MAX_TOTAL_BYTES {
                        diagnostics.push("capture byte quota exhausted".into());
                        break;
                    }
                    files.insert(name, bytes);
                }
                Err(e) if e.to_string() == "per-file byte limit" => exclusions.push(format!("per-file byte limit: {name}")),
                Err(e) => diagnostics.push(format!("skipped {name}: {e}")),
            }
        }
        // Detect edits/additions/removals racing the entire capture, including equal-size edits.
        let (mut after, errors, _, after_tracked) = paths(&root)?;
        after.retain(|p| !excluded.contains(p));
        diagnostics.extend(errors);
        if after != discovered || after_tracked != tracked {
            diagnostics.push("file membership changed during capture".into());
        }
        for (name, bytes) in &files {
            if std::fs::read(root.join(name)).ok().as_ref() != Some(bytes) {
                diagnostics.push(format!("source changed during capture: {name}"));
            }
        }
        if let Some(policy) = policy {
            let bytes = std::fs::read(policy)?;
            require(
                bytes.len() <= super::source::MAX_SOURCE_BYTES as usize,
                "CAPTURE",
                "Policy byte limit exceeded",
            )?;
            super::declarations::validate(&identity::parse(&bytes)?)?;
            files.insert(super::declarations::ARTIFACT_PATH.into(), bytes);
        }
        require(!files.is_empty(), "CAPTURE", "No selected source inputs")?;
        let missing: Vec<_> = tracked.iter().flatten().filter(|p| !files.contains_key(*p)).cloned().collect();
        let capture_head_after=super::freshness::head(&root).ok();
        if capture_head_before!=capture_head_after {diagnostics.push("Git HEAD changed while source bytes were captured; source/revision alignment is unestablished.".into());}
        let capture_head=if capture_head_before==capture_head_after {capture_head_after}else{None};
        let inventory = json!({"capture_head":capture_head,"method":"tracked-plus-unignored-text-v1", "tracked_inventory_available":tracked.is_some(),
            "tracked_files":tracked.as_ref().map(BTreeSet::len), "retained_files":files.len(),
            "unretained_tracked_paths":missing, "tracked_text_complete":tracked.is_some() && missing.is_empty() && diagnostics.is_empty(),
            "exclusions":exclusions, "diagnostics":diagnostics});
        Ok(Self {
            captured_at_unix_ms: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            ),
            root,
            files,
            complete: diagnostics.is_empty(),
            inventory,
            diagnostics,
            exclusions,
        })
    }
    pub(crate) fn metadata(&self) -> Value {
        json!({"repository_root":self.root,"captured_at_unix_ms":self.captured_at_unix_ms,"capture_head":self.inventory["capture_head"],"inventory":self.inventory})
    }
    pub fn seed(
        &self,
        repository: &str,
        build_settings: &Value,
    ) -> Result<(Value, Payloads, Value)> {
        let mut b = super::contracts::empty_bundle();
        let mut payloads = Payloads::new();
        let manifest: Vec<_> = self
            .files
            .iter()
            .map(|(p, bytes)| json!({"path":p,"digest":identity::bytes_digest(bytes)}))
            .collect();
        let digest = identity::digest(&json!(manifest))?;
        let sid = identity::id(
            "snapshot",
            &json!({"repository_id":repository,"files":manifest}),
        )?;
        add(
            &mut b,
            "snapshots",
            json!({"id":sid,"repository_id":repository,"manifest_digest":digest,"files":manifest,"capture_state":if self.complete {"complete"}else{"partial"}}),
        );
        let config: Vec<_> = manifest
            .iter()
            .filter(|f| {
                f["path"].as_str().is_some_and(|p| {
                    p.ends_with("Cargo.toml")
                        || p.ends_with("Cargo.lock")
                        || p.contains(".cargo/")
                        || p.contains("rust-toolchain")
                        || (build_settings["name"] == "go-ast-types"
                            && ["go.mod", "go.sum", "go.work", "go.work.sum"]
                                .iter()
                                .any(|n| p.rsplit('/').next() == Some(n)))
                        || (build_settings["name"] == "jdk-compiler-api"
                            && [
                                "pom.xml",
                                "build.gradle",
                                "build.gradle.kts",
                                "settings.gradle",
                                "settings.gradle.kts",
                                "gradle.properties",
                                "gradle.lockfile",
                            ]
                            .iter()
                            .any(|n| p.rsplit('/').next() == Some(n)))
                        || (build_settings["name"] == "roslyn"
                            && (p.ends_with(".csproj")
                                || p.ends_with(".sln")
                                || p.ends_with(".slnx")
                                || p.ends_with(".props")
                                || p.ends_with(".targets")
                                || p.ends_with("global.json")
                                || p.ends_with("NuGet.Config")
                                || p.ends_with("nuget.config")))
                        || (build_settings["name"] == "clang-ast"
                            && [
                                "CMakeLists.txt",
                                "CMakePresets.json",
                                "compile_commands.json",
                                "Makefile",
                                "GNUmakefile",
                                "meson.build",
                                "meson_options.txt",
                                "configure.ac",
                            ]
                            .iter()
                            .any(|n| p.rsplit('/').next() == Some(n)))
                        || (build_settings["name"] == "cpython-ast-symtable"
                            && (p.ends_with("pyproject.toml")
                                || p.ends_with("setup.cfg")
                                || p.ends_with("Pipfile")
                                || p.ends_with("requirements.txt")
                                || p.ends_with("poetry.lock")
                                || p.ends_with("uv.lock")
                                || p.ends_with("Pipfile.lock")))
                        || (build_settings["name"] == "typescript-compiler-api"
                            && (p.ends_with(".json")
                                || p.ends_with("yarn.lock")
                                || p.ends_with("pnpm-lock.yaml")))
                })
            })
            .cloned()
            .collect();
        let mut build = json!({"snapshot_id":sid,"settings_digest":identity::digest(build_settings)?,"dependency_digest":identity::digest(&json!(config))?,"generated_inputs_digest":identity::digest(&json!([]))?});
        let bid = identity::id("build", &build)?;
        build["id"] = json!(bid);
        add(&mut b, "build_contexts", build);
        for (path, bytes) in &self.files {
            let source_kind = if super::source::code_extension(path).is_some() {
                "code"
            } else if path.ends_with(".prisma") || path.ends_with(".sql") {
                "schema"
            } else if path.ends_with(".md") {
                "documentation"
            } else {
                "configuration"
            };
            let digest = identity::bytes_digest(bytes);
            let aid = identity::id(
                "artifact",
                &json!({"path":path,"digest":digest,"source_kind":source_kind}),
            )?;
            add(
                &mut b,
                "artifacts",
                json!({"id":aid,"path":path,"digest":digest,"source_kind":source_kind,"availability":"available"}),
            );
            payloads.insert(aid, bytes.clone());
        }
        Ok((
            b,
            payloads,
            json!({"kind":"source","snapshot_id":sid,"build_context_id":bid}),
        ))
    }
    pub fn materialize(&self) -> Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        for (name, bytes) in &self.files {
            let path = dir.path().join(name);
            require(path.starts_with(dir.path()), "CAPTURE", "Unsafe path")?;
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, bytes)?;
        }
        Ok(dir)
    }
}
