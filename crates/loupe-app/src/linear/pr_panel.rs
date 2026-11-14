//! The Linear issues linked to a pull request: issues with an attachment for
//! the PR URL plus identifiers mentioned in its branch, title, and body.

use serde::{Deserialize, Serialize};

use gpui_kit::component::button::Button;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_linear::{IssueSummary, extract_identifiers};

use crate::linear::data::{self, Loadable};
use crate::linear::widgets;
use crate::links::PrLinkContext;
use crate::route::Route;
use crate::services::LinearWorkspace;
use crate::shell::OpenOnClick as _;
use crate::shell::navigate;
use crate::theme::scale::dp;

/// How a PR relates to an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LinkSource {
    Attachment,
    Mention,
    Both,
}

impl LinkSource {
    fn tag(self) -> &'static str {
        match self {
            LinkSource::Attachment => "linked",
            LinkSource::Mention => "mentioned",
            LinkSource::Both => "linked · mentioned",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LinkedIssue {
    issue: IssueSummary,
    source: LinkSource,
}

/// What the panel shows once it has data. `warning` is set when only one of
/// the two lookups worked.
#[derive(Clone, Debug)]
struct Linked {
    issues: Vec<LinkedIssue>,
    warning: Option<SharedString>,
}

enum Panel {
    NotConnected,
    Linked(Loadable<Linked>),
}

type Lookup = Result<Vec<IssueSummary>, SharedString>;

/// Issues attached to the PR first, then issues only mentioned in its text, each
/// issue once. `mentioned` must already be in identifier order.
fn merge_linked(attached: Vec<IssueSummary>, mentioned: Vec<IssueSummary>) -> Vec<LinkedIssue> {
    let mut merged: Vec<LinkedIssue> = Vec::with_capacity(attached.len() + mentioned.len());
    for issue in attached {
        if merged.iter().all(|linked| linked.issue.id != issue.id) {
            merged.push(LinkedIssue {
                issue,
                source: LinkSource::Attachment,
            });
        }
    }
    for issue in mentioned {
        match merged.iter_mut().find(|linked| linked.issue.id == issue.id) {
            Some(linked) => {
                if linked.source == LinkSource::Attachment {
                    linked.source = LinkSource::Both;
                }
            }
            None => merged.push(LinkedIssue {
                issue,
                source: LinkSource::Mention,
            }),
        }
    }
    merged
}

/// Orders issues like `identifiers`; issues Linear returned for another
/// identifier (a moved issue keeps its old one) go last.
fn sort_by_identifiers(mut issues: Vec<IssueSummary>, identifiers: &[String]) -> Vec<IssueSummary> {
    issues.sort_by_key(|issue| {
        identifiers
            .iter()
            .position(|identifier| identifier.eq_ignore_ascii_case(&issue.identifier))
            .unwrap_or(identifiers.len())
    });
    issues
}

/// Combines the two lookups. One failure still shows what the other found; both
/// failing is an error.
fn combine(attached: Lookup, mentioned: Lookup, identifiers: &[String]) -> Loadable<Linked> {
    match (attached, mentioned) {
        (Ok(attached), Ok(mentioned)) => Loadable::Ready(Linked {
            issues: merge_linked(attached, sort_by_identifiers(mentioned, identifiers)),
            warning: None,
        }),
        (Ok(attached), Err(warning)) => Loadable::Ready(Linked {
            issues: merge_linked(attached, Vec::new()),
            warning: Some(warning),
        }),
        (Err(warning), Ok(mentioned)) => Loadable::Ready(Linked {
            issues: merge_linked(Vec::new(), sort_by_identifiers(mentioned, identifiers)),
            warning: Some(warning),
        }),
        (Err(error), Err(_)) => Loadable::Failed(error),
    }
}

/// Identifiers in the branch name, title, and body, in order of first
/// appearance.
fn mentioned_identifiers(context: &PrLinkContext) -> Vec<String> {
    extract_identifiers(&format!(
        "{}\n{}\n{}",
        context.head_ref, context.title, context.body
    ))
}

pub fn view(context: PrLinkContext, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| PrPanel::new(context, cx)).into()
}

struct PrPanel {
    context: PrLinkContext,
    /// The workspace the shown data came from: the one Settings maps the
    /// repository to, else the active one. A different one reloads.
    workspace: Option<LinearWorkspace>,
    panel: Panel,
    load_task: Option<Task<()>>,
}

impl PrPanel {
    fn new(context: PrLinkContext, cx: &mut Context<Self>) -> Self {
        let workspace = data::workspace_for_repo(&context.pr.repo.owner, &context.pr.repo.name, cx);
        let mut panel = PrPanel {
            context,
            workspace,
            panel: Panel::NotConnected,
            load_task: None,
        };
        panel.load(cx);
        panel
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            self.panel = Panel::NotConnected;
            self.load_task = None;
            return;
        };
        let client = workspace.client.clone();
        let viewer = &workspace.viewer;
        let key = Some(format!(
            "linear/{}/{}/{}/{}",
            viewer.user.id,
            viewer.organization.id,
            self.context.pr.url(),
            self.context.head_oid
        ));
        let cached = key
            .as_deref()
            .and_then(crate::cache::get::<Vec<LinkedIssue>>);
        self.panel = Panel::Linked(match cached {
            Some(issues) => Loadable::Ready(Linked {
                issues,
                warning: None,
            }),
            None => Loadable::Loading,
        });

