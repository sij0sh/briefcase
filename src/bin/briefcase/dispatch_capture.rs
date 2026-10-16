use super::dispatch::{Ctx, Dispatch};
use super::*;
pub(crate) fn handle(command: Command, ledger: &mut Ledger, _ctx: &Ctx) -> Result<Dispatch> {
    Ok(Dispatch::Value(match command {
        Command::CaptureText {path,repository_id,label} => {
            let root=path.canonicalize()?;
            let repo=repository_id.unwrap_or(identity::id("repository",&json!({"root":root.to_string_lossy()}))?);
            let generation=ledger.begin(&repo,label.as_deref())?;
            let result=(|| {
                let capture=Capture::read(&root,None)?;
                let prepared=capture.text_publication(&repo,generation)?;
                let view=ledger.commit(generation,&prepared)?;
                ledger.publish(&view,true)?;
                Ok::<_,Box<dyn std::error::Error+Send+Sync>>(json!({"status":"published","view_id":view,"layer":"source_text","diagnostics":prepared.diagnostics}))
            })();
            match result {Ok(v)=>v,Err(error)=>{ledger.fail(generation,&error.to_string())?;return Ok(Dispatch::Done(Err(error)));}}
        },
        Command::Scan {
            path,
            language,
            typescript,
            python,
            go,
            java,
            csharp,
            cc,
            calls: _,
            no_calls,
            repository_id,
            label,
            policy,
            syntax_only,
            analyzer,
            timeout,
            features,
        } => {
            let calls=!no_calls;
            let rust_execution=matches!(language,Language::Rust)&&calls&&!syntax_only;
            let root = path.canonicalize()?;
            let repo = repository_id.unwrap_or(identity::id(
                "repository",
                &json!({"root":root.to_string_lossy()}),
            )?);
            let generation = ledger.begin(&repo, label.as_deref())?;
            let result = (|| {
                let capture = Capture::read(&root, policy.as_deref())?;
                let prepared = match language {
                    Language::Rust => {
                        rust::produce(
                            capture,
                            &repo,
                            generation,
                            &ScanOptions {
                                analyzer: if syntax_only { None } else { Some(analyzer.clone()) },
                                timeout: Duration::from_secs(timeout),
                                features:features.clone(),
                            },
                        )?
                    }
                    Language::C | Language::Cpp => {
                        briefcase::architecture::require(
                            features.is_empty(),
                            "ANALYZER",
                            "Cargo features do not apply to C/C++; use --cc-define",
                        )?;
                        let language = if matches!(language, Language::Cpp) {
                            clang::Language::Cpp
                        } else {
                            clang::Language::C
                        };
                        clang::produce(
                            capture,
                            &repo,
                            generation,
                            &cc.options(language, timeout, calls, syntax_only),
                        )?
                    }
                    Language::Csharp => {
                        briefcase::architecture::require(
                            features.is_empty(),
                            "ANALYZER",
                            "Cargo features do not apply to C#; use --csharp-define",
                        )?;
                        csharp::produce(
                            capture,
                            &repo,
                            generation,
                            &csharp.options(timeout, calls, syntax_only),
                        )?
                    }
                    Language::Go => {
                        briefcase::architecture::require(
                            features.is_empty(),
                            "ANALYZER",
                            "Cargo features do not apply to Go; use --go-tags",
                        )?;
                        go::produce(
                            capture,
                            &repo,
                            generation,
                            &go.options(timeout, calls, syntax_only),
                        )?
                    }
                    Language::Java => {
                        briefcase::architecture::require(
                            features.is_empty(),
                            "ANALYZER",
                            "Cargo features do not apply to Java",
                        )?;
                        java::produce(
                            capture,
                            &repo,
                            generation,
                            &java.options(timeout, calls, syntax_only),
                        )?
                    }
                    Language::Python => {
                        briefcase::architecture::require(
                            features.is_empty(),
                            "ANALYZER",
                            "Cargo features do not apply to Python",
                        )?;
                        python::produce(
                            capture,
                            &repo,
                            generation,
                            &python.options(timeout, calls, syntax_only),
                        )?
                    }
                    Language::Typescript => {
                        briefcase::architecture::require(!syntax_only && features.is_empty(), "ANALYZER", "TS/JS requires project-aware compiler resolution; Cargo features do not apply")?;
                        typescript::produce(
                            capture,
                            &repo,
                            generation,
                            &typescript.options(timeout, calls),
                        )?
                    }
                };
                let view = ledger.commit(generation, &prepared)?;
                let projections = ledger.publish(&view, true)?;
                let static_calls=if rust_execution {
                    Some(execution::analyze(ledger,&Options{view:view.clone(),..Default::default()},&ScanOptions{
                        analyzer:Some(analyzer.clone()),timeout:Duration::from_secs(timeout),features:features.clone()})
                        .unwrap_or_else(|error|json!({"state":"failed","reason":error.to_string()})))
                }else{None};
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                    json!({"view_id":view,"snapshot_id":prepared.bundle["snapshots"][0]["id"],"context":prepared.bundle["runs"][0]["context"],"projections":projections,"static_calls":static_calls,"diagnostics":prepared.diagnostics,"assertion_count":prepared.bundle["assertions"].as_array().unwrap().len(),"status":"published"}),
                )
            })();
            match result {
                Ok(v) => v,
                Err(e) => {
                    ledger.fail(generation, &e.to_string())?;
                    return Ok(Dispatch::Done(Err(e)));
                }
            }
        }
        Command::Dependencies(q) => q.call(&*ledger, "dependencies", json!({}))?,
        Command::AnalyzeExecution {
            query: q,
            language,
            typescript,
            python,
            go,
            java,
            csharp,
            cc,
            analyzer,
            timeout,
        } => match language {
            Language::Rust => execution::analyze(
                &mut *ledger,
                &q.into(),
                &ScanOptions {
                    analyzer: Some(analyzer),
                    timeout: Duration::from_secs(timeout),
                    features: vec![],
                },
            )?,
            Language::C | Language::Cpp => {
                let language = if matches!(language, Language::Cpp) {
                    clang::Language::Cpp
                } else {
                    clang::Language::C
                };
                clang::analyze(
                    &mut *ledger,
                    &q.into(),
                    &cc.options(language, timeout, true, false),
                )?
            }
            Language::Csharp => csharp::analyze(
                &mut *ledger,
                &q.into(),
                &csharp.options(timeout, true, false),
            )?,
            Language::Go => {
                go::analyze(&mut *ledger, &q.into(), &go.options(timeout, true, false))?
            }
            Language::Java => {
                java::analyze(&mut *ledger, &q.into(), &java.options(timeout, true, false))?
            }
            Language::Python => python::analyze(
                &mut *ledger,
                &q.into(),
                &python.options(timeout, true, false),
            )?,
            Language::Typescript => {
                typescript::analyze(&mut *ledger, &q.into(), &typescript.options(timeout, true))?
            }
        },
        Command::AnalyzeData {
            query: q,
            typescript,
            timeout,
        } => {
            let o = q.into();
            let mut out = data::analyze(&mut *ledger, &o, &typescript.options(timeout, false))?;
            // Endpoints exist now: retry declared bindings deferred at scan.
            out["declaration_binding"] = declarations::rebind(&mut *ledger, &o)?;
            out
        }
        Command::BindDeclarations(q) => declarations::rebind(&mut *ledger, &q.into())?,
        Command::CaptureDeployment {
            path,
            environment,
            source_view,
            label,
        } => deployment::capture(
            &mut *ledger,
            &path,
            &deployment::CaptureOptions {
                environment,
                source_view,
                label,
            },
        )?,
        Command::IngestRuntime {
            otlp,
            environment,
            interval_start,
            interval_end,
            collector,
            sampling,
            label,
        } => {
            let raw = std::fs::read(&otlp)?;
            runtime_observations::ingest(
                &mut *ledger,
                &raw,
                &runtime_observations::IngestOptions {
                    environment,
                    interval_start,
                    interval_end,
                    collector,
                    sampling,
                    label,
                    source_name: otlp
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("otlp.json")
                        .to_owned(),
                },
            )?
        }
        other => return Ok(Dispatch::Unhandled(other)),
    }))
}
