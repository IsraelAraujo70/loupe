//! The review and state actions of the meta line, the band under it where
//! a confirmation or a write error opens, and the review composer: a column
//! at the trailing edge of every tab, so opening it moves nothing above or
//! below it.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{
    ChecksState, MergeMethod, NodeId, PullRequestDetail, ReviewEvent, StackEntry, StackGraph,
    StackMergePlan, StackMergeStep, StepStatus,
};

use super::header::OverflowItems;
use super::model::{
    DraftAction, Panel, PrActions, PrTab, ReviewButton, StackMerge, StateAction, can_approve,
    discard_confirm_text, merge_button_label, merge_confirm_text, pending_comments_label,
    ready_subset_button_label, resolve_method, review_blocker, review_button,
    stack_confirm_headline, stack_entry_problems, stack_merge, stack_merge_button_label,
    stack_merge_confirm_text,
};
use super::{PullRequestView, StackMergeRun, Write};
use crate::github::common::Submit;
use crate::github::widgets;
use crate::shell::Refreshable as _;
use crate::theme::{scale, tokens};
use crate::ui;
use crate::ui::{ControlSize as _, MenuTriggerExt as _};

gpui_kit::actions!(
    github,
    [
        /// Opens the review composer of the pull request on screen, on any
        /// tab, with the caret in its summary. Dispatch it from inside the
        /// page (the Guide's last step, the inline comment box).
        OpenReviewComposer
    ]
);

/// How many comments each pull request's pending review holds, by the pull
/// request's node id, as its page last loaded it: the inline comment box
/// says what its comment joins.
#[derive(Default)]
pub(in crate::github::pull_request) struct PendingComments(HashMap<NodeId, u32>);

impl Global for PendingComments {}

impl PendingComments {
    pub(super) fn set(pull_request_id: &NodeId, count: u32, cx: &mut App) {
        if Self::count(pull_request_id, cx) == count && cx.has_global::<Self>() {
            return;
        }
        cx.default_global::<Self>()
            .0
            .insert(pull_request_id.clone(), count);
    }

    pub(in crate::github::pull_request) fn count(pull_request_id: &NodeId, cx: &App) -> u32 {
        cx.try_global::<Self>()
            .and_then(|counts| counts.0.get(pull_request_id))
            .copied()
            .unwrap_or(0)
    }
}

/// The narrowest window, 1880 px, where the Files tab keeps its tree, a
/// readable diff, and its whole toolbar beside a docked composer. The other
/// tabs dock from the details column's breakpoint, whose place it takes;
/// under either, the composer is a bar under the content.
const FILES_DOCK_BREAKPOINT: Rems = Rems(1880. / 16.);

/// How a confirmation commits: the merge is the pull request's goal, the
/// others destroy something.
#[derive(Clone, Copy)]
enum Commit {
    Merge,
    Destructive,
}

