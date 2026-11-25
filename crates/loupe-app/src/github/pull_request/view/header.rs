//! The header: one line with the title, the repository and number, and the
//! pull request actions, and under it the meta line (state, author,
//! branches, size, checks). Neither wraps: the title is cut, and labeled
//! actions that would cut it too short move into "More actions".

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::PullRequestDetail;

use super::PullRequestView;
use super::model::{
    ReviewButton, available_actions, head_label, pending_comments_label, review_button,
};
use crate::github::guide_service;
use crate::github::pull_request::checks::ChecksSummary;
use crate::github::widgets;
use crate::theme::scale::{self, dp};
use crate::ui::{self, ControlSize as _, WindowDragArea as _};

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

/// The least of the title the header keeps in sight, in px at 100% zoom,
/// before it moves a labeled action into "More actions".
const TITLE_MIN: f32 = 400.;

/// Space between two actions, and between the title and the actions, in px.
const ACTION_GAP: f32 = 8.;
const TITLE_GAP: f32 = 12.;

/// The widths the header's line weighs, in px at 100% zoom.
struct LineWidths {
    /// The line's width on the last frame; zero before the first.
    room: f32,
    /// The title, whole.
    title: f32,
    /// The repository and number after the title, with the gap before them.
    reference: f32,
    open_on_github: f32,
    summarize: f32,
    /// The actions that never move: the review button and "More actions".
    fixed: f32,
}

/// The labeled actions that moved into "More actions".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Overflow {
    pub open_on_github: bool,
    pub summarize: bool,
}

/// Which actions leave the line so the title keeps [`TITLE_MIN`] (all of
/// itself when shorter) beside its reference: Open on GitHub first, then
/// Summarize. The review button and "More actions" always stay.
fn overflow(widths: &LineWidths) -> Overflow {
    let title = widths.title.min(TITLE_MIN) + widths.reference;
    let fits = |actions: &[f32]| {
        let gaps = ACTION_GAP * actions.len() as f32;
        widths.room - TITLE_GAP - widths.fixed - gaps - actions.iter().sum::<f32>() >= title
    };
    if widths.room <= 0. || fits(&[widths.open_on_github, widths.summarize]) {
        return Overflow::default();
    }
    Overflow {
        open_on_github: true,
        summarize: !fits(&[widths.summarize]),
    }
}

/// The width of a labeled 28 px button, in px at 100% zoom: its side
/// padding, the 16 px icon and its gap, the 14 px label, and the border.
fn button_width(label: &str, family: &SharedString, cx: &App) -> f32 {
    20. + 16. + 4. + ui::text_width(label, 14., family, cx) + 2.
}

/// The header's labeled actions "More actions" holds while they are out of
/// the line.
#[derive(Clone)]
pub(super) struct OverflowItems {
    overflow: Overflow,
    url: String,
    summarizing: bool,
    view: WeakEntity<PullRequestView>,
}

impl OverflowItems {
    /// `menu` with the actions that left the line, then a separator.
    pub(super) fn add_to(&self, menu: PopupMenu) -> PopupMenu {
        if self.overflow == Overflow::default() {
            return menu;
        }
        let menu = if self.overflow.open_on_github {
            let url = self.url.clone();
            menu.item(
                PopupMenuItem::new("Open on GitHub")
                    .icon(IconName::ExternalLink)
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
        } else {
            menu
        };
        let menu = if self.overflow.summarize {
            let view = self.view.clone();
            menu.item(
                PopupMenuItem::new(if self.summarizing {
                    "Summarizing…"
                } else {
                    "Summarize"
                })
                .icon(IconName::Sparkles)
                .disabled(self.summarizing)
                .on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.summarize_pull_request(cx))
                        .ok();
                }),
            )
        } else {
            menu
        };
        menu.separator()
    }
}

