//! The conversation tab: the PR body, then the Comments section: its filter
//! (All, Unresolved, Mine) and sort menu, the timeline of comments and
//! reviews interleaved with review threads, and the new-comment box on top or
//! at the end. The order and the box's place are saved in `Settings` for
//! every pull request; the filter is per tab.

mod fold;
mod model;
mod segments;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Enter, InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{
    Actor, BodyEdit, IssueComment, NodeId, PullRequestDetail, Review, ReviewState, ReviewThread,
};

use self::model::{
    CommentFilter, Entry, Format, HunkLine, HunkLineKind, format_edit, hunk_tail, thread_label,
    visible_entries,
};
use super::tab::{PrTabContext, PrTabEvent};
use super::thread::{HeadRef, ThreadEvent, ThreadView};
use crate::github::common::{self, Submit};
use crate::github::highlight::{highlight_lines, language_for_path};
use crate::github::widgets;
use crate::services::{Accounts, Connection};
use crate::store::{self, CommentBoxPlace, CommentOrder, Settings};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;
use crate::ui::MenuTriggerExt as _;

/// One line of a thread's code context with its syntax runs.
type SnippetLine = (HunkLine, Vec<(Range<usize>, HighlightStyle)>);
/// The widest a description's text runs, for comfortable reading.
use crate::ui::READING_WIDTH;
/// Avatar size in the timeline gutter.
const GUTTER_AVATAR: f32 = 28.;

struct ThreadSlot {
    view: Entity<ThreadView>,
    /// The commented lines with their syntax runs, highlighted once per sync.
    snippet: Vec<SnippetLine>,
    _events: Subscription,
}

/// An issue comment being edited in place.
struct Edit {
    text: Entity<TextareaState>,
    submit: Submit,
    task: Option<Task<()>>,
}

pub struct ConversationTab {
    context: PrTabContext,
    threads: HashMap<NodeId, ThreadSlot>,
    edits: HashMap<NodeId, Edit>,
    /// The pull request description being edited.
    body_edit: Option<Edit>,
    new_comment: Entity<TextareaState>,
    comment_submit: Submit,
    comment_task: Option<Task<()>>,
    clear_comment: bool,
    /// The new-comment box shows the rendered Markdown instead of the editor.
    preview: bool,
    filter: CommentFilter,
    scroll: ScrollHandle,
    folds: fold::Folds,
    _comment_changes: Subscription,
    _theme_changes: Subscription,
}

