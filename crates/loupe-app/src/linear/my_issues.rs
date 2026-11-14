//! My issues: open issues assigned to me, grouped by workflow state type,
//! with a team picker that opens any team.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_linear::views::ViewConfig;
use loupe_linear::{IssueSummary, Linear, StateType, TeamSummary};

use crate::linear::data::{self, Loadable};
use crate::linear::widgets;
use crate::route::Route;
use crate::services::background;
use crate::shell::OpenOnClick as _;
use crate::shell::item_menu::{Item, with_item_menu};
use crate::shell::navigate;
use crate::theme::scale;
use crate::theme::scale::dp;
use crate::ui;
use crate::ui::MenuTriggerExt as _;

type Group = (StateType, Vec<IssueSummary>);

/// The issues of one state type, groups in `StateType::rank()` order and
/// newest `updated_at` first inside each group. Types with no issue are
/// omitted.
fn group_by_state_type(issues: Vec<IssueSummary>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for issue in issues {
        let kind = issue.state.kind;
        match groups
            .iter_mut()
            .find(|(group_kind, _)| *group_kind == kind)
        {
            Some((_, members)) => members.push(issue),
            None => groups.push((kind, vec![issue])),
        }
    }
    groups.sort_by_key(|(kind, _)| kind.rank());
    for (_, members) in &mut groups {
        members.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));
    }
    groups
}

/// Puts `issue` in place of the listed copy with its id, regrouped by its
/// state. False when it is not listed.
fn replace_issue(groups: &mut Vec<Group>, issue: IssueSummary) -> bool {
    let listed = groups
        .iter()
        .any(|(_, issues)| issues.iter().any(|known| known.id == issue.id));
    if !listed {
        return false;
    }
    let issues = std::mem::take(groups)
        .into_iter()
        .flat_map(|(_, issues)| issues)
        .map(|known| {
            if known.id == issue.id {
                issue.clone()
            } else {
                known
            }
        })
        .collect();
    *groups = group_by_state_type(issues);
    true
}

struct MyIssues {
    client: Option<Arc<Linear>>,
    issues: Loadable<Vec<Group>>,
    teams: Loadable<Arc<Vec<TeamSummary>>>,
    collapsed: HashSet<StateType>,
    issues_task: Option<Task<()>>,
    teams_task: Option<Task<()>>,
}

pub fn view(_window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(MyIssues::new).into()
}

impl MyIssues {
    fn new(cx: &mut Context<Self>) -> Self {
        // An issue changed on its page, from a menu, or from the palette:
        // the copy listed here follows, in its new state's group.
        cx.observe_global::<data::IssueChange>(|view: &mut Self, cx| {
            let Some(issue) = cx.global::<data::IssueChange>().issue.clone() else {
                return;
            };
            if let Loadable::Ready(groups) = &mut view.issues
                && replace_issue(groups, issue)
            {
                cx.notify();
            }
        })
        .detach();
        let mut view = Self {
            client: None,
            issues: Loadable::Loading,
            teams: Loadable::Loading,
            collapsed: HashSet::new(),
            issues_task: None,
            teams_task: None,
        };
        view.sync_client(cx);
        view
    }

    /// Reloads from scratch when the signed-in client changed. Replacing the
    /// tasks drops the old client's loads.
    fn sync_client(&mut self, cx: &mut Context<Self>) {
        let client = data::client(cx);
        if data::same_client(&self.client, &client) {
            return;
        }
        self.client = client;
        self.issues = Loadable::Loading;
        self.teams = Loadable::Loading;
        self.issues_task = None;
        self.teams_task = None;
        if self.client.is_some() {
            self.load_issues(cx);
            self.load_teams(cx);
        }
    }