impl PullRequestView {
    /// The page header of a loaded pull request: the title's line with the
    /// actions at its trailing edge, then the meta line. 72 px in all.
    pub(super) fn render_header(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let url = detail.url.clone();
        let can_edit = detail.viewer_can_update && self.title_edit.is_none();
        let muted = cx.theme().muted_foreground;
        let ui_font = cx.theme().font_family.clone();
        let reference = format!("{} #{}", detail.pr.repo, detail.pr.number);
        let pr_actions = available_actions(detail);
        let review = review_button(detail);
        let review_width = match (review, detail.pending_review.as_ref()) {
            (ReviewButton::Hidden, _) => 0.,
            (_, None) => button_width("Review changes", &ui_font, cx) + ACTION_GAP,
            // The pending count's chip and the discard button come with it.
            (_, Some(pending)) => {
                let chip = 18.
                    + ui::text_width(
                        &pending_comments_label(pending.comment_count),
                        12.,
                        &ui_font,
                        cx,
                    );
                button_width("Finish review", &ui_font, cx) + chip + 24. + 3. * ACTION_GAP
            }
        };
        let overflow = overflow(&LineWidths {
            room: f32::from(self.header_line.get()) / f32::from(dp(1.)),
            // The semibold face runs about 4% wider than the regular one
            // `text_width` measures.
            title: ui::text_width(&detail.title, 18., &ui_font, cx) * 1.04,
            reference: ACTION_GAP + ui::text_width(&reference, 13., &ui_font, cx),
            open_on_github: button_width("Open on GitHub", &ui_font, cx),
            // The split button's caret half is a 28 px square.
            summarize: button_width("Summarize", &ui_font, cx) + 28.,
            fixed: review_width + 28.,
        });
        let view = cx.entity().downgrade();
        let items = OverflowItems {
            overflow,
            url: url.clone(),
            summarizing: guide_service::is_generating(cx, &detail.pr),
            view: view.clone(),
        };
        // The actions with a label are in sight while the title keeps its
        // room; Copy link and the ones that were icons alone (edit the
        // title, the repository's list, refresh) are items of the "More
        // actions" menu, with the state changes.
        let actions = h_flex()
            .flex_none()
            .gap_2()
            .items_center()
            .when(!overflow.open_on_github, |actions| {
                actions.child(
                    Button::new("pr-open-in-browser")
                        .ghost()
                        .control()
                        .icon(IconName::ExternalLink)
                        .label("Open on GitHub")
                        .tooltip("Open this pull request in the browser")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .when(!overflow.summarize, |actions| {
                actions.child(self.render_summarize_button(detail, cx))
            })
            .when(review != ReviewButton::Hidden, |actions| {
                actions.child(self.render_review_controls(
                    detail,
                    review == ReviewButton::Primary,
                    cx,
                ))
            })
            .child(self.render_action_controls(&pr_actions, detail, can_edit, items, cx));
        // The line's width decides what fits on the next frame; a change
        // renders the header again.
        let line = self.header_line.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                if (f32::from(line.get()) - f32::from(bounds.size.width)).abs() >= 1. {
                    line.set(bounds.size.width);
                    view.update(cx, |_, cx| cx.notify()).ok();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let title = SharedString::from(detail.title.clone());
        v_flex()
            .id("page-header")
            .flex_none()
            .w_full()
            .px(scale::content_inset())
            .py_2()
            .gap_2()
            .window_drag_area()
            .child(
                h_flex()
                    .relative()
                    .w_full()
                    .h(scale::control_height())
                    .gap_3()
                    .items_center()
                    .child(measure)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_baseline()
                            .child(
                                div()
                                    .id("pr-title")
                                    .role(Role::Heading)
                                    .aria_label(title.clone())
                                    .min_w_0()
                                    .truncate()
                                    .text_size(scale::heading())
                                    .line_height(rems(1.625))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .tooltip({
                                        let title = title.clone();
                                        move |window, cx| {
                                            Tooltip::new(title.clone()).build(window, cx)
                                        }
                                    })
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(scale::body())
                                    .text_color(muted)
                                    .child(reference),
                            ),
                    )
                    .child(actions),
            )
            .child(self.render_meta(detail, cx))
    }

    /// Summarize: opens the Guide tab and generates a guided review with local
    /// Claude Code (or shows the cached guide when already up to date). In a
    /// stack it summarizes the whole stack unless "this PR only" was picked;
    /// its menu picks the scope and the language.
    fn render_summarize_button(&self, detail: &PullRequestDetail, cx: &mut Context<Self>) -> Div {
        use crate::github::guide_service::{self, GuideScope};
        let stack = self.stack_graph.as_ref();
        let busy = guide_service::is_generating(cx, &detail.pr);
        let fresh = guide_service::has_fresh_scoped_guide(cx, detail, stack);
        let scope = guide_service::wanted_scope(cx, &detail.pr, stack);
        let stack_size = stack
            .filter(|graph| graph.len() >= 2 && graph.contains(detail.pr.number))
            .map(|graph| graph.len());
        let tooltip = match (busy, fresh, scope) {
            (true, _, _) => "Generating guided review with Claude Code",
            (false, true, GuideScope::Stack) => "Open the guided review of this stack",
            (false, true, GuideScope::PullRequest) => "Open the guided review for this commit",
            (false, false, GuideScope::Stack) => {
                "Generate a guided review of the whole stack using local Claude Code"
            }
            (false, false, GuideScope::PullRequest) => {
                "Generate a guided review using local Claude Code"
            }
        };
        let current = guide_service::wanted_language(cx, detail);
        let choices = guide_service::language_choices(cx, detail);
        let view = cx.entity().downgrade();
        div()
            .flex_none()
            .debug_selector(|| "pr-summarize".into())
            .child(
                DropdownButton::new("pr-summarize")
                    .outline()
                    .button(
                        Button::new("pr-summarize-action")
                            .outline()
                            .control()
                            .icon(IconName::Sparkles)
                            .label(if busy { "Summarizing…" } else { "Summarize" })
                            .tooltip(tooltip)
                            .loading(busy)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.summarize_pull_request(cx)),
                            ),
                    )
                    .menu_accessibility_label("Summarize options")
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu.min_w(scale::menu_min_width());
                        if let Some(size) = stack_size {
                            for (label, item_scope) in [
                                (format!("Summarize stack ({size} PRs)"), GuideScope::Stack),
                                ("Summarize this PR only".to_owned(), GuideScope::PullRequest),
                            ] {
                                let view = view.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .checked(item_scope == scope)
                                        .on_click(move |_, _, cx| {
                                            view.update(cx, |this, cx| {
                                                this.summarize_scope(item_scope, cx)
                                            })
                                            .ok();
                                        }),
                                );
                            }
                            menu = menu.separator();
                        }
                        let menu = menu.label("Summarize in");
                        choices
                            .iter()
                            .enumerate()
                            .fold(menu, |menu, (ix, language)| {
                                let label = if ix == 0 {
                                    format!("{} (Settings)", language.name())
                                } else {
                                    language.name().to_owned()
                                };
                                let language = language.clone();
                                let view = view.clone();
                                menu.item(
                                    PopupMenuItem::new(label)
                                        .checked(language == current)
                                        .on_click(move |_, _, cx| {
                                            let language = language.clone();
                                            view.update(cx, |this, cx| {
                                                this.summarize_in(language, cx)
                                            })
                                            .ok();
                                        }),
                                )
                            })
                    }),
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