impl ConversationTab {
    pub fn new(context: PrTabContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let new_comment = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 14)
                .placeholder("Leave a comment (⌘↵ to send)")
        });
        let changes = cx.subscribe_in(&new_comment, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        // Code snippets carry the syntax colors they were highlighted with:
        // a new theme highlights them again.
        let theme_changes = cx.observe_global_in::<gpui_kit::component::Theme>(
            window,
            |this: &mut Self, window, cx| this.sync_threads(window, cx),
        );
        let mut tab = Self {
            context,
            threads: HashMap::new(),
            edits: HashMap::new(),
            body_edit: None,
            new_comment,
            comment_submit: Submit::Idle,
            comment_task: None,
            clear_comment: false,
            preview: false,
            filter: CommentFilter::All,
            scroll: ScrollHandle::new(),
            folds: fold::Folds::default(),
            _comment_changes: changes,
            _theme_changes: theme_changes,
        };
        tab.sync_threads(window, cx);
        tab
    }

    /// Takes the reloaded pull request, keeping drafts and scroll position.
    pub fn set_detail(
        &mut self,
        detail: Arc<PullRequestDetail>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context.detail = detail;
        self.sync_threads(window, cx);
        cx.notify();
    }

    /// Creates a view per thread once and updates it on reload, so reply
    /// drafts and expanded state survive.
    fn sync_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let detail = self.context.detail.clone();
        self.threads
            .retain(|id, _| detail.threads.iter().any(|thread| &thread.id == id));
        let highlight_theme = cx.theme().highlight_theme.clone();
        let head = HeadRef::of(&detail);
        for thread in &detail.threads {
            let snippet = thread_snippet(thread, &highlight_theme);
            // The card header already names the file and lines.
            let label = None;
            match self.threads.get(&thread.id) {
                Some(slot) => {
                    slot.view.update(cx, |view, cx| {
                        view.update(thread.clone(), label, head.clone(), cx)
                    });
                    if let Some(slot) = self.threads.get_mut(&thread.id) {
                        slot.snippet = snippet;
                    }
                }
                None => {
                    let view = cx
                        .new(|cx| ThreadView::new(thread.clone(), label, head.clone(), window, cx));
                    let events = cx.subscribe(&view, |_, _, event: &ThreadEvent, cx| match event {
                        ThreadEvent::Wrote => cx.emit(PrTabEvent::Reload),
                    });
                    self.threads.insert(
                        thread.id.clone(),
                        ThreadSlot {
                            view,
                            snippet,
                            _events: events,
                        },
                    );
                }
            }
        }
    }

    fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let body = self.new_comment.read(cx).value().trim_end().to_owned();
        if body.is_empty() || self.comment_submit.is_busy() {
            return;
        }
        self.comment_submit = Submit::Busy;
        cx.notify();
        let subject = self.context.detail.id.clone();
        self.comment_task = Some(common::run(
            cx,
            move |gh| gh.add_comment(&subject, &body),
            |this, result, cx| match result {
                Ok(()) => {
                    this.comment_submit = Submit::Idle;
                    this.clear_comment = true;
                    cx.emit(PrTabEvent::Reload);
                }
                Err(error) => this.comment_submit = Submit::Failed(error.detail()),
            },
        ));
    }

    fn start_edit(&mut self, comment: &IssueComment, window: &mut Window, cx: &mut Context<Self>) {
        let body = comment.body.clone();
        let text = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(3, 16);
            state.set_value(body, window, cx);
            state
        });
        text.update(cx, |text, cx| text.focus(window, cx));
        self.edits.insert(
            comment.id.clone(),
            Edit {
                text,
                submit: Submit::Idle,
                task: None,
            },
        );
        cx.notify();
    }

    fn save_edit(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let Some(edit) = self.edits.get_mut(&id) else {
            return;
        };
        let body = edit.text.read(cx).value().trim_end().to_owned();
        if body.is_empty() || edit.submit.is_busy() {
            return;
        }
        edit.submit = Submit::Busy;
        let comment_id = id.clone();
        edit.task = Some(common::run(
            cx,
            move |gh| gh.edit_issue_comment(&comment_id, &body),
            move |this, result, cx| match result {
                Ok(()) => {
                    this.edits.remove(&id);
                    cx.emit(PrTabEvent::Reload);
                }
                Err(error) => {
                    if let Some(edit) = this.edits.get_mut(&id) {
                        edit.submit = Submit::Failed(error.detail());
                    }
                }
            },
        ));
        cx.notify();
    }

    fn start_body_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.context.detail.body.clone();
        let text = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(4, 20);
            state.set_value(body, window, cx);
            state
        });
        text.update(cx, |text, cx| text.focus(window, cx));
        self.body_edit = Some(Edit {
            text,
            submit: Submit::Idle,
            task: None,
        });
        cx.notify();
    }

    /// Writes the description as typed; an empty one clears it.
    fn save_body_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &mut self.body_edit else {
            return;
        };
        if edit.submit.is_busy() {
            return;
        }
        let body = edit.text.read(cx).value().trim_end().to_owned();
        edit.submit = Submit::Busy;
        let pull_request_id = self.context.detail.id.clone();
        edit.task = Some(common::run(
            cx,
            move |gh| gh.update_pull_request(&pull_request_id, None, Some(&body)),
            |this, result, cx| match result {
                Ok(()) => {
                    this.body_edit = None;
                    cx.emit(PrTabEvent::Reload);
                }
                Err(error) => {
                    if let Some(edit) = &mut this.body_edit {
                        edit.submit = Submit::Failed(error.detail());
                    }
                }
            },
        ));
        cx.notify();
    }

    fn is_author(&self, author: Option<&Actor>) -> bool {
        match (author, self.context.detail.author.as_ref()) {
            (Some(a), Some(b)) => a.login == b.login,
            _ => false,
        }
    }

    fn body_item(&self, now: DateTime<Utc>, cx: &mut Context<Self>) -> AnyElement {
        let detail = &self.context.detail;
        let body = if let Some(edit) = &self.body_edit {
            let busy = edit.submit.is_busy();
            v_flex()
                .gap_2()
                .child(
                    div()
                        .key_context("CommentEdit")
                        .capture_action(cx.listener(|this, action: &Enter, _, cx| {
                            if action.secondary {
                                cx.stop_propagation();
                                this.save_body_edit(cx);
                            }
                        }))
                        .child(Textarea::new(&edit.text).disabled(busy)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .children(edit.submit.error().map(|message| error_text(message, cx)))
                        .child(
                            Button::new("pr-body-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.body_edit = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("pr-body-save")
                                .outline()
                                .small()
                                .label("Save description")
                                .tooltip("Save (⌘↵)")
                                .loading(busy)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.save_body_edit(cx))),
                        ),
                )
                .into_any_element()
        } else if detail.body.trim().is_empty() {
            div()
                .text_size(scale::body())
                .italic()
                .text_color(cx.theme().muted_foreground)
                .child("No description provided.")
                .into_any_element()
        } else {
            div()
                .w_full()
                .max_w(dp(READING_WIDTH))
                .child(self.capped_body(&detail.id, &detail.body, cx.theme().group_box, cx))
                .into_any_element()
        };
        let can_edit = detail.viewer_can_update && self.body_edit.is_none();
        let header = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(ui::avatar_named(
                detail
                    .author
                    .as_ref()
                    .map(|a| a.avatar_url.clone())
                    .unwrap_or_default(),
                login_of(detail.author.as_ref()),
                dp(20.),
            ))
            .child(author_line(
                detail.author.as_ref(),
                "opened this pull request",
                detail.created_at,
                now,
                false,
                cx,
            ))
            .child(div().flex_1())
            .children(edited_menu(&detail.body_edits, now))
            .when(can_edit, |row| {
                row.child(
                    ui::icon_button("pr-edit-body", IconName::Pencil, "Edit description")
                        .xsmall()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_body_edit(window, cx)),
                        ),
                )
            });
        let theme = cx.theme();
        div()
            .id("pr-description")
            .role(Role::Article)
            .aria_label("Description")
            .w_full()
            .mb_6()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .overflow_hidden()
            .child(card_header(header, cx))
            .child(div().p_4().child(body))
            .into_any_element()
    }

    fn comment_item(
        &self,
        comment: &IssueComment,
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = comment.id.clone();
        let is_author = self.is_author(comment.author.as_ref());
        let editing = self.edits.contains_key(&id);
        let folded = !editing
            && self
                .folds
                .is_folded(&id, comment.author_is_bot || comment.hidden);
        let header = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                author_line(
                    comment.author.as_ref(),
                    "commented",
                    comment.created_at,
                    now,
                    is_author,
                    cx,
                )
                .flex_none()
                .when(comment.author_is_bot, |row| {
                    row.child(crate::ui::chip().xsmall().child("bot"))
                }),
            )
            .child(if folded {
                fold::preview_line(&comment.body, cx)
            } else {
                div().flex_1()
            })
            .when(comment.hidden, |row| {
                row.child(fold::hidden_tag(comment.hidden_reason.as_deref()))
            })
            .when(
                comment.viewer_can_update && !self.edits.contains_key(&id),
                |row| {
                    let comment = comment.clone();
                    row.child(
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
                },
            )
            .when(!editing, |row| row.child(self.fold_button(&id, folded, cx)));
        let content = match self.edits.get(&id) {
            Some(edit) => {
                let busy = edit.submit.is_busy();
                let save_id = id.clone();
                let cancel_id = id.clone();
                let action_id = id.clone();
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .key_context("CommentEdit")
                            .capture_action(cx.listener(move |this, action: &Enter, _, cx| {
                                if action.secondary {
                                    cx.stop_propagation();
                                    this.save_edit(action_id.clone(), cx);
                                }
                            }))
                            .child(Textarea::new(&edit.text).disabled(busy)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .children(edit.submit.error().map(|message| error_text(message, cx)))
                            .child(
                                Button::new(SharedString::from(format!("cancel-{id}")))
                                    .ghost()
                                    .small()
                                    .label("Cancel")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.edits.remove(&cancel_id);
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.save_edit(save_id.clone(), cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            None => self.capped_body(&id, &comment.body, cx.theme().group_box, cx),
        };
        timeline_item(
            actor_avatar(comment.author.as_ref()),
            comment_card(header, (!folded).then_some(content), cx),
            true,
            cx,
        )
    }

    fn review_item(
        &self,
        review: &Review,
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (icon, color, verb) = review_look(review.state);
        let at = review
            .submitted_at
            .unwrap_or(self.context.detail.created_at);
        let is_author = self.is_author(review.author.as_ref());
        let has_body = !review.body.trim().is_empty();
        let folded = has_body
            && self
                .folds
                .is_folded(&review.id, review.author_is_bot || review.hidden);
        let event = h_flex()
            .w_full()
            .h(dp(GUTTER_AVATAR))
            .gap_1p5()
            .items_center()
            .child(ui::avatar_named(
                review
                    .author
                    .as_ref()
                    .map_or_else(String::new, |author| author.avatar_url.clone()),
                login_of(review.author.as_ref()),
                dp(16.),
            ))
            .child(
                author_line(review.author.as_ref(), verb, at, now, is_author, cx)
                    .flex_none()
                    .when(review.author_is_bot, |row| {
                        row.child(crate::ui::chip().xsmall().child("bot"))
                    }),
            )
            .child(if folded {
                fold::preview_line(&review.body, cx)
            } else {
                div().flex_1()
            })
            .when(review.hidden, |row| {
                row.child(fold::hidden_tag(review.hidden_reason.as_deref()))
            })
            .when(has_body, |row| {
                row.child(self.fold_button(&review.id, folded, cx))
            });
        let body = (has_body && !folded).then(|| {
            ui::card(cx).child(self.capped_body(&review.id, &review.body, cx.theme().group_box, cx))
        });
        let content = v_flex().gap_2().child(event).children(body);
        timeline_item(gutter_icon(icon, color, cx), content, true, cx)
    }

    fn thread_item(&self, thread: &ReviewThread, cx: &App) -> AnyElement {
        let Some(slot) = self.threads.get(&thread.id) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .text_size(scale::meta())
            .child(
                Icon::new(widgets::file_icon(&thread.path))
                    .xsmall()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.mono_font_family.clone())
                    .child(thread_label(thread)),
            )
            .when(thread.is_outdated, |row| {
                row.child(widgets::tinted_tag(tokens::pending()).child("Outdated"))
            })
            .when(thread.is_resolved, |row| {
                row.child(widgets::tinted_tag(tokens::open()).child("Resolved"))
            });
        let card = v_flex()
            .w_full()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .overflow_hidden()
            .child(card_header(header, cx))
            .when(!slot.snippet.is_empty(), |column| {
                column.child(code_context(&slot.snippet, cx))
            })
            .child(div().p(scale::card_padding()).child(slot.view.clone()));
        timeline_item(
            gutter_icon(IconName::MessageSquareCode, theme.muted_foreground, cx),
            card,
            true,
            cx,
        )
    }

    /// "Comments", the filter, and the menu with the order and the place of
    /// the new-comment box.
    fn comments_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = Settings::get(cx);
        let order = settings.comment_order;
        let place = settings.comment_box_place();
        let view = cx.weak_entity();
        let filter_view = view.clone();
        let selected = CommentFilter::ALL
            .iter()
            .position(|filter| *filter == self.filter)
            .unwrap_or(0);
        let order_label = match order {
            CommentOrder::OldestFirst => "Oldest first",
            CommentOrder::NewestFirst => "Newest first",
        };
        let sort = Button::new("comment-sort")
            .ghost()
            .small()
            .icon(IconName::ArrowDownUp)
            .label(order_label)
            .tooltip("Sort comments")
            .accessibility_label(format!("Sort comments: {order_label}"))
            .menu_trigger()
            .dropdown_menu(move |menu, _, _| {
                let set = |label: &'static str, checked: bool, change: fn(&mut Settings)| {
                    let view = view.clone();
                    PopupMenuItem::new(label)
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            store::update_settings(cx, change);
                            view.update(cx, |_, cx| cx.notify()).ok();
                        })
                };
                menu.min_w(dp(220.))
                    .item(set(
                        "Oldest first",
                        order == CommentOrder::OldestFirst,
                        |settings| settings.comment_order = CommentOrder::OldestFirst,
                    ))
                    .item(set(
                        "Newest first",
                        order == CommentOrder::NewestFirst,
                        |settings| settings.comment_order = CommentOrder::NewestFirst,
                    ))
                    .item(PopupMenuItem::separator())
                    .item(set(
                        "Comment box on top",
                        place == CommentBoxPlace::Top,
                        |settings| settings.comment_box = Some(CommentBoxPlace::Top),
                    ))
                    .item(set(
                        "Comment box at the end",
                        place == CommentBoxPlace::Bottom,
                        |settings| settings.comment_box = Some(CommentBoxPlace::Bottom),
                    ))
            });
        // In a pane too narrow for the heading and the controls on one line,
        // the controls go to the next one: the heading keeps its width
        // instead of stacking its letters.
        h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .pb_4()
            .items_center()
            .child(
                div()
                    .flex_grow(1.)
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_size(scale::title())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Comments"),
            )
            .child(ui::segmented(
                "comment-filter",
                "Filter comments",
                CommentFilter::ALL.map(|filter| (None, filter.label())),
                selected,
                move |index, _, cx| {
                    filter_view
                        .update(cx, |this, cx| {
                            this.filter = CommentFilter::ALL[index];
                            cx.notify();
                        })
                        .ok();
                },
            ))
            .child(sort)
            .into_any_element()
    }

    /// Applies a toolbar `format` to the new comment's selection, then puts
    /// the caret back in the box.
    fn apply_format(&mut self, format: Format, window: &mut Window, cx: &mut Context<Self>) {
        self.new_comment.update(cx, |state, cx| {
            let edit = format_edit(&state.value(), state.selected_range(), format);
            state.set_selected_range(edit.range, cx);
            state.replace(edit.text, window, cx);
            state.set_selected_range(edit.selection, cx);
            state.focus(window, cx);
        });
    }

    /// GitHub's Markdown buttons, each named for screen readers.
    fn format_toolbar(&self, cx: &mut Context<Self>) -> Div {
        const BUTTONS: [(Format, IconName, &str); 9] = [
            (Format::Heading, IconName::Heading, "Heading"),
            (Format::Bold, IconName::Bold, "Bold"),
            (Format::Italic, IconName::Italic, "Italic"),
            (Format::Quote, IconName::TextQuote, "Quote"),
            (Format::Code, IconName::Code, "Code"),
            (Format::Link, IconName::Link, "Link"),
            (Format::BulletList, IconName::List, "Bulleted list"),
            (Format::NumberedList, IconName::ListOrdered, "Numbered list"),
            (Format::TaskList, IconName::ListTodo, "Task list"),
        ];
        let busy = self.comment_submit.is_busy();
        h_flex()
            .gap_0p5()
            .children(BUTTONS.map(|(format, icon, name)| {
                ui::icon_button(SharedString::from(format!("format-{name}")), icon, name)
                    .xsmall()
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.apply_format(format, window, cx)
                        }),
                    )
            }))
    }

    /// Shown when the filter keeps nothing.
    fn no_entries(&self, cx: &App) -> AnyElement {
        let (icon, text) = match self.filter {
            CommentFilter::All => (IconName::MessageSquare, "No comments yet"),
            CommentFilter::Unresolved => (IconName::CircleCheck, "No unresolved threads"),
            CommentFilter::Mine => (IconName::User, "You have not commented here"),
        };
        div()
            .pb_5()
            .child(ui::empty_line(icon, text, cx))
            .into_any_element()
    }

    fn composer_item(&self, connect: bool, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.comment_submit.is_busy();
        let empty = self.new_comment.read(cx).value().trim().is_empty();
        let viewer = match &Accounts::get(cx).github {
            Connection::Connected(viewer) => {
                Some((viewer.avatar_url.clone(), viewer.login.clone()))
            }
            _ => None,
        };
        let avatar = match viewer {
            Some((url, login)) => {
                ui::avatar_named(url, login, dp(GUTTER_AVATAR)).into_any_element()
            }
            None => ui::avatar("", dp(GUTTER_AVATAR)).into_any_element(),
        };
        let muted = cx.theme().muted_foreground;
        let view = cx.weak_entity();
        let mode = h_flex()
            .gap_2()
            .items_center()
            .child(ui::segmented(
                "comment-mode",
                "New comment",
                [(None, "Write"), (None, "Preview")],
                usize::from(self.preview),
                move |index, _, cx| {
                    view.update(cx, |this, cx| {
                        this.preview = index == 1;
                        cx.notify();
                    })
                    .ok();
                },
            ))
            .child(div().flex_1())
            .when(!self.preview, |row| row.child(self.format_toolbar(cx)));
        let editor = if self.preview {
            let text = self.new_comment.read(cx).value().clone();
            if text.trim().is_empty() {
                div()
                    .min_h(dp(64.))
                    .text_size(scale::body())
                    .text_color(muted)
                    .child("Nothing to preview")
                    .into_any_element()
            } else {
                div()
                    .min_h(dp(64.))
                    .child(crate::ui::markdown("comment-preview", text))
                    .into_any_element()
            }
        } else {
            div()
                .key_context("NewComment")
                .capture_action(cx.listener(|this, action: &Enter, _, cx| {
                    if action.secondary {
                        cx.stop_propagation();
                        this.submit_comment(cx);
                    }
                }))
                .child(Textarea::new(&self.new_comment).disabled(busy))
                .into_any_element()
        };
        let content = ui::card(cx).child(mode).child(editor).child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child("Markdown is supported."),
                )
                .children(
                    self.comment_submit
                        .error()
                        .map(|message| error_text(message, cx)),
                )
                .child(
                    Button::new("comment")
                        .primary()
                        .small()
                        .label("Comment")
                        .tooltip("Comment (⌘↵)")
                        .loading(busy)
                        .disabled(busy || empty)
                        .on_click(cx.listener(|this, _, _, cx| this.submit_comment(cx))),
                ),
        );
        timeline_item(avatar, content, connect, cx)
    }
}

