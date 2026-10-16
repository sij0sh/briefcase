use super::*;
pub(crate) fn canonical_writer(command: &Command) -> bool {
    matches!(
        command,
        Command::Scan { .. }
            | Command::CaptureText { .. }
            | Command::AnalyzeExecution { .. }
            | Command::AnalyzeData { .. }
            | Command::BindDeclarations(_)
            | Command::CaptureDeployment { .. }
            | Command::IngestRuntime { .. }
            | Command::Sessions {
                action: Some(SessionsAction::Import { dry_run: false, .. })
            }
            | Command::Sessions {
                action: Some(SessionsAction::ImportLocal { dry_run: false, .. })
            }
            | Command::Compare { .. }
            | Command::HistoryLineage { .. }
            | Command::Evolution { base: Some(_), .. }
            | Command::ClassifyHistoryLineage { .. }
            | Command::Passes { .. }
            | Command::Classify { .. }
            | Command::Recover
            | Command::Rebuild { .. }
            | Command::Retract { .. }
    )
}
pub(crate) fn output(json_output: bool, value: &Value) -> Result<()> {
    if json_output {
        let bounded = value["budget"]["max_response_bytes"].is_number();
        let encoded = if bounded {
            serde_json::to_string(value)?
        } else {
            serde_json::to_string_pretty(value)?
        };
        println!("{encoded}");
        Ok(())
    } else {
        render(value)
    }
}
/// The local history directory of a harness: explicit, or the harness
/// registry default.
pub(crate) fn local_sessions_dir(
    harness: session::Harness,
    explicit: Option<PathBuf>,
) -> Result<PathBuf> {
    match explicit {
        Some(dir) => Ok(dir),
        None => harness.default_dir(),
    }
}
pub(crate) fn session_options(
    path: &std::path::Path,
    profile: &str,
    root: &std::path::Path,
    label: Option<String>,
    repository_id: Option<String>,
    retention: &str,
) -> session::ImportOptions {
    let recorded = root.to_string_lossy();
    let root = if session::paths::normalize(&recorded).is_some() {
        root.to_path_buf()
    } else {
        std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf())
    };
    session::ImportOptions {
        profile: profile.to_owned(),
        repository_root: root.to_string_lossy().into_owned(),
        label,
        repository_id,
        retention: retention.parse().expect("clap validated retention"),
        source_name: path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("session.jsonl")
            .to_owned(),
    }
}
pub(crate) fn session_report(json_output: bool, report: Value) -> Result<()> {
    if json_output {
        return output(true, &report);
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
pub(crate) fn render(value: &Value) -> Result<()> {
    if let Some(response) = value.get("response") {
        let rows = response["results"].as_array().unwrap();
        println!(
            "{}: {} ({} results, ledger watermark {})",
            response["operation"].as_str().unwrap(),
            response["outcome"].as_str().unwrap(),
            rows.len(),
            response["ledger_watermark"]
        );
        println!("Scope: {}", response["context"]);
        for row in rows {
            let detail = &value["details"][row["id"].as_str().unwrap()];
            let from = detail["from"].as_str().unwrap_or("");
            let to = detail["to"].as_str().unwrap_or("");
            let label = detail["rule_id"]
                .as_str()
                .or_else(|| detail["change"].as_str())
                .or_else(|| detail["predicate"].as_str())
                .or_else(|| detail["reason"].as_str())
                .unwrap_or("finding");
            println!(
                "{}: {label} {from} -> {to}",
                row["conclusion"].as_str().unwrap()
            );
            println!(
                "  briefcase explain {}",
                row["explanation_handle"].as_str().unwrap()
            );
        }
        for limitation in response["limitations"].as_array().unwrap() {
            println!("Coverage: {}", limitation.as_str().unwrap());
        }
        if let Some(token) = response["continuation_token"].as_str() {
            println!("Continue: briefcase continue {token}");
        }
    } else if value["status"] == "published" {
        println!(
            "Published {} ({} assertions)",
            value["view_id"].as_str().unwrap(),
            value["assertion_count"]
        );
        println!("Scope: {}", value["context"]);
        println!(
            "Import resolution complete: {}",
            value["diagnostics"]["coverage"]["resolution_complete"]
        );
        for limitation in value["diagnostics"]["limitations"].as_array().unwrap() {
            println!("Coverage: {}", limitation.as_str().unwrap());
        }
    } else {
        let bounded = value["budget"]["max_response_bytes"].is_number();
        let encoded = if bounded {
            serde_json::to_string(value)?
        } else {
            serde_json::to_string_pretty(value)?
        };
        println!("{encoded}");
    }
    Ok(())
}
