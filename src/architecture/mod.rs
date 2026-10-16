//! Canonical knowledge authority. Producers return validated batches; queries read pinned views.
pub mod analyzer;
pub mod capture;
pub mod clang;
pub mod contracts;
pub mod csharp;
pub mod data;
pub mod declarations;
pub mod deployment;
pub mod evolution;
pub mod execution;
pub(crate) mod freshness;
pub mod git_history;
pub mod go;
pub mod history;
pub mod hooks;
pub mod identity;
pub mod inference;
pub mod integrations;
pub mod java;
pub(crate) mod knowledge;
pub mod ledger;
pub mod lineage;
pub mod mcp;
pub mod muse;
pub(crate) mod native_host;
pub(crate) mod native_source;
pub mod passes;
pub mod pi;
pub mod python;
pub mod query;
pub mod search;
/// Compatibility spelling for the shared ranked search implementation.
pub use search as recall;
pub(crate) mod evidence_index;
pub mod reconcile;
pub mod response;
pub(crate) mod retained_source;
pub mod retrieval;
pub mod runtime_observations;
pub mod rust;
pub mod scheduler;
pub mod service;
pub mod session;
pub mod source;
pub mod sync;
pub mod task;
pub mod typescript;
pub mod watch;
pub mod workspace;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub fn require(ok: bool, code: &str, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(format!("{code}: {message}").into())
    }
}

pub mod reports;
pub mod external;
pub mod adoption;
