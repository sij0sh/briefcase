use super::dispatch::{dispatch, Ctx};
use super::helpers::*;
use super::*;

fn require_event(bytes: &[u8]) -> Result<Option<PathBuf>> {
    if bytes.len() > 65536 {
        return Err("HOOK: Event exceeds 64 KiB".into());
    }
    let event: Value = serde_json::from_slice(bytes)?;
    let cwd = event["cwd"].as_str().ok_or("HOOK: Event has no cwd")?;
    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        return Err("HOOK: cwd must be absolute".into());
    }
    Ok(Some(path))
}
pub(crate) fn run(mut cli: Cli) -> Result<()> {
    use super::command::{ClaimsAction, ClaimsKind, MonitorAction};
    use briefcase::architecture::{integrations, reconcile::DirtySet, scheduler};
    cli.command = match cli.command {
        Command::Claims {
            action: ClaimsAction::Sync { kind },
        } => match kind {
            ClaimsKind::Domains {
                query,
                provider,
                taxonomy,
                max_assertions,
                max_bytes,
            } => Command::Classify {
                query,
                provider,
                taxonomy,
                max_assertions,
                max_bytes,
            },
            ClaimsKind::Lineage {
                base,
                query,
                provider,
                max_bytes,
            } => Command::ClassifyHistoryLineage {
                base,
                query,
                provider,
                max_bytes,
            },
        },
        other => other,
    };
    let cwd = std::env::current_dir()?;
    let root = || workspace::repository_root(&cwd, cli.project_root.as_deref());
    let db = || {
        workspace::resolve_db(
            cli.db.as_deref(),
            std::env::var_os(workspace::DB_ENV),
            &cwd,
            cli.project_root.as_deref(),
        )
    };
    if let Command::ContextHook {phase}=&cli.command {
        use std::io::Read;
        let outcome=(||->Result<Value>{
            let mut bytes=Vec::new();std::io::stdin().take(65537).read_to_end(&mut bytes)?;
            briefcase::architecture::require(bytes.len()<=65536,"HOOK","Event exceeds 64 KiB")?;
            let input:Value=serde_json::from_slice(&bytes)?;
            let event_cwd=input["cwd"].as_str().ok_or("HOOK: cwd is required")?;
            let event_root=workspace::repository_root(std::path::Path::new(event_cwd),cli.project_root.as_deref())?;
            let event_db=workspace::resolve_db(cli.db.as_deref(),std::env::var_os(workspace::DB_ENV),&event_root,Some(&event_root))?;
            briefcase::architecture::adoption::event(&event_db,&event_root,phase,&input)
        })();
        // Lifecycle diagnostics never block a prompt or force a turn continuation.
        return output(true,&outcome.unwrap_or_else(|e|json!({"systemMessage":format!("Briefcase context unavailable: {e}")})));
    }
    // Commands that do not open the database here.
    match &cli.command {
        Command::Operations {
            operation,
            search,
            names,
            limit,
        } => {
            let value = if operation.is_none() && search.is_none() && !names && limit.is_none() {
                // Preserve the existing full-catalog JSON interface.
                service::describe()
            } else {
                service::discover(
                    &json!({"operation":operation,"query":search,"names":names,"limit":limit}),
                )?
            };
            if !cli.json && (*names || search.is_some() || limit.is_some()) {
                for item in value.as_array().into_iter().flatten() {
                    if let Some(name) = item.as_str() {
                        println!("{name}");
                    } else {
                        println!(
                            "{}\n  {}\n  args: {}",
                            item["name"].as_str().unwrap(),
                            item["description"].as_str().unwrap(),
                            strings(&item["arguments"]).join(", ")
                        );
                    }
                }
                return Ok(());
            }
            return output(true, &value);
        }
        Command::Sessions {
            action: Some(SessionsAction::Profiles),
        } => {
            let profiles = session::profile::BUILTIN
                .iter()
                .map(|(name, _)| {
                    let p = session::profile::load(name)?;
                    Ok(json!({"name":name,"harness":p.profile.harness,"version":p.profile.version,
                        "digest":p.digest,"description":p.profile.description,"guarantees":p.profile.guarantees}))
                })
                .collect::<Result<Vec<_>>>()?;
            return output(true, &json!(profiles));
        }
        Command::Sessions {
            action:
                Some(SessionsAction::Import {
                    path,
                    profile,
                    repository_root,
                    dry_run: true,
                    session_retention,
                    ..
                }),
        } => {
            let root = match repository_root {
                Some(r) => r.clone(),
                None => root()?,
            };
            let raw = std::fs::read(path)?;
            return output(
                true,
                &session::inspect(
                    &raw,
                    &session_options(path, profile, &root, None, None, session_retention),
                )?,
            );
        }
        Command::Sessions {
            action:
                Some(SessionsAction::ImportLocal {
                    harness,
                    sessions_dir,
                    repository_root,
                    session_retention,
                    dry_run: true,
                }),
        } => {
            let root = match repository_root {
                Some(r) => r.clone(),
                None => root()?,
            };
            let harness: session::Harness = harness.parse()?;
            let dir = local_sessions_dir(harness, sessions_dir.clone())?;
            let tmp = tempfile::tempdir()?;
            let mut scratch = Ledger::open(&tmp.path().join("inspect.db"))?;
            return output(
                true,
                &session::import_local(
                    &mut scratch,
                    harness,
                    &dir,
                    &root.to_string_lossy(),
                    session_retention.parse()?,
                    true,
                )?,
            );
        }
        Command::Init {
            agents,
            dry_run,
            repair,
            timeout,
        } => {
            let root = root()?;
            let db = db()?;
            let binary = std::env::current_exe()?.canonicalize()?;
            let selected = integrations::select(agents, &root)?;
            // Validate every requested integration before changing any of them.
            let plans = selected
                .iter()
                .map(|a| integrations::install(a, &root, &binary, true, *repair))
                .collect::<Result<Vec<_>>>()?;
            let blocked = plans.iter().any(|p| p["status"] == "blocked");
            let mut options =
                sync::SyncOptions::local(root.clone(), db.clone(), Duration::from_secs(*timeout));
            let mut report = json!({"init":if blocked {"blocked"} else if *dry_run {"planned"} else {"complete"},
                "database":db,"database_state":if db.is_file() {"existing"} else {"missing"},
                "dry_run":dry_run,"integrations":plans});
            sync::validate_database(&root, &db)?;
            if *dry_run || blocked {
                report["local"] = sync::check(
                    &options,
                    &briefcase::architecture::reconcile::configured_domains(&root)
                        .unwrap_or_else(|_| DirtySet::all()),
                )?;
            } else {
                // Database creation and every reconciliation share writer admission.
                {
                    let _lock = workspace::writer_lock(&db)?;
                    Ledger::open(&db)?;
                }
                report["integrations"] = json!(selected
                    .iter()
                    .map(|a| integrations::install(a, &root, &binary, false, *repair))
                    .collect::<Result<Vec<_>>>()?);
                report["monitor"] = super::scheduling::ensure_monitor(&root, &db, *timeout)?;
                options.lock_wait = Duration::from_secs(30);
                report["local"] = sync::sync(&options)?;
                if report["local"]["ready"] != true {
                    report["init"] = json!("incomplete");
                }
            }
            output(true, &report)?;
            if blocked {
                std::process::exit(2);
            }
            if !*dry_run && report["init"] == "incomplete" {
                std::process::exit(3);
            }
            return Ok(());
        }
        Command::Monitor {
            action:
                MonitorAction::Ensure {
                    hook,
                    quiet,
                    timeout,
                },
        } => {
            let event_cwd = if *hook {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::io::stdin().take(65537).read_to_end(&mut bytes)?;
                require_event(&bytes)?
            } else {
                None
            };
            let cwd = event_cwd.as_deref().unwrap_or(&cwd);
            let root = workspace::repository_root(cwd, cli.project_root.as_deref())?;
            let db = workspace::resolve_db(
                cli.db.as_deref(),
                std::env::var_os(workspace::DB_ENV),
                cwd,
                cli.project_root.as_deref(),
            )?;
            let report = super::scheduling::ensure_monitor(&root, &db, *timeout)?;
            return if *quiet {
                Ok(())
            } else {
                output(true, &report)
            };
        }
        Command::Install {
            agent,
            agent_dir,
            project,
            dry_run,
            force,
        } => {
            let binary = std::env::current_exe()?.canonicalize()?;
            let report = match agent.as_str() {
                "codex" | "claude-code" => briefcase::architecture::hooks::install(
                    agent,
                    &match project {
                        Some(p) => cwd.join(p).canonicalize()?,
                        None => root()?,
                    },
                    &binary,
                    *dry_run,
                    *force,
                )?,
                "muse" => briefcase::architecture::muse::install(
                    &briefcase::architecture::muse::InstallOptions {
                        project: Some(match project {
                            Some(project) => cwd.join(project),
                            None => root().unwrap_or_else(|_| cwd.clone()),
                        }),
                        binary,
                        dry_run: *dry_run,
                        force: *force,
                    },
                )?,
                _ => pi::install(&pi::InstallOptions {
                    agent_dir: agent_dir.clone(),
                    project: project.clone(),
                    binary,
                    dry_run: *dry_run,
                    force: *force,
                })?,
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report["status"] == "blocked" {
                std::process::exit(2);
            }
            return Ok(());
        }
        Command::SyncRequest {
            source,
            reason,
            timeout,
            quiet,
            hook,
            wait,
        } => {
            let event_cwd = if *hook {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::io::stdin().take(65537).read_to_end(&mut bytes)?;
                require_event(&bytes)?
            } else {
                None
            };
            let cwd = event_cwd.as_deref().unwrap_or(&cwd);
            let root = workspace::repository_root(cwd, cli.project_root.as_deref())?;
            let db = briefcase::architecture::scheduler::database(&workspace::resolve_db(
                cli.db.as_deref(),
                std::env::var_os(workspace::DB_ENV),
                cwd,
                cli.project_root.as_deref(),
            )?)?;
            let mut result = briefcase::architecture::scheduler::request(
                &root,
                &db,
                source,
                reason,
                *timeout,
                || super::scheduling::wake(&root, &db),
            )?;
            if *wait {
                if let Some(revision) = result["revision"].as_u64() {
                    result = briefcase::architecture::scheduler::wait(
                        &db,
                        revision,
                        Duration::from_secs(timeout + 30),
                    );
                }
            }
            return if *quiet {
                Ok(())
            } else {
                output(true, &result)
            };
        }
        Command::SyncWorker => {
            let root = root()?;
            let db = briefcase::architecture::scheduler::database(&db()?)?;
            return scheduler::work_domains(&root, &db, |timeout, domains| {
                super::scheduling::run_sync(&root, &db, timeout, domains)
            });
        }
        Command::Watch {
            interval,
            timeout,
            max_polls,
        } => {
            let root = root()?;
            let db = briefcase::architecture::scheduler::database(&db()?)?;
            let value = briefcase::architecture::watch::run(
                &root,
                &db,
                *interval,
                *timeout,
                *max_polls,
                || super::scheduling::wake(&root, &db),
            )?;
            return output(true, &value);
        }
        Command::Status => {
            let db = db()?;
            let mut value = if db.is_file() {
                Ledger::read_only(&db)?.status()?
            } else {
                json!({})
            };
            value["readiness"] = sync::readiness(&db);
            if let Ok(root) = root() {
                value["local"] = sync::check(
                    &sync::SyncOptions::local(root.clone(), db.clone(), Duration::from_secs(120)),
                    &briefcase::architecture::reconcile::configured_domains(&root)
                        .unwrap_or_else(|_| DirtySet::all()),
                )?;
                value["integrations"] = json!(integrations::status(
                    &root,
                    &std::env::current_exe()?.canonicalize()?
                )?);
            }
            value["claims"] = sync::claims_status(&db)?;
            return output(true, &value);
        }
        Command::Mcp { surface } => {
            let db = db()?;
            return mcp::serve_path_for_root(
                (*surface).into(),
                &db,
                root().ok().as_deref(),
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            );
        }
        Command::Sync {
            check,
            git,
            sessions,
            timeout,
            lock_wait,
            language,
            python,
            go,
            java,
            csharp,
            cc,
            project,
            policy,
            calls,
            no_calls,
            analyzer,
            node,
            typescript_compiler,
            require_ready,
        } => {
            let root = root()?;
            let mut domains = DirtySet::default();
            if project.is_none() && !*git && sessions.is_empty() {
                domains = briefcase::architecture::reconcile::configured_domains(&root)
                    .unwrap_or_else(|_| DirtySet::all());
            } else {
                domains.project = project.is_some();
                domains.git = *git;
                for h in sessions {
                    if h == "all" {
                        domains.sessions.extend(DirtySet::all().sessions);
                    } else {
                        domains.sessions.insert(h.parse()?);
                    }
                }
            }
            let options = sync::SyncOptions {
                db: db()?,
                root,
                timeout: Duration::from_secs(*timeout),
                lock_wait: Duration::from_secs(*lock_wait),
                language: language.map(|l| match l {
                    Language::Rust => sync::Language::Rust,
                    Language::C => sync::Language::C,
                    Language::Cpp => sync::Language::Cpp,
                    Language::Csharp => sync::Language::Csharp,
                    Language::Go => sync::Language::Go,
                    Language::Java => sync::Language::Java,
                    Language::Python => sync::Language::Python,
                    Language::Typescript => sync::Language::Typescript,
                }),
                project: project.clone().flatten(),
                policy: policy.as_ref().map(|p| cwd.join(p)),
                calls: if *no_calls {Some(false)}else{calls.then_some(true)},
                analyzer: analyzer.clone(),
                node: node.clone(),
                compiler: typescript_compiler.clone(),
                python: python.python.clone(),
                python_source_roots: python.python_source_root.clone(),
                go: go.go.clone(),
                go_source_roots: go.go_source_root.clone(),
                go_tags: go.go_tags.clone(),
                goos: go.goos.clone(),
                goarch: go.goarch.clone(),
                go_tests: go.go_tests,
                javac: java.javac.clone(),
                java_source_roots: java.java_source_root.clone(),
                java_release: java.java_release,
                dotnet: csharp.dotnet.clone(),
                csharp_source_roots: csharp.csharp_source_root.clone(),
                csharp_defines: csharp.csharp_define.clone(),
                csharp_language_version: csharp.csharp_language_version.clone(),
                clang: cc.clang.clone(),
                llvm_config: cc.llvm_config.clone(),
                cc_source_roots: cc.cc_source_root.clone(),
                cc_include_roots: cc.cc_include_root.clone(),
                cc_defines: cc.cc_define.clone(),
                cc_standard: cc.cc_standard.clone(),
                cc_target: cc.cc_target.clone(),
            };
            let result = if *check {
                sync::check(&options, &domains)?
            } else {
                sync::sync_domains(&options, &domains)?
            };
            output(true, &result)?;
            if result["sync"] == "failed" {
                std::process::exit(1);
            }
            if *require_ready && result["ready"] != true {
                std::process::exit(3);
            }
            return Ok(());
        }
        Command::Call {
            operation,
            args,
            wait_ready,
            fallback_published,
        } => {
            let arguments = identity::parse(args.as_bytes())?;
            if matches!(operation.as_str(), "describe" | "architecture_describe") {
                return output(true, &service::discover(&arguments)?);
            }
            let db = db()?;
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(*wait_ready)
                && workspace::writer_active(&db)
            {
                std::thread::sleep(Duration::from_millis(250));
            }
            let result=service::call_path(&db,root().ok().as_deref(),operation,&arguments,*fallback_published);
            return match result {
                Ok(v) => output(true, &v),
                Err(e) => {
                    let state = sync::readiness(&db)["state"].clone();
                    Err(format!(
                        "{e} (Briefcase readiness: {})",
                        state.as_str().unwrap_or("unknown")
                    )
                    .into())
                }
            };
        }
        _ => {}
    }
    if let Some((operation,arguments))=dispatch::context_request(&cli.command) {
        let result=service::call_path(&db()?,Some(&root()?),operation,&arguments,true)?;
        return output(cli.json,&result);
    }
    let db = db()?;
    let _lock = if canonical_writer(&cli.command) {
        Some(workspace::writer_lock(&db)?)
    } else {
        None
    };
    let mut ledger = if matches!(
        cli.command,
        Command::Scan { .. }
            | Command::CaptureText { .. }
            | Command::Sessions {
                action: Some(SessionsAction::Import { .. })
            }
            | Command::Sessions {
                action: Some(SessionsAction::ImportLocal { .. })
            }
    ) {
        Ledger::open(&db)?
    } else {
        Ledger::open_existing(&db)?
    };
    let ctx = Ctx {
        json: cli.json,
        root: &root,
    };
    match dispatch(cli.command, &mut ledger, &ctx)? {
        Some(value) => output(cli.json, &value),
        None => Ok(()),
    }
}
