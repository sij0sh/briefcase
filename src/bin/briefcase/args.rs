use super::*;

#[derive(Parser)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version,
    about = "Investigate captured architecture, changes and declared policy"
)]
pub(crate) struct Cli {
    /// Database path. Default: BRIEFCASE_DB, then <repository-root>/.briefcase/briefcase.db.
    #[arg(long, global = true)]
    pub(crate) db: Option<PathBuf>,
    /// Repository root for directories outside Git or for a selected subproject.
    #[arg(long, global = true)]
    pub(crate) project_root: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[command(subcommand)]
    pub(crate) command: Command,
}
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Language {
    Rust,
    C,
    #[value(name = "cpp", alias = "c++")]
    Cpp,
    #[value(name = "csharp", alias = "c#")]
    Csharp,
    Go,
    Java,
    Python,
    #[value(alias = "javascript")]
    Typescript,
}
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum McpSurface {
    Gateway,
    Compact,
}
impl From<McpSurface> for mcp::Surface {
    fn from(surface: McpSurface) -> Self {
        match surface {
            McpSurface::Gateway => mcp::Surface::Gateway,
            McpSurface::Compact => mcp::Surface::Compact,
        }
    }
}
#[derive(Args, Clone)]
pub(crate) struct TypeScript {
    #[arg(long, default_value = "tsconfig.json")]
    pub(crate) project: String,
    #[arg(long, default_value = "node")]
    pub(crate) node: PathBuf,
    #[arg(long)]
    pub(crate) typescript_compiler: Option<PathBuf>,
}
impl TypeScript {
    pub(crate) fn options(self, timeout: u64, calls: bool) -> typescript::ScanOptions {
        let mut options = typescript::ScanOptions {
            node: self.node,
            project: self.project,
            timeout: Duration::from_secs(timeout),
            calls,
            ..Default::default()
        };
        if let Some(compiler) = self.typescript_compiler {
            options.compiler = compiler;
        }
        options
    }
}
#[derive(Args, Clone)]
pub(crate) struct Python {
    /// Trusted CPython interpreter. Default: BRIEFCASE_PYTHON, then python3.
    #[arg(long)]
    pub(crate) python: Option<PathBuf>,
    /// Retained import roots, for example src,tests. Default: src plus root, or root.
    #[arg(long, value_delimiter = ',')]
    pub(crate) python_source_root: Vec<String>,
}
impl Python {
    pub(crate) fn options(
        self,
        timeout: u64,
        calls: bool,
        syntax_only: bool,
    ) -> python::ScanOptions {
        let mut options = python::ScanOptions {
            source_roots: self.python_source_root,
            timeout: Duration::from_secs(timeout),
            calls,
            syntax_only,
            ..Default::default()
        };
        if let Some(interpreter) = self.python {
            options.python = interpreter;
        }
        options
    }
}
#[derive(Args, Clone)]
pub(crate) struct Go {
    /// Trusted Go executable. Default: BRIEFCASE_GO, then go.
    #[arg(long)]
    pub(crate) go: Option<PathBuf>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) go_source_root: Vec<String>,
    /// Explicit build tags. GOFLAGS and repository commands are never loaded.
    #[arg(long, value_delimiter = ',')]
    pub(crate) go_tags: Vec<String>,
    #[arg(long)]
    pub(crate) goos: Option<String>,
    #[arg(long)]
    pub(crate) goarch: Option<String>,
    #[arg(long)]
    pub(crate) go_tests: bool,
}
impl Go {
    pub(crate) fn options(self, timeout: u64, calls: bool, syntax_only: bool) -> go::ScanOptions {
        let mut options = go::ScanOptions {
            source_roots: self.go_source_root,
            tags: self.go_tags,
            goos: self.goos,
            goarch: self.goarch,
            tests: self.go_tests,
            timeout: Duration::from_secs(timeout),
            calls,
            syntax_only,
            ..Default::default()
        };
        if let Some(executable) = self.go {
            options.go = executable;
        }
        options
    }
}
#[derive(Args, Clone)]
pub(crate) struct Java {
    /// Trusted javac executable. Its sibling java supplies the same JDK.
    #[arg(long)]
    pub(crate) javac: Option<PathBuf>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) java_source_root: Vec<String>,
    /// Non-preview source release. Default: selected JDK release.
    #[arg(long)]
    pub(crate) java_release: Option<u16>,
}
impl Java {
    pub(crate) fn options(self, timeout: u64, calls: bool, syntax_only: bool) -> java::ScanOptions {
        let mut options = java::ScanOptions {
            source_roots: self.java_source_root,
            release: self.java_release,
            timeout: Duration::from_secs(timeout),
            calls,
            syntax_only,
            ..Default::default()
        };
        if let Some(executable) = self.javac {
            options.javac = executable;
        }
        options
    }
}
#[derive(Args, Clone)]
pub(crate) struct CSharp {
    /// Trusted .NET SDK host. Default: BRIEFCASE_DOTNET, then dotnet. MSBuild/NuGet are never used.
    #[arg(long)]
    pub(crate) dotnet: Option<PathBuf>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) csharp_source_root: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) csharp_define: Vec<String>,
    /// Numeric non-preview language version. Default: selected Roslyn compiler.
    #[arg(long)]
    pub(crate) csharp_language_version: Option<String>,
}
impl CSharp {
    pub(crate) fn options(
        self,
        timeout: u64,
        calls: bool,
        syntax_only: bool,
    ) -> csharp::ScanOptions {
        let mut options = csharp::ScanOptions {
            source_roots: self.csharp_source_root,
            defines: self.csharp_define,
            language_version: self.csharp_language_version,
            timeout: Duration::from_secs(timeout),
            calls,
            syntax_only,
            ..Default::default()
        };
        if let Some(executable) = self.dotnet {
            options.dotnet = executable;
        }
        options
    }
}
#[derive(Args, Clone)]
pub(crate) struct CFamily {
    /// Trusted Clang compiler. Default: BRIEFCASE_CLANG, then clang.
    #[arg(long)]
    pub(crate) clang: Option<PathBuf>,
    /// Matching LLVM development installation. Default: BRIEFCASE_LLVM_CONFIG, then llvm-config.
    #[arg(long)]
    pub(crate) llvm_config: Option<PathBuf>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) cc_source_root: Vec<String>,
    /// Retained relative include roots. No external/system headers are loaded.
    #[arg(long, value_delimiter = ',')]
    pub(crate) cc_include_root: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) cc_define: Vec<String>,
    /// Non-extension standard, such as c17 or c++20.
    #[arg(long)]
    pub(crate) cc_standard: Option<String>,
    #[arg(long)]
    pub(crate) cc_target: Option<String>,
}
impl CFamily {
    pub(crate) fn options(
        self,
        language: clang::Language,
        timeout: u64,
        calls: bool,
        syntax_only: bool,
    ) -> clang::ScanOptions {
        let mut options = clang::ScanOptions {
            language,
            source_roots: self.cc_source_root,
            include_roots: self.cc_include_root,
            defines: self.cc_define,
            standard: self.cc_standard,
            target: self.cc_target,
            timeout: Duration::from_secs(timeout),
            calls,
            syntax_only,
            ..Default::default()
        };
        if let Some(executable) = self.clang {
            options.clang = executable;
        }
        if let Some(executable) = self.llvm_config {
            options.llvm_config = executable;
        }
        options
    }
}
#[derive(Args, Clone)]
pub(crate) struct Query {
    #[arg(long, default_value = "latest")]
    pub(crate) view: String,
    #[arg(long)]
    pub(crate) component: Option<String>,
    #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u16).range(1..=1000))]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) allow_partial: bool,
    #[arg(long)]
    pub(crate) historical: bool,
    #[arg(long)]
    pub(crate) include_structure: bool,
}
impl Query {
    /// Pass CLI defaults explicitly; never inherit transport-specific defaults.
    pub(crate) fn arguments(&self) -> Value {
        let mut args = json!({"view":self.view,"limit":self.limit,
            "allow_partial":self.allow_partial,"historical":self.historical,
            "include_structure":self.include_structure});
        if let Some(component) = &self.component {
            args["component"] = json!(component);
        }
        args
    }

