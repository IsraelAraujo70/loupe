//! The "New issue" modal, shaped like Linear's: the team as a breadcrumb
//! (it can be changed there), a borderless title over the description, one
//! row of property chips that each open a menu with a search field, and a
//! footer with "Create more" and Create issue (Cmd+Enter). It opens centered
//! over a dimmed window, can fill the window, and keeps a draft when it
//! closes without creating.
//!
//! Keys: Enter or Tab in the title goes to the description; Tab or Escape
//! there leaves the text, and then S, P, A, L (Linear's keys), Shift+P,
//! Shift+C, Shift+E, Shift+D, and T open the menus. In a menu, typing
//! narrows it, the arrows move, Enter picks, and Escape closes it. Escape
//! outside the text closes the modal.

mod fields;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::{IssueSummary, Linear, Priority, Team, TeamSummary};

use self::fields::{
    Choice, Fields, Mark, Pick, Property, choices, cycle_name, date_text, first_cursor, state_ref,
    step_cursor,
};
use crate::linear::{ChangeAssignee, ChangeLabels, ChangePriority, ChangeState, data, widgets};
use crate::route::Route;
use crate::services;
use crate::shell::svg_icon;
use crate::theme::scale::{self, dp};
use crate::theme::tokens;
use crate::ui;

gpui_kit::actions!(
    linear,
    [
        /// Shift+P in the new issue modal: open the project menu.
        SetProject,
        /// Shift+C in the new issue modal: open the cycle menu.
        SetCycle,
        /// Shift+E in the new issue modal: open the estimate menu.
        SetEstimate,
        /// Shift+D in the new issue modal: open the due date menu.
        SetDueDate,
        /// T in the new issue modal: open the team menu.
        MoveToTeam
    ]
);

/// What runs once Linear accepted an issue. `stays_open` is true when
/// "Create more" keeps the modal open for the next issue.
pub(super) type OnCreated = Box<dyn Fn(IssueSummary, bool, &mut Window, &mut App)>;

const WIDTH: f32 = 720.;
const MARK: f32 = 16.;
const ROW_HEIGHT: f32 = 28.;

/// Opens the modal with the draft left from the last one, if any, or else
/// for `team_id` or the first team the user is a member of. The created
/// issue opens in the current tab.
pub(super) fn compose(team_id: Option<String>, window: &mut Window, cx: &mut App) {
    let Some(client) = data::client(cx) else {
        return;
    };
    // C held down, or pressed twice before the catalog loads, opens one
    // modal. Once it is open the focus is in it, and C no longer reaches
    // the workspace.
    if cx
        .try_global::<Composing>()
        .is_some_and(|composing| composing.0)
    {
        return;
    }
    cx.set_global(Composing(true));
    let draft_team = draft_for(&client, cx).map(|draft| draft.fields.team_id);
    let my_teams = data::my_teams(cx);
    window
        .spawn(cx, async move |cx| {
            // The draft's team first; when it no longer loads (access lost,
            // team archived), the team asked for, and the draft waits.
            let mut team = Err(SharedString::default());
            if let Some(draft_team) = draft_team {
                team = match cx.update(|_, cx| data::team(draft_team, cx)) {
                    Ok(task) => task.await,
                    Err(_) => Err("The window closed.".into()),
                };
            }
            if team.is_err() {
                team = async {
                    let team_id = match team_id {
                        Some(id) => id,
                        None => my_teams
                            .await?
                            .first()
                            .map(|team| team.id.clone())
                            .ok_or_else(|| {
                                SharedString::from("Join a Linear team to create issues.")
                            })?,
                    };
                    cx.update(|_, cx| data::team(team_id, cx))
                        .map_err(|_| SharedString::from("The window closed."))?
                        .await
                }
                .await;
            }
            cx.update(|window, cx| {
                cx.set_global(Composing(false));
                match team {
                    Ok(team) => {
                        let on_created: OnCreated = Box::new(|issue, stays_open, window, cx| {
                            if !stays_open {
                                let identifier = issue.identifier;
                                crate::shell::navigate(
                                    Route::LinearIssue { identifier },
                                    window,
                                    cx,
                                );
                            }
                        });
                        open(team, client, None, on_created, window, cx);
                    }
                    Err(message) => {
                        window.push_notification(
                            Notification::error(message).title("Could not start a new issue"),
                            cx,
                        );
                    }
                }
            })
            .ok();
        })
        .detach();
}

