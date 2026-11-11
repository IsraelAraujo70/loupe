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
use loupe_github::{MergeMethod, PullRequestDetail, ReviewEvent};

use super::model::{
    DraftAction, Panel, PrActions, StateAction, can_approve, discard_confirm_text,
    merge_button_label, merge_confirm_text, pending_comments_label, resolve_method,
};
use super::{PullRequestView, Write};
use crate::github::widgets;
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
    /// The band under the meta line: the open panel and the last write error.
    pub(super) fn render_panel_band(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = cx.theme().border;
        let surface = cx.theme().group_box;
        let error = self.write.error().map(str::to_owned);
        v_flex()
            .flex_none()
            .when(
                !matches!(self.panel, Panel::Closed | Panel::ConfirmMerge(_)),
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

    /// The "More actions" menu: edit the title, the repository's pull
    /// requests, refresh, and the less used state changes (draft, close,
    /// reopen). Close is the one item in the danger color, and only here.
    pub(super) fn render_action_controls(
        &self,
        actions: &PrActions,
        detail: &PullRequestDetail,
        can_edit: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (draft, state) = (actions.draft, actions.state);
        let this = cx.entity().downgrade();
        let title = detail.title.clone();
        let repo = detail.pr.repo.clone();
        ui::outline_icon_button("pr-more", IconName::Ellipsis, "More actions")
            .loading(self.detail.is_busy())
            .disabled(self.write.is_busy())
            .selected(self.panel == Panel::ConfirmClose)
            .menu_trigger()
            // The button is at the header's trailing edge: the menu's
            // trailing edge sits under it.
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
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
                                this.update(cx, |this, cx| this.reload(cx)).ok();
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(selected) = resolve_method(methods, self.merge_method) else {
            return div().into_any_element();
        };
        let confirming = matches!(self.panel, Panel::ConfirmMerge(_));
        // The button names the action; the method it will use is in its
        // tooltip and in the menu behind the caret.
        let merge = Button::new("pr-merge")
            .success()
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
                        Button::new("pr-merge-method")
                            .success()
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
                                                    if matches!(this.panel, Panel::ConfirmMerge(_))
                                                    {
                                                        this.panel = Panel::ConfirmMerge(method);
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
                    cx,
                )
                .into_any_element()
            }
            Panel::ConfirmMerge(method) => self
                .render_confirm(
                    merge_confirm_text(detail.pr.number, &detail.base_ref, method),
                    "Merge",
                    Commit::Merge,
                    Write::Merge(method),
                    cx,
                )
                .into_any_element(),
            Panel::ConfirmClose => self
                .render_confirm(
                    format!("Close #{} without merging?", detail.pr.number),
                    "Close pull request",
                    Commit::Destructive,
                    Write::Close,
                    cx,
                )
                .into_any_element(),
        }
    }

    fn render_confirm(
        &self,
        question: String,
        confirm_label: &'static str,
        commit: Commit,
        write: Write,
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
        h_flex()
            .gap_2()
            .items_center()
            .child(Icon::new(icon).small().text_color(color))
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
            .child(confirm)
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
                                "Submitting…"
                            } else if may_approve {
                                "⌘↵ submits as a comment."
                            } else {
                                "You are the author, so you can only comment."
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
