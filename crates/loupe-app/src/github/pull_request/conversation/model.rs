//! Pure helpers for the conversation tab.

use std::ops::Range;

use chrono::{DateTime, Utc};
use loupe_github::{
    IssueComment, PullRequestDetail, Review, ReviewState, ReviewThread, Side, TimelineItem,
};

use crate::store::CommentOrder;

/// One item of the conversation column.
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    Comment(&'a IssueComment),
    Review(&'a Review),
    Thread(&'a ReviewThread),
}

impl Entry<'_> {
    fn at(&self) -> Option<DateTime<Utc>> {
        match self {
            Entry::Comment(comment) => Some(comment.created_at),
            Entry::Review(review) => review.submitted_at,
            Entry::Thread(thread) => thread.comments.first().map(|comment| comment.created_at),
        }
    }
}

/// The timeline interleaved with review threads by time, oldest first.
/// A `Commented` review with no body is left out: its inline comments show
/// as threads.
pub fn entries(detail: &PullRequestDetail) -> Vec<Entry<'_>> {
    let mut all: Vec<Entry<'_>> = detail
        .timeline
        .iter()
        .filter_map(|item| match item {
            TimelineItem::Comment(comment) => Some(Entry::Comment(comment)),
            TimelineItem::Review(review)
                if review.state == ReviewState::Commented && review.body.trim().is_empty() =>
            {
                None
            }
            TimelineItem::Review(review) => Some(Entry::Review(review)),
        })
        .chain(detail.threads.iter().map(Entry::Thread))
        .collect();
    all.sort_by_key(|entry| entry.at());
    all
}

/// Which entries of the conversation show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommentFilter {
    #[default]
    All,
    /// Review threads not resolved yet.
    Unresolved,
    /// What the signed-in user wrote.
    Mine,
}

impl CommentFilter {
    pub const ALL: [CommentFilter; 3] = [
        CommentFilter::All,
        CommentFilter::Unresolved,
        CommentFilter::Mine,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CommentFilter::All => "All",
            CommentFilter::Unresolved => "Unresolved",
            CommentFilter::Mine => "Mine",
        }
    }

    fn keeps(self, entry: &Entry<'_>, viewer_login: &str) -> bool {
        match (self, entry) {
            (CommentFilter::All, _) => true,
            (CommentFilter::Unresolved, Entry::Thread(thread)) => !thread.is_resolved,
            (CommentFilter::Unresolved, _) => false,
            (CommentFilter::Mine, Entry::Comment(comment)) => comment.viewer_did_author,
            (CommentFilter::Mine, Entry::Review(review)) => review
                .author
                .as_ref()
                .is_some_and(|author| author.login == viewer_login),
            (CommentFilter::Mine, Entry::Thread(thread)) => thread
                .comments
                .iter()
                .any(|comment| comment.viewer_did_author),
        }
    }
}

/// The [`entries`] `filter` keeps, in `order`.
pub fn visible_entries(
    detail: &PullRequestDetail,
    filter: CommentFilter,
    order: CommentOrder,
) -> Vec<Entry<'_>> {
    let mut visible: Vec<Entry<'_>> = entries(detail)
        .into_iter()
        .filter(|entry| filter.keeps(entry, &detail.viewer_login))
        .collect();
    if order == CommentOrder::NewestFirst {
        visible.reverse();
    }
    visible
}

/// A Markdown button of the new-comment box's toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Heading,
    Bold,
    Italic,
    Quote,
    Code,
    Link,
    BulletList,
    NumberedList,
    TaskList,
}

