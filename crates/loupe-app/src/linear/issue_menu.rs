//! What an issue's context menu and the command palette change on it: its
//! status, priority, assignee, labels, and project, each a list of choices
//! from the team's catalog. A pick shows on every screen at once (through
//! [`data::publish_issue`]) and goes to Linear; if Linear refuses it, the
//! issue comes back as it was and a notification says why.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Side, WindowExt as _, h_flex};
use gpui_kit::*;
use loupe_linear::{IssueSummary, IssueUpdate, Team};

use super::data;
use super::issue::optimistic;
use super::issue::properties::{self, Choice, Mark, Pick, Property};
use crate::services::background;
use crate::theme::scale::dp;

/// A property of an issue that its menu and the palette change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Status,
    Priority,
    Assignee,
    Labels,
    Project,
}

impl Field {
    pub const ALL: [Field; 5] = [
        Field::Status,
        Field::Priority,
        Field::Assignee,
        Field::Labels,
        Field::Project,
    ];

    /// The menu's name for it, as Linear's.
    pub fn label(self) -> &'static str {
        match self {
            Field::Status => "Status",
            Field::Priority => "Priority",
            Field::Assignee => "Assignee",
            Field::Labels => "Labels",
            Field::Project => "Project",
        }
    }

    /// The palette's name for the command that opens its list.
    pub fn command(self) -> &'static str {
        match self {
            Field::Status => "Change status…",
            Field::Priority => "Change priority…",
            Field::Assignee => "Assign to…",
            Field::Labels => "Change labels…",
            Field::Project => "Move to project…",
        }
    }

    /// What the palette's field says while its list is open.
    pub fn placeholder(self, identifier: &str) -> String {
        match self {
            Field::Status => format!("Change the status of {identifier}…"),
            Field::Priority => format!("Change the priority of {identifier}…"),
            Field::Assignee => format!("Assign {identifier} to…"),
            Field::Labels => format!("Add or remove a label on {identifier}…"),
            Field::Project => format!("Move {identifier} to a project…"),
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Field::Status => IconName::CircleDashed,
            Field::Priority => IconName::SignalHigh,
            Field::Assignee => IconName::CircleUser,
            Field::Labels => IconName::Tag,
            Field::Project => IconName::Box,
        }
    }

    /// The action whose key opens the list, the same key an issue's page
    /// uses for its picker.
    pub fn action(self) -> Box<dyn Action> {
        match self {
            Field::Status => Box::new(super::ChangeState),
            Field::Priority => Box::new(super::ChangePriority),
            Field::Assignee => Box::new(super::ChangeAssignee),
            Field::Labels => Box::new(super::ChangeLabels),
            Field::Project => Box::new(super::SetProject),
        }
    }
}

/// One choice of a field's list.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldChoice(Choice);