    /// Who opened it, which branches, how big, whether checks pass: the line
    /// under the title.
    fn render_meta(&self, detail: &PullRequestDetail, cx: &mut Context<Self>) -> Div {
        let head = head_label(detail);
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
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
        h_flex()
            .w_full()
            .h(scale::chip_height())
            // What does not fit is cut at the trailing edge; the head branch
            // gives way first.
            .min_w_0()
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
    use super::{LineWidths, Overflow, branch_label, overflow};

    fn widths(room: f32) -> LineWidths {
        LineWidths {
            room,
            title: 700.,
            reference: 100.,
            open_on_github: 130.,
            summarize: 140.,
            fixed: 180.,
        }
    }

    #[test]
    fn the_title_keeps_its_room_and_the_labeled_actions_give_way_in_order() {
        let open_moved = Overflow {
            open_on_github: true,
            summarize: false,
        };
        // Everything: 12 + 180 + 8 + 130 + 8 + 140, 400 of the title, and
        // the reference.
        assert_eq!(overflow(&widths(978.)), Overflow::default());
        assert_eq!(overflow(&widths(977.)), open_moved);
        // Without Open on GitHub: 12 + 180 + 8 + 140 + 400 + 100.
        assert_eq!(overflow(&widths(840.)), open_moved);
        assert_eq!(
            overflow(&widths(839.)),
            Overflow {
                open_on_github: true,
                summarize: true,
            }
        );
    }

    #[test]
    fn a_short_title_asks_only_for_itself() {
        let short = |room| LineWidths {
            title: 200.,
            ..widths(room)
        };
        assert_eq!(overflow(&short(778.)), Overflow::default());
        assert!(overflow(&short(777.)).open_on_github);
    }

    #[test]
    fn nothing_moves_before_the_line_is_measured() {
        assert_eq!(overflow(&widths(0.)), Overflow::default());
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