impl EventEmitter<PrTabEvent> for ConversationTab {}

impl Render for ConversationTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_comment {
            self.clear_comment = false;
            self.new_comment
                .update(cx, |text, cx| text.set_value("", window, cx));
        }
        let now = Utc::now();
        let detail = self.context.detail.clone();
        let settings = Settings::get(cx);
        let (order, place) = (settings.comment_order, settings.comment_box_place());
        let mut items: Vec<AnyElement> = vec![self.body_item(now, cx), self.comments_header(cx)];
        if place == CommentBoxPlace::Top {
            items.push(self.composer_item(true, cx));
        }
        let entries = visible_entries(&detail, self.filter, order);
        if entries.is_empty() {
            items.push(self.no_entries(cx));
        }
        for entry in entries {
            items.push(match entry {
                Entry::Comment(comment) => self.comment_item(comment, now, cx),
                Entry::Review(review) => self.review_item(review, now, cx),
                Entry::Thread(thread) => self.thread_item(thread, cx),
            });
        }
        if place == CommentBoxPlace::Bottom {
            items.push(self.composer_item(false, cx));
        }
        div()
            .id("conversation")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .w_full()
                    .px(scale::content_inset())
                    .py_5()
                    .children(items),
            )
    }
}

/// One timeline entry: the gutter (avatar or event icon) with the thin line
/// that joins it to the next entry, and the content beside it.
fn timeline_item(
    gutter: impl IntoElement,
    content: impl IntoElement,
    connect: bool,
    cx: &App,
) -> AnyElement {
    let border = cx.theme().border;
    h_flex()
        .w_full()
        .items_stretch()
        .gap_3()
        .child(
            v_flex()
                .flex_none()
                .w(dp(GUTTER_AVATAR))
                .items_center()
                .child(gutter)
                .when(connect, |column| {
                    column.child(div().flex_1().w_px().mt_1().bg(border))
                }),
        )
        .child(div().flex_1().min_w_0().pb_5().child(content))
        .into_any_element()
}

