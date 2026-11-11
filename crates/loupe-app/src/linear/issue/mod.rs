//! One issue: the title, the description, the linked pull requests, and the
//! comments in one reading column, with the comment box at its foot; the
//! editable properties in a side column; and "Open in Linear".

mod activity;
mod links;
pub(super) mod optimistic;
pub(super) mod properties;

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_linear::{
    Attachment, Comment, IssueDetail, IssueSummary, IssueUpdate, Linear, Team, User,
};

use self::activity::CommentEntry;
use self::optimistic::OptimisticIssue;
use self::properties::{OpenPicker, Place, Property};
use crate::linear::data::{self, Loadable};
use crate::linear::widgets;
use crate::route::Route;
use crate::services::background;
use crate::shell::OpenOnClick as _;
use crate::shell::item_menu::{self, Item};
use crate::store::{self, Favorite, Settings};
use crate::theme::scale::dp;
use crate::ui;

gpui_kit::actions!(
    linear,
    [
        /// S on an issue: open the state picker.
        ChangeState,
        /// P on an issue: open the priority picker.
        ChangePriority,
        /// A on an issue: open the assignee picker.
        ChangeAssignee,
        /// L on an issue: open the labels picker.
        ChangeLabels,
        /// C on an issue: put the caret in the comment box.
        WriteComment
    ]
);

/// The copy of the page's issue (confirmed as `shown`) that `change`
/// brings, if Linear returned it. A copy shown before Linear answered is
/// left to the lists: this page queues its own edits on Linear's copy.
fn from_linear(change: &data::IssueChange, shown: &IssueSummary) -> Option<IssueSummary> {
    change
        .issue
        .as_ref()
        .filter(|issue| change.confirmed && issue.id == shown.id)
        .cloned()
}

/// Whether `incoming` is a newer copy of the issue than `current`.
fn newer_than(incoming: &IssueSummary, current: &IssueSummary) -> bool {
    incoming.id == current.id && incoming.updated_at >= current.updated_at && incoming != current
}

/// The issue screen for `identifier`, such as `ENG-123`.
pub fn view(identifier: String, window: &mut Window, cx: &mut App) -> AnyView {
    let composer = cx.new(|cx| {
        TextareaState::new(window, cx)
            // One row while empty, so the box at the foot of the column
            // takes one line of the page; it grows as the comment does.
            .auto_grow(1, 10)
            .placeholder("Add a comment…")
    });
    cx.new(|cx| IssueView::new(identifier, composer, cx)).into()
}

/// What the issue query returned, split into what the screen mutates.
struct Loaded {
    model: OptimisticIssue,
    description: SharedString,
    creator: Option<User>,
    attachments: Vec<Attachment>,
    comments: Vec<CommentEntry>,
}

impl Loaded {
    fn new(detail: IssueDetail) -> Self {
        Self {
            model: OptimisticIssue::new(detail.issue),
            description: detail.description.into(),
            creator: detail.creator,
            attachments: detail.attachments,
            comments: detail.comments.into_iter().map(CommentEntry::new).collect(),
        }
    }
}