impl FieldChoice {
    pub fn id(&self) -> &str {
        &self.0.id
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The issue has this value now.
    pub fn selected(&self) -> bool {
        self.0.selected
    }

    /// The 16 px mark before the name: the state's icon, the priority's
    /// bars, the avatar, the label's or the project's color.
    pub fn mark(&self, cx: &App) -> AnyElement {
        properties::mark(&self.0.mark, cx)
    }

    /// What picking it sends for `issue`. A label flips: added when the
    /// issue lacks it, removed when it has it, the other labels untouched.
    pub fn update_for(&self, issue: &IssueSummary) -> IssueUpdate {
        match &self.0.pick {
            Pick::Update(update) => (**update).clone(),
            Pick::Label(label_id) => {
                let had = issue.labels.iter().any(|label| &label.id == label_id);
                optimistic::label_delta(label_id, !had)
            }
        }
    }
}

/// `field`'s choices for `issue` from `team`'s catalog, the current value
/// marked. The list of each field is the one its picker on the issue's page
/// shows; Project lists "No project", then the team's projects.
pub fn choices(field: Field, team: &Team, issue: &IssueSummary) -> Vec<FieldChoice> {
    let property = match field {
        Field::Status => Property::State,
        Field::Priority => Property::Priority,
        Field::Assignee => Property::Assignee,
        Field::Labels => Property::Labels,
        Field::Project => return project_choices(team, issue),
    };
    properties::choices(property, team, issue)
        .into_iter()
        .map(FieldChoice)
        .collect()
}

fn project_choices(team: &Team, issue: &IssueSummary) -> Vec<FieldChoice> {
    let current = issue.project.as_ref().map(|project| project.id.as_str());
    let none = Choice {
        id: "project-none".into(),
        name: "No project".into(),
        mark: Mark::Project(None),
        selected: current.is_none(),
        pick: Pick::Update(Box::new(IssueUpdate {
            project_id: Some(None),
            ..IssueUpdate::default()
        })),
    };
    std::iter::once(none)
        .chain(team.projects.iter().map(|project| Choice {
            id: format!("project-{}", project.id),
            name: project.name.clone(),
            mark: Mark::Project(project.color.clone()),
            selected: current == Some(project.id.as_str()),
            pick: Pick::Update(Box::new(IssueUpdate {
                project_id: Some(Some(project.id.clone())),
                ..IssueUpdate::default()
            })),
        }))
        .map(FieldChoice)
        .collect()
}

/// The issue's team catalog if a screen already loaded it.
pub fn cached_team(issue: &IssueSummary, cx: &mut App) -> Option<Arc<Team>> {
    data::cached_team(&issue.team.id, cx)
}

/// Loads the issue's team catalog, from the cache when it is there.
pub fn load_team(issue: &IssueSummary, cx: &mut App) -> Task<Result<Arc<Team>, SharedString>> {
    data::team(issue.team.id.clone(), cx)
}

/// Picks `choice` for `issue`, the copy the user saw it on: see [`change`].
pub fn pick(
    issue: &IssueSummary,
    choice: &FieldChoice,
    team: Arc<Team>,
    window: &mut Window,
    cx: &mut App,
) {
    let update = choice.update_for(issue);
    change(freshest(issue, cx), update, team, window, cx);
}

/// The newest copy of `issue` the app has shown: the last one published,
/// when it is this issue. A row or a palette list may hold an older one.
pub fn freshest(issue: &IssueSummary, cx: &App) -> IssueSummary {
    cx.try_global::<data::IssueChange>()
        .and_then(|change| change.issue.as_ref())
        .filter(|latest| latest.id == issue.id)
        .unwrap_or(issue)
        .clone()
}

/// The picks of each issue still waiting for Linear: the number of the
/// last one, and the last copy Linear returned (or the one shown before
/// the first pick). Only the last pick's answer is shown, so an older
/// answer that lands late never hides a newer pick.
#[derive(Default)]
struct Picks {
    next: u64,
    issues: HashMap<String, Waiting>,
}

struct Waiting {
    last: u64,
    base: IssueSummary,
}

impl Global for Picks {}

/// Shows `update` on `before` on every screen at once and sends it to
/// Linear. The answer to the issue's last pick replaces what is shown. If
/// Linear refuses that pick, the issue is read again from Linear (or, when
/// that fails too, its last known copy comes back) and a notification says
/// why.
pub fn change(
    before: IssueSummary,
    update: IssueUpdate,
    team: Arc<Team>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = data::client(cx) else {
        window.push_notification(
            Notification::error("Connect Linear in Settings.").title("Could not change the issue"),
            cx,
        );
        return;
    };
    let picks = cx.default_global::<Picks>();
    let seq = picks.next;
    picks.next += 1;
    picks
        .issues
        .entry(before.id.clone())
        .or_insert_with(|| Waiting {
            last: seq,
            base: before.clone(),
        })
        .last = seq;
    let mut shown = before.clone();
    optimistic::apply(&mut shown, &update, &team);
    data::publish_shown(shown, cx);
    let issue_id = before.id.clone();
    let task = background(cx, move || client.update_issue(&issue_id, &update));
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            cx.update(|window, cx| {
                let picks = cx.default_global::<Picks>();
                let Some(waiting) = picks.issues.get_mut(&before.id) else {
                    return;
                };
                let last = waiting.last == seq;
                match result {
                    Ok(server) if last => {
                        picks.issues.remove(&before.id);
                        data::publish_issue(server, cx);
                    }
                    Ok(server) => waiting.base = server,
                    Err(error) => {
                        let base = if last {
                            picks.issues.remove(&before.id).map(|waiting| waiting.base)
                        } else {
                            None
                        };
                        window.push_notification(
                            Notification::error(data::error_text(&error))
                                .title(format!("Could not change {}", before.identifier)),
                            cx,
                        );
                        if let Some(base) = base {
                            reload(before.identifier.clone(), base, cx);
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
}

/// Reads the issue again after Linear refused a pick and shows Linear's
/// copy; when Linear cannot be read either, shows `base`, the last copy
/// Linear returned.
fn reload(identifier: String, base: IssueSummary, cx: &mut App) {
    let Some(client) = data::client(cx) else {
        data::publish_shown(base, cx);
        return;
    };
    let read = background(cx, move || client.issue(&identifier));
    cx.spawn(async move |cx| {
        let result = read.await;
        cx.update(|cx| match result {
            Ok(detail) => data::publish_issue(detail.issue, cx),
            Err(_) => data::publish_shown(base, cx),
        });
    })
    .detach();
}

/// Fills the submenu of `field` in an issue's context menu: its choices with
/// a check on the current one, or one disabled row while the team's catalog
/// loads. A click changes the issue.
pub fn fill_submenu(
    menu: PopupMenu,
    field: Field,
    issue: &IssueSummary,
    team: Option<Arc<Team>>,
) -> PopupMenu {
    let menu = menu
        .min_w(dp(200.))
        .max_h(dp(360.))
        .scrollable(true)
        .check_side(Side::Right);
    let Some(team) = team else {
        return menu.item(PopupMenuItem::new("Loading…").disabled(true));
    };
    choices(field, &team, issue)
        .into_iter()
        .fold(menu, |menu, choice| {
            let shown = choice.clone();
            let issue = issue.clone();
            let team = team.clone();
            let selected = choice.selected();
            menu.item(
                PopupMenuItem::element(move |_, cx| {
                    let name = if shown.selected() {
                        format!("{}, selected", shown.name())
                    } else {
                        shown.name().to_owned()
                    };
                    // The stock row names only text items; this one draws a
                    // mark, so its content carries the name.
                    h_flex()
                        .id(SharedString::from(format!("menu-{}", shown.id())))
                        .role(Role::Group)
                        .aria_label(name)
                        .gap_2()
                        .items_center()
                        .child(shown.mark(cx))
                        .child(shown.name().to_owned())
                })
                .checked(selected)
                .on_click(move |_, window, cx| pick(&issue, &choice, team.clone(), window, cx)),
            )
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use chrono::DateTime;
    use gpui_kit::{
        AppContext as _, Context, IntoElement, Render, TestAppContext, VisualTestContext, Window,
        div,
    };
    use loupe_linear::{
        IssueSummary, Label, Priority, Project, StateRef, StateType, Team, TeamRef, WorkflowState,
    };

    use super::{Field, choices, pick};
    use crate::linear::data::IssueChange;
    use crate::services::Services;
    use crate::store::Settings;

    fn label(id: &str) -> Label {
        Label {
            id: id.into(),
            name: id.into(),
            color: "#00ff00".into(),
            parent_id: None,
        }
    }

    pub(crate) fn team() -> Team {
        let state = |id: &str, kind: StateType| WorkflowState {
            id: id.into(),
            name: id.into(),
            kind,
            position: 0.,
            color: "#888888".into(),
        };
        Team {
            id: "team".into(),
            key: "ENG".into(),
            name: "Engineering".into(),
            states: vec![
                state("todo", StateType::Unstarted),
                state("done", StateType::Completed),
            ],
            members: Vec::new(),
            labels: vec![label("bug"), label("ui")],
            projects: vec![Project {
                id: "p1".into(),
                name: "Launch".into(),
                color: Some("#ff0000".into()),
            }],
            cycles: Vec::new(),
            label_groups: Vec::new(),
            estimation: Default::default(),
        }
    }

    pub(crate) fn issue() -> IssueSummary {
        let at = DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .unwrap()
            .to_utc();
        IssueSummary {
            id: "i1".into(),
            identifier: "ENG-1".into(),
            title: "Fix it".into(),
            priority: Priority::Urgent,
            sort_order: 0.,
            state: StateRef {
                id: "todo".into(),
                name: "todo".into(),
                kind: StateType::Unstarted,
                color: "#888888".into(),
            },
            team: TeamRef {
                id: "team".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            assignee: None,
            labels: vec![label("bug")],
            project: None,
            cycle: None,
            estimate: None,
            due_date: None,
            url: String::new(),
            branch_name: String::new(),
            created_at: at,
            updated_at: at,
            creator_id: None,
            parent_id: None,
            has_sub_issues: false,
            milestone: None,
            closed_at: None,
            pull_requests: Vec::new(),
            pull_requests_complete: true,
        }
    }

    fn names(field: Field) -> Vec<(String, bool)> {
        choices(field, &team(), &issue())
            .iter()
            .map(|choice| (choice.name().to_owned(), choice.selected()))
            .collect()
    }

    #[test]
    fn each_field_lists_the_teams_values_and_marks_the_issues() {
        assert_eq!(
            names(Field::Status),
            [("todo".to_owned(), true), ("done".to_owned(), false)]
        );
        assert!(names(Field::Priority).contains(&("Urgent".to_owned(), true)));
        assert_eq!(names(Field::Assignee), [("No assignee".to_owned(), true)]);
        assert_eq!(
            names(Field::Labels),
            [("bug".to_owned(), true), ("ui".to_owned(), false)]
        );
        assert_eq!(
            names(Field::Project),
            [
                ("No project".to_owned(), true),
                ("Launch".to_owned(), false)
            ]
        );
    }

    #[test]
    fn picking_a_label_flips_it_and_keeps_the_others() {
        let issue = issue();
        let labels = choices(Field::Labels, &team(), &issue);
        let off = labels[0].update_for(&issue);
        assert_eq!(
            (off.label_ids, off.removed_label_ids, off.added_label_ids),
            (None, vec!["bug".to_owned()], Vec::new()),
            "bug is on: it goes, alone"
        );
        let on = labels[1].update_for(&issue);
        assert_eq!(
            (on.label_ids, on.added_label_ids, on.removed_label_ids),
            (None, vec!["ui".to_owned()], Vec::new()),
            "ui is off: it joins, and the labels Linear holds stay"
        );
        let project = choices(Field::Project, &team(), &issue)[1].update_for(&issue);
        assert_eq!(project.project_id, Some(Some("p1".to_owned())));
    }

    /// Puts `team` in the connected client's cache, so lists of choices
    /// need no request.
    pub(crate) fn cache_team(team: Team, cx: &mut gpui_kit::App) {
        crate::linear::data::cache_team(team, cx);
    }

    /// Shows `issue` on every screen, as a pick does before Linear answers.
    pub(crate) fn publish_shown(issue: IssueSummary, cx: &mut gpui_kit::App) {
        crate::linear::data::publish_shown(issue, cx);
    }

    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// A window with Linear connected to an address that refuses every
    /// request, and the states every published issue had, in order.
    pub(crate) fn refusing_linear(
        cx: &mut TestAppContext,
    ) -> (VisualTestContext, Rc<RefCell<Vec<String>>>) {
        let published = Rc::new(RefCell::new(Vec::new()));
        let record = published.clone();
        let (handle, _) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services {
                linear: Some(Arc::new(loupe_linear::Linear::with_endpoint(
                    "lin_api_test",
                    "http://127.0.0.1:9/graphql",
                ))),
                ..Services::default()
            });
            cx.observe_global::<IssueChange>(move |cx| {
                if let Some(issue) = &cx.global::<IssueChange>().issue {
                    record.borrow_mut().push(issue.state.id.clone());
                }
            })
            .detach();
            gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Blank))
                .expect("open a window")
        });
        (VisualTestContext::from_window(handle, cx), published)
    }

    /// The status changes on every screen before Linear answers, and comes
    /// back when Linear refuses it.
    #[gpui_kit::test]
    fn a_pick_shows_at_once_and_comes_back_when_linear_refuses(cx: &mut TestAppContext) {
        let (mut cx, published) = refusing_linear(cx);
        cx.update(|window, cx| {
            let issue = issue();
            let done = choices(Field::Status, &team(), &issue)
                .into_iter()
                .find(|choice| choice.name() == "done")
                .expect("done");
            pick(&issue, &done, Arc::new(team()), window, cx);
        });
        assert_eq!(
            *published.borrow(),
            ["done"],
            "the new status is out before the request ends"
        );
        cx.run_until_parked();
        assert_eq!(
            *published.borrow(),
            ["done", "todo"],
            "Linear refused it: the issue is back in its status"
        );
    }
    /// Two quick picks that Linear both refuses, with Linear out of reach:
    /// the issue ends as it was before the first, not with the first pick
    /// that the second was built on.
    #[gpui_kit::test]
    fn two_refused_picks_end_where_the_issue_started(cx: &mut TestAppContext) {
        let (mut cx, published) = refusing_linear(cx);
        cx.update(|window, cx| {
            let issue = issue();
            let done = choices(Field::Status, &team(), &issue)
                .into_iter()
                .find(|choice| choice.name() == "done")
                .expect("done");
            pick(&issue, &done, Arc::new(team()), window, cx);
            // The row now shows the first pick; the second starts there.
            let shown = super::freshest(&issue, cx);
            assert_eq!(shown.state.id, "done");
            let low = choices(Field::Priority, &team(), &shown)
                .into_iter()
                .find(|choice| choice.name() == "Low")
                .expect("Low");
            pick(&shown, &low, Arc::new(team()), window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            published.borrow().last().map(String::as_str),
            Some("todo"),
            "Linear holds todo, and so does every screen"
        );
    }
}
