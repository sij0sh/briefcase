//! Stage 3 source-backed regressions, registered as an explicit Cargo target.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use briefcase::architecture::{contracts::rows,source};
use serde_json::{json,Value};
use std::collections::BTreeSet;

#[test]
fn small_briefs_keep_the_retained_case_and_its_exact_completeness() {
    let f=Fixture::new();let args=json!({"task":"inspect entrypoint","target":"src/lib.rs","duty":"review","limit":1000,"max_bytes":1048576});
    let full=f.call("brief",args.clone());let mut args=args;args["limit"]=json!(2);args["max_bytes"]=json!(8000);
    let small=f.call("brief",args);
    assert!(serde_json::to_vec(&small).unwrap().len()+1<=8000);
    assert_eq!(small["case_id"],full["case_id"]);assert_eq!(small["outcome"],full["outcome"]);
    let proof=f.call("explain",json!({"report_id":small["case_id"],"ref":"/coverage/completeness","max_bytes":1048576}));
    assert_eq!(proof["entry"],full["coverage"]["completeness"]);
    assert!(proof["freshness"]["views"].is_array());
}

#[test]
fn a_shared_suite_does_not_fill_the_case_with_other_implementations() {
    let mut f=Fixture::new();std::fs::create_dir_all(f.root.join("tests")).unwrap();
    std::fs::write(f.root.join("tests/shared.rs"),"// src/lib.rs and src/unrelated.rs\n").unwrap();
    std::fs::write(f.root.join("src/unrelated.rs"),"pub fn unrelated() {}\n").unwrap();f.recapture();
    let packet=f.call("brief",json!({"task":"repair entrypoint","target":"src/lib.rs","duty":"extend","depth":2,
        "exclude_facets":["lexical"],"limit":1000,"max_bytes":1048576}));
    let paths:BTreeSet<_>=source::ARTIFACT_ROLES.iter().flat_map(|role|rows(&packet["slice"],role))
        .filter_map(|row|row["path"].as_str()).collect();
    assert!(paths.contains("tests/shared.rs"),"{packet}");
    assert!(!paths.contains("src/unrelated.rs"),"{packet}");
    assert_eq!(packet["coverage"]["completeness"]["verification"],"unknown");
}

#[test]
fn invalid_profile_preflight_reports_configuration_without_creating_a_database() {
    use briefcase::architecture::{reconcile::DirtySet,sync};
    use std::time::Duration;
    let f=Fixture::new();let runtime=f.root.join(".briefcase");std::fs::create_dir_all(&runtime).unwrap();
    let config=runtime.join("sync.json");
    let database=f.dir.path().join("uninitialized.db");
    let options=sync::SyncOptions::local(f.root.clone(),database.clone(),Duration::from_secs(10));
    for directory in [true,false] {
        if directory {std::fs::create_dir(&config).unwrap();}else{std::fs::write(&config,"{").unwrap();}
        let checked=sync::project::check(&options).unwrap();
        assert_eq!(checked.detail["sync"],"needs_configuration","{:?}",checked);
        let domains=DirtySet {project:true,git:false,sessions:BTreeSet::new()};
        let result=sync::sync_domains(&options,&domains).unwrap();
        assert_eq!(result["sync"],"needs_configuration","{result}");assert_eq!(result["ready"],false);
        assert!(result["message"].as_str().unwrap().contains(".briefcase/sync.json"));
        assert!(!database.exists(),"configuration preflight created a ledger");
        if directory {std::fs::remove_dir(&config).unwrap();}else{std::fs::remove_file(&config).unwrap();}
    }
}