impl PullRequestView {
    /// The band under the meta line: the open panel, the last write error,
    /// and what the last write left to say.
    pub(super) fn render_panel_band(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = cx.theme().border;
        let surface = cx.theme().group_box;
        // Ready for review reports in the merge box, which only the
        // Conversation tab shows; elsewhere its error comes here. A write's
        // error shows in the review composer while it is open.
        let error = self
            .write
            .error()
            .filter(|_| !self.composer_open())
            .or(self
                .ready
                .error()
                .filter(|_| self.tab != PrTab::Conversation))
            .map(str::to_owned);
        v_flex()
            .flex_none()
            .when(
                !matches!(
                    self.panel,
                    Panel::Closed
                        | Panel::Review
                        | Panel::ConfirmDiscard
                        | Panel::ConfirmMerge(_)
                        | Panel::ConfirmStackMerge(_)
                        | Panel::StackMergeProgress
                ),
                |column| {
                    column.child(
                        div()
                            .px(scale::content_inset())
                            .py_3()
                            .bg(surface)
                            .border_b_1()
                            .border_color(border)
                            .child(self.render_panel(detail, cx)),
                    )
                },
            )
            .when_some(error, |column, message| {
                column.child(
                    h_flex()
                        .px(scale::content_inset())
                        .py_2()
                        .gap_2()
                        .items_center()
                        .border_b_1()
                        .border_color(border)
                        .child(Alert::error("pr-write-error", message).flex_1())
                        .child(
                            ui::icon_button("pr-write-error-dismiss", IconName::Close, "Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.write = crate::github::common::Submit::Idle;
                                    this.ready = crate::github::common::Submit::Idle;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.write_note, |column, note| {
                column.child(
                    h_flex()
                        .px(scale::content_inset())
                        .py_2()
                        .gap_2()
                        .items_center()
                        .border_b_1()
                        .border_color(border)
                        .child(Alert::info("pr-write-note", note).flex_1())
                        .child(
                            ui::icon_button("pr-write-note-dismiss", IconName::Close, "Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.write_note = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
    }

    /// The review button, filled when `primary`: it is the page's main
    /// action while the pull request cannot be merged, and an outlined
    /// secondary action once the merge button is on screen. It opens the
    /// review composer, or brings the caret back to it when it is open; the
    /// composer's own × and Esc close it. Discarding a pending review is in
    /// the composer.
    pub(super) fn render_review_controls(
        &self,
        detail: &PullRequestDetail,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pending = detail.pending_review.as_ref();
        h_flex()
            .gap_2()
            .items_center()
            .when_some(pending, |row, review| {
                row.child(
                    widgets::tinted_tag(tokens::pending())
                        .child(pending_comments_label(review.comment_count)),
                )
            })
            .child(
                div().debug_selector(|| "pr-review".to_owned()).child(
                    Button::new("pr-review")
                        .control()
                        .map(|button| {
                            if primary {
                                button.primary()
                            } else {
                                button.outline()
                            }
                        })
                        .icon(IconName::MessageSquareDiff)
                        // No ellipsis: at this size it reads as a cut label.
                        .label(if pending.is_some() {
                            "Finish review"
                        } else {
                            "Review changes"
                        })
                        .tooltip("Open the review: summary, pending comments, and verdict")
                        .selected(self.composer_open())
                        .on_click(cx.listener(|this, _, _, cx| this.open_review_composer(cx))),
                ),
            )
    }

    /// Whether the review composer is on screen: writing the review, or
    /// confirming that its pending comments go away.
    pub(super) fn composer_open(&self) -> bool {
        matches!(self.panel, Panel::Review | Panel::ConfirmDiscard)
    }

    /// Opens the review composer with the caret in its summary, on a pull
    /// request that has something to review; an open composer only takes
    /// the caret back.
    pub(super) fn open_review_composer(&mut self, cx: &mut Context<Self>) {
        let reviewable = self
            .detail
            .value()
            .is_some_and(|detail| review_button(detail) != ReviewButton::Hidden);
        if !reviewable {
            return;
        }
        if !self.composer_open() {
            self.panel = Panel::Review;
        }
        self.focus_review_body = true;
        if self.write.error().is_some() {
            self.write = Submit::Idle;
        }
        cx.notify();
    }

    /// Whether the composer takes a column of its own beside the tab's
    /// content (a wide window) or a bar under it (a narrow one), so a narrow
    /// window never squeezes the diff, cuts a toolbar, or hides an action.
    pub(super) fn composer_docks(&self, window: &Window) -> bool {
        let needed = if self.tab == PrTab::Files {
            FILES_DOCK_BREAKPOINT
        } else {
            scale::side_column_breakpoint()
        };
        window.viewport_size().width >= needed.to_pixels(window.rem_size())
    }

    /// Closes the composer and keeps what was typed in the summary.
    fn close_review_composer(&mut self, cx: &mut Context<Self>) {
        if self.write.is_busy() {
            return;
        }
        self.panel = Panel::Closed;
        if self.write.error().is_some() {
            self.write = Submit::Idle;
        }
        cx.notify();
    }

    /// The "More actions" menu: the header's labeled actions that left its
    /// line (`overflow`), edit the title, the repository's pull requests,
    /// refresh, and the less used state changes (draft, close, reopen).
    /// Close is the one item in the danger color, and only here.
    pub(super) fn render_action_controls(
        &self,
        actions: &PrActions,
        detail: &PullRequestDetail,
        can_edit: bool,
        overflow: OverflowItems,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (draft, state) = (actions.draft, actions.state);
        let this = cx.entity().downgrade();
        let title = detail.title.clone();
        let repo = detail.pr.repo.clone();
        let (url, reference) = (detail.url.clone(), detail.pr.to_string());
        ui::outline_icon_button("pr-more", IconName::Ellipsis, "More actions")
            .loading(self.detail.is_busy())
            .disabled(self.write.is_busy())
            .selected(self.panel == Panel::ConfirmClose)
            .menu_trigger()
            // The button is at the header's trailing edge: the menu's
            // trailing edge sits under it.
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let menu = overflow.add_to(menu);
                let menu = if can_edit {
                    let (this, title) = (this.clone(), title.clone());
                    menu.item(
                        PopupMenuItem::new("Edit title")
                            .icon(IconName::Pencil)
                            .on_click(move |_, window, cx| {
                                this.update(cx, |this, cx| {
                                    this.start_title_edit(title.clone(), window, cx)
                                })
                                .ok();
                            }),
                    )
                } else {
                    menu
                };
                let menu = {
                    let (url, reference) = (url.clone(), reference.clone());
                    menu.item(
                        PopupMenuItem::new("Copy link")
                            .icon(IconName::Link)
                            .on_click(move |_, window, cx| {
                                crate::shell::item_menu::copy_link(&url, &reference, window, cx)
                            }),
                    )
                };
                let menu = {
                    let repo = repo.clone();
                    menu.item(
                        PopupMenuItem::new("Repository pull requests")
                            .icon(IconName::FolderGit2)
                            .on_click(move |event, window, cx| {
                                crate::shell::open(
                                    crate::route::Route::Repo(repo.clone()),
                                    event,
                                    window,
                                    cx,
                                );
                            }),
                    )
                };
                let menu = {
                    let this = this.clone();
                    menu.item(
                        PopupMenuItem::new("Refresh")
                            .icon(IconName::RefreshCw)
                            .on_click(move |_, _, cx| {
                                this.update(cx, |this, cx| this.refresh_page(cx)).ok();
                            }),
                    )
                };
                let menu = if draft.is_some() || state.is_some() {
                    menu.item(PopupMenuItem::separator())
                } else {
                    menu
                };
                {
                    let menu = match draft {
                        Some(action) => {
                            let (label, icon, write) = match action {
                                DraftAction::MarkReady => (
                                    "Ready for review",
                                    IconName::GitPullRequest,
                                    Write::MarkReady,
                                ),
                                DraftAction::ConvertToDraft => (
                                    "Convert to draft",
                                    IconName::GitPullRequestDraft,
                                    Write::ConvertToDraft,
                                ),
                            };
                            let this = this.clone();
                            menu.item(PopupMenuItem::new(label).icon(icon).on_click(
                                move |_, _, cx| {
                                    this.update(cx, |this, cx| this.start_write(write.clone(), cx))
                                        .ok();
                                },
                            ))
                        }
                        None => menu,
                    };
                    let this = this.clone();
                    match state {
                        Some(StateAction::Close) => menu.item(PopupMenuItem::separator()).item(
                            PopupMenuItem::element(|_, cx| {
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .text_color(cx.theme().danger)
                                    .child(Icon::new(IconName::GitPullRequestClosed).small())
                                    .child("Close pull request")
                            })
                            .on_click(move |_, _, cx| {
                                this.update(cx, |this, cx| this.set_panel(Panel::ConfirmClose, cx))
                                    .ok();
                            }),
                        ),
                        Some(StateAction::Reopen) => menu.item(
                            PopupMenuItem::new("Reopen")
                                .icon(IconName::GitPullRequestArrow)
                                .on_click(move |_, _, cx| {
                                    this.update(cx, |this, cx| this.start_write(Write::Reopen, cx))
                                        .ok();
                                }),
                        ),
                        None => menu,
                    }
                }
            })
            .into_any_element()
    }

    /// The merge button, split with a method menu when the repository allows
    /// more than one method.
    pub(super) fn render_merge_controls(
        &self,
        methods: &[MergeMethod],
        busy: bool,
        filled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(selected) = resolve_method(methods, self.merge_method) else {
            return div().into_any_element();
        };
        let confirming = matches!(self.panel, Panel::ConfirmMerge(_));
        // The button names the action; the method it will use is in its
        // tooltip and in the menu behind the caret.
        let style = |button: Button| {
            if filled {
                button.success()
            } else {
                button.outline()
            }
        };
        let merge = style(Button::new("pr-merge"))
            .icon(IconName::GitMerge)
            .label("Merge pull request")
            .tooltip(merge_button_label(selected))
            .disabled(busy)
            .selected(confirming)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_panel(Panel::ConfirmMerge(selected), cx);
            }));
        let this = cx.entity().downgrade();
        let choices: Vec<MergeMethod> = methods.to_vec();
        h_flex()
            .gap_2()
            .items_center()
            .map(|row| {
                if choices.len() < 2 {
                    return row.child(merge);
                }
                // A split button, as on GitHub: the left half merges with the
                // chosen method, the caret picks another.
                row.child(
                    h_flex().gap(px(1.)).child(merge.rounded_r_none()).child(
                        style(Button::new("pr-merge-method"))
                            .w_8()
                            .dropdown_caret(true)
                            .disabled(busy)
                            .rounded_l_none()
                            .tooltip("Merge method")
                            .accessibility_label("Merge method")
                            .menu_trigger()
                            .dropdown_menu(move |menu, _, _| {
                                choices.iter().fold(menu, |menu, method| {
                                    let (method, this) = (*method, this.clone());
                                    menu.item(
                                        PopupMenuItem::new(merge_button_label(method))
                                            .checked(method == selected)
                                            .on_click(move |_, _, cx| {
                                                this.update(cx, |this, cx| {
                                                    this.merge_method = Some(method);
                                                    match this.panel {
                                                        Panel::ConfirmMerge(_) => {
                                                            this.panel =
                                                                Panel::ConfirmMerge(method);
                                                        }
                                                        Panel::ConfirmStackMerge(_) => {
                                                            this.panel =
                                                                Panel::ConfirmStackMerge(method);
                                                        }
                                                        _ => {}
                                                    }
                                                    cx.notify();
                                                })
                                                .ok();
                                            }),
                                    )
                                })
                            }),
                    ),
                )
            })
            .into_any_element()
    }

    /// The "Merge stack (N PRs)" split button on the root PR of a stack, with
    /// the same merge-method menu as "Merge pull request".
    pub(super) fn render_stack_merge_controls(
        &self,
        graph: &StackGraph,
        plan: &StackMergePlan,
        methods: &[MergeMethod],
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(selected) = resolve_method(methods, self.merge_method) else {
            return div().into_any_element();
        };
        let all_ready = plan.all_ready();
        let confirming = matches!(self.panel, Panel::ConfirmStackMerge(_));
        let label = stack_merge_button_label(graph.len());
        let tooltip = if all_ready {
            format!("{} ({})", label, merge_button_label(selected))
        } else {
            let blocked = plan.blocking_steps();
            let first = blocked[0];
            let reasons: Vec<&str> = first.blockers.iter().map(|b| b.label()).collect();
            format!(
                "#{} is not ready ({}) — click to review stack",
                first.node.pr.number,
                reasons.join(", ")
            )
        };
        let style = |button: Button| {
            if all_ready {
                button.success()
            } else {
                button.outline()
            }
        };
        let trigger = style(Button::new("pr-merge-stack"))
            .icon(IconName::Layers)
            .label(label)
            .tooltip(tooltip)
            .disabled(busy)
            .selected(confirming)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_panel(Panel::ConfirmStackMerge(selected), cx);
            }));
        let this = cx.entity().downgrade();
        let choices: Vec<MergeMethod> = methods.to_vec();
        h_flex()
            .flex_none()
            .debug_selector(|| "pr-merge-stack".to_owned())
            .map(|row| {
                if choices.len() < 2 {
                    return row.child(trigger);
                }
                row.child(
                    h_flex().gap(px(1.)).child(trigger.rounded_r_none()).child(
                        style(Button::new("pr-merge-stack-method"))
                            .w_8()
                            .dropdown_caret(true)
                            .disabled(busy)
                            .rounded_l_none()
                            .tooltip("Merge stack method")
                            .accessibility_label("Merge stack method")
                            .menu_trigger()
                            .dropdown_menu(move |menu, _, _| {
                                choices.iter().fold(menu, |menu, method| {
                                    let (method, this) = (*method, this.clone());
                                    menu.item(
                                        PopupMenuItem::new(merge_button_label(method))
                                            .checked(method == selected)
                                            .on_click(move |_, _, cx| {
                                                this.update(cx, |this, cx| {
                                                    this.merge_method = Some(method);
                                                    match this.panel {
                                                        Panel::ConfirmMerge(_) => {
                                                            this.panel =
                                                                Panel::ConfirmMerge(method);
                                                        }
                                                        Panel::ConfirmStackMerge(_) => {
                                                            this.panel =
                                                                Panel::ConfirmStackMerge(method);
                                                        }
                                                        _ => {}
                                                    }
                                                    cx.notify();
                                                })
                                                .ok();
                                            }),
                                    )
                                })
                            }),
                    ),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_panel(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.panel {
            // The review composer draws these.
            Panel::Closed | Panel::Review | Panel::ConfirmDiscard => div().into_any_element(),
            Panel::ConfirmMerge(method) => match stack_merge(detail) {
                Some(stack) => self
                    .render_confirm(
                        stack_merge_confirm_text(stack.entries.len(), stack.base_ref, method),
                        "Merge",
                        Commit::Merge,
                        Write::Merge(method),
                        Some(render_stack_merge(&stack, cx)),
                        cx,
                    )
                    .into_any_element(),
                None => self
                    .render_confirm(
                        merge_confirm_text(detail.pr.number, &detail.base_ref, method),
                        "Merge",
                        Commit::Merge,
                        Write::Merge(method),
                        None,
                        cx,
                    )
                    .into_any_element(),
            },
            Panel::ConfirmStackMerge(method) => match &self.stack_graph {
                Some(graph) if graph.is_root(detail.pr.number) => {
                    self.render_stack_merge_confirm(graph, method, cx)
                }
                _ => div().into_any_element(),
            },
            Panel::StackMergeProgress => match &self.stack_merge_run {
                Some(run) => self.render_stack_merge_progress(run, cx),
                None => div().into_any_element(),
            },
            Panel::ConfirmClose => self
                .render_confirm(
                    format!("Close #{} without merging?", detail.pr.number),
                    "Close pull request",
                    Commit::Destructive,
                    Write::Close,
                    None,
                    cx,
                )
                .into_any_element(),
        }
    }

    fn render_stack_merge_confirm(
        &self,
        graph: &StackGraph,
        method: MergeMethod,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let plan = graph.merge_plan();
        let all_ready = plan.all_ready();
        let ready_subset = plan.ready_subset();
        let all_numbers: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();
        let busy = self.write.is_busy();
        let headline = stack_confirm_headline(&plan, method);
        let (icon, color) = if all_ready {
            (IconName::Layers, tokens::merged())
        } else {
            (IconName::CircleAlert, tokens::closed())
        };
        let confirm_disabled = !all_ready || busy;
        let confirm_selector = if confirm_disabled {
            "pr-stack-confirm:disabled"
        } else {
            "pr-stack-confirm:enabled"
        };
        let confirm_tooltip = if all_ready {
            format!(
                "Merge all {} pull requests with {}",
                plan.steps.len(),
                super::model::method_label(method)
            )
        } else {
            headline.clone()
        };
        let partial_button = (!all_ready && !ready_subset.is_empty()).then(|| {
            let subset = ready_subset.clone();
            let label = ready_subset_button_label(subset.len(), plan.shape);
            let tip = match plan.shape {
                loupe_github::StackShape::Chain => {
                    "Merge only the ready bottom pull requests in this stack"
                }
                loupe_github::StackShape::Tree => {
                    "Merge only the ready child pull requests into their parent branches"
                }
            };
            div()
                .flex_none()
                .debug_selector(|| "pr-stack-confirm-partial".to_owned())
                .child(
                    Button::new("pr-stack-confirm-partial")
                        .small()
                        .outline()
                        .label(label)
                        .tooltip(tip)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start_stack_merge(method, subset.clone(), cx);
                        })),
                )
        });
        let row = h_flex()
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .flex_none()
                    .size_4()
                    .justify_center()
                    .child(Icon::new(icon).small().text_color(color)),
            )
            .child(
                div()
                    .id("pr-stack-confirm-headline")
                    .debug_selector(|| "pr-stack-confirm-headline".to_owned())
                    .flex_1()
                    .min_w_0()
                    .text_size(scale::body())
                    .child(headline),
            )
            .child(
                Button::new("pr-stack-cancel")
                    .small()
                    .ghost()
                    .label("Cancel")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Panel::Closed;
                        cx.notify();
                    })),
            )
            .children(partial_button)
            .child(
                div()
                    .flex_none()
                    .debug_selector(move || confirm_selector.to_owned())
                    .child(
                        Button::new("pr-stack-confirm")
                            .small()
                            .success()
                            .label(stack_merge_button_label(plan.steps.len()))
                            .tooltip(confirm_tooltip)
                            .loading(busy)
                            .disabled(confirm_disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start_stack_merge(method, all_numbers.clone(), cx);
                            })),
                    ),
            );
        let list = v_flex()
            .id("pr-stack-confirm-list")
            .role(Role::List)
            .aria_label(format!("{} pull requests in merge order", plan.steps.len()))
            .children(
                plan.steps
                    .iter()
                    .enumerate()
                    .map(|(ix, step)| render_plan_step_row(ix + 1, step, cx)),
            );
        let chain_method_note = (plan.shape == loupe_github::StackShape::Chain
            && method != MergeMethod::Merge)
            .then(|| {
                div()
                    .pl_6()
                    .text_size(scale::meta())
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} rewrites commits after each step; if {} requires branches to be up to date, later PRs will pause to update and retry.",
                        super::model::method_label(method),
                        plan.base_ref
                    ))
            });
        v_flex()
            .gap_1p5()
            .child(row)
            .child(div().pl_6().child(list))
            .children(chain_method_note)
            .into_any_element()
    }

    fn render_stack_merge_progress(
        &self,
        run: &StackMergeRun,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total = run.steps.len();
        let merged_count = run
            .steps
            .iter()
            .filter(|s| {
                matches!(
                    run.statuses.get(&s.node.pr.number),
                    Some(StepStatus::Merged)
                )
            })
            .count();
        let (icon, color, headline) = if run.running {
            (
                IconName::Loader,
                tokens::pending(),
                format!(
                    "Merging stack ({} of {total})…",
                    (merged_count + 1).min(total)
                ),
            )
        } else if let Some(failed_pr) = run.failed_pr {
            (
                IconName::CircleX,
                tokens::closed(),
                format!("Stack merge stopped at #{failed_pr}"),
            )
        } else {
            let noun = if total == 1 {
                "pull request"
            } else {
                "pull requests"
            };
            (
                IconName::CircleCheck,
                tokens::open(),
                format!("Merged {total} {noun}"),
            )
        };
        let retry = (!run.running && run.can_retry && run.failed_pr.is_some()).then(|| {
            let failed_pr = run.failed_pr.unwrap();
            div()
                .flex_none()
                .debug_selector(|| "pr-stack-retry".to_owned())
                .child(
                    Button::new("pr-stack-retry")
                        .small()
                        .success()
                        .icon(IconName::RefreshCw)
                        .label(format!("Retry from #{failed_pr}"))
                        .on_click(cx.listener(|this, _, _, cx| this.retry_stack_merge(cx))),
                )
        });
        let close_btn = (!run.running).then(|| {
            let label = if run.failed_pr.is_some() {
                "Dismiss"
            } else {
                "Done"
            };
            Button::new("pr-stack-progress-done")
                .small()
                .ghost()
                .label(label)
                .on_click(cx.listener(|this, _, _, cx| this.dismiss_stack_merge_progress(cx)))
        });
        let header = h_flex()
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .flex_none()
                    .size_4()
                    .justify_center()
                    .child(Icon::new(icon).small().text_color(color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(scale::body())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(headline),
            )
            .children(close_btn)
            .children(retry);

        let list = v_flex()
            .id("pr-stack-progress-list")
            .debug_selector(|| "pr-stack-progress-list".to_owned())
            .role(Role::List)
            .aria_label("Stack merge progress")
            .gap_0p5()
            .children(run.steps.iter().enumerate().map(|(ix, step)| {
                let status = run
                    .statuses
                    .get(&step.node.pr.number)
                    .cloned()
                    .unwrap_or(StepStatus::Waiting);
                render_progress_step_row(ix + 1, step, &status, cx)
            }));

        v_flex()
            .gap_1p5()
            .child(header)
            .child(div().pl_6().child(list))
            .into_any_element()
    }

    /// The confirmation row: the question with Cancel and the commit at its
    /// trailing edge, and `details` under the question when it has any.
    fn render_confirm(
        &self,
        question: String,
        confirm_label: &'static str,
        commit: Commit,
        write: Write,
        details: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = self.write.is_busy();
        let (icon, color) = match commit {
            Commit::Merge => (IconName::GitMerge, tokens::merged()),
            Commit::Destructive => (IconName::TriangleAlert, tokens::pending()),
        };
        let confirm = Button::new("pr-confirm")
            .small()
            .label(confirm_label)
            .loading(busy)
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.start_write(write.clone(), cx);
            }));
        let confirm = match commit {
            Commit::Merge => confirm.success(),
            Commit::Destructive => confirm.danger(),
        };
        let row = h_flex()
            .gap_2()
            .items_center()
            .child(
                // A 16 px slot, so the details under the question start on
                // the question's edge.
                h_flex()
                    .flex_none()
                    .size_4()
                    .justify_center()
                    .child(Icon::new(icon).small().text_color(color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(scale::body())
                    .child(question),
            )
            .child(
                Button::new("pr-cancel")
                    .small()
                    .ghost()
                    .label("Cancel")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panel = Panel::Closed;
                        cx.notify();
                    })),
            )
            .child(confirm);
        v_flex()
            .gap_1()
            .child(row)
            .children(details.map(|details| div().pl_6().child(details)))
    }

    /// The review composer, a column at the trailing edge of the tab's
    /// content (`docked`) or a bar under it in a narrow window: what goes out with the review (files viewed, pending inline
    /// comments), the summary, and the verdict. A verdict that cannot go out
    /// yet is outlined and neutral, with the reason in its tooltip.
    pub(super) fn render_review_composer(
        &self,
        detail: &PullRequestDetail,
        docked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = tokens::border_subtle();
        let busy = self.write.is_busy();
        let may_approve = can_approve(detail);
        let pending = detail.pending_review.as_ref();
        let pending_count = pending.map_or(0, |review| review.comment_count);
        let body = self.review_body.read(cx).value().to_string();
        let blocker = |event: ReviewEvent| -> Option<&'static str> {
            match event {
                ReviewEvent::Approve if !may_approve => {
                    Some("You cannot approve your own pull request")
                }
                ReviewEvent::RequestChanges if !may_approve => {
                    Some("You cannot request changes on your own pull request")
                }
                _ => review_blocker(event, &body, pending_count),
            }
        };
        let verdict = |id: &'static str,
                       label: &'static str,
                       event: ReviewEvent,
                       style: fn(Button) -> Button,
                       tooltip: String,
                       cx: &mut Context<Self>| {
            let reason = blocker(event);
            let button = Button::new(id)
                .control()
                .disabled(busy || reason.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.submit_review(event, cx)));
            let button = match reason {
                // The kit's disabled fill keeps a trace of the variant's
                // color; a verdict that cannot go out reads as neutral.
                Some(reason) => button
                    .outline()
                    .accessibility_label(label)
                    .tooltip(reason)
                    .child(div().text_color(muted).child(label)),
                None => style(button.label(label)).tooltip(tooltip),
            };
            div()
                .flex_none()
                .debug_selector(move || id.to_owned())
                .child(button)
        };

        let meter = self.render_review_progress(cx);
        let (bar_meter, meter) = if docked { (None, meter) } else { (meter, None) };
        let header = h_flex()
            .flex_none()
            .h(scale::toolbar_height())
            .px(scale::content_inset())
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .id("pr-review-composer-title")
                    .role(Role::Heading)
                    .aria_label(format!("Review #{}", detail.pr.number))
                    .text_size(scale::title())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Review"),
            )
            .child(
                div()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(format!("#{}", detail.pr.number)),
            )
            .children(bar_meter)
            .child(div().flex_1())
            .child(
                ui::icon_button("pr-review-close", IconName::Close, "Close review")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.close_review_composer(cx))),
            );

        let viewed = docked.then(|| {
            v_flex()
                .gap_1()
                .child(ui::section_label("Viewed", cx))
                .child(match meter {
                    Some(meter) => h_flex().child(meter).into_any_element(),
                    None => {
                        ui::empty_line(IconName::Loader, "Counting files…", cx).into_any_element()
                    }
                })
        });

        let pending_body = match (self.panel, pending) {
            (Panel::ConfirmDiscard, Some(review)) => {
                let review_id = review.id.clone();
                v_flex()
                    .id("pr-discard-confirm")
                    .role(Role::Group)
                    .aria_label("Discard pending review")
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(
                                h_flex().flex_none().h_5().items_center().child(
                                    Icon::new(IconName::TriangleAlert)
                                        .small()
                                        .text_color(tokens::pending()),
                                ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(scale::body())
                                    .child(discard_confirm_text(review.comment_count)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("pr-discard-cancel")
                                    .small()
                                    .ghost()
                                    .label("Keep comments")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.panel = Panel::Review;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "pr-discard-confirm-button".to_owned())
                                    .child(
                                        Button::new("pr-discard-confirm-button")
                                            .small()
                                            .danger()
                                            .label("Discard review")
                                            .loading(busy)
                                            .disabled(busy)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.start_write(
                                                    Write::DiscardReview {
                                                        review_id: review_id.clone(),
                                                    },
                                                    cx,
                                                );
                                            })),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
            (_, Some(review)) => h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .id("pr-review-pending")
                        .debug_selector(|| "pr-review-pending".to_owned())
                        .role(Role::Status)
                        .aria_label(pending_comments_label(review.comment_count))
                        .flex_1()
                        .min_w_0()
                        .child(
                            widgets::tinted_tag(tokens::pending())
                                .child(pending_comments_label(review.comment_count)),
                        ),
                )
                .child(
                    Button::new("pr-discard-review")
                        .small()
                        .ghost()
                        .icon(IconName::Trash)
                        .label("Discard…")
                        .tooltip("Discard the pending review and its inline comments")
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.set_panel(Panel::ConfirmDiscard, cx)),
                        ),
                )
                .into_any_element(),
            (_, None) => div()
                .id("pr-review-pending")
                .debug_selector(|| "pr-review-pending".to_owned())
                .role(Role::Status)
                .aria_label(pending_comments_label(0))
                .text_size(scale::meta())
                .text_color(muted)
                .child(format!(
                    "{}. Comments you add on lines in Files go out with this review.",
                    pending_comments_label(0)
                ))
                .into_any_element(),
        };
        let pending_section = v_flex()
            .gap_1()
            .child(ui::section_label("Pending comments", cx))
            .child(pending_body);

        let summary = v_flex()
            .gap_1()
            .when(docked, |summary| {
                summary.child(ui::section_label("Summary", cx))
            })
            .child(
                Textarea::new(&self.review_body)
                    .aria_label(format!("Review summary for #{}", detail.pr.number))
                    .disabled(busy),
            )
            .child(ui::send_hint(cx).child("as a comment"));

        let error = self.write.error().map(|message| {
            div()
                .debug_selector(|| "pr-review-error".to_owned())
                .child(Alert::error("pr-review-error", message.to_owned()))
        });

        let status = if busy {
            "Submitting…".to_owned()
        } else if !may_approve {
            "You are the author, so you can only comment.".to_owned()
        } else {
            match pending_count {
                0 => "The summary goes out with your verdict.".to_owned(),
                count => format!(
                    "The summary and {} go out with your verdict.",
                    pending_comments_label(count).to_lowercase()
                ),
            }
        };
        let status = div()
            .id("pr-review-status")
            .role(Role::Status)
            .aria_label(status.clone())
            .text_size(scale::meta())
            .text_color(muted)
            .child(status);
        let verdicts = h_flex()
            .gap_2()
            .justify_end()
            .child(verdict(
                "pr-review-comment",
                "Comment",
                ReviewEvent::Comment,
                |button| button.outline(),
                format!("Comment ({})", ui::shortcut("cmd-enter")),
                cx,
            ))
            .child(verdict(
                "pr-review-changes",
                "Request changes",
                ReviewEvent::RequestChanges,
                |button| button.danger().outline(),
                "Request changes before this can merge".to_owned(),
                cx,
            ))
            .child(verdict(
                "pr-review-approve",
                "Approve",
                ReviewEvent::Approve,
                // The screen's one primary tone. Green is for the
                // merge alone.
                |button| button.primary(),
                "Approve these changes".to_owned(),
                cx,
            ));

        let composer = v_flex()
            .id("pr-review-composer")
            .debug_selector(|| "pr-review-composer".to_owned())
            .role(Role::Group)
            .aria_label(format!("Review #{}", detail.pr.number))
            .flex_none()
            .bg(tokens::panel())
            .border_color(border)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_review_composer(cx);
                }
            }))
            .child(header);
        if docked {
            return composer
                .w(scale::side_wide_width())
                .h_full()
                .border_l_1()
                .child(
                    v_flex()
                        .id("pr-review-composer-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p(scale::content_inset())
                        .gap(scale::section_gap())
                        .children(viewed)
                        .child(pending_section)
                        .child(summary)
                        .children(error),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .px(scale::content_inset())
                        .py_3()
                        .gap_2()
                        .border_t_1()
                        .border_color(border)
                        .child(status)
                        .child(verdicts),
                )
                .into_any_element();
        }
        // A narrow window: a bar under the tab's content, which gives up
        // height and keeps its width and every control.
        composer
            .w_full()
            .border_t_1()
            .child(
                h_flex()
                    .p(scale::content_inset())
                    .gap_4()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(summary))
                    .child(
                        v_flex()
                            .flex_none()
                            .w(scale::side_wide_width())
                            .gap_2()
                            .child(pending_section)
                            .children(error)
                            .child(status)
                            .child(verdicts),
                    ),
            )
            .into_any_element()
    }
}