/// A person's avatar in the gutter, with initials when the image is missing.
fn actor_avatar(actor: Option<&Actor>) -> AnyElement {
    ui::avatar_named(
        actor.map_or_else(String::new, |actor| actor.avatar_url.clone()),
        login_of(actor),
        dp(GUTTER_AVATAR),
    )
    .into_any_element()
}

/// An event glyph in a ring, the size of a gutter avatar.
fn gutter_icon(icon: IconName, color: Hsla, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .size(dp(GUTTER_AVATAR))
        .flex_none()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(Icon::new(icon).small().text_color(color))
        .into_any_element()
}

/// "Edited ▾": who edited the description and when, newest first.
fn edited_menu(edits: &[BodyEdit], now: DateTime<Utc>) -> Option<AnyElement> {
    let latest = edits.first()?;
    let lines: Vec<String> = edits
        .iter()
        .map(|edit| {
            let verb = if edit.created { "created" } else { "edited" };
            format!(
                "{} {verb} {}",
                login_of(edit.editor.as_ref()),
                ui::relative_time(edit.edited_at, now)
            )
        })
        .collect();
    Some(
        Button::new("pr-body-edited")
            .ghost()
            .xsmall()
            .label("Edited")
            .dropdown_caret(true)
            .tooltip(format!(
                "Last edited by {} {}",
                login_of(latest.editor.as_ref()),
                ui::relative_time(latest.edited_at, now)
            ))
            .accessibility_label("Edit history")
            .menu_trigger()
            .dropdown_menu(move |menu, _, _| {
                lines.iter().fold(menu.min_w(dp(220.)), |menu, line| {
                    menu.item(PopupMenuItem::new(line.clone()))
                })
            })
            .into_any_element(),
    )
}

