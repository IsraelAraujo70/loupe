//! The comment thread and the new-comment box.

use chrono::Utc;
use gpui_kit::component::avatar::Avatar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::{Author, Comment};

use super::{IssueView, Loaded};
use crate::linear::widgets;
use crate::theme::scale::dp;
use crate::theme::tokens;
use crate::ui::ControlSize as _;

/// A comment with its markdown body kept as a `SharedString`, so the text
/// view is not handed a fresh allocation on every frame.
pub(super) struct CommentEntry {
    comment: Comment,
    body: SharedString,
}

impl CommentEntry {
    pub(super) fn new(comment: Comment) -> Self {
        let body = SharedString::from(comment.body.clone());
        Self { comment, body }
    }
}

/// Indices into `comments` in display order: each top-level comment followed
/// by its replies, oldest first. A reply to a reply sits under the top-level
/// comment it descends from, and a reply whose parent is not in the list is
/// shown as a top-level comment. The boolean marks replies.
fn thread_order(comments: &[Comment]) -> Vec<(usize, bool)> {
    let parent_of = |index: usize| -> Option<usize> {
        let parent_id = comments[index].parent_id.as_deref()?;
        comments.iter().position(|comment| comment.id == parent_id)
    };
    let root_of = |mut index: usize| -> usize {
        // Linear threads are one level deep; the bound only protects against
        // a malformed response that loops.
        for _ in 0..comments.len() {
            match parent_of(index) {
                Some(parent) => index = parent,
                None => break,
            }
        }
        index
    };

    let mut order = Vec::with_capacity(comments.len());
    for root in 0..comments.len() {
        if root_of(root) != root {
            continue;
        }
        order.push((root, false));
        order.extend(
            (0..comments.len())
                .filter(|&index| index != root && root_of(index) == root)
                .map(|index| (index, true)),
        );
    }
    order
}

struct AuthorLine {
    name: SharedString,
    bot: bool,
    avatar: AnyElement,
}

fn author_line(author: &Author) -> AuthorLine {
    let size = dp(20.);
    match author {
        Author::User(user) => AuthorLine {
            name: if user.display_name.is_empty() {
                user.name.clone().into()
            } else {
                user.display_name.clone().into()
            },
            bot: false,
            avatar: widgets::avatar(Some(user), size),
        },
        Author::Bot { name, avatar_url } => AuthorLine {
            name: name.clone().into(),
            bot: true,
            avatar: Avatar::new()
                .with_size(Size::Size(size))
                .name(name.clone())
                .when_some(avatar_url.clone(), |avatar, url| avatar.src(url))
                .into_any_element(),
        },
        Author::Unknown => AuthorLine {
            name: "Unknown".into(),
            bot: false,
            avatar: widgets::avatar(None, size),
        },
    }
}

/// The Cmd+Enter keyboard hint.
fn send_hint() -> Option<AnyElement> {
    Keystroke::parse("cmd-enter")
        .ok()
        .map(|keystroke| Kbd::new(keystroke).into_any_element())
}

impl IssueView {
    /// The comments' heading: the real count when there are comments, and
    /// words when there are none.
    fn render_comments_heading(&self, loaded: &Loaded, cx: &App) -> Div {
        let count = loaded.comments.len();
        h_flex()
            .h_5()
            .gap_2()
            .items_center()
            .child(crate::ui::section_label("Comments", cx))
            .map(|row| match count {
                0 => row.child(
                    div()
                        .text_size(crate::theme::scale::meta())
                        .text_color(cx.theme().muted_foreground)
                        .child("No comments yet"),
                ),
                count => row.child(crate::ui::count_badge(count, cx)),
            })
    }

    /// The comments, at the end of the reading column: the heading, then
    /// the thread.
    pub(super) fn render_comments(&self, loaded: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        let now = Utc::now();
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let radius = cx.theme().radius_lg;
        let card = tokens::surface();
        let comments: Vec<Comment> = loaded
            .comments
            .iter()
            .map(|entry| entry.comment.clone())
            .collect();

        let thread = thread_order(&comments).into_iter().map(|(index, reply)| {
            let entry = &loaded.comments[index];
            let comment = &entry.comment;
            let author = author_line(&comment.author);
            let age = crate::ui::relative_time(comment.created_at, now);
            let edited = comment.edited_at.is_some();

            let meta = h_flex()
                .gap_2()
                .items_center()
                .child(author.avatar)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(author.name),
                )
                .children(
                    author
                        .bot
                        .then(|| crate::ui::chip().small().rounded_full().child("Bot")),
                )
                .child(div().text_xs().text_color(muted).child(age))
                .children(edited.then(|| div().text_xs().text_color(muted).child("edited")));

            v_flex()
                .gap_2()
                .p_3()
                .rounded(radius)
                .border_1()
                .border_color(border)
                .bg(card)
                .when(reply, |this| this.ml_8())
                .child(meta)
                .child(div().w_full().child(crate::ui::markdown(
                    SharedString::from(format!("comment-{}", comment.id)),
                    entry.body.clone(),
                )))
        });

