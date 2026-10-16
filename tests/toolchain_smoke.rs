//! Required PR suite: one real toolchain invocation per language.
//!
//! Each included file contributes its single capability-fixture test (two
//! for TypeScript/JavaScript, plus the Rust semantic workflow); every
//! other test is marked `#[ignore = "conformance: ..."]` and runs in the
//! conformance lane. Limit compiler concurrency with
//! `-- --test-threads=2`; never serialize the whole Rust suite.
#![allow(clippy::duplicate_mod, private_interfaces)]

#[path = "briefcase_clang.rs"]
mod briefcase_clang;
#[path = "briefcase_csharp.rs"]
mod briefcase_csharp;
#[path = "briefcase_go.rs"]
mod briefcase_go;
#[path = "briefcase_java.rs"]
mod briefcase_java;
#[path = "briefcase_python.rs"]
mod briefcase_python;
#[path = "briefcase_typescript.rs"]
mod briefcase_typescript;
#[path = "briefcase_workflow.rs"]
mod briefcase_workflow;
