//! Required PR suite: fast, in-process integration tests.
//!
//! Only the tests named in `.agents/artifacts/ci-refactor.md` run by
//! default; every other test in the included files is marked
//! `#[ignore = "conformance: ..."]` and runs in the conformance lane
//! (`tests/conformance.rs`). Run with `cargo test --locked --test core`
//! using the default (parallel) test harness.
#![allow(clippy::duplicate_mod, private_interfaces)]

#[path = "briefcase_task.rs"]
mod briefcase_task;

#[path = "briefcase_contracts.rs"]
mod briefcase_contracts;
#[path = "briefcase_data.rs"]
mod briefcase_data;
#[path = "briefcase_deployment.rs"]
mod briefcase_deployment;
#[path = "briefcase_evolution.rs"]
mod briefcase_evolution;
#[path = "briefcase_integration.rs"]
mod briefcase_integration;
#[path = "briefcase_runtime.rs"]
mod briefcase_runtime;
#[path = "briefcase_session.rs"]
mod briefcase_session;
#[path = "briefcase_substrate.rs"]
mod briefcase_substrate;