#[test]
fn typescript_sync_names_both_compiler_knobs_instead_of_a_path_error() {
    use std::process::Command;
    let mut f=Fixture::new();std::fs::create_dir_all(f.root.join("src")).unwrap();
    std::fs::write(f.root.join("tsconfig.json"),r#"{"include":["src/**/*.ts"]}"#).unwrap();
    std::fs::write(f.root.join("src/a.ts"),"export const a = 1;\n").unwrap();f.recapture();
    let database=f.dir.path().join("compiler.db");
    let output=Command::new(env!("CARGO_BIN_EXE_briefcase")).current_dir(&f.root)
        .env_remove("BRIEFCASE_DB").env_remove("BRIEFCASE_SYNC").env_remove("BRIEFCASE_TYPESCRIPT_COMPILER")
        .args(["--db",database.to_str().unwrap(),"--project-root",f.root.to_str().unwrap(),"--json","sync","--language","typescript"])
        .output().unwrap();
    let text=String::from_utf8_lossy(&output.stdout).into_owned();
    let result:Value=serde_json::from_str(&text).unwrap_or_else(|e|panic!("{e}: {text} {}",String::from_utf8_lossy(&output.stderr)));
    assert_eq!(result["sync"],"needs_configuration","{result}");
    assert_eq!(result["ready"],false);
    let message=result["message"].as_str().unwrap();
    assert!(message.contains("--typescript-compiler"),"{message}");
    assert!(message.contains("BRIEFCASE_TYPESCRIPT_COMPILER"),"{message}");
    assert!(!message.contains("No such file"),"{message}");
}

#[cfg(unix)]
#[test]
fn recall_policy_switches_do_not_make_alignment_dirty_or_enqueue_the_watcher() {
    use std::{path::PathBuf,process::Command};
    let f=Fixture::new();
    std::fs::write(f.root.join("Cargo.toml"),"[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n").unwrap();
    std::fs::write(f.root.join("EVALUATION_RERUN.md"),"evaluation_only_needle\n").unwrap();
    let commands=f.dir.path().join("commands");std::fs::create_dir(&commands).unwrap();
    let git=std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path|path.join("git")).find(|path|path.is_file()).expect("Git required");
    std::os::unix::fs::symlink(git.canonicalize().unwrap(),commands.join("git")).unwrap();
    let invoke=|args:&[&str]|->Value {
        let output=Command::new(env!("CARGO_BIN_EXE_briefcase")).current_dir(&f.root)
            .env_remove("BRIEFCASE_DB").env_remove("BRIEFCASE_SYNC").env("PATH",&commands)
            .env("PI_CODING_AGENT_DIR",f.dir.path().join("pi")).env("CODEX_HOME",f.dir.path().join("codex"))
            .env("CLAUDE_CONFIG_DIR",f.dir.path().join("claude")).env("MUSE_HOME",f.dir.path().join("muse"))
            .args(["--db",f.db.to_str().unwrap(),"--project-root",f.root.to_str().unwrap(),"--json"]).args(args).output().unwrap();
        assert!(output.status.success(),"{args:?}: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let synced=invoke(&["sync","--language","rust","--timeout","20"]);assert_eq!(synced["ready"],true,"{synced}");
    invoke(&["call","search","--args",r#"{"text":"evaluation_only_needle","include_evaluation":true}"#]);
    let before=std::fs::read(&f.db).unwrap();
    let status_path=PathBuf::from(format!("{}.sync.json",f.db.display()));let status=std::fs::read(&status_path).unwrap();
    let checked=invoke(&["sync","--check","--language","rust"]);assert_eq!(checked["ready"],true,"{checked}");
    assert_eq!(std::fs::read(&f.db).unwrap(),before);assert_eq!(std::fs::read(status_path).unwrap(),status);
    assert_eq!(invoke(&["sync","--language","rust","--timeout","20"])["sync"],"up-to-date");
    let watched=invoke(&["watch","--max-polls","2","--interval","1","--timeout","10"]);
    assert_eq!(watched["watch"],"stopped");
    assert_eq!(briefcase::architecture::scheduler::status(&f.db)["requested_revision"].as_u64().unwrap_or(0),0);
}