    /// Replaces any load still in flight, so a slow earlier answer cannot
    /// overwrite a newer one.
    fn load_issues(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.issues = Loadable::Loading;
        cx.notify();
        let task = background(cx, move || client.my_open_issues());
        self.issues_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.issues = match result {
                    Ok(issues) => Loadable::Ready(group_by_state_type(issues)),
                    Err(error) => Loadable::Failed(data::error_text(&error)),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn load_teams(&mut self, cx: &mut Context<Self>) {
        let task = data::teams(cx);
        self.teams_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.teams = match result {
                    Ok(teams) => Loadable::Ready(teams),
                    Err(message) => Loadable::Failed(message),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let total = match &self.issues {
            Loadable::Ready(groups) => Some(groups.iter().map(|(_, issues)| issues.len()).sum()),
            _ => None,
        };
        let teams = self.teams.clone();
        let actions = vec![
            ui::icon_button("my-issues-refresh", IconName::RefreshCw.into(), "Refresh")
                .on_click(cx.listener(|view, _, _, cx| view.load_issues(cx)))
                .into_any_element(),
            Button::new("my-issues-teams")
                .ghost()
                .small()
                .icon(IconName::LayoutDashboard)
                .label("Teams")
                .dropdown_caret(true)
                .menu_trigger()
                .dropdown_menu(move |menu, _, _| team_menu(menu, &teams))
                .into_any_element(),
        ];
        let header = ui::page_header("My issues").icon(IconName::CircleUser.into());
        let header = match total {
            Some(total) => header.count(total),
            None => header,
        };
        actions
            .into_iter()
            .fold(header, |header, action| header.action(action))
            .into_any_element()
    }

    fn group_section(
        &self,
        kind: StateType,
        issues: &[IssueSummary],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.collapsed.contains(&kind);
        let theme = cx.theme();
        let (border, muted, header_bg) = (theme.border, theme.muted_foreground, theme.muted);
        let marker = issues
            .first()
            .map(|issue| widgets::state_icon(&issue.state, cx));
        let now = Utc::now();
        let props = ViewConfig::default().properties;
        let header = h_flex()
            .id(SharedString::from(format!(
                "my-issues-group-{}",
                kind.as_str()
            )))
            .role(Role::Button)
            .aria_label(format!(
                "{} {}",
                if collapsed { "Expand" } else { "Collapse" },
                kind.label()
            ))
            .h(scale::nav_row_height())
            .flex_none()
            .px(scale::content_inset())
            .gap_2()
            .bg(header_bg.opacity(0.35))
            .border_y_1()
            .border_color(border)
            .on_click(cx.listener(move |view, _, _, cx| {
                if !view.collapsed.remove(&kind) {
                    view.collapsed.insert(kind);
                }
                cx.notify();
            }))
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(dp(12.))
                .text_color(muted),
            )
            .children(marker)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(kind.label()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(issues.len().to_string()),
            );

        let rows: Vec<AnyElement> = if collapsed {
            Vec::new()
        } else {
            issues
                .iter()
                .map(|issue| {
                    let route = Route::LinearIssue {
                        identifier: issue.identifier.clone(),
                    };
                    with_item_menu(
                        widgets::issue_row(issue, now, &props, cx).opens(route),
                        Item::issue(issue),
                    )
                    .into_any_element()
                })
                .collect()
        };
        v_flex().child(header).children(rows).into_any_element()
    }

    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.issues {
            Loadable::Loading => div().py_2().child(ui::skeleton_rows(8)).into_any_element(),
            Loadable::Failed(message) => widgets::load_error(
                message.clone(),
                cx.listener(|view, _, _, cx| view.load_issues(cx)),
            ),
            Loadable::Ready(groups) if groups.is_empty() => ui::empty_state(
                IconName::CircleCheck.into(),
                "No open issues assigned to you",
                "Issues assigned to you that are not done or canceled show up here.",
            )
            .into_any_element(),
            Loadable::Ready(groups) => {
                let sections: Vec<AnyElement> = groups
                    .iter()
                    .map(|(kind, issues)| self.group_section(*kind, issues, cx))
                    .collect();
                v_flex()
                    .id("my-issues-body")
                    .size_full()
                    .overflow_y_scrollbar()
                    .children(sections)
                    .into_any_element()
            }
        }
    }
}

/// The Teams menu: every team the key can see; picking one opens it.
fn team_menu(menu: PopupMenu, teams: &Loadable<Arc<Vec<TeamSummary>>>) -> PopupMenu {
    let menu = menu.min_w(dp(220.)).max_h(dp(360.)).scrollable(true);
    match teams {
        Loadable::Loading => menu.item(PopupMenuItem::new("Loading teams…").disabled(true)),
        Loadable::Failed(message) => menu.item(PopupMenuItem::new(message.clone()).disabled(true)),
        Loadable::Ready(teams) if teams.is_empty() => {
            menu.item(PopupMenuItem::new("No teams").disabled(true))
        }
        Loadable::Ready(teams) => teams.iter().fold(menu, |menu, team| {
            let route = Route::LinearTeam {
                team_id: team.id.clone(),
                name: team.name.clone(),
                view_id: None,
            };
            menu.item(
                PopupMenuItem::new(format!("{}  {}", team.key, team.name))
                    .on_click(move |_, window, cx| navigate(route.clone(), window, cx)),
            )
        }),
    }
}

impl Render for MyIssues {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_client(cx);
        if self.client.is_none() {
            return v_flex()
                .size_full()
                .child(widgets::not_connected(IconName::CircleUser, "My issues"));
        }
        v_flex()
            .size_full()
            .child(self.header(cx))
            .child(div().flex_1().min_h_0().child(self.body(cx)))
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use loupe_linear::{IssueSummary, Priority, StateRef, StateType, TeamRef};