struct IssueView {
    identifier: String,
    /// The client the shown data came from; a different one reloads.
    client: Option<Arc<Linear>>,
    /// Bumped on every load so a late answer to an older load is dropped.
    generation: u64,
    load: Loadable<Loaded>,
    team: Loadable<Arc<Team>>,
    /// The last failed property edit, shown until the next edit or dismissal.
    edit_error: Option<SharedString>,
    /// The property picker that is open. The pickers are controlled
    /// popovers, so a click (which accessibility clients can send) and the
    /// property's key open them, and not only a mouse press.
    picker: Option<OpenPicker>,
    /// Focused while a picker is open, so its rows get the arrow keys.
    picker_focus: FocusHandle,
    /// Focused while nothing more specific is, so S, P, A, L, and C reach
    /// the screen.
    focus: FocusHandle,
    /// The property name of each queued edit, for its error message.
    edit_labels: Vec<(u64, &'static str)>,
    /// The latest copy of the issue Linear returned to an edit made
    /// elsewhere, held while this page has edits on their way.
    from_linear: Option<IssueSummary>,
    composer: Entity<TextareaState>,
    sending: bool,
    comment_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl IssueView {
    fn new(identifier: String, composer: Entity<TextareaState>, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let composer_focus = composer.read(cx).focus_handle(cx);
        // The textarea inserts a newline on Cmd+Enter as well as emitting the
        // submit event, so the shortcut is taken before the textarea sees it.
        let shortcut = cx.intercept_keystrokes(move |event, window, cx| {
            let keystroke = &event.keystroke;
            if keystroke.key == "enter"
                && keystroke.modifiers.secondary(cx)
                && !keystroke.modifiers.shift
                && composer_focus.is_focused(window)
            {
                cx.stop_propagation();
                weak.update(cx, |view, cx| view.submit_comment(window, cx))
                    .ok();
            }
        });
        let typing = cx.observe(&composer, |_, _, cx| cx.notify());
        // The issue changed from a menu or the palette (here or in another
        // pane). Only a copy Linear returned counts here: this page queues
        // its own edits on top of what Linear holds. While one of them is on
        // its way, the copy waits for the queue to drain.
        let changes = cx.observe_global::<data::IssueChange>(|view: &mut Self, cx| {
            if let Loadable::Ready(loaded) = &view.load
                && let Some(issue) =
                    from_linear(cx.global::<data::IssueChange>(), loaded.model.confirmed())
            {
                view.from_linear = Some(issue);
                view.adopt_from_linear(cx);
            }
        });
        let page = cx.entity_id();
        cx.on_release(move |_, cx| item_menu::set_page_item(page, None, cx))
            .detach();
        Self {
            identifier,
            client: None,
            generation: 0,
            load: Loadable::Loading,
            team: Loadable::Loading,
            edit_error: None,
            picker: None,
            picker_focus: cx.focus_handle(),
            focus: cx.focus_handle(),
            edit_labels: Vec::new(),
            from_linear: None,
            composer,
            sending: false,
            comment_error: None,
            _subscriptions: vec![shortcut, typing, changes],
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = data::client(cx) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.load = Loadable::Loading;
        self.from_linear = None;
        self.team = Loadable::Loading;
        self.edit_labels.clear();
        self.edit_error = None;
        self.sending = false;
        self.comment_error = None;
        cx.notify();

        let identifier = self.identifier.clone();
        let task = background(cx, move || client.issue(&identifier));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                match result {
                    Ok(detail) => {
                        let team_id = detail.issue.team.id.clone();
                        view.load = Loadable::Ready(Loaded::new(detail));
                        view.load_team(team_id, cx);
                    }
                    Err(error) => view.load = Loadable::Failed(data::error_text(&error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_team(&mut self, team_id: String, cx: &mut Context<Self>) {
        self.team = Loadable::Loading;
        let generation = self.generation;
        let task = data::team(team_id, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                view.team = match result {
                    Ok(team) => Loadable::Ready(team),
                    Err(message) => Loadable::Failed(message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes the copy Linear returned to an edit made elsewhere once this
    /// page has no edit on its way, unless this page's own answers are
    /// newer.
    fn adopt_from_linear(&mut self, cx: &mut Context<Self>) {
        let Loadable::Ready(loaded) = &mut self.load else {
            return;
        };
        if loaded.model.has_pending() {
            return;
        }
        let Some(issue) = self.from_linear.take() else {
            return;
        };
        if newer_than(&issue, loaded.model.confirmed()) {
            loaded.model = OptimisticIssue::new(issue);
            cx.notify();
        }
    }

    fn retry_team(&mut self, cx: &mut Context<Self>) {
        if let Loadable::Ready(loaded) = &self.load {
            let team_id = loaded.model.confirmed().team.id.clone();
            self.load_team(team_id, cx);
            cx.notify();
        }
    }

    /// Applies `update` at once and queues it. `what` names the property for
    /// the error message ("priority", "state", ...).
    fn edit(&mut self, what: &'static str, update: IssueUpdate, cx: &mut Context<Self>) {
        let Loadable::Ready(loaded) = &mut self.load else {
            return;
        };
        self.edit_error = None;
        let seq = loaded.model.begin(update);
        self.edit_labels.push((seq, what));
        self.send_next_edit(cx);
        cx.notify();
    }

    /// Sends the oldest queued edit when none is in flight. The queue belongs
    /// to the client this view loaded with: after an account switch it stops,
    /// so no edit goes out under the new identity, and `render` reloads.
    fn send_next_edit(&mut self, cx: &mut Context<Self>) {
        let current = data::client(cx);
        if !data::same_client(&self.client, &current) {
            return;
        }
        let Some(client) = current else {
            return;
        };
        let Loadable::Ready(loaded) = &mut self.load else {
            return;
        };
        let Some((seq, update)) = loaded.model.take_next() else {
            return;
        };
        let issue_id = loaded.model.confirmed().id.clone();
        let generation = self.generation;
        let task = background(cx, move || client.update_issue(&issue_id, &update));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                let what = view
                    .edit_labels
                    .iter()
                    .position(|(label_seq, _)| *label_seq == seq)
                    .map(|index| view.edit_labels.remove(index).1)
                    .unwrap_or("the issue");
                let Loadable::Ready(loaded) = &mut view.load else {
                    return;
                };
                match result {
                    Ok(server) => {
                        // Boards and lists that hold this issue show the
                        // change without a reload.
                        data::publish_issue(server.clone(), cx);
                        loaded.model.confirm(seq, server);
                    }
                    Err(error) => {
                        loaded.model.fail(seq);
                        view.edit_error = Some(
                            format!("Could not change {what}: {}", data::error_text(&error)).into(),
                        );
                    }
                }
                view.adopt_from_linear(cx);
                view.send_next_edit(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn submit_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let body = self.composer.read(cx).value().trim_end().to_owned();
        if body.is_empty() {
            return;
        }
        let Loadable::Ready(loaded) = &self.load else {
            return;
        };
        let Some(client) = data::client(cx) else {
            return;
        };
        let issue_id = loaded.model.confirmed().id.clone();
        self.sending = true;
        self.comment_error = None;
        cx.notify();

        let generation = self.generation;
        let task = background(cx, move || client.create_comment(&issue_id, &body));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |view, window, cx| {
                if view.generation == generation {
                    view.comment_sent(result, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn comment_sent(
        &mut self,
        result: Result<Comment, loupe_linear::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sending = false;
        match result {
            Ok(comment) => {
                if let Loadable::Ready(loaded) = &mut self.load {
                    loaded.comments.push(CommentEntry::new(comment));
                }
                self.composer
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => {
                self.comment_error = Some(
                    format!("Could not send the comment: {}", data::error_text(&error)).into(),
                );
            }
        }
        cx.notify();
    }

    fn render_ready(
        &self,
        loaded: &Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = match &self.team {
            Loadable::Ready(team) => loaded.model.displayed(team),
            _ => loaded.model.confirmed().clone(),
        };

        let description = if loaded.description.trim().is_empty() {
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No description")
                .into_any_element()
        } else {
            div()
                .w_full()
                .child(crate::ui::markdown(
                    "issue-description",
                    loaded.description.clone(),
                ))
                .into_any_element()
        };

        // One reading column that scrolls as a whole: the title, the
        // description down to its last line, the pull requests that close
        // the issue right under it, then the links and the comments. Nothing
        // is laid over it.
        let scroll = v_flex()
            .id("issue-main")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    // On the header's own edge, with 760 px of text at most.
                    .max_w(dp(crate::ui::READING_WIDTH + 32.))
                    .px(crate::theme::scale::content_inset())
                    .py_5()
                    .gap_5()
                    .child(
                        div()
                            // 22 / 28: clearly above the description's
                            // own headings (18 and 16 px).
                            .text_size(rems(1.375))
                            .line_height(rems(1.75))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(shown.title.clone()),
                    )
                    .child(description)
                    .children(links::pull_requests(&loaded.attachments, cx))
                    .children(links::other_attachments(&loaded.attachments, cx))
                    .child(self.render_comments(loaded, cx)),
            );
        // The comment box stays at the foot of the column: one line while
        // it is closed, an editor with its Comment button once it has the
        // focus or a draft.
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let main = v_flex().flex_1().min_w_0().h_full().child(scroll).child(
            div()
                .flex_none()
                .w_full()
                .max_w(dp(crate::ui::READING_WIDTH + 32.))
                .px(crate::theme::scale::content_inset())
                .pt_2()
                .pb_3()
                .child(self.render_composer(focused, cx)),
        );

        let side = v_flex()
            .id("issue-side")
            // Wider than a pull request's side column: a value sits beside
            // its label, and three labels or a full name fit on one line
            // with the picker's caret after them.
            .w(dp(360.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(crate::theme::tokens::border_subtle())
            .child(self.render_properties(loaded, &shown, cx));

        h_flex()
            .flex_1()
            .min_h_0()
            .child(main)
            .child(side)
            .into_any_element()
    }
}

impl IssueView {
    /// The page header: the team as a crumb that opens its board, the
    /// identifier as the title, the state as a chip that opens the state
    /// picker, then a star for Favorites and Open in Linear.
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let Loadable::Ready(loaded) = &self.load else {
            return ui::page_header(self.identifier.clone())
                .icon(gpui_kit::assets::IconName::Users)
                .into_any_element();
        };
        let issue = match &self.team {
            Loadable::Ready(team) => loaded.model.displayed(team),
            _ => loaded.model.confirmed().clone(),
        };
        let url = issue.url.clone();
        let favorite = Favorite::Issue {
            identifier: self.identifier.clone(),
            title: issue.title.clone(),
        };
        let starred = Settings::get(cx).is_favorite(&favorite);
        let star_label = if starred {
            "Remove from favorites"
        } else {
            "Add to favorites"
        };
        let star = Button::new("issue-favorite")
            .ghost()
            .small()
            .icon(if starred {
                IconName::StarFill
            } else {
                IconName::Star
            })
            .tooltip(star_label)
            .accessibility_label(star_label)
            .on_click(move |_, _, cx| {
                let favorite = favorite.clone();
                store::update_settings(cx, move |settings| settings.toggle_favorite(favorite));
            });
        let open_in_linear = Button::new("open-in-linear")
            .ghost()
            .small()
            .icon(IconName::ExternalLink)
            .label("Open in Linear")
            .on_click(move |_, _, cx| cx.open_url(&url));

        // The team the issue belongs to: one click back to its board.
        let team_route = Route::LinearTeam {
            team_id: issue.team.id.clone(),
            name: issue.team.name.clone(),
            view_id: None,
        };
        let open_team = move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
            crate::shell::open(team_route.clone(), event, window, cx)
        };
        let crumb = h_flex()
            .gap_1()
            .items_center()
            .child(
                Button::new("issue-team-crumb")
                    .ghost()
                    .xsmall()
                    .label(issue.team.name.clone())
                    .accessibility_label(format!("Team {}", issue.team.name))
                    .on_click(open_team.clone())
                    .on_middle_click(open_team),
            )
            .child(div().text_color(cx.theme().muted_foreground).child("›"));

        // Beside the identifier, where the issue stands, in sight however
        // far the page is scrolled: the state, and the way to change it.
        let header_open = self.picker.is_some_and(|open| open.place == Place::Header);
        let chip = properties::state_chip_trigger(&issue, header_open, cx);
        let state: AnyElement = match &self.team {
            Loadable::Ready(team) => self
                .picker(Property::State, Place::Header, chip, team, &issue, cx)
                .into_any_element(),
            _ => chip.into_any_element(),
        };
        // The pull requests that close the issue, one click away however
        // far the page is scrolled; their titles are under the description.
        let pull_requests = links::pull_request_chips(&loaded.attachments, cx);
        pull_requests
            .into_iter()
            .fold(
                ui::page_header(self.identifier.clone())
                    .icon(gpui_kit::assets::IconName::Users)
                    .prefix(crumb)
                    .after_title(state),
                |header, chip| header.action(chip),
            )
            .action(star)
            .action(open_in_linear)
            .into_any_element()
    }

    /// The keys of the screen. They do nothing while a text field has the
    /// focus: the bindings ask for no `Input` in the context.
    fn key_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        let open = |property: Property| {
            move |view: &mut Self, cx: &mut Context<Self>| {
                view.open_picker(property, Place::Side, cx)
            }
        };
        let (state, priority, assignee, labels) = (
            open(Property::State),
            open(Property::Priority),
            open(Property::Assignee),
            open(Property::Labels),
        );
        root.on_action(cx.listener(move |view, _: &ChangeState, _, cx| state(view, cx)))
            .on_action(cx.listener(move |view, _: &ChangePriority, _, cx| priority(view, cx)))
            .on_action(cx.listener(move |view, _: &ChangeAssignee, _, cx| assignee(view, cx)))
            .on_action(cx.listener(move |view, _: &ChangeLabels, _, cx| labels(view, cx)))
            .on_action(cx.listener(|view, _: &WriteComment, window, cx| {
                let composer = view.composer.read(cx).focus_handle(cx);
                composer.focus(window, cx);
                cx.notify();
            }))
    }
}

impl Render for IssueView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The palette and the item keys act on the issue on screen.
        let shown = match (&self.load, &self.team) {
            (Loadable::Ready(loaded), Loadable::Ready(team)) => {
                Some(Item::issue(&loaded.model.displayed(team)))
            }
            (Loadable::Ready(loaded), _) => Some(Item::issue(loaded.model.confirmed())),
            _ => None,
        };
        item_menu::set_page_item(cx.entity_id(), shown, cx);
        let client = data::client(cx);
        if !data::same_client(&self.client, &client) {
            self.client = client;
            self.load = Loadable::Loading;
            self.generation += 1;
            self.sending = false;
            self.comment_error = None;
            self.composer
                .update(cx, |state, cx| state.set_value("", window, cx));
            if self.client.is_some() {
                self.load(cx);
            }
        }
        if self.client.is_none() {
            return widgets::not_connected(IconName::FileText, self.identifier.clone());
        }
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        let header = self.render_header(cx);
        let body = match &self.load {
            Loadable::Loading => div().py_2().child(ui::skeleton_rows(6)).into_any_element(),
            Loadable::Failed(message) => {
                widgets::load_error(message.clone(), cx.listener(|view, _, _, cx| view.load(cx)))
            }
            Loadable::Ready(loaded) => self.render_ready(loaded, window, cx),
        };
        let root = v_flex()
            .size_full()
            .key_context("IssueView")
            .track_focus(&self.focus);
        self.key_actions(root, cx)
            .child(header)
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::{from_linear, newer_than};
    use crate::linear::data::IssueChange;
    use crate::linear::issue_menu::tests::issue;

    /// A pick shown before Linear answered never becomes the page's
    /// confirmed issue; Linear's copy does, unless the page's own is newer.
    #[test]
    fn the_page_takes_only_copies_linear_returned() {
        let page = issue();
        let mut picked = issue();
        picked.state.id = "done".into();
        picked.updated_at = page.updated_at + Duration::seconds(5);
        let shown = IssueChange {
            issue: Some(picked.clone()),
            confirmed: false,
        };
        assert_eq!(from_linear(&shown, &page), None);
        let answered = IssueChange {
            issue: Some(picked.clone()),
            confirmed: true,
        };
        assert_eq!(from_linear(&answered, &page), Some(picked.clone()));
        let mut other = picked.clone();
        other.id = "another".into();
        let elsewhere = IssueChange {
            issue: Some(other),
            confirmed: true,
        };
        assert_eq!(from_linear(&elsewhere, &page), None, "another issue");

        assert!(newer_than(&picked, &page));
        assert!(!newer_than(&page, &picked), "an older answer waits out");
        assert!(!newer_than(&page, &page));
    }
}
