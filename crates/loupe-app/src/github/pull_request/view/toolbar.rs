//! The review and state actions of the meta line, and the band under it
//! where the review form, a confirmation, or a write error opens.

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
    ChecksState, MergeMethod, PullRequestDetail, ReviewEvent, StackEntry, StackGraph,
    StackMergePlan, StackMergeStep, StepStatus,
};

use super::header::OverflowItems;
use super::model::{
    DraftAction, Panel, PrActions, PrTab, StackMerge, StateAction, can_approve,
    discard_confirm_text, merge_button_label, merge_confirm_text, pending_comments_label,
    ready_subset_button_label, resolve_method, stack_confirm_headline, stack_entry_problems,
    stack_merge, stack_merge_button_label, stack_merge_confirm_text,
};
use super::{PullRequestView, StackMergeRun, Write};
use crate::github::widgets;
use crate::shell::Refreshable as _;
use crate::theme::{scale, tokens};
use crate::ui;
use crate::ui::{ControlSize as _, MenuTriggerExt as _};

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
        // Conversation tab shows; elsewhere its error comes here.
        let error = self
            .write
            .error()
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
    /// secondary action once the merge button is on screen.
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
                    .selected(self.panel == Panel::Review)
                    .on_click(cx.listener(|this, _, _, cx| this.set_panel(Panel::Review, cx))),
            )
            .when(pending.is_some(), |row| {
                row.child(
                    ui::icon_button(
                        "pr-discard-review",
                        IconName::Trash,
                        "Discard pending review…",
                    )
                    .selected(self.panel == Panel::ConfirmDiscard)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_panel(Panel::ConfirmDiscard, cx)),
                    ),
                )
            })
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
            Panel::Closed => div().into_any_element(),
            Panel::Review => self.render_review_panel(detail, cx).into_any_element(),
            Panel::ConfirmDiscard => {
                let count = detail
                    .pending_review
                    .as_ref()
                    .map_or(0, |r| r.comment_count);
                let Some(review_id) = detail.pending_review.as_ref().map(|r| r.id.clone()) else {
                    return div().into_any_element();
                };
                self.render_confirm(
                    discard_confirm_text(count),
                    "Discard review",
                    Commit::Destructive,
                    Write::DiscardReview { review_id },
                    None,
                    cx,
                )
                .into_any_element()
            }
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

    fn render_review_panel(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let busy = self.write.is_busy();
        let may_approve = can_approve(detail);
        let submit = |id: &'static str,
                      label: &'static str,
                      event: ReviewEvent,
                      allowed: bool,
                      cx: &mut Context<Self>| {
            Button::new(id)
                .small()
                .label(label)
                .disabled(busy || !allowed)
                .when(!allowed, |button| {
                    button.tooltip("You cannot approve or request changes on your own pull request")
                })
                .on_click(cx.listener(move |this, _, _, cx| this.submit_review(event, cx)))
        };
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(scale::meta())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Review summary"),
            )
            .child(Textarea::new(&self.review_body).disabled(busy))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .child(if busy {
                                "Submitting…".to_owned()
                            } else if may_approve {
                                format!("{} submits as a comment.", ui::shortcut("cmd-enter"))
                            } else {
                                "You are the author, so you can only comment.".to_owned()
                            }),
                    )
                    .child(
                        Button::new("pr-review-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel = Panel::Closed;
                                cx.notify();
                            })),
                    )
                    .child(
                        submit(
                            "pr-review-comment",
                            "Comment",
                            ReviewEvent::Comment,
                            true,
                            cx,
                        )
                        .outline(),
                    )
                    .child(
                        submit(
                            "pr-review-changes",
                            "Request changes",
                            ReviewEvent::RequestChanges,
                            may_approve,
                            cx,
                        )
                        .danger(),
                    )
                    .child(
                        submit(
                            "pr-review-approve",
                            "Approve",
                            ReviewEvent::Approve,
                            may_approve,
                            cx,
                        )
                        // The screen's one primary tone. Green is for the
                        // merge alone.
                        .primary(),
                    ),
            )
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
