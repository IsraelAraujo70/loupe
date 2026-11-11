//! The contract between the pull request view and its tabs. The view owns the
//! loaded `PullRequestDetail`; tabs receive it through `set_detail` and ask the
//! view to reload by emitting `PrTabEvent::Reload` after a successful write.

use std::sync::Arc;

use loupe_github::{ActionsJob, PrRef, PullRequestDetail};

/// What a tab is built from.
#[derive(Clone)]
pub struct PrTabContext {
    pub pr: PrRef,
    pub detail: Arc<PullRequestDetail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrTabEvent {
    /// A write succeeded; reload the pull request so every tab sees it.
    Reload,
    /// These failed jobs were re-run: reload, and keep asking GitHub until
    /// their replacements show up, even before any shows as pending.
    ChecksRequested(Vec<ActionsJob>),
}
