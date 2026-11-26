//! The box under a diff line where a new inline comment is written. One
//! composer exists at a time; picking another range moves it and keeps the
//! draft. The comment joins the viewer's pending review, which the box
//! counts and opens in the review composer.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Enter, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_github::{LineAnchor, NewThread, NodeId, Side};

use super::model::composer_label;
use crate::github::common::{self, Submit};
use crate::github::pull_request::view::{OpenReviewComposer, PendingComments};
use crate::theme::scale;
use crate::ui;

/// Where the composer sits and what it would send.
#[derive(Clone, Debug)]
pub struct ComposerTarget {
    pub path: String,
    /// The last line of the range, where the box is drawn.
    pub line: u32,
    pub side: Side,
    /// `Err` holds why GitHub cannot take a comment on this range.
    pub anchor: Result<LineAnchor, String>,
}

pub enum ComposerEvent {
    /// GitHub took the comment; the pull request needs a reload.
    Submitted,
    Closed,
}

pub struct ComposerView {
    pull_request_id: NodeId,
    target: ComposerTarget,
    text: Entity<TextareaState>,
    submit: Submit,
    task: Option<Task<()>>,
    _changes: Subscription,
    _pending: Subscription,
}

impl EventEmitter<ComposerEvent> for ComposerView {}

impl ComposerView {
    pub fn new(
        pull_request_id: NodeId,
        target: ComposerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 10)
                .placeholder("Leave a comment")
        });
        let changes = cx.subscribe_in(&text, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let pending = cx.observe_global::<PendingComments>(|_, cx| cx.notify());
        let view = Self {
            pull_request_id,
            target,
            text,
            submit: Submit::Idle,
            task: None,
            _changes: changes,
            _pending: pending,
        };
        view.focus_text(window, cx);
        view
    }

    /// Moves the composer to another range, keeping the draft.
    pub fn retarget(
        &mut self,
        target: ComposerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.submit.is_busy() {
            return;
        }
        self.target = target;
        self.submit = Submit::Idle;
        self.focus_text(window, cx);
        cx.notify();
    }

    /// The diff moved under the draft (a new push): keep the text, drop the
    /// anchor, and ask for a fresh selection.
    /// A request already in flight keeps its own copy of the old anchor.
    pub fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.target.anchor =
            Err("The pull request changed. Select the lines again to keep this comment.".into());
        cx.notify();
    }

    pub fn target(&self) -> &ComposerTarget {
        &self.target
    }

    fn focus_text(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.target.anchor.is_ok() {
            self.text.update(cx, |text, cx| text.focus(window, cx));
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Ok(anchor) = self.target.anchor.clone() else {
            return;
        };
        let body = self.text.read(cx).value().trim_end().to_owned();
        if body.is_empty() || self.submit.is_busy() {
            return;
        }
        self.submit = Submit::Busy;
        cx.notify();
        let thread = NewThread {
            pull_request_id: self.pull_request_id.clone(),
            path: self.target.path.clone(),
            body,
            anchor,
        };
        self.task = Some(common::run(
            cx,
            move |gh| gh.add_review_thread(&thread),
            |this, result, cx| match result {
                Ok(()) => {
                    this.submit = Submit::Idle;
                    cx.emit(ComposerEvent::Submitted);
                }
                Err(error) => this.submit = Submit::Failed(error.detail()),
            },
        ));
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.submit.is_busy() {
            cx.emit(ComposerEvent::Closed);
        }
    }
}

impl Render for ComposerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let mono = cx.theme().mono_font_family.clone();
        let busy = self.submit.is_busy();
        let close = Button::new("cancel")
            .ghost()
            .small()
            .label(if self.target.anchor.is_ok() {
                "Cancel"
            } else {
                "Close"
            })
            .disabled(busy)
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)));

        let anchor = match &self.target.anchor {
            Ok(anchor) => anchor.clone(),
            Err(message) => {
                return h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::CircleAlert).small().text_color(danger))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(scale::body())
                            .child(message.clone()),
                    )
                    .child(close)
                    .into_any_element();
            }
        };
        let empty = self.text.read(cx).value().trim().is_empty();
        let pending = PendingComments::count(&self.pull_request_id, cx);
        let joins = match pending {
            0 => "Starts your review".to_owned(),
            1 => "Joins your review · 1 pending".to_owned(),
            count => format!("Joins your review · {count} pending"),
        };
        let review = Button::new("composer-open-review")
            .ghost()
            .xsmall()
            .icon(IconName::MessageSquareDiff)
            .label(joins.clone())
            .text_color(muted)
            .tooltip("Nothing is posted until you submit the review")
            .accessibility_label(format!("{joins}. Open the review"))
            .on_click(|_, window, cx| window.dispatch_action(OpenReviewComposer.boxed_clone(), cx));
        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(Icon::new(IconName::MessageSquarePlus).xsmall())
                    .child(div().font_family(mono).child(composer_label(&anchor))),
            )
            .child(
                div()
                    .key_context("ThreadComposer")
                    .capture_action(cx.listener(|this, action: &Enter, _, cx| {
                        if action.secondary {
                            cx.stop_propagation();
                            this.submit(cx);
                        }
                    }))
                    .child(Textarea::new(&self.text).disabled(busy)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_end()
                    .child(review)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(scale::meta())
                            .text_color(danger)
                            .children(self.submit.error().map(str::to_owned)),
                    )
                    .child(close)
                    .child(
                        Button::new("add-comment")
                            .primary()
                            .small()
                            .label("Add review comment")
                            .tooltip(format!(
                                "Add review comment ({})",
                                ui::shortcut("cmd-enter")
                            ))
                            .loading(busy)
                            .disabled(busy || empty)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .into_any_element()
    }
}
