use super::*;
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Adapter lifecycle context; consumes a bounded harness JSON event on stdin.
    #[command(hide=true)]
    ContextHook {
        #[arg(long,value_parser=["prompt","complete"])]
        phase: String,
    },
    /// Observe selected installed files and compare their bytes with retained source.
    InspectExternal {
        #[arg(required=true)]
        paths: Vec<PathBuf>,
        #[arg(long,default_value="latest")]
        view: String,
    },
    /// Retain text and file inventory without requiring a language toolchain.
    CaptureText {
        #[arg(default_value=".")]
        path: PathBuf,
        #[arg(long)]
        repository_id: Option<String>,
        #[arg(long)]
        label: Option<String>,
    },
    /// Compose one task packet, including uncovered evidence and continuations.
    #[command(alias = "ask")]
    Brief {
        task: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long, value_enum)]
        duty: Option<briefcase::architecture::task::Duty>,
        #[arg(long,default_value="latest")]
        view: String,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..=3))]
        depth: Option<u64>,
        #[arg(long,default_value_t=32,value_parser=clap::value_parser!(u64).range(1..=1000))]
        max_files:u64,
        /// Emit the budgeted manifest used by automatic coding-harness context.
        #[arg(long)]
        agent_context:bool,
        #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u64).range(1..=1000))]
        limit: u64,
        #[arg(long,default_value_t=24576,value_parser=clap::value_parser!(u64).range(256..=1048576))]
        max_bytes: u64,
    },
    /// Exhaustive, deterministic literal occurrences, independent of graph readiness.
    Occurrences {
        text: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long,default_value="latest")]
        view: String,
        #[arg(long,default_value_t=0)]
        offset: u64,
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u64).range(1..=1000))]
        limit: u64,
        #[arg(long,default_value_t=24576,value_parser=clap::value_parser!(u64).range(256..=1048576))]
        max_bytes: u64,
    },
    /// Forward or reverse relationships, including TESTS and configuration consumers.
    Neighbors {
        target: String,
        #[arg(long,default_value="both",value_parser=["in","out","both"])]
        direction: String,
        #[arg(long)]
        predicate: Option<String>,
        #[arg(long,default_value="latest")]
        view: String,
        #[arg(long,default_value_t=1,value_parser=clap::value_parser!(u64).range(1..=3))]
        depth: u64,
        #[arg(long,default_value_t=0)]
        offset: u64,
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u64).range(1..=1000))]
        limit: u64,
        #[arg(long,default_value_t=24576,value_parser=clap::value_parser!(u64).range(256..=1048576))]
        max_bytes: u64,
    },
    /// Capture immutable inputs and publish a compatible graph/retrieval generation.
    Scan {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, value_enum, default_value = "rust")]
        language: Language,
        #[command(flatten)]
        typescript: TypeScript,
        #[command(flatten)]
        python: Python,
        #[command(flatten)]
        go: Go,
        #[command(flatten)]
        java: Java,
        #[command(flatten)]
        csharp: CSharp,
        #[command(flatten)]
        cc: CFamily,
        /// Include producer-supported direct calls during capture.
        #[arg(long, conflicts_with = "no_calls")]
        calls: bool,
        /// Disable static call extraction for this capture/profile.
        #[arg(long)]
        no_calls: bool,
        /// Bind two working directories/revisions to the same repository namespace.
        #[arg(long)]
        repository_id: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        policy: Option<PathBuf>,
        #[arg(long)]
        syntax_only: bool,
        #[arg(long, default_value = "rust-analyzer")]
        analyzer: PathBuf,
        #[arg(long,default_value_t=60,value_parser=clap::value_parser!(u64).range(1..=600))]
        timeout: u64,
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
    },
    #[command(hide = true)]
    Dependencies(Query),
    /// Add native direct-call Evidence and versioned static reachability to a captured view.
    AnalyzeExecution {
        #[command(flatten)]
        query: Query,
        #[arg(long, value_enum, default_value = "rust")]
        language: Language,
        #[command(flatten)]
        typescript: TypeScript,
        #[command(flatten)]
        python: Python,
        #[command(flatten)]
        go: Go,
        #[command(flatten)]
        java: Java,
        #[command(flatten)]
        csharp: CSharp,
        #[command(flatten)]
        cc: CFamily,
        #[arg(long, default_value = "rust-analyzer")]
        analyzer: PathBuf,
        #[arg(long, default_value_t=60, value_parser=clap::value_parser!(u64).range(1..=600))]
        timeout: u64,
    },
    /// Add schema, ORM, route and field-lineage Evidence to a captured TS/JS view.
    AnalyzeData {
        #[command(flatten)]
        query: Query,
        #[command(flatten)]
        typescript: TypeScript,
        #[arg(long, default_value_t=60, value_parser=clap::value_parser!(u64).range(1..=600))]
        timeout: u64,
    },
    /// Retry declared technical references that did not bind at scan (for
    /// example endpoints added by analyze-data) against the current source
    /// view, from the retained declaration artifact. analyze-data runs it.
    BindDeclarations(Query),
    /// Capture retained build manifests, OCI manifests and Kubernetes YAML as desired runtime.
    CaptureDeployment {
        path: PathBuf,
        #[arg(long)]
        environment: String,
        /// Pinned source view whose snapshot the build manifests must name.
        #[arg(long)]
        source_view: Option<String>,
        #[arg(long)]
        label: Option<String>,
    },
    /// Ingest one retained OTLP JSON trace export for an environment and interval.
    IngestRuntime {
        #[arg(long)]
        otlp: PathBuf,
        #[arg(long)]
        environment: String,
        #[arg(long)]
        interval_start: String,
        #[arg(long)]
        interval_end: String,
        #[arg(long)]
        collector: String,
        #[arg(long, default_value = "sampled")]
        sampling: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// Inspect supported direct calls and explicit unsupported source occurrences.
    #[command(hide = true)]
    Callgraph(Query),
    /// Query versioned direct-call reachability, not observed runtime behavior.
    Reachability {
        #[command(flatten)]
        query: Query,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
    },
    /// Inspect or request versioned dependency/SCC pass outputs.
    Passes {
        #[arg(long, default_value = "latest")]
        view: String,
        #[arg(long)]
        plan: Option<PathBuf>,
        #[arg(long)]
        require_complete: bool,
    },
    /// Run an explicitly selected local provider on a bounded evidence packet.
    Classify {
        #[command(flatten)]
        query: Query,
        #[arg(long)]
        provider: PathBuf,
        #[arg(long)]
        taxonomy: PathBuf,
        #[arg(long, default_value_t = 16)]
        max_assertions: usize,
        #[arg(long, default_value_t = 16384)]
        max_bytes: usize,
    },
    Domains(Query),
    Drift(Query),
    Compare {
        #[arg(long)]
        base: String,
        #[command(flatten)]
        query: Query,
    },
    /// Query exact, likely, combined, or ambiguous historical lineage with labeled certainty.
    HistoryLineage {
        #[arg(long)]
        base: String,
        #[command(flatten)]
        query: Query,
        #[arg(long, default_value = "combined")]
        mode: String,
    },
    /// Retained evolution: captured lineage, Git change activity, change coupling
    /// (co-change, never dependency) and captured architecture changes.
    /// With --base, retained correspondence is prepared first, as for compare.
    Evolution {
        #[command(flatten)]
        query: Query,
        /// Earlier captured view for lineage and architecture changes.
        #[arg(long)]
        base: Option<String>,
        /// Inclusive lower bound: Unix seconds, YYYY-MM-DD or RFC 3339.
        #[arg(long)]
        since: Option<String>,
        /// Exclusive upper bound, in the same forms.
        #[arg(long)]
        until: Option<String>,
        /// Shared commits a pair needs to be reported as coupled.
        #[arg(long, default_value_t = evolution::DEFAULT_MIN_SHARED)]
        min_shared: usize,
    },
    /// Run an explicitly selected local provider on a bounded lineage packet.
    ClassifyHistoryLineage {
        #[arg(long)]
        base: String,
        #[command(flatten)]
        query: Query,
        #[arg(long)]
        provider: PathBuf,
        #[arg(long, default_value_t = 65536)]
        max_bytes: usize,
    },
    Policy(Query),
    Unresolved(Query),
    /// Read a retained report section without recomputing it.
    Page {
        report_id: String,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        section: Option<String>,
        #[arg(long, default_value_t=0)]
        offset: u64,
        #[arg(long, default_value_t=100)]
        limit: u64,
    },
    /// Inspect one retained report reference, relationship, or explanation handle.
    Explain {
        #[arg(long)]
        report_id: Option<String>,
        reference: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 0)]
        source_offset: usize,
    },
    /// Agent session publications (newest first); `import` retains a transcript.
    Sessions {
        #[command(subcommand)]
        action: Option<SessionsAction>,
    },
    /// One session: metadata, exact user requests, files, commands and coverage gaps.
    Session(SessionQuery),
    /// Every normalized session event in transcript order.
    SessionEvents {
        #[command(flatten)]
        query: SessionQuery,
        /// Only these kinds (user_message, assistant_message, tool_call,
        /// tool_result, file_read, file_edit_attempt, file_edit_confirmed,
        /// command_started, command_finished, unknown).
        #[arg(long, value_delimiter = ',')]
        kind: Vec<String>,
    },
    /// Paths the session read or edited with supported tools, with outcomes.
    SessionFiles(SessionQuery),
    /// Commands the session issued, with recorded outcomes and exit codes.
    SessionCommands(SessionQuery),
    /// Recorded edits against the difference between two pinned source views.
    SessionChanges {
        #[command(flatten)]
        query: SessionQuery,
        /// Source view after the session.
        #[arg(long, default_value = "latest")]
        view: String,
        /// Source view before the session.
        #[arg(long)]
        base: Option<String>,
    },
    /// Cross-session activity per repository path in a time window: sessions
    /// reading, editing, only attempting edits or failing, beside net change
    /// between two source views and retained commits. Never attributed.
    SessionActivity {
        /// Path or path prefix.
        #[arg(long)]
        component: Option<String>,
        /// Inclusive lower bound on the session's recorded start.
        #[arg(long)]
        since: Option<String>,
        /// Exclusive upper bound on the session's recorded start.
        #[arg(long)]
        until: Option<String>,
        /// Source view after the window (module rollup and net change).
        #[arg(long)]
        view: Option<String>,
        /// Source view before the window (net change).
        #[arg(long)]
        base: Option<String>,
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        #[arg(long)]
        allow_partial: bool,
        #[arg(long)]
        historical: bool,
    },
    /// Establish the database, detected integrations, monitor, and local state.
    Init {
        #[arg(long, default_value = "detected")]
        agents: String,
        #[arg(long)]
        dry_run: bool,
        /// Back up and repair outdated owned integration entries.
        #[arg(long)]
        repair: bool,
        #[arg(long, default_value_t = 120, value_parser=clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
    /// Manage the shared repository observer.
    Monitor {
        #[command(subcommand)]
        action: MonitorAction,
    },
    /// Explicit provider-backed Claim inference, separate from local reconciliation.
    Claims {
        #[command(subcommand)]
        action: ClaimsAction,
    },
    /// Database selection, readiness and durable run/publication diagnostics.
    Status,
    /// Queue durable local reconciliation from an explicit host action.
    SyncRequest {
        #[arg(long, default_value = "manual")]
        source: String,
        #[arg(long, default_value = "explicit")]
        reason: String,
        #[arg(long, default_value_t = 120, value_parser=clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
        /// Hook-compatible output: no stdout or conversational decisions.
        #[arg(long)]
        quiet: bool,
        /// Read the harness JSON event on stdin and resolve its cwd as the repository.
        #[arg(long)]
        hook: bool,
        /// Wait for this revision's outcome; the durable worker is independent.
        #[arg(long)]
        wait: bool,
    },
    #[command(hide = true)]
    SyncWorker,
    /// Observe filesystem changes and reconcile dirty local domains, with a fallback poll.
    Watch {
        #[arg(long, default_value_t = 300, value_parser=clap::value_parser!(u64).range(1..=3600))]
        interval: u64,
        #[arg(long, default_value_t = 120, value_parser=clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
        /// Exit after this many polls (primarily for scripts); default: until interrupted.
        #[arg(long, value_parser=clap::value_parser!(u64).range(1..))]
        max_polls: Option<u64>,
    },
    /// Reconcile deterministic project, Git, and session state. Never invokes Claim providers.
    #[command(visible_alias = "ensure")]
    Sync {
        /// Report alignment without changing the database or starting a worker.
        #[arg(long)]
        check: bool,
        /// Reconcile Git history only (can be combined with other domain flags).
        #[arg(long)]
        git: bool,
        /// Reconcile sessions only, optionally selecting one harness; repeat to combine.
        #[arg(long, num_args=0..=1, default_missing_value="all", value_parser=session_selectors())]
        sessions: Vec<String>,
        /// Budget for the whole operation in seconds.
        #[arg(long, default_value_t = 120, value_parser=clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
        /// Seconds to wait for another writer before reporting `locked`.
        #[arg(long, default_value_t = 0)]
        lock_wait: u64,
        #[arg(long, value_enum)]
        language: Option<Language>,
        #[command(flatten)]
        python: Python,
        #[command(flatten)]
        go: Go,
        #[command(flatten)]
        java: Java,
        #[command(flatten)]
        csharp: CSharp,
        #[command(flatten)]
        cc: CFamily,
        /// Reconcile project inputs only; optionally select the TS/JS config path.
        #[arg(long, num_args=0..=1)]
        project: Option<Option<String>>,
        #[arg(long)]
        policy: Option<PathBuf>,
        #[arg(long, conflicts_with = "no_calls")]
        calls: bool,
        /// Disable static call extraction for this capture/profile.
        #[arg(long)]
        no_calls: bool,
        /// Rust analyzer executable. Default: `rust-analyzer` on PATH if it answers, else syntax-only.
        #[arg(long)]
        analyzer: Option<PathBuf>,
        #[arg(long, default_value = "node")]
        node: PathBuf,
        #[arg(long)]
        typescript_compiler: Option<PathBuf>,
        /// Exit nonzero unless the result is ready (`refreshed`, `up-to-date` or `rebuilt`).
        #[arg(long)]
        require_ready: bool,
    },
    /// Run one read operation from the shared catalog with JSON arguments.
    Call {
        operation: String,
        #[arg(long, default_value = "{}")]
        args: String,
        /// Wait up to this many seconds while a writer (such as a sync) is active.
        #[arg(long, default_value_t = 0, value_parser=clap::value_parser!(u64).range(0..=120))]
        wait_ready: u64,
        /// On pending source capture only, retry against the last published pin.
        #[arg(long)]
        fallback_published: bool,
    },
    /// Discover read operations or inspect one operation's full schema.
    Operations {
        #[arg(conflicts_with_all = ["search", "names", "limit"])]
        operation: Option<String>,
        /// Rank compact operation summaries by keywords.
        #[arg(long)]
        search: Option<String>,
        /// Return only operation names.
        #[arg(long)]
        names: bool,
        #[arg(long, value_parser=clap::value_parser!(u64).range(1..=100))]
        limit: Option<u64>,
    },
    /// Ranked search over retained text, history, and optional recorded sessions.
    #[command(aliases = ["recall", "retrieve"])]
    Search {
        text: String,
        #[command(flatten)]
        query: Query,
        #[arg(long, default_value_t = 24576)]
        max_bytes: usize,
        /// Search only the source capture, not retained Git history.
        #[arg(long)]
        no_history: bool,
        /// Also search imported agent sessions' user requests (labelled hits).
        #[arg(long)]
        sessions: bool,
    },
    /// Install the native agent integration.
    Install {
        #[arg(long, value_parser = ["pi", "muse", "codex", "claude-code"])]
        agent: String,
        /// Pi agent directory. Default: PI_CODING_AGENT_DIR, then ~/.pi/agent.
        #[arg(long)]
        agent_dir: Option<PathBuf>,
        /// Pi: install into <PROJECT>/.pi/extensions instead. Other agents:
        /// project root for their lifecycle hooks (Muse also configures MCP)
        /// (default: the repository root).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Show the planned changes without writing.
        #[arg(long)]
        dry_run: bool,
        /// Back up and replace locally changed owned extension/configuration entries.
        #[arg(long)]
        force: bool,
    },
    Recover,
    Rebuild {
        #[arg(long, default_value = "latest")]
        view: String,
    },
    Retract {
        assertion: String,
        #[arg(long)]
        reason: String,
    },
    /// Serve read-only architecture tools over MCP stdio.
    Mcp {
        /// Tool surface: `gateway` (briefcase_search, briefcase_query,
        /// briefcase_describe, briefcase_status) or `compact` (the Pi pair
        /// context and briefcase, for Muse).
        #[arg(long, value_enum, default_value = "gateway")]
        surface: McpSurface,
    },
}

#[derive(Subcommand)]
pub(crate) enum MonitorAction {
    /// Start one detached monitor, or return immediately when it is already running.
    Ensure {
        #[arg(long)]
        hook: bool,
        #[arg(long)]
        quiet: bool,
        #[arg(long, default_value_t=120, value_parser=clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
}
#[derive(Subcommand)]
pub(crate) enum ClaimsAction {
    /// May invoke the explicitly selected model/provider and retain validated Claims.
    Sync {
        #[command(subcommand)]
        kind: ClaimsKind,
    },
}
#[derive(Subcommand)]
pub(crate) enum ClaimsKind {
    Domains {
        #[command(flatten)]
        query: Query,
        #[arg(long)]
        provider: PathBuf,
        #[arg(long)]
        taxonomy: PathBuf,
        #[arg(long, default_value_t = 16)]
        max_assertions: usize,
        #[arg(long, default_value_t = 16384)]
        max_bytes: usize,
    },
    Lineage {
        #[arg(long)]
        base: String,
        #[command(flatten)]
        query: Query,
        #[arg(long)]
        provider: PathBuf,
        #[arg(long, default_value_t = 65536)]
        max_bytes: usize,
    },
}
