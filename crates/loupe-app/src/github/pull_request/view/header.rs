//! The header (the title, with the repository, number, and links on its
//! line) and the meta line (state, author, branches, size, checks, and the
//! pull request actions).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_github::PullRequestDetail;

use super::PullRequestView;
use super::model::{MergeAction, available_actions, head_label};
use crate::github::pull_request::checks::ChecksSummary;
use crate::github::widgets;
use crate::theme::scale::{self, dp};
use crate::ui::{self, ControlSize as _, WindowDragArea as _};

/// How long Copy link says "Copied" after a click.
const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1500);

/// Characters of a branch name the metadata line shows.
const BRANCH_CHARS: usize = 40;

/// `branch`, or "…" and its last [`BRANCH_CHARS`] characters when it is
/// longer: `feature/ENG-290-trusted-proxy-hops` ends in what it is for.
fn branch_label(branch: &str) -> String {
    let count = branch.chars().count();
    if count <= BRANCH_CHARS {
        return branch.to_owned();
    }
    let tail: String = branch.chars().skip(count - BRANCH_CHARS + 1).collect();
    format!("…{tail}")
}

/// The least of a long head branch the meta line shows beside the actions,
/// in px: about twelve characters and the ellipsis.
const HEAD_BRANCH_MIN: f32 = 96.;

/// The widths of the meta line's texts, in px at 100% zoom.
struct MetaWidths {
    state: f32,
    login: f32,
    base: f32,
    head: f32,
    additions: f32,
    deletions: f32,
    checks: f32,
}

/// The width of `text` in the meta line's 12 px.
fn text_width(text: &str, family: &SharedString, cx: &App) -> f32 {
    ui::text_width(text, 12., family, cx)
}

/// The width, in px at 100% zoom, the meta line asks for beside the actions:
/// everything whole except the head branch, which may be cut down to
/// [`HEAD_BRANCH_MIN`]. When the header is narrower than this and the
/// actions together, the actions go to a line of their own instead of the
/// branch shrinking to nothing.
///
/// It only chooses where the band breaks. That nothing is drawn over
/// anything else is the layout's own doing.
fn meta_basis(text: &MetaWidths) -> f32 {
    // A chip's side padding and border; the 14 px icon and 4 px gap of the
    // state and the checks; the avatar and its gap; a branch chip's padding.
    let tag = |text: f32| 18. + 14. + 4. + text;
    let author = (16. + 6. + text.login).min(160.);
    let chip = |text: f32| 12. + text;
    let head = chip(text.head).min(HEAD_BRANCH_MIN);
    // The base, the arrow, the head, and the copy button, 4 px apart.
    let branches = chip(text.base) + 4. + 12. + 4. + head + 4. + 20.;
    let stat = text.additions + 4. + text.deletions;
    // Five parts, 8 px apart.
    tag(text.state) + author + branches + stat + tag(text.checks) + 4. * 8.
}

