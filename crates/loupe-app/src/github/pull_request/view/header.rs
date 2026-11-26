//! The header: one line with the title, the repository and number, and the
//! pull request actions, and under it the meta line (state, author,
//! branches, size, checks). Neither wraps: labeled actions move into "More
//! actions" before the title is cut, and the reference gives way first.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::PullRequestDetail;
use std::cell::Cell;
use std::rc::Rc;

use super::PullRequestView;
use super::model::{ReviewButton, available_actions, head_label, review_button};
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

/// Space between two actions, between the title and the actions, and
/// between the repository and the number, in px at 100% zoom.
const ACTION_GAP: f32 = 8.;
const TITLE_GAP: f32 = 12.;
const NUMBER_GAP: f32 = 4.;

/// The least of the repository's name worth showing cut, in px.
const REPO_MIN: f32 = 48.;

/// The widths the header's line weighs, in px at 100% zoom.
struct LineWidths {
    /// The line's width on the last frame; zero before the first.
    room: f32,
    /// The title, the repository, and the number, whole.
    title: f32,
    repo: f32,
    number: f32,
    open_on_github: f32,
    summarize: f32,
    /// The actions that never move: the review button and "More actions".
    fixed: f32,
}

impl LineWidths {
    /// The title and its reference, whole, with the gaps between them.
    fn heading(&self) -> f32 {
        self.title + ACTION_GAP + self.repo + NUMBER_GAP + self.number
    }

    /// What the actions in sight leave the title and its reference.
    fn left_by(&self, overflow: Overflow) -> f32 {
        let moving = [
            (!overflow.open_on_github).then_some(self.open_on_github),
            (!overflow.summarize).then_some(self.summarize),
        ];
        let actions: f32 = moving
            .iter()
            .flatten()
            .map(|width| width + ACTION_GAP)
            .sum();
        self.room - TITLE_GAP - self.fixed - actions
    }
}

/// The labeled actions that moved into "More actions".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Overflow {
    pub open_on_github: bool,
    pub summarize: bool,
}

/// Which actions leave the line before the title or its reference is cut:
/// Open on GitHub first, then Summarize. The review button and "More
/// actions" always stay. Before the line is measured nothing moves.
fn overflow(widths: &LineWidths) -> Overflow {
    let open_moved = Overflow {
        open_on_github: true,
        summarize: false,
    };
    if widths.room <= 0. || widths.left_by(Overflow::default()) >= widths.heading() {
        Overflow::default()
    } else if widths.left_by(open_moved) >= widths.heading() {
        open_moved
    } else {
        Overflow {
            open_on_github: true,
            summarize: true,
        }
    }
}

/// How the title and its reference share what the actions leave. The
/// reference gives way first: the repository is cut, then hidden, and only
/// then the title is cut. The number always stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitleFit {
    Whole,
    RepoCut,
    RepoHidden,
    TitleCut,
}

fn title_fit(widths: &LineWidths, room: f32) -> TitleFit {
    let title = widths.title + ACTION_GAP + widths.number;
    if widths.room <= 0. || room >= widths.heading() {
        TitleFit::Whole
    } else if room >= title + REPO_MIN + NUMBER_GAP {
        TitleFit::RepoCut
    } else if room >= title {
        TitleFit::RepoHidden
    } else {
        TitleFit::TitleCut
    }
}

/// The width of a labeled 28 px button, in px at 100% zoom: its side
/// padding, the 16 px icon and its gap, the 14 px label, and the border.
/// Stands in until the button has been drawn and measured.
fn button_width(label: &str, family: &SharedString, cx: &App) -> f32 {
    20. + 16. + 4. + ui::text_width(label, 14., family, cx) + 2.
}

/// How wide `text` is set at `size` and `weight`, in px at 100% zoom.
fn shaped_width(
    text: &str,
    size: Rems,
    weight: FontWeight,
    family: &SharedString,
    window: &Window,
) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: Font {
            weight,
            ..font(family.clone())
        },
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let width = window
        .text_system()
        .shape_line(
            text.to_owned().into(),
            size.to_pixels(window.rem_size()),
            &[run],
            None,
        )
        .width;
    f32::from(width) / f32::from(dp(1.))
}

