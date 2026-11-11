//! One inline review thread as the diff draws it: comments (the viewer's own
//! editable in place), a reply box, and Resolve / Unresolve. Each thread is an
//! entity so its drafts and its expanded state survive the parent reloading
//! the pull request.

use chrono::{DateTime, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Enter, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::suggestion::{
    BodyPart, SuggestionCommit, split_suggestions, suggested_lines_original,
};
use loupe_github::{NodeId, PullRequestDetail, RepoRef, ReviewComment, ReviewThread, Side};

use super::files::resolved_summary;
use crate::github::common::{self, Submit};
use crate::github::widgets;
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

/// A write on the thread succeeded; the pull request needs a reload.
pub enum ThreadEvent {
    Wrote,
}

/// Where a suggestion commits: the head branch at the loaded head commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadRef {
    repo: RepoRef,
    branch: String,
    oid: String,
}

impl HeadRef {
    pub fn of(detail: &PullRequestDetail) -> Self {
        Self {
            repo: detail
                .head_repo
                .clone()
                .unwrap_or_else(|| detail.pr.repo.clone()),
            branch: detail.head_ref.clone(),
            oid: detail.head_oid.clone(),
        }
    }
}

/// One comment of the thread being edited in place.
struct CommentEdit {
    id: NodeId,
    text: Entity<TextareaState>,
    submit: Submit,
    task: Option<Task<()>>,
}

pub struct ThreadView {
    thread: ReviewThread,
    head: HeadRef,
    suggestion_submit: Submit,
    suggestion_task: Option<Task<()>>,
    /// Shown above the comments for threads drawn under a file header.
    label: Option<String>,
    /// The user's choice; `None` follows the thread (resolved is collapsed).
    expanded: Option<bool>,
    reply: Entity<TextareaState>,
    reply_submit: Submit,
    resolve_submit: Submit,
    clear_reply: bool,
    reply_task: Option<Task<()>>,
    resolve_task: Option<Task<()>>,
    edit: Option<CommentEdit>,
    _reply_changes: Subscription,
}

impl EventEmitter<ThreadEvent> for ThreadView {}

