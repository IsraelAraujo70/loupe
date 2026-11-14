//! GitHub API client for Loupe. Pure Rust, no GPUI: every call is blocking and
//! is meant to run on a background thread. Wire types stay private to this
//! crate; callers get the domain types exported here.

#![cfg_attr(test, recursion_limit = "256")]

mod actions;
mod client;
pub mod diff;
mod error;
mod gql;
mod ids;
mod merge;
mod rate;
mod rest;
pub mod reviews;
mod stack;
pub mod suggestion;
mod types;
mod workflow;
mod workflow_file;

pub use client::{GitHub, Viewer};
pub use error::{Error, Result};
pub use ids::{PrRef, RepoRef};
pub use merge::MergeOutcome;
pub use rate::{RateLimit, RateLimits};
pub use rest::{ChecksEtag, ChecksPoll, NOTIFICATIONS_LIMIT};
pub use stack::{PrStack, StackEntry, StackMember, StackSummary};
pub use types::*;
pub use workflow::{
    Annotation, AnnotationLevel, JobLog, JobStep, LogLine, LogLineKind, RUNS_PAGE, RunArtifact,
    WorkflowJob, WorkflowRun, split_log,
};
pub use workflow_file::{JobSpec, WorkflowSpec, assign_jobs, called_job_name, parse_workflow};

/// Every GraphQL document this crate sends, as `(name, source)`, for the
/// offline schema gate in `tests/schema.rs`.
#[doc(hidden)]
pub fn graphql_documents() -> Vec<(&'static str, &'static str)> {
    gql::documents()
}