/// What a [`Format`] does to the text: replace `range` with `text`, then
/// select `selection` (byte offsets in the new text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatEdit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// Applies `format` to the `selection` (byte offsets) of `text`, as
/// GitHub's toolbar does: inline marks wrap the selection, block marks
/// prefix every selected line, and the selection ends on the text the user
/// wrote (or the link's URL) so typing continues there.
pub fn format_edit(text: &str, selection: Range<usize>, format: Format) -> FormatEdit {
    let selected = &text[selection.clone()];
    let wrap = |before: &str, after: &str| {
        let start = selection.start + before.len();
        FormatEdit {
            range: selection.clone(),
            text: format!("{before}{selected}{after}"),
            selection: start..start + selected.len(),
        }
    };
    let prefix_lines = |prefix: &dyn Fn(usize) -> String| {
        let start = text[..selection.start].rfind('\n').map_or(0, |at| at + 1);
        let end = text[selection.end..]
            .find('\n')
            .map_or(text.len(), |at| selection.end + at);
        let block: Vec<String> = text[start..end]
            .split('\n')
            .enumerate()
            .map(|(index, line)| format!("{}{line}", prefix(index)))
            .collect();
        let block = block.join("\n");
        let selection = if selection.is_empty() {
            let caret = selection.start + prefix(0).len();
            caret..caret
        } else {
            start..start + block.len()
        };
        FormatEdit {
            range: start..end,
            text: block,
            selection,
        }
    };
    match format {
        Format::Bold => wrap("**", "**"),
        Format::Italic => wrap("_", "_"),
        Format::Code if selected.contains('\n') => wrap("```\n", "\n```"),
        Format::Code => wrap("`", "`"),
        Format::Link => {
            let url = "url";
            let text = format!("[{selected}]({url})");
            let start = selection.start + selected.len() + 3;
            FormatEdit {
                range: selection.clone(),
                text,
                selection: start..start + url.len(),
            }
        }
        Format::Heading => prefix_lines(&|_| "### ".to_owned()),
        Format::Quote => prefix_lines(&|_| "> ".to_owned()),
        Format::BulletList => prefix_lines(&|_| "- ".to_owned()),
        Format::NumberedList => prefix_lines(&|index| format!("{}. ", index + 1)),
        Format::TaskList => prefix_lines(&|_| "- [ ] ".to_owned()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkLineKind {
    Context,
    Added,
    Removed,
}

/// One line of a review thread's code context, with its line numbers in the
/// base (`old`) and head (`new`) files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HunkLine {
    pub kind: HunkLineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// The starting base and head line numbers of an `@@ -a,b +c,d @@` header.
fn hunk_start(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace().skip(1);
    let number = |part: Option<&str>, sign: char| -> Option<u32> {
        part?.strip_prefix(sign)?.split(',').next()?.parse().ok()
    };
    Some((number(parts.next(), '-')?, number(parts.next(), '+')?))
}

/// The last `count` lines of a stored diff hunk (the commented line is
/// last), without the `@@` header, classified by their prefix and numbered
/// from the header. Without a readable header the numbers are `None`.
pub fn hunk_tail(hunk: &str, count: usize) -> Vec<HunkLine> {
    let mut old = None;
    let mut new = None;
    let mut lines = Vec::new();
    for line in hunk.lines() {
        if line.starts_with("@@") {
            if let Some((o, n)) = hunk_start(line) {
                (old, new) = (Some(o), Some(n));
            }
            continue;
        }
        let (kind, text) = match line.chars().next() {
            Some('+') => (HunkLineKind::Added, &line[1..]),
            Some('-') => (HunkLineKind::Removed, &line[1..]),
            Some(' ') => (HunkLineKind::Context, &line[1..]),
            _ => (HunkLineKind::Context, line),
        };
        let take = |counter: &mut Option<u32>| {
            let value = *counter;
            *counter = counter.map(|n| n + 1);
            value
        };
        let (old_number, new_number) = match kind {
            HunkLineKind::Added => (None, take(&mut new)),
            HunkLineKind::Removed => (take(&mut old), None),
            HunkLineKind::Context => (take(&mut old), take(&mut new)),
        };
        lines.push(HunkLine {
            kind,
            old: old_number,
            new: new_number,
            text: text.to_owned(),
        });
    }
    let start = lines.len().saturating_sub(count);
    lines.split_off(start)
}

/// `src/a.rs L12`, `src/a.rs L10–L14`, or for an outdated thread the
/// original line; `(left)` marks the base side.
pub fn thread_label(thread: &ReviewThread) -> String {
    let (line, start) = match thread.line {
        Some(line) => (Some(line), thread.start_line),
        None => (thread.original_line, thread.original_start_line),
    };
    let range = match (start, line) {
        (Some(start), Some(end)) if start != end => format!("L{start}–L{end}"),
        (_, Some(end)) => format!("L{end}"),
        (_, None) => String::new(),
    };
    let side = if thread.side == Side::Left {
        " (left)"
    } else {
        ""
    };
    let outdated = if thread.line.is_none() {
        " · outdated"
    } else {
        ""
    };
    format!("{} {range}{side}{outdated}", thread.path)
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use loupe_github::{
        Checks, IssueComment, MergeStateStatus, Mergeable, PrRef, PrState, PullRequestDetail,
        RepoRef, Review, ReviewComment, ReviewState, ReviewThread, Side, TimelineItem,
    };

    use super::{
        CommentFilter, Entry, Format, FormatEdit, HunkLine, HunkLineKind, entries, format_edit,
        hunk_tail, thread_label, visible_entries,
    };
    use crate::store::CommentOrder;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + seconds, 0).expect("valid timestamp")
    }

    fn comment(id: &str, seconds: i64) -> IssueComment {
        IssueComment {
            id: id.into(),
            author: None,
            author_is_bot: false,
            hidden: false,
            hidden_reason: None,
            body: "hi".into(),
            created_at: at(seconds),
            url: String::new(),
            viewer_can_update: false,
            viewer_did_author: false,
        }
    }

    fn review(id: &str, state: ReviewState, body: &str, seconds: i64) -> Review {
        Review {
            id: id.into(),
            author: None,
            author_is_bot: false,
            hidden: false,
            hidden_reason: None,
            state,
            body: body.into(),
            submitted_at: Some(at(seconds)),
            url: String::new(),
            comment_count: 0,
            viewer_can_update: false,
        }
    }

    fn thread(id: &str, line: Option<u32>, start: Option<u32>, seconds: i64) -> ReviewThread {
        ReviewThread {
            id: id.into(),
            path: "src/a.rs".into(),
            line,
            start_line: start,
            original_line: Some(7),
            original_start_line: None,
            side: Side::Right,
            start_side: None,
            is_outdated: line.is_none(),
            is_resolved: false,
            resolved_by: None,
            viewer_can_reply: true,
            viewer_can_resolve: true,
            viewer_can_unresolve: false,
            comments: vec![ReviewComment {
                id: format!("{id}-c"),
                author: None,
                body: "x".into(),
                created_at: at(seconds),
                url: String::new(),
                diff_hunk: String::new(),
                is_pending: false,
                viewer_can_update: false,
                viewer_did_author: false,
            }],
        }
    }

    fn ids(entries: &[Entry<'_>]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Comment(comment) => comment.id.clone(),
                Entry::Review(review) => review.id.clone(),
                Entry::Thread(thread) => thread.id.clone(),
            })
            .collect()
    }

    fn detail(timeline: Vec<TimelineItem>, threads: Vec<ReviewThread>) -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_1".into(),
            pr: PrRef::new(RepoRef::new("acme", "web"), 1),
            url: String::new(),
            title: String::new(),
            body: String::new(),
            state: PrState::Open,
            is_draft: false,
            mergeable: Mergeable::Unknown,
            merge_state: MergeStateStatus::Unknown,
            merge_methods: Vec::new(),
            base_ref: "main".into(),
            base_oid: "base123".into(),
            head_ref: "feature".into(),
            head_repo: None,
            head_oid: "abc".into(),
            author: None,
            created_at: at(0),
            updated_at: at(0),
            body_edits: Vec::new(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            review_decision: None,
            reviewers: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            commits: Vec::new(),
            checks: Checks {
                state: None,
                runs: Vec::new(),
            },
            timeline,
            threads,
            pending_review: None,
            viewer_login: "me".into(),
            viewer_can_update: true,
        }
    }

    #[test]
    fn interleaves_by_time_and_drops_empty_commented_reviews() {
        let detail = detail(
            vec![
                TimelineItem::Comment(comment("c1", 10)),
                TimelineItem::Review(review("r-empty", ReviewState::Commented, " ", 20)),
                TimelineItem::Review(review("r-ok", ReviewState::Approved, "", 40)),
                TimelineItem::Comment(comment("c2", 50)),
            ],
            vec![thread("t1", Some(3), None, 30)],
        );
        assert_eq!(ids(&entries(&detail)), ["c1", "t1", "r-ok", "c2"]);
    }

    /// Two comments, an approval and two threads, one resolved; the viewer
    /// (`me`) wrote `c2`, the approval and the reply in `t-open`.
    fn mixed() -> PullRequestDetail {
        let me = || {
            Some(loupe_github::Actor {
                login: "me".into(),
                avatar_url: String::new(),
            })
        };
        let mut mine = comment("c2", 50);
        mine.viewer_did_author = true;
        let mut approval = review("r-me", ReviewState::Approved, "", 40);
        approval.author = me();
        let mut open = thread("t-open", Some(3), None, 30);
        let mut reply = open.comments[0].clone();
        reply.id = "t-open-reply".into();
        reply.viewer_did_author = true;
        open.comments.push(reply);
        let mut resolved = thread("t-done", Some(5), None, 20);
        resolved.is_resolved = true;
        detail(
            vec![
                TimelineItem::Comment(comment("c1", 10)),
                TimelineItem::Review(approval),
                TimelineItem::Comment(mine),
            ],
            vec![open, resolved],
        )
    }

    #[test]
    fn filters_unresolved_threads_and_what_the_viewer_wrote() {
        let detail = mixed();
        let shown = |filter| ids(&visible_entries(&detail, filter, CommentOrder::OldestFirst));
        assert_eq!(
            shown(CommentFilter::All),
            ["c1", "t-done", "t-open", "r-me", "c2"]
        );
        assert_eq!(shown(CommentFilter::Unresolved), ["t-open"]);
        assert_eq!(shown(CommentFilter::Mine), ["t-open", "r-me", "c2"]);
    }

    #[test]
    fn newest_first_reverses_the_filtered_entries() {
        let detail = mixed();
        assert_eq!(
            ids(&visible_entries(
                &detail,
                CommentFilter::All,
                CommentOrder::NewestFirst
            )),
            ["c2", "r-me", "t-open", "t-done", "c1"]
        );
        assert_eq!(
            ids(&visible_entries(
                &detail,
                CommentFilter::Mine,
                CommentOrder::NewestFirst
            )),
            ["c2", "r-me", "t-open"]
        );
    }

    /// The text after applying `format` to the part of `text` between the
    /// two `|` marks, with the new selection marked the same way.
    fn formatted(text: &str, format: Format) -> String {
        let start = text.find('|').expect("selection start");
        let end = text.rfind('|').expect("selection end") - 1;
        let plain = text.replace('|', "");
        let FormatEdit {
            range,
            text: replacement,
            selection,
        } = format_edit(&plain, start..end, format);
        let mut out = plain.clone();
        out.replace_range(range, &replacement);
        out.insert(selection.end, '|');
        out.insert(selection.start, '|');
        out
    }

    #[test]
    fn inline_formats_wrap_the_selection_and_keep_it_selected() {
        assert_eq!(formatted("say |hi| now", Format::Bold), "say **|hi|** now");
        assert_eq!(formatted("say |hi| now", Format::Italic), "say _|hi|_ now");
        assert_eq!(formatted("||", Format::Code), "`||`");
        assert_eq!(formatted("|a\nb|", Format::Code), "```\n|a\nb|\n```");
        assert_eq!(formatted("see |docs|", Format::Link), "see [docs](|url|)");
    }

    #[test]
    fn block_formats_prefix_every_selected_line() {
        assert_eq!(formatted("one\nt|wo|", Format::Quote), "one\n|> two|");
        assert_eq!(
            formatted("|a\nb|\nc", Format::NumberedList),
            "|1. a\n2. b|\nc"
        );
        assert_eq!(formatted("x\n||", Format::TaskList), "x\n- [ ] ||");
        assert_eq!(formatted("ti||tle", Format::Heading), "### ti||tle");
        assert_eq!(formatted("a||b", Format::BulletList), "- a||b");
    }

    #[test]
    fn hunk_tail_keeps_last_lines_and_classifies() {
        let hunk = "@@ -1,4 +1,4 @@\n keep\n-old\n+new\n tail";
        let line = |kind, old, new, text: &str| HunkLine {
            kind,
            old,
            new,
            text: text.to_owned(),
        };
        assert_eq!(
            hunk_tail(hunk, 3),
            vec![
                line(HunkLineKind::Removed, Some(2), None, "old"),
                line(HunkLineKind::Added, None, Some(2), "new"),
                line(HunkLineKind::Context, Some(3), Some(3), "tail"),
            ]
        );
        assert_eq!(hunk_tail(hunk, 10).len(), 4);
        assert!(hunk_tail("", 4).is_empty());
    }

    #[test]
    fn hunk_tail_numbers_from_the_header_and_survives_a_bad_one() {
        let hunk = "@@ -10,3 +20,4 @@ fn main()\n ctx\n+add";
        let numbers: Vec<_> = hunk_tail(hunk, 5)
            .into_iter()
            .map(|line| (line.old, line.new))
            .collect();
        assert_eq!(numbers, [(Some(10), Some(20)), (None, Some(21))]);
        let unnumbered = hunk_tail("@@ garbage @@\n ctx", 5);
        assert_eq!((unnumbered[0].old, unnumbered[0].new), (None, None));
    }

    #[test]
    fn labels_lines_ranges_sides_and_outdated() {
        assert_eq!(
            thread_label(&thread("a", Some(12), None, 0)),
            "src/a.rs L12"
        );
        assert_eq!(
            thread_label(&thread("a", Some(14), Some(10), 0)),
            "src/a.rs L10–L14"
        );
        let mut left = thread("a", Some(5), None, 0);
        left.side = Side::Left;
        assert_eq!(thread_label(&left), "src/a.rs L5 (left)");
        assert_eq!(
            thread_label(&thread("a", None, None, 0)),
            "src/a.rs L7 · outdated"
        );
    }
}
