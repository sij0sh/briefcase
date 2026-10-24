use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};
use briefcase::architecture::{
    capture::Capture, clang, csharp, data, declarations, deployment, evolution, execution, go,
    identity, inference, java, ledger::Ledger, lineage, mcp, passes, pi, python,
    query::{self, Options}, runtime_observations,
    rust::{self, ScanOptions}, service,
    session::{self, query as session_query}, sync, typescript, workspace, Result,
};
fn strings(v: &Value) -> Vec<String> {
    briefcase::architecture::contracts::strings(v)
}
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[path = "briefcase/args.rs"]
mod args;
#[path = "briefcase/command.rs"]
mod command;
#[path = "briefcase/dispatch.rs"]
mod dispatch;
#[path = "briefcase/dispatch_capture.rs"]
mod dispatch_capture;
#[path = "briefcase/dispatch_history.rs"]
mod dispatch_history;
#[path = "briefcase/dispatch_sessions.rs"]
mod dispatch_sessions;
#[path = "briefcase/helpers.rs"]
mod helpers;
#[path = "briefcase/run.rs"]
mod run;
#[path = "briefcase/scheduling.rs"]
mod scheduling;

use args::*;
use command::*;
use helpers::*;

fn main() {
    if let Err(e) = run::run(Cli::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