/// The widths of the header's line and of its actions on the last frame
/// they were drawn, in window pixels; zero until then.
#[derive(Default)]
pub(super) struct HeaderMeasure {
    line: Cell<Pixels>,
    open_on_github: Cell<Pixels>,
    summarize: Cell<Pixels>,
    fixed: Cell<Pixels>,
}

/// An invisible layer over its parent that keeps the parent's width in
/// `slot` and renders the header again when it changes.
fn measure(
    measured: &Rc<HeaderMeasure>,
    slot: fn(&HeaderMeasure) -> &Cell<Pixels>,
    view: WeakEntity<PullRequestView>,
) -> impl IntoElement {
    let measured = measured.clone();
    canvas(
        move |bounds, _, cx| {
            let cell = slot(&measured);
            if (f32::from(cell.get()) - f32::from(bounds.size.width)).abs() >= 1. {
                cell.set(bounds.size.width);
                view.update(cx, |_, cx| cx.notify()).ok();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
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
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let url = detail.url.clone();
        let can_edit = detail.viewer_can_update && self.title_edit.is_none();
        let muted = cx.theme().muted_foreground;
        let ui_font = cx.theme().font_family.clone();
        let repo = detail.pr.repo.to_string();
        let number = format!("#{}", detail.pr.number);
        let pr_actions = available_actions(detail);
        let review = review_button(detail);
        let measured = &self.header_measure;
        let drawn = |cell: &Cell<Pixels>, fallback: f32| {
            let width = f32::from(cell.get());
            if width > 0. {
                width / f32::from(dp(1.))
            } else {
                fallback
            }
        };
        let widths = LineWidths {
            room: drawn(&measured.line, 0.),
            title: shaped_width(
                &detail.title,
                scale::heading(),
                FontWeight::SEMIBOLD,
                &ui_font,
                window,
            ),
            repo: shaped_width(&repo, scale::body(), FontWeight::NORMAL, &ui_font, window),
            number: shaped_width(&number, scale::body(), FontWeight::NORMAL, &ui_font, window),
            open_on_github: drawn(
                &measured.open_on_github,
                button_width("Open on GitHub", &ui_font, cx),
            ),
            // The split button's caret half is a 28 px square.
            summarize: drawn(
                &measured.summarize,
                button_width("Summarize", &ui_font, cx) + 28.,
            ),
            fixed: drawn(&measured.fixed, 0.),
        };
        let overflow = overflow(&widths);
        let fit = title_fit(&widths, widths.left_by(overflow));
        let view = cx.entity().downgrade();
        let items = OverflowItems {
            overflow,
            url: url.clone(),
            summarizing: guide_service::is_generating(cx, &detail.pr),
            view: view.clone(),
        };
        // The actions with a label are in sight while the title fits whole;
        // Copy link and the ones that were icons alone (edit the title, the
        // repository's list, refresh) are items of the "More actions" menu,
        // with the state changes.
        let actions = h_flex()
            .flex_none()
            .gap_2()
            .items_center()
            .when(!overflow.open_on_github, |actions| {
                actions.child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("pr-open-in-browser")
                                .ghost()
                                .control()
                                .icon(IconName::ExternalLink)
                                .label("Open on GitHub")
                                .tooltip("Open this pull request in the browser")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                        .child(measure(measured, |m| &m.open_on_github, view.clone())),
                )
            })
            .when(!overflow.summarize, |actions| {
                actions.child(
                    self.render_summarize_button(detail, cx)
                        .relative()
                        .child(measure(measured, |m| &m.summarize, view.clone())),
                )
            })
            .child(
                h_flex()
                    .relative()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .when(review != ReviewButton::Hidden, |actions| {
                        actions.child(self.render_review_controls(
                            detail,
                            review == ReviewButton::Primary,
                            cx,
                        ))
                    })
                    .child(self.render_action_controls(&pr_actions, detail, can_edit, items, cx))
                    .child(measure(measured, |m| &m.fixed, view.clone())),
            );
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
                    .child(measure(measured, |m| &m.line, view))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            // The last guard: what the measures missed is cut
                            // here, never drawn over the actions.
                            .overflow_hidden()
                            .gap_2()
                            .items_baseline()
                            .child(
                                div()
                                    .id("pr-title")
                                    .role(Role::Heading)
                                    .aria_label(title.clone())
                                    .map(|title| match fit {
                                        TitleFit::TitleCut => title.min_w_0(),
                                        _ => title.flex_none(),
                                    })
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
                                h_flex()
                                    .id("pr-reference")
                                    .role(Role::Group)
                                    .aria_label(format!("{repo} {number}"))
                                    .min_w_0()
                                    .gap_1()
                                    .text_size(scale::body())
                                    .text_color(muted)
                                    .when(
                                        matches!(fit, TitleFit::Whole | TitleFit::RepoCut),
                                        |row| {
                                            row.child(
                                                div()
                                                    .min_w_0()
                                                    .map(|repo| match fit {
                                                        TitleFit::Whole => repo.flex_none(),
                                                        _ => repo.flex_shrink_1(),
                                                    })
                                                    .truncate()
                                                    .child(repo.clone()),
                                            )
                                        },
                                    )
                                    .child(div().flex_none().child(number)),
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
            .children(
                self.render_review_progress(cx)
                    .map(|progress| h_flex().flex_1().min_w_0().justify_end().child(progress)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{LineWidths, Overflow, TitleFit, branch_label, overflow, title_fit};

    /// A 600 px title, "owner/repo" of 140 px, "#123" of 30 px: 782 px with
    /// the gaps. Actions: 130 + 140 movable, 180 fixed.
    fn widths(room: f32) -> LineWidths {
        LineWidths {
            room,
            title: 600.,
            repo: 140.,
            number: 30.,
            open_on_github: 130.,
            summarize: 140.,
            fixed: 180.,
        }
    }

    fn fit(room: f32) -> (Overflow, TitleFit) {
        let widths = widths(room);
        let overflow = overflow(&widths);
        (overflow, title_fit(&widths, widths.left_by(overflow)))
    }

    const NONE: Overflow = Overflow {
        open_on_github: false,
        summarize: false,
    };
    const OPEN: Overflow = Overflow {
        open_on_github: true,
        summarize: false,
    };
    const BOTH: Overflow = Overflow {
        open_on_github: true,
        summarize: true,
    };

    #[test]
    fn the_labeled_actions_leave_before_the_title_or_its_reference_is_cut() {
        // Everything: 12 + 180 + 138 + 148 + 782.
        assert_eq!(fit(1260.), (NONE, TitleFit::Whole));
        assert_eq!(fit(1259.), (OPEN, TitleFit::Whole));
        // Without Open on GitHub: 12 + 180 + 148 + 782.
        assert_eq!(fit(1122.), (OPEN, TitleFit::Whole));
        assert_eq!(fit(1121.).0, BOTH);
    }

    #[test]
    fn then_the_repository_gives_way_and_the_title_is_cut_last() {
        // With both actions in the menu the title and reference get
        // room - 192: whole down to 782.
        assert_eq!(fit(974.), (BOTH, TitleFit::Whole));
        assert_eq!(fit(973.), (BOTH, TitleFit::RepoCut));
        // The repository is cut down to 48 px: 600 + 8 + 30 + 48 + 4.
        assert_eq!(fit(882.), (BOTH, TitleFit::RepoCut));
        assert_eq!(fit(881.), (BOTH, TitleFit::RepoHidden));
        // Then only the number stays beside the whole title: 638.
        assert_eq!(fit(830.), (BOTH, TitleFit::RepoHidden));
        assert_eq!(fit(829.), (BOTH, TitleFit::TitleCut));
    }

    #[test]
    fn nothing_moves_before_the_line_is_measured() {
        assert_eq!(fit(0.), (NONE, TitleFit::Whole));
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