        v_flex()
            .gap_3()
            .child(self.render_comments_heading(loaded, cx))
            .children(thread)
            .into_any_element()
    }

    /// The comment box. Closed, it is one line that says what it is for.
    /// With the focus, a draft, or an error it is an editor that grows with
    /// the comment, with the send hint and the Comment button under the
    /// text.
    pub(super) fn render_composer(&self, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let empty = self.composer.read(cx).value().trim().is_empty();
        let open = focused || !empty || self.sending || self.comment_error.is_some();
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let border = if focused { theme.ring } else { theme.input };

        let send = Button::new("send-comment")
            .control()
            .loading(self.sending)
            .disabled(self.sending || empty)
            .on_click(cx.listener(|view, _, window, cx| view.submit_comment(window, cx)));
        // With nothing to send the button is neutral and its label keeps
        // the muted color, which stays readable; the kit's disabled primary
        // is a faint label on a faint fill.
        let send = if empty {
            send.outline()
                .accessibility_label("Comment")
                .child(div().text_color(muted).child("Comment"))
        } else {
            send.primary().label("Comment")
        };
        v_flex()
            .w_full()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(border)
            .bg(tokens::background())
            .child(
                Textarea::new(&self.composer)
                    .appearance(false)
                    .disabled(self.sending),
            )
            .when(open, |composer| {
                composer.child(
                    h_flex()
                        .px_2()
                        .pb_1p5()
                        .gap_3()
                        .items_center()
                        .child(div().flex_1().min_w_0().text_xs().map(|this| {
                            match &self.comment_error {
                                Some(error) => this.text_color(danger).child(error.clone()),
                                None => this
                                    .text_color(muted)
                                    .child(h_flex().gap_1().children(send_hint()).child("to send")),
                            }
                        }))
                        .child(send),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::thread_order;
    use chrono::DateTime;
    use loupe_linear::{Author, Comment};

    fn comment(id: &str, parent: Option<&str>) -> Comment {
        let at = DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .unwrap()
            .to_utc();
        Comment {
            id: id.into(),
            body: id.into(),
            author: Author::Unknown,
            created_at: at,
            edited_at: None,
            parent_id: parent.map(str::to_owned),
        }
    }

    fn ids(comments: &[Comment]) -> Vec<(String, bool)> {
        thread_order(comments)
            .into_iter()
            .map(|(index, reply)| (comments[index].id.clone(), reply))
            .collect()
    }

    #[test]
    fn replies_follow_their_parent_oldest_first() {
        let comments = [
            comment("a", None),
            comment("b", None),
            comment("a1", Some("a")),
            comment("a2", Some("a")),
            comment("b1", Some("b")),
        ];
        let expected = [
            ("a", false),
            ("a1", true),
            ("a2", true),
            ("b", false),
            ("b1", true),
        ]
        .map(|(id, reply)| (id.to_owned(), reply));
        assert_eq!(ids(&comments), expected);
    }

    #[test]
    fn a_reply_to_a_reply_sits_under_the_top_level_comment() {
        let comments = [
            comment("a", None),
            comment("a1", Some("a")),
            comment("a1x", Some("a1")),
        ];
        let expected =
            [("a", false), ("a1", true), ("a1x", true)].map(|(id, reply)| (id.to_owned(), reply));
        assert_eq!(ids(&comments), expected);
    }

    #[test]
    fn an_orphan_reply_is_shown_as_top_level() {
        let comments = [comment("a", None), comment("x", Some("gone"))];
        let expected = [("a", false), ("x", false)].map(|(id, reply)| (id.to_owned(), reply));
        assert_eq!(ids(&comments), expected);
    }

    #[test]
    fn a_parent_cycle_does_not_hang_or_drop_comments() {
        let comments = [comment("a", Some("b")), comment("b", Some("a"))];
        assert_eq!(thread_order(&comments).len(), 2);
    }
}
