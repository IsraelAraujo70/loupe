//! The contract between the pull request view and its tabs. The view owns the
//! loaded `PullRequestDetail`; tabs receive it through `set_detail` and ask the
//! view to reload by emitting `PrTabEvent::Reload` after a successful write.

use std::sync::Arc;

use loupe_github::{ActionsJob, PrRef, PullRequestDetail, ReviewEvent, Side};

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
    /// Switch to the Files tab and reveal `path` (and `line` on `side` when given).
    OpenFile {
        path: String,
        line: Option<u32>,
        side: Side,
    },
    /// Open another pull request (one of this one's stack) on its Files tab
    /// at `path` (and `line` on `side` when given).
    OpenFileIn {
        pr: PrRef,
        path: String,
        line: Option<u32>,
        side: Side,
    },
    /// Submit a review to `pr`: this pull request, or another of its stack
    /// whose Guide chapter is on screen.
    SubmitReview {
        pr: PrRef,
        event: ReviewEvent,
        body: String,
    },
}
