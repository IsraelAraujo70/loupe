//! The review progress: one meter for the whole pull request on every tab,
//! with the files viewed and the Guide's steps viewed ("12 / 28 files · 2 /
//! 4 steps"). A click on it, or N, shows the next file not viewed yet. It
//! also keeps the Guide's steps in step with the files' Viewed marks.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{PrRef, Side};

use super::PullRequestView;
use super::model::PrTab;
use crate::theme::scale;

gpui_kit::actions!(
    github,
    [
        /// N on a pull request's page: show the next file not viewed yet in
        /// the Files tab.
        NextUnviewedFile
    ]
);

/// Where N acts: a pull request's page, while no text field has the focus.
pub const NEXT_UNVIEWED_KEYS: &str = "PullRequestView && !Input";

/// What the meter reports: viewed and total files, and viewed and total
/// steps of the pull request's guide when it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReviewProgress {
    pub files: (usize, usize),
    pub steps: Option<(usize, usize)>,
}

impl ReviewProgress {
    /// Every file is viewed: there is no next file to show.
    fn files_done(self) -> bool {
        self.files.0 >= self.files.1
    }

    /// "12 / 28 files · 2 / 4 steps".
    fn label(self) -> String {
        let (viewed, total) = self.files;
        let files = format!("{viewed} / {total} files");
        match self.steps {
            Some((viewed, total)) => format!("{files} · {viewed} / {total} steps"),
            None => files,
        }
    }

    /// The meter's name for a screen reader, with what a click does.
    fn spoken(self) -> String {
        let (viewed, total) = self.files;
        let mut text = format!("Review progress: {viewed} of {total} files viewed");
        if let Some((viewed, total)) = self.steps {
            text.push_str(&format!(", {viewed} of {total} steps viewed"));
        }
        if !self.files_done() {
            text.push_str(". Next unviewed file");
        }
        text
    }

    /// The share of files viewed, from 0 to 100.
    fn percent(self) -> f32 {
        let (viewed, total) = self.files;
        if total == 0 {
            100.
        } else {
            viewed as f32 * 100. / total as f32
        }
    }
}

impl PullRequestView {
    /// What the meter reports, once the files have loaded.
    fn review_progress(&self, cx: &App) -> Option<ReviewProgress> {
        let loaded = self.loaded.as_ref()?;
        let counts = loaded.files.read(cx).viewed_progress()?;
        Some(ReviewProgress {
            files: (counts.viewed, counts.total),
            steps: loaded.guide.read(cx).step_progress(cx),
        })
    }

    /// After the Files tab changed: when a file's Viewed mark changed, the
    /// Guide's steps follow and the meter is drawn again.
    pub(super) fn files_changed(&mut self, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        let marks = loaded.files.read(cx).viewed_marks();
        if marks == self.viewed_marks {
            return;
        }
        self.viewed_marks = marks;
        self.sync_guide_with_files(cx);
        cx.notify();
    }

    /// Brings the Guide's steps in line with the files' Viewed marks.
    pub(super) fn sync_guide_with_files(&mut self, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        let Some(files) = loaded.files.read(cx).viewed_by_path() else {
            return;
        };
        loaded
            .guide
            .update(cx, |guide, cx| guide.sync_viewed_files(&files, cx));
    }

    /// A Guide step of `pr` was marked: its files of this pull request get
    /// the same mark, on screen and on GitHub.
    pub(super) fn step_viewed(
        &mut self,
        pr: &PrRef,
        paths: &[String],
        viewed: bool,
        cx: &mut Context<Self>,
    ) {
        if pr != &self.pr {
            return;
        }
        if let Some(loaded) = &self.loaded {
            loaded
                .files
                .update(cx, |files, cx| files.set_paths_viewed(paths, viewed, cx));
        }
    }

    /// Shows the next file not viewed yet: after the one on screen in the
    /// Files tab, or from it when coming from another tab.
    pub(super) fn next_unviewed_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        let on_files = self.tab == PrTab::Files && self.commit.is_none();
        let Some(path) = loaded.files.read(cx).next_unviewed_path(!on_files, cx) else {
            return;
        };
        self.reveal_file(&path, None, Side::Right, window, cx);
    }

    /// The meter at the trailing edge of the header's meta line, once the
    /// files have loaded.
    pub(super) fn render_review_progress(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let progress = self.review_progress(cx)?;
        let theme = cx.theme();
        let (muted, foreground, success) =
            (theme.muted_foreground, theme.foreground, theme.success);
        let done = progress.files_done();
        let mark = if done {
            Icon::new(IconName::CircleCheck)
                .small()
                .text_color(success)
                .into_any_element()
        } else {
            ProgressCircle::new("pr-review-progress-ring")
                .small()
                .value(progress.percent())
                .into_any_element()
        };
        let tooltip = if done {
            "Every file is viewed"
        } else {
            "Next unviewed file"
        };
        Some(
            Button::new("pr-review-progress")
                .ghost()
                .xsmall()
                .flex_none()
                .accessibility_label(progress.spoken())
                .tooltip_with_action(tooltip, &NextUnviewedFile, Some("PullRequestView"))
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .text_size(scale::meta())
                        .text_color(if done { muted } else { foreground })
                        .child(mark)
                        .child(progress.label()),
                )
                .when(!done, |button| {
                    button.on_click(
                        cx.listener(|this, _, window, cx| this.next_unviewed_file(window, cx)),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ReviewProgress;

    #[test]
    fn the_meter_counts_files_then_steps() {
        let progress = ReviewProgress {
            files: (12, 28),
            steps: Some((2, 4)),
        };
        assert_eq!(progress.label(), "12 / 28 files · 2 / 4 steps");
        assert_eq!(
            progress.spoken(),
            "Review progress: 12 of 28 files viewed, 2 of 4 steps viewed. Next unviewed file"
        );
        assert!(!progress.files_done());
        assert!((progress.percent() - 42.857).abs() < 0.01);
    }

    #[test]
    fn without_a_guide_the_meter_counts_files_alone() {
        let progress = ReviewProgress {
            files: (28, 28),
            steps: None,
        };
        assert_eq!(progress.label(), "28 / 28 files");
        assert_eq!(progress.spoken(), "Review progress: 28 of 28 files viewed");
        assert!(progress.files_done());
        assert_eq!(progress.percent(), 100.);
    }
}