/// Opens the modal for `team`, starting in `state_id` when given (a board
/// column's "+") or else in the team's first Todo state. A draft left for
/// the same team comes back, in that column when one was given.
pub(super) fn open(
    team: Arc<Team>,
    client: Arc<Linear>,
    state_id: Option<String>,
    on_created: OnCreated,
    window: &mut Window,
    cx: &mut App,
) {
    let draft = draft_for(&client, cx).filter(|draft| draft.fields.team_id == team.id);
    let form =
        cx.new(|cx| NewIssueForm::new(team, client, state_id, draft, on_created, window, cx));
    let focus_title = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus_title.replace(false) {
            form.read(cx).title.focus_handle(cx).focus(window, cx);
        }
        let sending = form.read(cx).sending();
        let width = if form.read(cx).expanded {
            window.viewport_size().width
        } else {
            dp(WIDTH)
        };
        dialog
            .w(width)
            .centered()
            .p_0()
            .bg(tokens::overlay())
            .keyboard(false)
            // A create in flight must land in a living form, or its draft
            // would survive the issue and make a second one.
            .overlay_closable(!sending)
            .close_button(false)
            .child(form.clone())
    });
}

/// The text and properties of a modal that closed without creating. Like
/// Linear's local draft, it lives until the app quits.
#[derive(Clone, Debug)]
struct Draft {
    fields: Fields,
    title: String,
    description: String,
}

/// The draft and the client it was written for: a draft of another
/// workspace names teams this one does not have.
#[derive(Default)]
struct DraftSlot {
    owner: Option<Weak<Linear>>,
    draft: Option<Draft>,
}

impl Global for DraftSlot {}

/// A `compose` is loading the catalog of the team it will open the modal
/// for.
#[derive(Default)]
struct Composing(bool);

impl Global for Composing {}

fn draft_for(client: &Arc<Linear>, cx: &App) -> Option<Draft> {
    let slot = cx.try_global::<DraftSlot>()?;
    let owner = slot.owner.as_ref()?.upgrade()?;
    Arc::ptr_eq(&owner, client)
        .then(|| slot.draft.clone())
        .flatten()
}

/// Where the form is in its life: editing, waiting for Linear, or showing why
/// the last attempt failed.
enum Submit {
    Idle,
    Sending,
    Failed(SharedString),
}

/// The menu on screen and the row its keyboard cursor is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenMenu {
    property: Property,
    cursor: usize,
}

struct NewIssueForm {
    client: Arc<Linear>,
    team: Arc<Team>,
    /// The teams the issue can move to; the current one until the list
    /// loads.
    teams: Vec<TeamSummary>,
    /// The team whose catalog is loading after a pick in the team menu.
    loading_team: Option<String>,
    fields: Fields,
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    /// The search field of the open menu.
    search: Entity<InputState>,
    menu: Option<OpenMenu>,
    menu_scroll: ScrollHandle,
    /// The modal itself: focused while no field or menu is, so the
    /// property keys reach it.
    focus: FocusHandle,
    create_more: bool,
    expanded: bool,
    submit: Submit,
    /// "Created ENG-12", after an issue made with "Create more" on.
    created: Option<SharedString>,
    /// The draft slot holds this modal's draft: it came back from there,
    /// or this modal has written text. A modal that only borrowed the slot
    /// leaves another team's draft alone until it has text of its own.
    owns_draft: bool,
    on_created: OnCreated,
    _subscriptions: Vec<Subscription>,
}