/// A comment card: a header strip over the body, or the header alone when
/// the comment is folded.
fn comment_card(header: impl IntoElement, body: Option<AnyElement>, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .overflow_hidden()
        .child(card_header(header, cx))
        .children(body.map(|body| div().p(scale::card_padding()).child(body)))
}

fn card_header(content: impl IntoElement, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .h_9()
        .px_4()
        .py_1()
        .items_center()
        .bg(theme.list_hover)
        .border_b_1()
        .border_color(theme.border)
        .child(content)
}

/// The commented lines of a review thread, with base and head line numbers
/// and the diff colors.
fn code_context(lines: &[SnippetLine], cx: &App) -> Div {
    let theme = cx.theme();
    let number = |value: Option<u32>| {
        div()
            .flex_none()
            .w_8()
            .pr_2()
            .text_right()
            .text_color(theme.muted_foreground.opacity(0.7))
            .child(value.map(|n| n.to_string()).unwrap_or_default())
    };
    v_flex()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.background)
        .font_family(theme.mono_font_family.clone())
        .text_size(scale::code())
        .children(lines.iter().map(|(line, runs)| {
            let (background, color, sign) = match line.kind {
                HunkLineKind::Added => (tokens::diff_added_bg(), tokens::diff_added_text(), "+"),
                HunkLineKind::Removed => {
                    (tokens::diff_removed_bg(), tokens::diff_removed_text(), "−")
                }
                HunkLineKind::Context => (gpui_kit::transparent_black(), theme.foreground, " "),
            };
            h_flex()
                .bg(background)
                .child(number(line.old))
                .child(number(line.new))
                .child(div().flex_none().w_4().text_color(color).child(sign))
                // Code takes syntax colors over the line's diff background,
                // as the Files tab draws it.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pr_3()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(theme.foreground)
                        .child(
                            StyledText::new(SharedString::from(line.text.clone()))
                                .with_highlights(runs.iter().cloned()),
                        ),
                )
        }))
}