impl ThreadView {
    pub fn new(
        thread: ReviewThread,
        label: Option<String>,
        head: HeadRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let reply = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder("Reply…")
        });
        let changes = cx.subscribe_in(&reply, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            thread,
            head,
            suggestion_submit: Submit::Idle,
            suggestion_task: None,
            label,
            expanded: None,
            reply,
            reply_submit: Submit::Idle,
            resolve_submit: Submit::Idle,
            clear_reply: false,
            reply_task: None,
            resolve_task: None,
            edit: None,
            _reply_changes: changes,
        }
    }

    /// Takes the reloaded thread. A change of the resolved state resets the
    /// expanded choice, so a freshly resolved thread collapses. An edit of a
    /// comment the thread no longer has (a discarded pending review, a
    /// deletion on github.com) is dropped, or it would hide every Edit button.
    pub fn update(
        &mut self,
        thread: ReviewThread,
        label: Option<String>,
        head: HeadRef,
        cx: &mut Context<Self>,
    ) {
        if self.thread == thread && self.label == label && self.head == head {
            return;
        }
        self.head = head;
        if thread.is_resolved != self.thread.is_resolved {
            self.expanded = None;
        }
        if self
            .edit
            .as_ref()
            .is_some_and(|edit| !thread.comments.iter().any(|comment| comment.id == edit.id))
        {
            self.edit = None;
        }
        self.thread = thread;
        self.label = label;
        cx.notify();
    }

    fn reply_is_empty(&self, cx: &App) -> bool {
        self.reply.read(cx).value().trim().is_empty()
    }

    fn submit_reply(&mut self, cx: &mut Context<Self>) {
        let body = self.reply.read(cx).value().trim_end().to_owned();
        if body.is_empty() || self.reply_submit.is_busy() || !self.thread.viewer_can_reply {
            return;
        }
        self.reply_submit = Submit::Busy;
        cx.notify();
        let thread_id = self.thread.id.clone();
        self.reply_task = Some(common::run(
            cx,
            move |gh| gh.reply_to_thread(&thread_id, &body),
            |this, result, cx| match result {
                Ok(()) => {
                    this.reply_submit = Submit::Idle;
                    this.clear_reply = true;
                    cx.emit(ThreadEvent::Wrote);
                }
                Err(error) => this.reply_submit = Submit::Failed(error.detail()),
            },
        ));
    }

    fn start_edit(&mut self, comment: &ReviewComment, window: &mut Window, cx: &mut Context<Self>) {
        let body = comment.body.clone();
        let text = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(2, 12);
            state.set_value(body, window, cx);
            state
        });
        text.update(cx, |text, cx| text.focus(window, cx));
        self.edit = Some(CommentEdit {
            id: comment.id.clone(),
            text,
            submit: Submit::Idle,
            task: None,
        });
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &mut self.edit else {
            return;
        };
        let body = edit.text.read(cx).value().trim_end().to_owned();
        if body.trim().is_empty() || edit.submit.is_busy() {
            return;
        }
        edit.submit = Submit::Busy;
        let comment_id = edit.id.clone();
        edit.task = Some(common::run(
            cx,
            move |gh| gh.edit_review_comment(&comment_id, &body),
            |this, result, cx| match result {
                Ok(()) => {
                    this.edit = None;
                    cx.emit(ThreadEvent::Wrote);
                }
                Err(error) => {
                    if let Some(edit) = &mut this.edit {
                        edit.submit = Submit::Failed(error.detail());
                    }
                }
            },
        ));
        cx.notify();
    }

    /// The lines a suggestion in this thread replaces, `(start, end)`; `None`
    /// when it cannot be committed (outdated, or on the old side).
    fn suggestion_range(&self) -> Option<(u32, u32)> {
        if self.thread.is_outdated || self.thread.side != Side::Right {
            return None;
        }
        let end = self.thread.line?;
        Some((self.thread.start_line.unwrap_or(end), end))
    }

    fn commit_suggestion(&mut self, replacement: String, cx: &mut Context<Self>) {
        let Some((start, end)) = self.suggestion_range() else {
            return;
        };
        if self.suggestion_submit.is_busy() {
            return;
        }
        let change = SuggestionCommit {
            repo: self.head.repo.clone(),
            branch: self.head.branch.clone(),
            head_oid: self.head.oid.clone(),
            path: self.thread.path.clone(),
            start,
            end,
            replacement,
            message: format!("Apply suggestion to {}", self.thread.path),
        };
        self.suggestion_submit = Submit::Busy;
        self.suggestion_task = Some(common::run(
            cx,
            move |gh| gh.commit_suggestion(&change),
            |this, result, cx| match result {
                Ok(()) => {
                    this.suggestion_submit = Submit::Idle;
                    cx.emit(ThreadEvent::Wrote);
                }
                Err(error) => this.suggestion_submit = Submit::Failed(error.detail()),
            },
        ));
        cx.notify();
    }

    /// A ```suggestion block: the lines it replaces, in red, then the
    /// suggested ones, in green, with "Commit suggestion" when it applies.
    fn suggestion_block(
        &self,
        comment: &ReviewComment,
        index: usize,
        suggested: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let range = self.suggestion_range();
        let count = range.map_or(1, |(start, end)| (end - start + 1) as usize);
        let original = suggested_lines_original(&comment.diff_hunk, count);
        let busy = self.suggestion_submit.is_busy();
        let line = |sign: &'static str, text: String, bg: Hsla, color: Hsla| {
            div()
                .px_2()
                .bg(bg)
                .text_color(color)
                .child(format!("{sign} {text}"))
        };
        let replacement = suggested.to_owned();
        v_flex()
            .w_full()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(scale::meta())
                    .text_color(theme.muted_foreground)
                    .child("Suggested change")
                    .child(div().flex_1())
                    .when(range.is_some() && !comment.is_pending, |row| {
                        row.child(
                            Button::new(SharedString::from(format!(
                                "commit-suggestion-{}-{index}",
                                comment.id
                            )))
                            .outline()
                            .xsmall()
                            .label("Commit suggestion")
                            .loading(busy)
                            .disabled(busy)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.commit_suggestion(replacement.clone(), cx)
                                },
                            )),
                        )
                    }),
            )
            .child(
                v_flex()
                    .py_1()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(scale::code())
                    .children(original.into_iter().map(|text| {
                        line(
                            "-",
                            text,
                            tokens::diff_removed_bg(),
                            tokens::diff_removed_text(),
                        )
                    }))
                    .children(suggested.lines().map(|text| {
                        line(
                            "+",
                            text.to_owned(),
                            tokens::diff_added_bg(),
                            tokens::diff_added_text(),
                        )
                    })),
            )
            .into_any_element()
    }

    fn set_resolved(&mut self, resolved: bool, cx: &mut Context<Self>) {
        if self.resolve_submit.is_busy() {
            return;
        }
        self.resolve_submit = Submit::Busy;
        cx.notify();
        let thread_id = self.thread.id.clone();
        self.resolve_task = Some(common::run(
            cx,
            move |gh| {
                if resolved {
                    gh.resolve_thread(&thread_id)
                } else {
                    gh.unresolve_thread(&thread_id)
                }
            },
            |this, result, cx| match result {
                Ok(()) => {
                    this.resolve_submit = Submit::Idle;
                    cx.emit(ThreadEvent::Wrote);
                }
                Err(error) => this.resolve_submit = Submit::Failed(error.detail()),
            },
        ));
    }

    fn collapsed_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .id("thread-collapsed")
            .role(Role::Button)
            .aria_label(format!("Expand: {}", resolved_summary(&self.thread)))
            .gap_2()
            .items_center()
            .text_size(scale::meta())
            .text_color(muted)
            .child(
                Icon::new(IconName::CircleCheck)
                    .small()
                    .text_color(tokens::open()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(resolved_summary(&self.thread)),
            )
            .child(Icon::new(IconName::ChevronDown).xsmall().text_color(muted))
            .on_click(cx.listener(|this, _, _, cx| {
                this.expanded = Some(true);
                cx.notify();
            }))
            .into_any_element()
    }

    fn reply_box(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .key_context("ThreadReply")
            .capture_action(cx.listener(|this, action: &Enter, _, cx| {
                if action.secondary {
                    cx.stop_propagation();
                    this.submit_reply(cx);
                }
            }))
            .child(Textarea::new(&self.reply).disabled(self.reply_submit.is_busy()))
            .into_any_element()
    }

    fn actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let thread = &self.thread;
        let can_toggle = if thread.is_resolved {
            thread.viewer_can_unresolve
        } else {
            thread.viewer_can_resolve
        };
        let reply_busy = self.reply_submit.is_busy();
        let resolve_busy = self.resolve_submit.is_busy();
        let reply_disabled = reply_busy || self.reply_is_empty(cx);
        let resolved = thread.is_resolved;
        h_flex()
            .gap_2()
            .items_center()
            .when(thread.viewer_can_reply, |this| {
                this.child(
                    Button::new("reply")
                        .outline()
                        .small()
                        .icon(IconName::Reply)
                        .label("Reply")
                        .tooltip("Reply (⌘↵)")
                        .loading(reply_busy)
                        .disabled(reply_disabled)
                        .on_click(cx.listener(|this, _, _, cx| this.submit_reply(cx))),
                )
            })
            .when(can_toggle, |this| {
                this.child(
                    Button::new("resolve")
                        .ghost()
                        .small()
                        .icon(if resolved {
                            IconName::RotateCcw
                        } else {
                            IconName::Check
                        })
                        .label(if resolved {
                            "Unresolve conversation"
                        } else {
                            "Resolve conversation"
                        })
                        .loading(resolve_busy)
                        .disabled(resolve_busy)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_resolved(!resolved, cx)),
                        ),
                )
            })
            .when(resolved, |this| {
                this.child(
                    ui::icon_button("collapse", IconName::ChevronUp, "Collapse")
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.expanded = Some(false);
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    /// One comment: avatar, name, age, a Pending tag and, for the viewer's own
    /// comments, Edit on one line; the body or its editor under it, aligned
    /// with the name.
    fn comment_row(
        &self,
        comment: &ReviewComment,
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (login, avatar) = match &comment.author {
            Some(author) => (author.login.clone(), author.avatar_url.clone()),
            None => ("ghost".to_owned(), String::new()),
        };
        let id = comment.id.clone();
        let editing = self.edit.as_ref().filter(|edit| edit.id == id);
        let header =
            h_flex()
                .h_5()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(scale::body())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(login.clone()),
                )
                .child(
                    div()
                        .text_size(scale::meta())
                        .text_color(cx.theme().muted_foreground)
                        .child(ui::relative_time(comment.created_at, now)),
                )
                .when(comment.is_pending, |this| {
                    this.child(widgets::tinted_tag(tokens::pending()).child("Pending"))
                })
                .when(comment.viewer_can_update && self.edit.is_none(), |this| {
                    let comment = comment.clone();
                    this.child(div().flex_1()).child(
                        ui::icon_button(
                            SharedString::from(format!("edit-{id}")),
                            IconName::Pencil,
                            "Edit comment",
                        )
                        .xsmall()
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.start_edit(&comment, window, cx),
                        )),
                    )
                });
        let body = match editing {
            Some(edit) => {
                let busy = edit.submit.is_busy();
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .key_context("CommentEdit")
                            .capture_action(cx.listener(|this, action: &Enter, _, cx| {
                                if action.secondary {
                                    cx.stop_propagation();
                                    this.save_edit(cx);
                                }
                            }))
                            .child(Textarea::new(&edit.text).disabled(busy)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .children(edit.submit.error().map(|message| {
                                div()
                                    .flex_1()
                                    .text_size(scale::meta())
                                    .text_color(cx.theme().danger)
                                    .child(message.to_owned())
                            }))
                            .child(
                                Button::new(SharedString::from(format!("cancel-{id}")))
                                    .ghost()
                                    .small()
                                    .label("Cancel")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.edit = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("save-{id}")))
                                    .outline()
                                    .small()
                                    .label("Save")
                                    .tooltip("Save (⌘↵)")
                                    .loading(busy)
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))),
                            ),
                    )
                    .into_any_element()
            }
            None => v_flex()
                .w_full()
                .gap_2()
                .children(
                    split_suggestions(&comment.body)
                        .into_iter()
                        .enumerate()
                        .map(|(index, part)| match part {
                            BodyPart::Markdown(text) => crate::ui::markdown(
                                SharedString::from(format!("body-{id}-{index}")),
                                text,
                            )
                            .into_any_element(),
                            BodyPart::Suggestion(suggested) => {
                                self.suggestion_block(comment, index, &suggested, cx)
                            }
                        }),
                )
                .into_any_element(),
        };
        h_flex()
            .w_full()
            .gap_2()
            .items_start()
            .child(ui::avatar_named(avatar, login, dp(20.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(header)
                    .child(body),
            )
            .into_any_element()
    }
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_reply {
            self.clear_reply = false;
            self.reply
                .update(cx, |reply, cx| reply.set_value("", window, cx));
        }
        if !self.expanded.unwrap_or(!self.thread.is_resolved) {
            return self.collapsed_row(cx);
        }
        let now = Utc::now();
        let errors = [
            self.reply_submit.error(),
            self.resolve_submit.error(),
            self.suggestion_submit.error(),
        ]
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        v_flex()
            .w_full()
            .gap_3()
            .when_some(self.label.clone(), |this, label| {
                this.child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(scale::code())
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
            })
            .children(
                self.thread
                    .comments
                    .iter()
                    .map(|comment| self.comment_row(comment, now, cx)),
            )
            .when(self.thread.viewer_can_reply, |this| {
                this.child(self.reply_box(cx))
            })
            .child(self.actions(cx))
            .children(errors.into_iter().map(|message| {
                div()
                    .text_size(scale::meta())
                    .text_color(cx.theme().danger)
                    .child(message)
            }))
            .into_any_element()
    }
}