impl NewIssueForm {
    fn new(
        team: Arc<Team>,
        client: Arc<Linear>,
        state_id: Option<String>,
        draft: Option<Draft>,
        on_created: OnCreated,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owns_draft = draft.is_some();
        let (mut fields, title_text, description_text) = match draft {
            Some(draft) => (draft.fields, draft.title, draft.description),
            None => (Fields::new(&team, None), String::new(), String::new()),
        };
        if let Some(state_id) = state_id.filter(|id| team.states.iter().any(|s| &s.id == id)) {
            fields.state_id = Some(state_id);
        }
        let title = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Issue title");
            state.set_value(title_text, window, cx);
            state
        });
        let description = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .auto_grow(4, 14)
                .placeholder("Add description\u{2026}");
            state.set_value(description_text, window, cx);
            state
        });
        let search = cx.new(|cx| InputState::new(window, cx));
        let subscriptions =
            vec![
                cx.subscribe_in(&title, window, |form, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter {
                            secondary: false, ..
                        } => {
                            form.description.focus_handle(cx).focus(window, cx);
                        }
                        InputEvent::Change => form.text_changed(cx),
                        _ => {}
                    }
                }),
                cx.subscribe(&description, |form, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        form.text_changed(cx);
                    }
                }),
                cx.subscribe(&search, |form, _, event: &InputEvent, cx| {
                    if let (InputEvent::Change, Some(open)) = (event, form.menu) {
                        let cursor = first_cursor(&form.rows(open.property, cx));
                        form.menu = Some(OpenMenu { cursor, ..open });
                        form.menu_scroll.scroll_to_item(cursor);
                        cx.notify();
                    }
                }),
                Self::keys(cx),
            ];

        let summary = TeamSummary {
            id: team.id.clone(),
            key: team.key.clone(),
            name: team.name.clone(),
        };
        let my_teams = data::my_teams(cx);
        cx.spawn(async move |this, cx| {
            let Ok(teams) = my_teams.await else {
                return;
            };
            this.update(cx, |form, cx| {
                let mut teams = teams.to_vec();
                // A board of a team the user is not a member of can still
                // create in it.
                if !teams.iter().any(|team| team.id == form.team.id) {
                    teams.insert(0, form.teams[0].clone());
                }
                form.teams = teams;
                cx.notify();
            })
            .ok();
        })
        .detach();

        NewIssueForm {
            client,
            team,
            teams: vec![summary],
            loading_team: None,
            fields,
            title,
            description,
            search,
            menu: None,
            menu_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            create_more: false,
            expanded: false,
            submit: Submit::Idle,
            created: None,
            owns_draft,
            on_created,
            _subscriptions: subscriptions,
        }
    }

    /// The keys that act wherever the focus is in the modal, ahead of the
    /// text fields and the dialog: Cmd+Enter creates (a textarea would
    /// insert a newline), Tab and Escape move between the text and the
    /// modal, and a menu takes the arrows and Enter from its search field.
    fn keys(cx: &mut Context<Self>) -> Subscription {
        let form = cx.weak_entity();
        cx.intercept_keystrokes(move |event, window, cx| {
            form.update(cx, |form, cx| form.key(&event.keystroke, window, cx))
                .ok();
        })
    }

    fn key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let search = self.search.focus_handle(cx);
        if !(self.focus.contains_focused(window, cx) || search.is_focused(window)) {
            return;
        }
        let key = keystroke.key.as_str();
        let modifiers = keystroke.modifiers;
        if key == "enter" && modifiers.secondary(cx) && !modifiers.shift {
            cx.stop_propagation();
            self.send(window, cx);
            return;
        }
        if let Some(open) = self.menu.filter(|_| search.is_focused(window)) {
            if modifiers.modified() {
                return;
            }
            let count = self.rows(open.property, cx).len();
            if let Some(cursor) = step_cursor(open.cursor, count, key) {
                self.menu = Some(OpenMenu { cursor, ..open });
                self.menu_scroll.scroll_to_item(cursor);
                cx.notify();
            } else if key == "enter" {
                self.pick(open.cursor, window, cx);
            } else if key == "escape" {
                self.close_menu(window, cx);
            } else {
                return;
            }
            cx.stop_propagation();
            return;
        }
        let title = self.title.focus_handle(cx);
        let description = self.description.focus_handle(cx);
        let target = match (key, modifiers.shift) {
            _ if modifiers.secondary(cx) || modifiers.alt || modifiers.control => return,
            ("tab", false) if title.is_focused(window) => description,
            ("tab", true) if description.is_focused(window) => title,
            ("tab", false) | ("escape", _) if description.is_focused(window) => self.focus.clone(),
            ("escape", _) if title.is_focused(window) => self.focus.clone(),
            ("escape", _) if self.menu.is_none() => {
                cx.stop_propagation();
                if !self.sending() {
                    window.close_dialog(cx);
                }
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        target.focus(window, cx);
        cx.notify();
    }

    fn sending(&self) -> bool {
        matches!(self.submit, Submit::Sending)
    }

    fn busy(&self) -> bool {
        self.sending() || self.loading_team.is_some()
    }

    fn text_changed(&mut self, cx: &mut Context<Self>) {
        if matches!(self.submit, Submit::Failed(_)) {
            self.submit = Submit::Idle;
        }
        self.save_draft(cx);
        cx.notify();
    }

    /// Keeps the draft in step with the modal, so closing it any way (the
    /// close button, Escape, a click outside) leaves it. Blank text is no
    /// draft.
    fn save_draft(&mut self, cx: &mut App) {
        let title = self.title.read(cx).value().to_string();
        let description = self.description.read(cx).value().to_string();
        let draft = (!title.trim().is_empty() || !description.trim().is_empty()).then(|| Draft {
            fields: self.fields.clone(),
            title,
            description,
        });
        if draft.is_none() && !self.owns_draft {
            return;
        }
        self.owns_draft = draft.is_some();
        let slot = cx.default_global::<DraftSlot>();
        slot.owner = Some(Arc::downgrade(&self.client));
        slot.draft = draft;
    }

    fn rows(&self, property: Property, cx: &App) -> Vec<Choice> {
        let query = self.search.read(cx).value();
        let today = chrono::Local::now().date_naive();
        choices(
            property,
            &self.team,
            &self.teams,
            &self.fields,
            &query,
            today,
        )
    }

    /// Opens `property`'s menu with an empty search and the cursor on the
    /// current value. The popover moves the focus to its search field.
    fn open_menu(&mut self, property: Property, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || !property.applies_to(&self.team) {
            return;
        }
        self.search.update(cx, |search, cx| {
            search.set_value("", window, cx);
            search.set_placeholder(property.placeholder(), window, cx);
        });
        let cursor = first_cursor(&self.rows(property, cx));
        self.menu = Some(OpenMenu { property, cursor });
        self.menu_scroll.scroll_to_item(cursor);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Applies row `index` of the open menu. Labels keep the menu open; any
    /// other pick closes it and leaves the focus on the modal, ready for the
    /// next key.
    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu.filter(|_| !self.busy()) else {
            return;
        };
        let Some(choice) = self.rows(open.property, cx).into_iter().nth(index) else {
            return;
        };
        match &choice.pick {
            Pick::Team(team_id) => {
                if *team_id != self.fields.team_id {
                    self.switch_team(team_id.clone(), cx);
                }
            }
            pick => self.fields.apply(pick, &self.team),
        }
        if open.property.keeps_open() {
            self.menu = Some(OpenMenu {
                cursor: index,
                ..open
            });
            // A click moved the focus to the popover: typing and the arrows
            // belong to the search field.
            self.search.focus_handle(cx).focus(window, cx);
        } else {
            self.close_menu(window, cx);
        }
        self.save_draft(cx);
        cx.notify();
    }

    /// Loads `team_id`'s catalog and moves the issue there, keeping what the
    /// team also has.
    fn switch_team(&mut self, team_id: String, cx: &mut Context<Self>) {
        // The catalog cache follows the current client; this modal creates
        // with the one it opened with.
        if !data::client(cx).is_some_and(|current| Arc::ptr_eq(&current, &self.client)) {
            self.submit =
                Submit::Failed("Linear's workspace changed. Close this and start again.".into());
            cx.notify();
            return;
        }
        self.loading_team = Some(team_id.clone());
        let task = data::team(team_id, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |form, cx| {
                form.loading_team = None;
                match result {
                    Ok(team) => {
                        form.fields.retarget(&team);
                        form.team = team;
                        form.save_draft(cx);
                    }
                    Err(message) => form.submit = Submit::Failed(message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let title = self.title.read(cx).value();
        let description = self.description.read(cx).value();
        let Some(input) = self.fields.request(&title, &description) else {
            self.submit = Submit::Failed("Give the issue a title.".into());
            self.title.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        };

        self.submit = Submit::Sending;
        self.created = None;
        cx.notify();
        let client = self.client.clone();
        let task = services::background(cx, move || client.create_issue(&input));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |form, window, cx| match result {
                Ok(issue) => form.created(issue, window, cx),
                Err(error) => {
                    form.submit = Submit::Failed(data::error_text(&error));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// With "Create more" on, the modal stays with the same properties and
    /// empty text for the next issue; otherwise it closes. Either way the
    /// draft is gone.
    fn created(&mut self, issue: IssueSummary, window: &mut Window, cx: &mut Context<Self>) {
        self.submit = Submit::Idle;
        if self.create_more {
            self.created = Some(format!("Created {}", issue.identifier).into());
            self.title
                .update(cx, |title, cx| title.set_value("", window, cx));
            self.description
                .update(cx, |description, cx| description.set_value("", window, cx));
            self.save_draft(cx);
            self.title.focus_handle(cx).focus(window, cx);
            (self.on_created)(issue, true, window, cx);
        } else {
            if self.owns_draft {
                cx.default_global::<DraftSlot>().draft = None;
            }
            window.close_dialog(cx);
            (self.on_created)(issue, false, window, cx);
        }
        cx.notify();
    }

    /// The breadcrumb's team: the team's key, a button that opens the team
    /// menu.
    fn team_button(&self, cx: &mut Context<Self>) -> Popover {
        let loading = self.loading_team.is_some();
        let trigger = Button::new("new-issue-team")
            .ghost()
            .small()
            .px_1p5()
            .disabled(self.sending())
            .selected(
                self.menu
                    .is_some_and(|open| open.property == Property::Team),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_size(scale::body())
                    .child(svg_icon(IconName::Users, dp(14.), tokens::text_faint()))
                    .child(if loading {
                        SharedString::from("\u{2026}")
                    } else {
                        self.team.key.clone().into()
                    }),
            )
            .accessibility_label(format!("Team: {}", self.team.name))
            .tooltip(format!(
                "Team: {} ({})",
                self.team.name,
                Property::Team.key()
            ));
        self.menu_popover(Property::Team, trigger, cx)
    }

    /// The chip of `property`: its value, or its name in the muted color
    /// while it has none.
    fn chip(&self, property: Property, cx: &mut Context<Self>) -> Popover {
        let (mark, text, set) = self.chip_value(property, cx);
        let muted = cx.theme().muted_foreground;
        let trigger = Button::new(SharedString::from(format!("new-issue-{}", slug(property))))
            .outline()
            .small()
            .px_2()
            .disabled(self.busy())
            .selected(self.menu.is_some_and(|open| open.property == property))
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_size(scale::meta())
                    .when(!set, |chip| chip.text_color(muted))
                    .child(mark)
                    .child(text.clone()),
            )
            .accessibility_label(format!("{}: {text}", property.name()))
            .tooltip(format!("{} ({})", property.name(), property.key()));
        self.menu_popover(property, trigger, cx)
    }

    /// What a chip shows: a mark, a text, and whether the property is set.
    fn chip_value(&self, property: Property, cx: &App) -> (AnyElement, String, bool) {
        let team = &self.team;
        let fields = &self.fields;
        let icon =
            |name: IconName| svg_icon(name, dp(14.), tokens::text_faint()).into_any_element();
        let unset = |name: IconName| (icon(name), property.name().to_owned(), false);
        match property {
            Property::Team => (icon(IconName::Users), team.key.clone(), true),
            Property::State => {
                match team
                    .states
                    .iter()
                    .find(|state| fields.state_id.as_ref() == Some(&state.id))
                {
                    Some(state) => (
                        widgets::state_icon(&state_ref(state), cx),
                        state.name.clone(),
                        true,
                    ),
                    None => unset(IconName::CircleDashed),
                }
            }
            Property::Priority => (
                widgets::priority_icon(fields.priority, cx),
                if fields.priority == Priority::None {
                    property.name().to_owned()
                } else {
                    fields.priority.label().to_owned()
                },
                fields.priority != Priority::None,
            ),
            Property::Assignee => {
                match team
                    .members
                    .iter()
                    .find(|member| fields.assignee_id.as_ref() == Some(&member.id))
                {
                    Some(member) => (
                        widgets::avatar(Some(member), dp(MARK)),
                        member.name.clone(),
                        true,
                    ),
                    None => (widgets::avatar(None, dp(MARK)), "Assignee".into(), false),
                }
            }
            Property::Labels => {
                let on: Vec<_> = team
                    .labels
                    .iter()
                    .filter(|label| fields.label_ids.contains(&label.id))
                    .collect();
                match on.as_slice() {
                    [] => unset(IconName::Tag),
                    [label] => (swatch(Some(&label.color), cx), label.name.clone(), true),
                    [first, ..] => (
                        swatch(Some(&first.color), cx),
                        format!("{} labels", on.len()),
                        true,
                    ),
                }
            }
            Property::Project => {
                match team
                    .projects
                    .iter()
                    .find(|project| fields.project_id.as_ref() == Some(&project.id))
                {
                    Some(project) => (
                        match &project.color {
                            Some(color) => swatch(Some(color), cx),
                            None => icon(IconName::Box),
                        },
                        project.name.clone(),
                        true,
                    ),
                    None => unset(IconName::Box),
                }
            }
            Property::Cycle => {
                match team
                    .cycles
                    .iter()
                    .find(|cycle| fields.cycle_id.as_ref() == Some(&cycle.id))
                {
                    Some(cycle) => (
                        icon(IconName::RefreshCw),
                        cycle_name(cycle.number, cycle.name.as_deref()),
                        true,
                    ),
                    None => unset(IconName::RefreshCw),
                }
            }
            Property::Estimate => match fields.estimate {
                Some(points) => (
                    icon(IconName::Triangle),
                    team.estimation.label(points),
                    true,
                ),
                None => unset(IconName::Triangle),
            },
            Property::DueDate => match fields.due_date {
                Some(date) => (
                    icon(IconName::CalendarClock),
                    date_text(date, chrono::Local::now().date_naive()),
                    true,
                ),
                None => unset(IconName::CalendarClock),
            },
        }
    }

    /// `trigger` with `property`'s menu hanging from it. The popover is
    /// controlled by `NewIssueForm::menu`, so a click and the property's key
    /// open it alike.
    fn menu_popover(&self, property: Property, trigger: Button, cx: &mut Context<Self>) -> Popover {
        let open = self.menu.filter(|open| open.property == property);
        let rows = Rc::new(match open {
            Some(_) => self.rows(property, cx),
            None => Vec::new(),
        });
        let cursor = open.map_or(0, |open| open.cursor);
        let view = cx.weak_entity();
        let (on_change, on_click) = (view.clone(), view.clone());
        let search = self.search.clone();
        let scroll = self.menu_scroll.clone();
        Popover::new(slug(property))
            .anchor(Anchor::TopLeft)
            .p_1()
            .w(dp(280.))
            .open(open.is_some())
            .track_focus(&self.search.focus_handle(cx))
            .on_open_change(move |open, window, cx| {
                on_change
                    .update(cx, |form, cx| {
                        // Another menu may have opened since: only the one
                        // that reports closing clears the state.
                        let this = form.menu.is_some_and(|now| now.property == property);
                        if !*open && this {
                            form.close_menu(window, cx);
                        }
                    })
                    .ok();
            })
            .trigger(trigger.on_click(move |_, window, cx| {
                on_click
                    .update(cx, |form, cx| form.open_menu(property, window, cx))
                    .ok();
            }))
            .content(move |_, _, cx| menu(&search, rows.clone(), cursor, &scroll, view.clone(), cx))
    }

    fn key_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        fn open<A: Action>(
            property: Property,
        ) -> impl Fn(&mut NewIssueForm, &A, &mut Window, &mut Context<NewIssueForm>) {
            move |form, _, window, cx| form.open_menu(property, window, cx)
        }
        root.on_action(cx.listener(open::<ChangeState>(Property::State)))
            .on_action(cx.listener(open::<ChangePriority>(Property::Priority)))
            .on_action(cx.listener(open::<ChangeAssignee>(Property::Assignee)))
            .on_action(cx.listener(open::<ChangeLabels>(Property::Labels)))
            .on_action(cx.listener(open::<SetProject>(Property::Project)))
            .on_action(cx.listener(open::<SetCycle>(Property::Cycle)))
            .on_action(cx.listener(open::<SetEstimate>(Property::Estimate)))
            .on_action(cx.listener(open::<SetDueDate>(Property::DueDate)))
            .on_action(cx.listener(open::<MoveToTeam>(Property::Team)))
    }
}

/// The element ids of a property's chip and menu.
fn slug(property: Property) -> &'static str {
    match property {
        Property::Team => "team",
        Property::State => "status",
        Property::Priority => "priority",
        Property::Assignee => "assignee",
        Property::Labels => "labels",
        Property::Project => "project",
        Property::Cycle => "cycle",
        Property::Estimate => "estimate",
        Property::DueDate => "due-date",
    }
}

/// A color dot in a 16 px slot: a label's or a project's color.
fn swatch(color: Option<&String>, cx: &App) -> AnyElement {
    h_flex()
        .size(dp(MARK))
        .flex_none()
        .justify_center()
        .child(
            div().size_2().rounded_full().bg(color
                .and_then(|color| widgets::hex_color(color))
                .unwrap_or(cx.theme().muted_foreground)),
        )
        .into_any_element()
}

fn mark(mark: &Mark, cx: &App) -> AnyElement {
    let icon = |name: IconName| svg_icon(name, dp(MARK), tokens::text_faint()).into_any_element();
    match mark {
        Mark::Team => icon(IconName::Users),
        Mark::State(state) => widgets::state_icon(state, cx),
        Mark::Priority(priority) => widgets::priority_icon(*priority, cx),
        Mark::Person(user) => widgets::avatar(user.as_ref(), dp(MARK)),
        Mark::Swatch(color) => swatch(color.as_ref(), cx),
        Mark::Project => icon(IconName::Box),
        Mark::Cycle => icon(IconName::RefreshCw),
        Mark::Estimate => icon(IconName::Triangle),
        Mark::Date => icon(IconName::CalendarClock),
        Mark::None(property) => icon(match property {
            Property::Project => IconName::Box,
            Property::Cycle => IconName::RefreshCw,
            Property::Estimate => IconName::Triangle,
            Property::DueDate => IconName::CalendarClock,
            _ => IconName::CircleDashed,
        }),
    }
}

/// An open menu: its search field over its rows. The fill is the keyboard
/// cursor; a click picks a row as Enter does.
fn menu(
    search: &Entity<InputState>,
    rows: Rc<Vec<Choice>>,
    cursor: usize,
    scroll: &ScrollHandle,
    view: WeakEntity<NewIssueForm>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (fill, hover, faint) = (theme.list_active, theme.accent, tokens::text_faint());
    let radius = theme.radius;
    let list = if rows.is_empty() {
        div()
            .px_2()
            .py_1p5()
            .text_size(scale::body())
            .text_color(theme.muted_foreground)
            .child("No results")
            .into_any_element()
    } else {
        v_flex()
            .id("new-issue-menu-rows")
            .track_scroll(scroll)
            .max_h(dp(300.))
            .overflow_y_scroll()
            .gap_0p5()
            .children(rows.iter().enumerate().map(|(index, choice)| {
                let view = view.clone();
                let name = if choice.selected {
                    format!("{}, selected", choice.name)
                } else {
                    choice.name.clone()
                };
                h_flex()
                    .id(SharedString::from(choice.id.clone()))
                    .role(Role::MenuItem)
                    .aria_label(name)
                    .h(dp(ROW_HEIGHT))
                    .flex_none()
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(radius)
                    .text_size(scale::body())
                    .when(index == cursor, |row| row.bg(fill))
                    .hover(move |row| row.bg(hover))
                    .child(mark(&choice.mark, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(choice.name.clone()),
                    )
                    .children(choice.detail.clone().map(|detail| {
                        div()
                            .flex_none()
                            .text_size(scale::meta())
                            .text_color(faint)
                            .child(detail)
                    }))
                    .when(choice.selected, |row| {
                        row.child(svg_icon(IconName::Check, dp(MARK), theme.foreground))
                    })
                    .on_click(move |_, window, cx| {
                        view.update(cx, |form, cx| form.pick(index, window, cx))
                            .ok();
                    })
            }))
            .into_any_element()
    };
    v_flex()
        .gap_1()
        .child(
            Input::new(search)
                .appearance(false)
                .small()
                .text_size(scale::body()),
        )
        .child(div().h_px().bg(tokens::border_subtle()))
        .child(list)
        .into_any_element()
}

impl Render for NewIssueForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, faint) = (theme.muted_foreground, theme.danger, tokens::text_faint());
        let sending = self.sending();
        let expanded = self.expanded;
        let message = match &self.submit {
            Submit::Failed(message) => Some((message.clone(), danger)),
            Submit::Idle | Submit::Sending => self.created.clone().map(|text| (text, muted)),
        };
        let shortcut = if cfg!(target_os = "macos") {
            "\u{2318}\u{21a9}"
        } else {
            "Ctrl+Enter"
        };

        let header = h_flex()
            .h_11()
            .flex_none()
            .pl_3()
            .pr_2()
            .gap_1()
            .items_center()
            .child(self.team_button(cx))
            .child(svg_icon(IconName::ChevronRight, dp(14.), faint))
            .child(
                div()
                    .px_1()
                    .text_size(scale::body())
                    .text_color(muted)
                    .child("New issue"),
            )
            .child(div().flex_1())
            .child(
                ui::icon_button(
                    "new-issue-expand",
                    if expanded {
                        IconName::Minimize2
                    } else {
                        IconName::Maximize2
                    },
                    if expanded {
                        "Exit full screen"
                    } else {
                        "Full screen"
                    },
                )
                .on_click(cx.listener(|form, _, _, cx| {
                    form.expanded = !form.expanded;
                    cx.notify();
                })),
            )
            .child(
                ui::icon_button("new-issue-close", IconName::Close, "Close")
                    .disabled(sending)
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            );

        // The text area takes what the header, the chips, and the footer
        // leave of a full-window modal.
        let full_height = window.viewport_size().height - dp(48.);
        // Extra small keeps the editor's own inset to 4 px; the title is
        // inset as much, so the two start on one edge.
        let description = Textarea::new(&self.description)
            .appearance(false)
            .xsmall()
            .px_0()
            .text_size(rems(0.875))
            .aria_label("Description")
            .disabled(sending)
            .when(expanded, |text| text.h(full_height - dp(220.)));

        let chips = Property::CHIPS
            .into_iter()
            .filter(|property| property.applies_to(&self.team))
            .map(|property| self.chip(property, cx).into_any_element())
            .collect::<Vec<_>>();

        let create = Button::new("new-issue-create")
            .primary()
            .label(if sending {
                "Creating\u{2026}"
            } else {
                "Create issue"
            })
            .loading(sending)
            .disabled(self.busy())
            .tooltip(format!("Create issue ({shortcut})"))
            .on_click(cx.listener(|form, _, window, cx| form.send(window, cx)));

        let footer = h_flex()
            .flex_none()
            .px_4()
            .py_3()
            .gap_4()
            .items_center()
            .border_t_1()
            .border_color(tokens::border_subtle())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::meta())
                    .children(message.map(|(text, color)| div().text_color(color).child(text))),
            )
            .child(
                Switch::new("new-issue-create-more")
                    .checked(self.create_more)
                    .label("Create more")
                    .small()
                    .tooltip("Keep this open to create the next issue")
                    .on_click(cx.listener(|form, checked: &bool, _, cx| {
                        form.create_more = *checked;
                        cx.notify();
                    })),
            )
            .child(create);

        let root = v_flex()
            .key_context("NewIssue")
            .track_focus(&self.focus)
            .w_full()
            .when(expanded, |root| root.h(full_height))
            .child(header)
            .child(
                v_flex()
                    .px_4()
                    .gap_1()
                    .when(expanded, |body| body.flex_1().min_h_0())
                    .child(
                        Input::new(&self.title)
                            .appearance(false)
                            .pl_1()
                            .pr_0()
                            .h(dp(36.))
                            .text_size(rems(1.375))
                            .font_weight(FontWeight::SEMIBOLD)
                            .aria_label("Issue title")
                            .disabled(sending),
                    )
                    .child(description),
            )
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .px_4()
                    .pt_2()
                    .pb_4()
                    .gap_1p5()
                    .children(chips),
            )
            .child(footer);
        self.key_actions(root, cx)
    }
}