/// The last lines of a thread's hunk with syntax runs for its file's
/// language.
fn thread_snippet(
    thread: &ReviewThread,
    theme: &gpui_kit::component::highlighter::HighlightTheme,
) -> Vec<SnippetLine> {
    let lines = thread
        .comments
        .first()
        .map(|comment| hunk_tail(&comment.diff_hunk, 4))
        .unwrap_or_default();
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    let runs = highlight_lines(language_for_path(&thread.path), &texts, theme);
    lines.into_iter().zip(runs).collect()
}

fn login_of(actor: Option<&Actor>) -> String {
    actor.map_or_else(|| "ghost".to_owned(), |actor| actor.login.clone())
}

/// `login verb age`, with an Author tag when the pull request's author wrote it.
fn author_line(
    author: Option<&Actor>,
    verb: &'static str,
    at: DateTime<Utc>,
    now: DateTime<Utc>,
    is_author: bool,
    cx: &App,
) -> Div {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap_1p5()
        .items_center()
        .text_size(scale::body())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(login_of(author)),
        )
        .when(is_author, |row| {
            row.child(crate::ui::chip().xsmall().child("Author"))
        })
        .child(div().text_color(muted).child(verb))
        .child(
            div()
                .text_size(scale::meta())
                .text_color(muted)
                .child(ui::relative_time(at, now)),
        )
}

fn error_text(message: &str, cx: &App) -> Div {
    div()
        .text_size(scale::meta())
        .text_color(cx.theme().danger)
        .child(message.to_owned())
}

fn review_look(state: ReviewState) -> (IconName, Hsla, &'static str) {
    match state {
        ReviewState::Approved => (
            IconName::CircleCheck,
            tokens::open(),
            "approved these changes",
        ),
        ReviewState::ChangesRequested => (IconName::CircleX, tokens::closed(), "requested changes"),
        ReviewState::Commented => (IconName::MessageSquare, tokens::text_muted(), "reviewed"),
        ReviewState::Dismissed => (
            IconName::CircleDashed,
            tokens::text_faint(),
            "review dismissed",
        ),
        ReviewState::Pending => (IconName::CircleDashed, tokens::pending(), "pending review"),
    }
}