impl PullRequestView {
    /// The page header of a loaded pull request.
    pub(super) fn render_header(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let url = detail.url.clone();
        let can_edit = detail.viewer_can_update && self.title_edit.is_none();
        let muted = cx.theme().muted_foreground;
        let reference = format!("{} #{}", detail.pr.repo, detail.pr.number);
        let pr_actions = available_actions(detail);
        // Review is the page's filled action on an open pull request until it
        // can be merged; once merged or closed it steps back to outlined.
        let review_is_primary = detail.state == loupe_github::PrState::Open
            && !matches!(pr_actions.merge, MergeAction::Available { .. });
        // The actions with a label are in sight; the ones that were icons
        // alone (edit the title, the repository's list, refresh) are items
        // of the "More actions" menu, with the state changes.
        let actions = h_flex()
            .flex_none()
            .gap_2()
            .items_center()
            .child(self.render_copy_link(detail, cx))
            .child(
                Button::new("pr-open-in-browser")
                    .ghost()
                    .control()
                    .icon(IconName::ExternalLink)
                    .label("Open on GitHub")
                    .tooltip("Open this pull request in the browser")
                    .accessibility_label("Open in browser")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .child(self.render_summarize_button(detail, cx))
            .child(self.render_review_controls(detail, review_is_primary, cx))
            .child(self.render_action_controls(&pr_actions, detail, can_edit, cx));
        let meta = self.render_meta(detail, cx);
        // The title has a line of its own, where it can wrap, with the
        // repository and number after it. Under it, one 32 px band: what the
        // pull request is at the leading edge, its actions at the trailing
        // edge. In a window too narrow for both, the actions go to a line
        // of their own under the meta line.
        v_flex()
            .id("page-header")
            .flex_none()
            .w_full()
            .px(scale::content_inset())
            .pt_3()
            .pb_1()
            .gap_2()
            .window_drag_area()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_baseline()
                    .gap_x_2()
                    .child(
                        div()
                            .id("pr-title")
                            .role(Role::Heading)
                            .aria_label(detail.title.clone())
                            .min_w_0()
                            .text_size(scale::heading())
                            .line_height(rems(1.625))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(detail.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(scale::body())
                            .text_color(muted)
                            .child(reference),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_h_8()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_2()
                    .items_center()
                    .child(meta)
                    .child(actions),
            )
    }

    /// Summarize: opens the Guide tab and generates a guided review with local
    /// Claude Code (or shows the cached guide when already up to date).
    fn render_summarize_button(&self, detail: &PullRequestDetail, cx: &mut Context<Self>) -> Div {
        let busy = crate::github::guide_service::is_generating(cx, &detail.pr);
        let fresh = crate::github::guide_service::has_fresh_guide(cx, &detail.pr, &detail.head_oid);
        let tooltip = if busy {
            "Generating guided review with Claude Code"
        } else if fresh {
            "Open the guided review for this commit"
        } else {
            "Generate a guided review using local Claude Code"
        };
        div()
            .flex_none()
            .debug_selector(|| "pr-summarize".into())
            .child(
                Button::new("pr-summarize")
                    .outline()
                    .control()
                    .icon(IconName::Sparkles)
                    .label(if busy { "Summarizing…" } else { "Summarize" })
                    .tooltip(tooltip)
                    .loading(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.summarize_pull_request(cx))),
            )
    }

    /// Copy link, which copies the pull request's address on GitHub and
    /// says "Copied" for [`COPIED_FOR`]. It comes before Open on GitHub, so
    /// the shorter label moves nothing else on the line.
    fn render_copy_link(&self, detail: &PullRequestDetail, cx: &mut Context<Self>) -> Div {
        let url = detail.url.clone();
        let (icon, label) = match self.link_copied {
            Some(_) => (IconName::Check, "Copied"),
            None => (IconName::Link, "Copy link"),
        };
        div()
            .flex_none()
            .debug_selector(move || format!("pr-copy-link:{label}"))
            .child(
                Button::new("pr-copy-link")
                    .ghost()
                    .control()
                    .icon(icon)
                    .label(label)
                    .tooltip("Copy the link to this pull request on GitHub")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        // A new task drops the one before it: a second click
                        // starts the time over.
                        this.link_copied = Some(cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(COPIED_FOR).await;
                            this.update(cx, |this, cx| {
                                this.link_copied = None;
                                cx.notify();
                            })
                            .ok();
                        }));
                        cx.notify();
                    })),
            )
    }

    /// The title editor under the header, while it is open.
    pub(super) fn render_title_edit(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let input = self.title_edit.as_ref()?;
        let busy = self.write.is_busy();
        Some(
            h_flex()
                .px(scale::content_inset())
                .py_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(div().flex_1().child(Input::new(input).disabled(busy)))
                .child(
                    Button::new("pr-title-cancel")
                        .ghost()
                        .small()
                        .label("Cancel")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.title_edit = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("pr-title-save")
                        .outline()
                        .small()
                        .label("Save title")
                        .loading(busy)
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.save_title(cx))),
                ),
        )
    }

    /// Who opened it, which branches, how big, whether checks pass: the
    /// leading part of the band under the title.
    fn render_meta(&self, detail: &PullRequestDetail, cx: &mut Context<Self>) -> Div {
        let head = head_label(detail);
        let theme = cx.theme();
        let (muted, foreground, mono) = (
            theme.muted_foreground,
            theme.foreground,
            theme.mono_font_family.clone(),
        );
        let summary = ChecksSummary::of(&detail.checks.runs);
        let (login, avatar_url) = detail.author.as_ref().map_or_else(
            || ("ghost".to_owned(), String::new()),
            |author| (author.login.clone(), author.avatar_url.clone()),
        );
        let branch = |text: String| ui::branch_chip(text, cx).max_w(dp(360.));
        let checks = match detail.checks.state {
            Some(state) => ui::status_chip(widgets::checks_color(state), cx)
                .text_color(foreground)
                .child(widgets::checks_icon(state))
                .child(summary.label())
                .into_any_element(),
            None => div()
                .text_size(scale::meta())
                .text_color(muted)
                .child(summary.label())
                .into_any_element(),
        };
        let ui_font = theme.font_family.clone();
        let basis = meta_basis(&MetaWidths {
            state: text_width(
                widgets::state_label(detail.state, detail.is_draft),
                &ui_font,
                cx,
            ),
            login: text_width(&login, &ui_font, cx),
            base: text_width(&detail.base_ref, &mono, cx),
            head: text_width(&branch_label(&head), &mono, cx),
            additions: text_width(&format!("+{}", detail.additions), &ui_font, cx),
            deletions: text_width(&format!("−{}", detail.deletions), &ui_font, cx),
            checks: text_width(&summary.label(), &ui_font, cx),
        });
        h_flex()
            // It takes what the actions leave, and asks for `basis`: with
            // less than that beside the actions, the band wraps them.
            .flex_grow_1()
            .flex_shrink_1()
            .flex_basis(dp(basis))
            .min_w_0()
            // The last guard, for a header narrower than the line itself:
            // what does not fit is cut at the trailing edge, never drawn
            // over the actions.
            .overflow_hidden()
            .gap_2()
            .items_center()
            .text_size(scale::meta())
            .text_color(muted)
            .child(
                div()
                    .flex_none()
                    .child(widgets::state_tag(detail.state, detail.is_draft)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .max_w(dp(160.))
                    .gap_1p5()
                    .items_center()
                    .child(div().flex_none().child(ui::avatar_named(
                        avatar_url,
                        login.clone(),
                        scale::avatar_xs(),
                    )))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(foreground)
                            .child(login),
                    ),
            )
            .child(
                h_flex()
                    // The one part of the line that gives way. The state,
                    // the author, the size, and the checks keep their
                    // width, so a branch that does not fit is cut and
                    // nothing is drawn over what follows it.
                    .flex_shrink_1()
                    .min_w_0()
                    .overflow_hidden()
                    .gap_1()
                    .items_center()
                    // The base is short and stays whole; the head gives way.
                    .child(branch(detail.base_ref.clone()).flex_none())
                    .child(
                        Icon::new(IconName::ArrowLeft)
                            .xsmall()
                            .flex_none()
                            .text_color(muted),
                    )
                    // A branch that does not fit ends in an ellipsis; one
                    // over forty characters is also cut at its head and
                    // keeps its end, which is what names the change. The
                    // tooltip has it whole.
                    .child(branch(branch_label(&head)).id("pr-head-branch").tooltip({
                        let head = head.clone();
                        move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(head.clone())
                                .build(window, cx)
                        }
                    }))
                    .child(
                        ui::icon_button("pr-copy-branch", IconName::Copy, "Copy branch name")
                            .xsmall()
                            .flex_none()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(head.clone()));
                            }),
                    ),
            )
            // The size and the checks keep their width whatever the branches
            // do.
            .child(
                div()
                    .flex_none()
                    .child(widgets::diff_stat(detail.additions, detail.deletions)),
            )
            .child(div().flex_none().child(checks))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Bounds, Modifiers, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };

    use super::super::{PullRequestView, model};
    use super::{COPIED_FOR, HEAD_BRANCH_MIN, MetaWidths, branch_label, meta_basis};
    use crate::github::common::Loadable;
    use crate::services::{Accounts, Services};
    use crate::store::Settings;

    fn widths(head: f32, checks: f32) -> MetaWidths {
        MetaWidths {
            state: 30.,
            login: 64.,
            base: 29.,
            head,
            additions: 24.,
            deletions: 20.,
            checks,
        }
    }

    #[test]
    fn the_meta_line_asks_for_everything_whole_but_a_long_head_branch() {
        let short = meta_basis(&widths(36., 60.));
        let long = meta_basis(&widths(288., 60.));
        let longer = meta_basis(&widths(400., 60.));
        // A short branch is asked for whole; a long one only down to its
        // minimum, however long it is.
        assert_eq!(long - short, HEAD_BRANCH_MIN - (12. + 36.));
        assert_eq!(long, longer);
        // What never shrinks is asked for whole.
        assert_eq!(meta_basis(&widths(36., 200.)) - short, 140.);
    }

    /// Copy link puts the pull request's address on GitHub on the
    /// clipboard and says "Copied" for a moment.
    #[gpui_kit::test]
    fn copy_link_copies_the_pull_requests_url(cx: &mut TestAppContext) {
        let detail = Arc::new(model::tests::detail());
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts::default());
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| PullRequestView::new(detail.pr.clone(), window, cx)),
            )
            .expect("open a window")
        });
        let cx = &mut VisualTestContext::from_window(handle, cx);
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.incoming = Some(detail.clone());
            view.detail = Loadable::Loaded(detail.clone());
            cx.notify();
        });
        let button = |cx: &mut VisualTestContext, label: &'static str| {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            cx.debug_bounds(format!("pr-copy-link:{label}").leak())
        };
        let at = button(cx, "Copy link").expect("Copy link is in the header");
        cx.simulate_click(at.center(), Modifiers::none());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
                .as_deref(),
            Some("https://github.com/acme/app/pull/12")
        );
        assert!(button(cx, "Copied").is_some(), "the button says it copied");
        cx.executor().advance_clock(COPIED_FOR);
        cx.run_until_parked();
        assert!(
            button(cx, "Copy link").is_some(),
            "and goes back to Copy link"
        );
    }

    #[test]
    fn a_long_branch_keeps_its_end() {
        assert_eq!(branch_label("main"), "main");
        let long = "feature/ENG-290-trusted-proxy-hops-and-signed-ingress-headers";
        let shown = branch_label(long);
        assert_eq!(shown.chars().count(), 40);
        assert!(shown.starts_with('…'));
        assert!(shown.ends_with("signed-ingress-headers"));
    }
}