        let url = self.context.pr.url();
        let identifiers = mentioned_identifiers(&self.context);

        let attached_task = {
            let client = client.clone();
            crate::services::background(cx, move || -> Lookup {
                client
                    .issues_for_url(&url)
                    .map_err(|error| data::error_text(&error))
            })
        };
        let mentioned_task = {
            let identifiers = identifiers.clone();
            crate::services::background(cx, move || -> Lookup {
                if identifiers.is_empty() {
                    return Ok(Vec::new());
                }
                client
                    .issues_by_identifiers(&identifiers)
                    .map_err(|error| data::error_text(&error))
            })
        };

        self.load_task = Some(cx.spawn(async move |this, cx| {
            let attached = attached_task.await;
            let mentioned = mentioned_task.await;
            let result = combine(attached, mentioned, &identifiers);
            this.update(cx, |view, cx| {
                match result {
                    Loadable::Ready(mut linked) => {
                        if linked.warning.is_some()
                            && let Panel::Linked(Loadable::Ready(stale)) = &view.panel
                        {
                            linked.issues = stale.issues.clone();
                        }
                        if linked.warning.is_none()
                            && let Some(key) = &key
                        {
                            crate::cache::put(key, &linked.issues);
                        }
                        view.panel = Panel::Linked(Loadable::Ready(linked));
                    }
                    Loadable::Failed(error) => {
                        if let Panel::Linked(Loadable::Ready(linked)) = &mut view.panel {
                            linked.warning = Some(error);
                        } else {
                            view.panel = Panel::Linked(Loadable::Failed(error));
                        }
                    }
                    Loadable::Loading => {}
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The workspace's name, shown once more than one is connected, so the
    /// column says which workspace the issues come from.
    fn workspace_name(&self, cx: &App) -> Option<SharedString> {
        let workspace = self.workspace.as_ref()?;
        (crate::services::Services::get(cx).linear_workspaces.len() > 1)
            .then(|| workspace.viewer.organization.name.clone().into())
    }

    /// The workspace to switch to before an issue opens: the panel's own,
    /// when it is not the active one (an issue's page reads the active one).
    fn switch_to(&self, cx: &App) -> Option<String> {
        let workspace = self.workspace.as_ref()?;
        let active = data::active_workspace(cx);
        (active.as_ref().map(LinearWorkspace::id) != Some(workspace.id()))
            .then(|| workspace.id().to_owned())
    }

    fn header(&self, cx: &App) -> Div {
        let count = match &self.panel {
            Panel::Linked(Loadable::Ready(linked)) if !linked.issues.is_empty() => {
                Some(linked.issues.len())
            }
            _ => None,
        };
        h_flex()
            .h_6()
            .gap_2()
            .items_center()
            .child(crate::ui::section_label("Linear issues", cx))
            .children(count.map(|count| crate::ui::count_badge(count, cx)))
            .children(self.workspace_name(cx).map(|name| {
                div()
                    .ml_auto()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::theme::scale::meta())
                    .text_color(cx.theme().muted_foreground)
                    .child(name)
            }))
    }

    fn not_connected(cx: &App) -> Div {
        v_flex()
            .gap_2()
            .items_start()
            .child(crate::ui::empty_line(
                gpui_kit::assets::IconName::Unplug,
                "Linear is not connected",
                cx,
            ))
            .child(
                Button::new("pr-panel-open-settings")
                    .outline()
                    .xsmall()
                    .icon(IconName::Settings)
                    .label("Connect in Settings…")
                    .on_click(|_, window, cx| navigate(Route::Settings, window, cx)),
            )
    }

    fn card(linked: &LinkedIssue, switch_to: Option<String>, cx: &App) -> Stateful<Div> {
        let issue = &linked.issue;
        let theme = cx.theme();
        let identifier = issue.identifier.clone();
        let title: SharedString = issue.title.clone().into();
        v_flex()
            .id(SharedString::from(format!("pr-linked-{}", issue.id)))
            .role(Role::Link)
            .aria_label(format!("{} {}", issue.identifier, issue.title))
            // A title longer than three lines is whole in the tooltip.
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(title.clone()).build(window, cx)
            })
            .w_full()
            .gap_2()
            .p_2p5()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .hover(|style| style.bg(theme.list_hover))
            // Line 1 says which issue it is and where it stands, whole;
            // the title follows on up to three lines; the last line says
            // how the issue is tied to the pull request.
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(widgets::state_icon(&issue.state, cx))
                    .child(widgets::identifier(issue.identifier.clone(), cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(issue.state.name.clone()),
                    )
                    .child(widgets::avatar(issue.assignee.as_ref(), dp(16.))),
            )
            .child(
                div()
                    .text_size(crate::theme::scale::body())
                    .line_height(rems(1.1875))
                    .line_clamp(3)
                    .text_ellipsis()
                    .child(issue.title.clone()),
            )
            .child(h_flex().child(crate::ui::chip().child(linked.source.tag())))
            .on_open(move |event, window, cx| {
                if let Some(workspace) = &switch_to {
                    crate::settings::session::switch_linear(workspace, cx);
                }
                crate::shell::open(
                    Route::LinearIssue {
                        identifier: identifier.clone(),
                    },
                    event,
                    window,
                    cx,
                )
            })
    }

    fn body(&self, cx: &mut Context<Self>) -> Div {
        match &self.panel {
            Panel::NotConnected => Self::not_connected(cx),
            Panel::Linked(Loadable::Loading) => v_flex()
                .gap_2()
                .child(Skeleton::new().h(dp(64.)).rounded(cx.theme().radius_lg))
                .child(Skeleton::new().h(dp(64.)).rounded(cx.theme().radius_lg)),
            Panel::Linked(Loadable::Failed(message)) => v_flex()
                .gap_2()
                .items_start()
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .text_size(crate::theme::scale::meta())
                        .child(
                            gpui_kit::component::Icon::new(IconName::CircleAlert)
                                .xsmall()
                                .text_color(cx.theme().danger),
                        )
                        .child(div().flex_1().min_w_0().child(message.clone())),
                )
                .child(
                    Button::new("pr-panel-retry")
                        .outline()
                        .xsmall()
                        .icon(IconName::RefreshCw)
                        .label("Retry")
                        .on_click(cx.listener(|view, _, _, cx| view.load(cx))),
                ),
            Panel::Linked(Loadable::Ready(linked)) => {
                let mut column = v_flex().gap_2();
                if linked.issues.is_empty() {
                    column = column.child(crate::ui::empty_line(
                        gpui_kit::assets::IconName::CircleDot,
                        "No Linear issue links this pull request",
                        cx,
                    ));
                }
                let switch_to = self.switch_to(cx);
                for item in &linked.issues {
                    column = column.child(Self::card(item, switch_to.clone(), cx));
                }
                if let Some(warning) = &linked.warning {
                    column = column.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(warning.clone()),
                    );
                }
                column
            }
        }
    }
}

impl Render for PrPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = &self.context.pr.repo;
        let workspace = data::workspace_for_repo(&repo.owner, &repo.name, cx);
        let client = |workspace: &Option<LinearWorkspace>| {
            workspace.as_ref().map(|workspace| workspace.client.clone())
        };
        if !data::same_client(&client(&self.workspace), &client(&workspace)) {
            self.workspace = workspace;
            self.load(cx);
        }
        v_flex()
            .w_full()
            .gap_2()
            .child(self.header(cx))
            .child(self.body(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkSource, LinkedIssue, combine, merge_linked, sort_by_identifiers};
    use crate::linear::data::Loadable;
    use chrono::DateTime;
    use gpui_kit::SharedString;
    use loupe_linear::{IssueSummary, Priority, StateRef, StateType, TeamRef};

    fn issue(identifier: &str) -> IssueSummary {
        let at = DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .to_utc();
        IssueSummary {
            id: format!("id-{identifier}"),
            identifier: identifier.into(),
            title: format!("Title of {identifier}"),
            priority: Priority::None,
            sort_order: 0.,
            state: StateRef {
                id: "s1".into(),
                name: "Todo".into(),
                kind: StateType::Unstarted,
                color: "#e2e2e2".into(),
            },
            team: TeamRef {
                id: "t1".into(),
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

    fn summary(linked: &[LinkedIssue]) -> Vec<(String, LinkSource)> {
        linked
            .iter()
            .map(|item| (item.issue.identifier.clone(), item.source))
            .collect()
    }

    #[test]
    fn attached_come_first_then_mentions() {
        let merged = merge_linked(
            vec![issue("ENG-2"), issue("ENG-1")],
            vec![issue("ENG-5"), issue("ENG-4")],
        );
        assert_eq!(
            summary(&merged),
            [
                ("ENG-2".to_owned(), LinkSource::Attachment),
                ("ENG-1".to_owned(), LinkSource::Attachment),
                ("ENG-5".to_owned(), LinkSource::Mention),
                ("ENG-4".to_owned(), LinkSource::Mention),
            ]
        );
    }

    #[test]
    fn issue_in_both_lists_appears_once_as_both() {
        let merged = merge_linked(
            vec![issue("ENG-1"), issue("ENG-2")],
            vec![issue("ENG-3"), issue("ENG-1")],
        );
        assert_eq!(
            summary(&merged),
            [
                ("ENG-1".to_owned(), LinkSource::Both),
                ("ENG-2".to_owned(), LinkSource::Attachment),
                ("ENG-3".to_owned(), LinkSource::Mention),
            ]
        );
    }

    #[test]
    fn duplicates_inside_one_list_collapse() {
        let merged = merge_linked(
            vec![issue("ENG-1"), issue("ENG-1")],
            vec![issue("ENG-2"), issue("ENG-2")],
        );
        assert_eq!(
            summary(&merged),
            [
                ("ENG-1".to_owned(), LinkSource::Attachment),
                ("ENG-2".to_owned(), LinkSource::Mention),
            ]
        );
    }

    #[test]
    fn empty_inputs_give_no_issues() {
        assert!(merge_linked(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn mentions_follow_identifier_order() {
        let ids = vec!["ENG-9".to_owned(), "ENG-3".to_owned(), "ENG-5".to_owned()];
        let sorted =
            sort_by_identifiers(vec![issue("ENG-5"), issue("ENG-9"), issue("ENG-3")], &ids);
        let order: Vec<&str> = sorted.iter().map(|i| i.identifier.as_str()).collect();
        assert_eq!(order, ["ENG-9", "ENG-3", "ENG-5"]);
    }

    #[test]
    fn one_failed_lookup_keeps_the_other_result() {
        let ids = vec!["ENG-2".to_owned()];
        let warning = SharedString::from("network down");
        match combine(Ok(vec![issue("ENG-1")]), Err(warning.clone()), &ids) {
            Loadable::Ready(linked) => {
                assert_eq!(linked.issues.len(), 1);
                assert_eq!(linked.warning, Some(warning.clone()));
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        match combine(Err(warning.clone()), Ok(vec![issue("ENG-2")]), &ids) {
            Loadable::Ready(linked) => {
                assert_eq!(
                    summary(&linked.issues),
                    [("ENG-2".to_owned(), LinkSource::Mention)]
                );
                assert_eq!(linked.warning, Some(warning));
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn both_failed_lookups_fail() {
        let result = combine(Err("first".into()), Err("second".into()), &[]);
        assert!(matches!(result, Loadable::Failed(message) if message.as_ref() == "first"));
    }
}
