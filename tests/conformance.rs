//! Non-PR conformance lane: every integration test, including the
//! permutations gated out of the required PR suite with
//! `#[ignore = "conformance: ..."]`.
//!
//! Run with `cargo test --locked --test conformance -- --ignored` for the
//! gated tests, or without `--ignored` to also re-run the required tests.
//! This lane owns compiler-version, hostile-environment, genuine
//! process-timeout/kill, platform and large-input coverage.
#![allow(clippy::duplicate_mod, private_interfaces)]

#[path = "briefcase_task.rs"]
mod briefcase_task;

#[path = "briefcase_clang.rs"]
mod briefcase_clang;
#[path = "briefcase_contracts.rs"]
mod briefcase_contracts;
#[path = "briefcase_csharp.rs"]
mod briefcase_csharp;
#[path = "briefcase_data.rs"]
mod briefcase_data;
#[path = "briefcase_deployment.rs"]
mod briefcase_deployment;
#[path = "briefcase_evolution.rs"]
mod briefcase_evolution;
#[path = "briefcase_execution.rs"]
mod briefcase_execution;
#[path = "briefcase_go.rs"]
mod briefcase_go;
#[path = "briefcase_history.rs"]
mod briefcase_history;
#[path = "briefcase_integration.rs"]
mod briefcase_integration;
#[path = "briefcase_java.rs"]
mod briefcase_java;
#[path = "briefcase_python.rs"]
mod briefcase_python;
#[path = "briefcase_runtime.rs"]
mod briefcase_runtime;
#[path = "briefcase_secondary.rs"]
mod briefcase_secondary;
#[path = "briefcase_session.rs"]
mod briefcase_session;
#[path = "briefcase_substrate.rs"]
mod briefcase_substrate;
#[path = "briefcase_typescript.rs"]
mod briefcase_typescript;
#[path = "briefcase_workflow.rs"]
mod briefcase_workflow;