/// The pull requests a stack merge lands, base first, one 28 px row each:
/// state, number, title, then what would make the merge go wrong (conflicts,
/// requested changes) as red chips, and the checks in a fixed lane at the
/// trailing edge, red when they fail.
fn render_stack_merge(stack: &StackMerge, cx: &App) -> AnyElement {
    v_flex()
        .id("pr-stack-merge")
        .role(Role::List)
        .aria_label(format!("{} pull requests to merge", stack.entries.len()))
        .children(
            stack
                .entries
                .iter()
                .map(|entry| render_stack_entry(entry, cx)),
        )
        .into_any_element()
}

fn render_stack_entry(entry: &StackEntry, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (muted, foreground) = (theme.muted_foreground, theme.foreground);
    let problems = stack_entry_problems(entry);
    let checks = entry.checks.map_or("No checks", widgets::checks_label);
    let mut name = format!(
        "#{} {}, {}, {}",
        entry.number,
        entry.title,
        widgets::state_label(entry.state, entry.is_draft).to_lowercase(),
        checks.to_lowercase()
    );
    for problem in &problems {
        name.push_str(", ");
        name.push_str(&problem.to_lowercase());
    }
    h_flex()
        .id(("pr-stack-entry", entry.number))
        .role(Role::ListItem)
        .aria_label(name)
        .h_7()
        .gap_2()
        .items_center()
        .child(widgets::state_icon(entry.state, entry.is_draft))
        // As in a list row, the number ends on one edge however many digits
        // it has, so the titles line up.
        .child(
            h_flex()
                .flex_none()
                .w_10()
                .justify_end()
                .text_size(scale::meta())
                .text_color(muted)
                .child(format!("#{}", entry.number)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(scale::body())
                .text_color(foreground)
                .child(entry.title.clone()),
        )
        .children(
            problems
                .into_iter()
                .map(|problem| ui::tinted_chip(tokens::closed()).child(problem)),
        )
        .child(
            h_flex()
                .flex_none()
                .w_32()
                .gap_1()
                .items_center()
                .text_size(scale::meta())
                .map(|lane| match entry.checks {
                    Some(state) => lane.child(widgets::checks_icon(state)).child(
                        div()
                            .text_color(if entry.checks_failing() {
                                widgets::checks_color(state)
                            } else {
                                muted
                            })
                            .child(widgets::checks_label(state)),
                    ),
                    None => lane.text_color(muted).child("No checks"),
                }),
        )
}

/// One PR row in the Merge stack confirmation list: step order, state,
/// `#number`, title, `→ target_base`, blockers (or Ready), and checks.
fn render_plan_step_row(order: usize, step: &StackMergeStep, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (muted, foreground) = (theme.muted_foreground, theme.foreground);
    let node = &step.node;
    let checks = node.checks.map_or("No checks", widgets::checks_label);
    let mut aria = format!(
        "{order}. #{} {} into {}, {}",
        node.pr.number,
        node.title,
        step.target_base,
        checks.to_lowercase()
    );
    for blocker in &step.blockers {
        aria.push_str(", ");
        aria.push_str(blocker.detail());
    }
    let step_num = node.pr.number;
    let ready = step.is_ready();
    h_flex()
        .id(("pr-stack-plan-step", step_num))
        .debug_selector(move || {
            if ready {
                format!("pr-stack-step-{step_num}:ready")
            } else {
                format!("pr-stack-step-{step_num}:blocked")
            }
        })
        .role(Role::ListItem)
        .aria_label(aria)
        .h_7()
        .gap_2()
        .items_center()
        .child(
            div()
                .flex_none()
                .w_4()
                .text_size(scale::meta())
                .text_color(muted)
                .child(format!("{order}.")),
        )
        .child(widgets::state_icon(node.state, node.is_draft))
        .child(
            h_flex()
                .flex_none()
                .w_10()
                .justify_end()
                .text_size(scale::meta())
                .text_color(muted)
                .child(format!("#{}", node.pr.number)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(scale::body())
                .text_color(foreground)
                .child(node.title.clone()),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_1()
                .items_center()
                .text_size(scale::meta())
                .text_color(muted)
                .child(Icon::new(IconName::ArrowRight).xsmall())
                .child(ui::branch_chip(step.target_base.clone(), cx)),
        )
        .children(
            step.blockers
                .iter()
                .map(|blocker| ui::tinted_chip(tokens::closed()).child(blocker.label())),
        )
        .when(step.is_ready(), |row| {
            row.child(ui::tinted_chip(tokens::open()).child("Ready"))
        })
        .child(
            h_flex()
                .flex_none()
                .w_32()
                .gap_1()
                .items_center()
                .text_size(scale::meta())
                .map(|lane| match node.checks {
                    Some(state) => lane.child(widgets::checks_icon(state)).child(
                        div()
                            .text_color(if state == ChecksState::Failing {
                                widgets::checks_color(state)
                            } else {
                                muted
                            })
                            .child(widgets::checks_label(state)),
                    ),
                    None => lane.text_color(muted).child("No checks"),
                }),
        )
}

/// One PR row in the live stack merge progress panel, with an inline error
/// message directly under the row when it failed.
fn render_progress_step_row(
    order: usize,
    step: &StackMergeStep,
    status: &StepStatus,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (muted, foreground) = (theme.muted_foreground, theme.foreground);
    let node = &step.node;
    let (icon, icon_color, label, state_key) = match status {
        StepStatus::Waiting => (IconName::CircleDashed, muted, "Waiting", "waiting"),
        StepStatus::Retargeting => (
            IconName::GitBranch,
            tokens::pending(),
            "Updating base…",
            "retargeting",
        ),
        StepStatus::WaitingForChecks => (
            IconName::CircleDashed,
            tokens::pending(),
            "Waiting for checks…",
            "waiting-checks",
        ),
        StepStatus::Merging => (IconName::Loader, tokens::pending(), "Merging…", "merging"),
        StepStatus::Merged => (IconName::GitMerge, tokens::merged(), "Merged", "merged"),
        StepStatus::Failed(_) => (IconName::CircleX, tokens::closed(), "Failed", "failed"),
    };
    let num = node.pr.number;
    let error = match status {
        StepStatus::Failed(reason) => Some(reason.clone()),
        _ => None,
    };
    v_flex()
        .id(("pr-stack-progress-step", num))
        .debug_selector(move || format!("pr-stack-progress-{num}:{state_key}"))
        .role(Role::ListItem)
        .aria_label(format!(
            "{order}. #{} {} into {}: {label}",
            num, node.title, step.target_base
        ))
        .gap_1()
        .child(
            h_flex()
                .h_7()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_none()
                        .w_4()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(format!("{order}.")),
                )
                .child(Icon::new(icon).small().text_color(icon_color))
                .child(
                    h_flex()
                        .flex_none()
                        .w_10()
                        .justify_end()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(format!("#{num}")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .text_color(foreground)
                        .child(node.title.clone()),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .items_center()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(Icon::new(IconName::ArrowRight).xsmall())
                        .child(ui::branch_chip(step.target_base.clone(), cx)),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .w(scale::dp(112.))
                        .justify_end()
                        .text_size(scale::meta())
                        .text_color(if matches!(status, StepStatus::Failed(_)) {
                            tokens::closed()
                        } else if matches!(status, StepStatus::Merged) {
                            tokens::merged()
                        } else {
                            muted
                        })
                        .child(label),
                ),
        )
        .when_some(error, |row, message| {
            row.child(div().pl_6().pb_1().child(Alert::error(
                SharedString::from(format!("pr-stack-error-{num}")),
                message,
            )))
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Bounds, Entity, Modifiers, TestAppContext, VisualTestContext,
        WindowBounds, WindowOptions, px, size,
    };
    use loupe_github::{Actor, PendingReview, PullRequestDetail, ReviewEvent};

    use super::super::model::PrTab;
    use super::super::{PullRequestView, Write, model};
    use super::OpenReviewComposer;
    use crate::github::common::Loadable;
    use crate::services::{Accounts, Services};
    use crate::store::Settings;

    /// The page of `detail` on its Conversation tab, 1600 px wide; GitHub
    /// is not connected, so every write lands in `sent_writes` only.
    fn open(
        detail: PullRequestDetail,
        cx: &mut TestAppContext,
    ) -> (Entity<PullRequestView>, VisualTestContext) {
        open_at(detail, px(1600.), cx)
    }

    /// [`open`] in a window `width` wide.
    fn open_at(
        detail: PullRequestDetail,
        width: gpui_kit::Pixels,
        cx: &mut TestAppContext,
    ) -> (Entity<PullRequestView>, VisualTestContext) {
        let detail = Arc::new(detail);
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
                        size(width, px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| PullRequestView::new(detail.pr.clone(), window, cx)),
            )
            .expect("open a window")
        });
        let mut cx = VisualTestContext::from_window(handle, cx);
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            view.incoming = Some(detail.clone());
            view.detail = Loadable::Loaded(detail);
            cx.notify();
        });
        draw(&mut cx);
        (view, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    /// Opens the composer the way the Guide does: the action, dispatched
    /// from inside the page.
    fn open_composer(view: &Entity<PullRequestView>, cx: &mut VisualTestContext) {
        view.update_in(cx, |view, window, cx| view.focus.focus(window, cx));
        cx.dispatch_action(OpenReviewComposer);
        draw(cx);
    }

    fn type_summary(view: &Entity<PullRequestView>, text: &str, cx: &mut VisualTestContext) {
        view.update_in(cx, |view, window, cx| {
            view.review_body
                .update(cx, |body, cx| body.set_value(text.to_owned(), window, cx));
        });
        draw(cx);
    }

    fn click(selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is on screen"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        draw(cx);
    }

    fn sent(view: &Entity<PullRequestView>, cx: &mut VisualTestContext) -> Vec<Write> {
        view.read_with(cx, |view, _| view.sent_writes.clone())
    }

    fn with_pending(detail: PullRequestDetail) -> PullRequestDetail {
        PullRequestDetail {
            pending_review: Some(PendingReview {
                id: "PRR_9".into(),
                comment_count: 2,
            }),
            ..detail
        }
    }

    #[gpui_kit::test]
    fn the_composer_opens_without_moving_the_tabs_or_the_page(cx: &mut TestAppContext) {
        let (view, mut cx) = open(model::tests::detail(), cx);
        let before = cx.debug_bounds("merge-box-line").expect("the merge box");
        assert!(cx.debug_bounds("pr-review-composer").is_none());

        open_composer(&view, &mut cx);
        let composer = cx
            .debug_bounds("pr-review-composer")
            .expect("the composer opens");
        let after = cx.debug_bounds("merge-box-line").expect("the merge box");
        assert_eq!(before.origin.y, after.origin.y, "nothing moves down");
        assert!(
            composer.origin.y >= after.origin.y - px(80.),
            "the composer starts under the tabs, not above them"
        );
        assert!(
            composer.origin.x > after.origin.x,
            "it is a column at the trailing edge"
        );
    }

    /// The composer as a narrow window shows it: a bar under the tab's
    /// content that covers none of it, whatever the tab.
    fn assert_bar_covers_nothing(width: f32, tab: PrTab, cx: &mut TestAppContext) {
        let (view, mut cx) = open_at(model::tests::detail(), px(width), cx);
        view.update(&mut cx, |view, cx| {
            view.tab = tab;
            cx.notify();
        });
        draw(&mut cx);
        let before = cx
            .debug_bounds("pr-tab-content")
            .expect("the tab's content");

        click("pr-review", &mut cx);
        let composer = cx
            .debug_bounds("pr-review-composer")
            .unwrap_or_else(|| panic!("the composer opens on {tab:?} at {width}"));
        let after = cx
            .debug_bounds("pr-tab-content")
            .expect("the tab's content");
        assert_eq!(
            before.size.width, after.size.width,
            "{tab:?} keeps its width"
        );
        assert!(
            composer.origin.y >= after.origin.y + after.size.height,
            "{tab:?}: the composer sits under the content, not over it"
        );
        assert!(after.size.height > px(200.), "{tab:?} keeps room to read");
    }

    #[gpui_kit::test]
    fn on_a_laptop_the_composer_is_a_bar_under_every_tab(cx: &mut TestAppContext) {
        for width in [1280., 1440.] {
            for tab in PrTab::ALL {
                assert_bar_covers_nothing(width, tab, cx);
            }
        }
    }

    #[gpui_kit::test]
    fn on_a_laptop_the_conversation_actions_stay_clickable(cx: &mut TestAppContext) {
        for width in [1280., 1440.] {
            let (view, mut cx) = open_at(model::tests::detail(), px(width), cx);
            open_composer(&view, &mut cx);
            let composer = cx.debug_bounds("pr-review-composer").expect("the composer");
            let content = cx.debug_bounds("pr-tab-content").expect("the content");
            for action in ["merge-box-merge", "merge-box-request-review"] {
                let bounds = cx
                    .debug_bounds(action)
                    .unwrap_or_else(|| panic!("{action} at {width}"));
                assert!(
                    bounds.origin.x + bounds.size.width <= content.origin.x + content.size.width
                        && bounds.origin.y + bounds.size.height <= composer.origin.y,
                    "{action} is whole and clear of the composer at {width}"
                );
            }
            let merge = cx.debug_bounds("merge-box-merge").expect("Merge");
            let point = gpui_kit::point(
                merge.origin.x + merge.size.width / 4.,
                merge.origin.y + merge.size.height / 2.,
            );
            cx.simulate_click(point, Modifiers::none());
            draw(&mut cx);
            assert!(
                view.read_with(&cx, |view, _| matches!(
                    view.panel,
                    super::Panel::ConfirmMerge(_)
                )),
                "the click reaches Merge at {width}"
            );
        }
    }

    #[gpui_kit::test]
    fn a_wide_window_docks_the_composer_beside_the_files_tab(cx: &mut TestAppContext) {
        let (view, mut cx) = open_at(model::tests::detail(), px(1920.), cx);
        view.update(&mut cx, |view, cx| {
            view.tab = PrTab::Files;
            cx.notify();
        });
        draw(&mut cx);
        open_composer(&view, &mut cx);
        let composer = cx.debug_bounds("pr-review-composer").expect("the composer");
        let content = cx
            .debug_bounds("pr-tab-content")
            .expect("the tab's content");
        assert!(composer.origin.x >= content.origin.x + content.size.width);
    }

    #[gpui_kit::test]
    fn the_review_button_never_closes_the_composer(cx: &mut TestAppContext) {
        let (view, mut cx) = open(with_pending(model::tests::detail()), cx);
        open_composer(&view, &mut cx);
        click("pr-review", &mut cx);
        assert!(
            cx.debug_bounds("pr-review-composer").is_some(),
            "Finish review keeps it open"
        );
    }

    #[gpui_kit::test]
    fn approve_sends_the_summary_and_the_pending_comments(cx: &mut TestAppContext) {
        let (view, mut cx) = open(with_pending(model::tests::detail()), cx);
        open_composer(&view, &mut cx);
        assert!(cx.debug_bounds("pr-review-pending").is_some());
        type_summary(&view, "Ship it", &mut cx);

        click("pr-review-approve", &mut cx);
        let writes = sent(&view, &mut cx);
        assert!(
            matches!(
                writes.as_slice(),
                [Write::Review { event: ReviewEvent::Approve, body, review_id: Some(id) }]
                    if body == "Ship it" && id == "PRR_9"
            ),
            "{writes:?}"
        );
    }

    #[gpui_kit::test]
    fn approve_needs_no_words(cx: &mut TestAppContext) {
        let (view, mut cx) = open(model::tests::detail(), cx);
        open_composer(&view, &mut cx);

        click("pr-review-approve", &mut cx);
        let writes = sent(&view, &mut cx);
        assert!(
            matches!(
                writes.as_slice(),
                [Write::Review {
                    event: ReviewEvent::Approve,
                    review_id: None,
                    ..
                }]
            ),
            "{writes:?}"
        );
    }

    #[gpui_kit::test]
    fn request_changes_waits_for_the_reason(cx: &mut TestAppContext) {
        let (view, mut cx) = open(model::tests::detail(), cx);
        open_composer(&view, &mut cx);

        click("pr-review-changes", &mut cx);
        assert!(
            sent(&view, &mut cx).is_empty(),
            "nothing goes out without words"
        );

        type_summary(&view, "Handle the empty list", &mut cx);
        click("pr-review-changes", &mut cx);
        let writes = sent(&view, &mut cx);
        assert!(
            matches!(
                writes.as_slice(),
                [Write::Review { event: ReviewEvent::RequestChanges, body, .. }]
                    if body == "Handle the empty list"
            ),
            "{writes:?}"
        );
    }

    #[gpui_kit::test]
    fn the_author_can_only_comment(cx: &mut TestAppContext) {
        let mut own = model::tests::detail();
        own.author = Some(Actor {
            login: own.viewer_login.clone(),
            avatar_url: String::new(),
        });
        let (view, mut cx) = open(own, cx);
        open_composer(&view, &mut cx);
        type_summary(&view, "Note to self", &mut cx);

        click("pr-review-approve", &mut cx);
        click("pr-review-changes", &mut cx);
        assert!(
            sent(&view, &mut cx).is_empty(),
            "both verdicts are disabled"
        );

        click("pr-review-comment", &mut cx);
        let writes = sent(&view, &mut cx);
        assert!(
            matches!(
                writes.as_slice(),
                [Write::Review {
                    event: ReviewEvent::Comment,
                    ..
                }]
            ),
            "{writes:?}"
        );
    }

    #[gpui_kit::test]
    fn discarding_asks_first_inside_the_composer(cx: &mut TestAppContext) {
        let (view, mut cx) = open(with_pending(model::tests::detail()), cx);
        open_composer(&view, &mut cx);
        view.update(&mut cx, |view, cx| {
            view.set_panel(super::Panel::ConfirmDiscard, cx)
        });
        draw(&mut cx);
        assert!(
            cx.debug_bounds("pr-review-composer").is_some(),
            "the composer stays"
        );

        click("pr-discard-confirm-button", &mut cx);
        let writes = sent(&view, &mut cx);
        assert!(
            matches!(writes.as_slice(), [Write::DiscardReview { review_id }] if review_id == "PRR_9"),
            "{writes:?}"
        );
    }
}