    use super::{group_by_state_type, replace_issue};

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, hour, 0, 0).unwrap()
    }

    fn issue(identifier: &str, kind: StateType, updated_at: DateTime<Utc>) -> IssueSummary {
        IssueSummary {
            id: format!("id-{identifier}"),
            identifier: identifier.into(),
            title: format!("Issue {identifier}"),
            priority: Priority::None,
            sort_order: 0.,
            state: StateRef {
                id: format!("state-{}", kind.as_str()),
                name: kind.label().into(),
                kind,
                color: "#888888".into(),
            },
            team: TeamRef {
                id: "team".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            assignee: None,
            labels: Vec::new(),
            project: None,
            cycle: None,
            estimate: None,
            due_date: None,
            url: format!("https://linear.app/acme/issue/{identifier}"),
            branch_name: String::new(),
            created_at: at(0),
            updated_at,
            creator_id: None,
            parent_id: None,
            has_sub_issues: false,
            milestone: None,
            closed_at: None,
            pull_requests: Vec::new(),
            pull_requests_complete: true,
        }
    }

    fn identifiers(issues: &[IssueSummary]) -> Vec<&str> {
        issues
            .iter()
            .map(|issue| issue.identifier.as_str())
            .collect()
    }

    #[test]
    fn groups_follow_state_rank_not_input_order() {
        let groups = group_by_state_type(vec![
            issue("ENG-1", StateType::Triage, at(1)),
            issue("ENG-2", StateType::Backlog, at(2)),
            issue("ENG-3", StateType::Started, at(3)),
            issue("ENG-4", StateType::Unstarted, at(4)),
        ]);
        let kinds: Vec<StateType> = groups.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(
            kinds,
            [
                StateType::Started,
                StateType::Unstarted,
                StateType::Backlog,
                StateType::Triage
            ]
        );
    }

    #[test]
    fn newest_update_comes_first_inside_a_group() {
        let groups = group_by_state_type(vec![
            issue("ENG-1", StateType::Started, at(1)),
            issue("ENG-2", StateType::Started, at(9)),
            issue("ENG-3", StateType::Started, at(5)),
        ]);
        assert_eq!(groups.len(), 1);
        assert_eq!(identifiers(&groups[0].1), ["ENG-2", "ENG-3", "ENG-1"]);
    }

    #[test]
    fn states_of_one_type_share_a_group() {
        let mut review = issue("ENG-1", StateType::Started, at(1));
        review.state.name = "In Review".into();
        let groups = group_by_state_type(vec![review, issue("ENG-2", StateType::Started, at(2))]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1.len(), 2);
    }

    #[test]
    fn types_without_issues_are_omitted() {
        let groups = group_by_state_type(vec![issue("ENG-1", StateType::Backlog, at(1))]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, StateType::Backlog);
        assert!(group_by_state_type(Vec::new()).is_empty());
    }

    /// An issue changed elsewhere moves to its new state's group; one this
    /// list does not hold changes nothing.
    #[test]
    fn a_changed_issue_moves_to_its_new_group() {
        let mut groups = group_by_state_type(vec![
            issue("ENG-1", StateType::Started, at(2)),
            issue("ENG-2", StateType::Started, at(1)),
        ]);
        let mut done = issue("ENG-2", StateType::Completed, at(3));
        done.title = "Renamed".into();
        assert!(replace_issue(&mut groups, done));
        let kinds: Vec<(StateType, Vec<&str>)> = groups
            .iter()
            .map(|(kind, issues)| (*kind, identifiers(issues)))
            .collect();
        assert_eq!(
            kinds,
            [
                (StateType::Started, vec!["ENG-1"]),
                (StateType::Completed, vec!["ENG-2"]),
            ]
        );
        assert!(!replace_issue(
            &mut groups,
            issue("ENG-9", StateType::Started, at(4))
        ));
    }
}