    pub(crate) fn call(self, ledger: &Ledger, operation: &str, extra: Value) -> Result<Value> {
        let mut args = self.arguments();
        for (key, value) in extra.as_object().expect("owned read arguments") {
            if !value.is_null() {
                args[key] = value.clone();
            }
        }
        service::call(ledger, operation, &args)
    }
}
impl From<Query> for Options {
    fn from(q: Query) -> Self {
        Self {
            view: q.view,
            component: q.component,
            limit: q.limit as usize,
            allow_partial: q.allow_partial,
            historical: q.historical,
            include_structure: q.include_structure,
        }
    }
}
/// One pinned session publication.
#[derive(Args)]
pub(crate) struct SessionQuery {
    /// Session publication: view ID or label, `latest`, session context ID,
    /// or the harness's own session ID.
    #[arg(long, visible_alias = "id", default_value = "latest")]
    pub(crate) session: String,
    #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u16).range(1..=1000))]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) allow_partial: bool,
    #[arg(long)]
    pub(crate) historical: bool,
}
impl From<SessionQuery> for Options {
    fn from(q: SessionQuery) -> Self {
        Self {
            view: q.session,
            limit: q.limit as usize,
            allow_partial: q.allow_partial,
            historical: q.historical,
            ..Options::default()
        }
    }
}
#[derive(Subcommand)]
pub(crate) enum SessionsAction {
    /// Retain one JSONL session transcript and publish a session view.
    Import {
        path: PathBuf,
        /// Built-in profile (claude-code-v1, codex-v1, pi-v1) or a profile JSON file.
        #[arg(long)]
        profile: String,
        #[arg(long)]
        label: Option<String>,
        /// Root that recorded paths are made relative to. Default: the repository root.
        #[arg(long)]
        repository_root: Option<PathBuf>,
        /// Namespace for a database with no capture yet (as for scan).
        #[arg(long)]
        repository_id: Option<String>,
        /// Full bytes, cited activity lines, or metadata without message/command text.
        #[arg(long, default_value = "full", value_parser = ["full", "cited", "metadata"])]
        session_retention: String,
        /// Interpret and report coverage without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Import a harness's local histories whose recorded cwd exactly matches
    /// this repository (`import-pi` is the Pi default).
    #[command(name = "import-local", visible_alias = "import-pi")]
    ImportLocal {
        /// A harness registry name.
        #[arg(long, default_value = "pi", value_parser = harness_names())]
        harness: String,
        /// Default: PI_CODING_AGENT_DIR/sessions or ~/.pi/agent/sessions (pi),
        /// CODEX_HOME/sessions or ~/.codex/sessions (codex),
        /// CLAUDE_CONFIG_DIR/projects or ~/.claude/projects (claude-code).
        #[arg(long)]
        sessions_dir: Option<PathBuf>,
        #[arg(long)]
        repository_root: Option<PathBuf>,
        #[arg(long, default_value = "full", value_parser = ["full", "cited", "metadata"])]
        session_retention: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// List the built-in session profiles.
    Profiles,
}

/// Harness names accepted on the command line, from the registry.
pub(crate) fn harness_names() -> clap::builder::PossibleValuesParser {
    briefcase::architecture::session::Harness::names().into()
}

/// `sync --sessions` selectors: `all` or any registered harness.
pub(crate) fn session_selectors() -> clap::builder::PossibleValuesParser {
    std::iter::once("all")
        .chain(briefcase::architecture::session::Harness::names())
        .into()
}
