//! The Linear workspace switcher, after Linear's: the sidebar's header opens
//! it, and so do O then W. It lists every connected workspace under the
//! email of the account it belongs to, marks the active one, and ends with
//! "Add a workspace…".

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use super::links::FloatingMenu;
use super::{Workspace, with_workspace};
use crate::route::Route;
use crate::services::{LinearWorkspace, Services};
use crate::settings::session;
use crate::theme::scale::dp;

/// Where the sidebar's header was last drawn, so the menu opens under it
/// from the keyboard too.
#[derive(Default)]
struct HeaderBounds(Option<Bounds<Pixels>>);

impl Global for HeaderBounds {}

/// An invisible layer over the sidebar's header that records where it is.
pub(super) fn track_header() -> impl IntoElement {
    canvas(
        |bounds, _, cx| cx.default_global::<HeaderBounds>().0 = Some(bounds),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// The workspaces grouped by the account they belong to, in the order they
/// were connected: one group per email.
fn by_account(workspaces: &[LinearWorkspace]) -> Vec<(String, Vec<&LinearWorkspace>)> {
    let mut groups: Vec<(String, Vec<&LinearWorkspace>)> = Vec::new();
    for workspace in workspaces {
        let email = workspace.viewer.email.clone();
        match groups.iter_mut().find(|(known, _)| *known == email) {
            Some((_, members)) => members.push(workspace),
            None => groups.push((email, vec![workspace])),
        }
    }
    groups
}

/// Opens the switcher under the sidebar's header and gives it the focus,
/// so the arrows move and Enter picks. `Workspace` calls it from its
/// `SwitchWorkspace` handler.
pub(super) fn show(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let services = Services::get(cx).clone();
    let active = crate::linear::active_workspace_id(cx);
    let menu = PopupMenu::build(window, cx, move |menu, _, _| {
        let mut menu = menu.min_w(dp(240.));
        for (email, members) in by_account(&services.linear_workspaces) {
            if !email.is_empty() {
                menu = menu.label(email);
            }
            for workspace in members {
                let id = workspace.id().to_owned();
                menu = menu.item(
                    PopupMenuItem::new(workspace.viewer.organization.name.clone())
                        .checked(active.as_deref() == Some(id.as_str()))
                        .on_click(move |_, window, cx| switch(&id, window, cx)),
                );
            }
            menu = menu.separator();
        }
        menu.item(
            PopupMenuItem::new("Add a workspace…")
                .on_click(|_, window, cx| super::navigate(Route::Settings, window, cx)),
        )
    });
    let position = cx
        .try_global::<HeaderBounds>()
        .and_then(|header| header.0)
        .map(|header| point(header.left() + dp(8.), header.bottom()))
        .unwrap_or_else(|| point(dp(8.), dp(44.)));
    let dismiss = cx.subscribe(
        &menu,
        |workspace: &mut Workspace, _, _: &DismissEvent, cx| {
            workspace.workspace_menu = None;
            cx.notify();
        },
    );
    workspace.workspace_menu = Some(FloatingMenu::new(menu.clone(), position, dismiss));
    cx.notify();
    menu.focus_handle(cx).focus(window, cx);
}

/// Makes the workspace active. A page that belongs to the workspace left
/// behind (a team, a project, an issue) gives way to My issues.
fn switch(workspace_id: &str, window: &mut Window, cx: &mut App) {
    session::switch_linear(workspace_id, cx);
    with_workspace(cx, |workspace, cx| {
        if workspace
            .panes
            .active()
            .active()
            .current()
            .route
            .is_in_linear_workspace()
        {
            workspace.push(Route::LinearMyIssues, window, cx);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };

    use super::{by_account, switch};
    use crate::route::Route;
    use crate::services::{Accounts, Connection, LinearWorkspace, Services};
    use crate::shell::Workspace;
    use crate::store::Settings;

    fn workspace(id: &str, email: &str) -> LinearWorkspace {
        LinearWorkspace::for_test(id, email)
    }

    #[test]
    fn workspaces_are_grouped_by_the_account_they_belong_to() {
        let workspaces = [
            workspace("hooli", "work@example.com"),
            workspace("globex", "me@example.com"),
            workspace("initech", "me@example.com"),
        ];
        let groups: Vec<(String, Vec<&str>)> = by_account(&workspaces)
            .into_iter()
            .map(|(email, members)| (email, members.iter().map(|w| w.id()).collect()))
            .collect();
        assert_eq!(
            groups,
            [
                ("work@example.com".to_owned(), vec!["hooli"]),
                ("me@example.com".to_owned(), vec!["globex", "initech"]),
            ]
        );
    }

    /// The window on a team of Hooli, with Hooli and Initech
    /// connected and Hooli active.
    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<Workspace>) {
        let hooli = workspace("hooli", "work@example.com");
        let developer = workspace("initech", "me@example.com");
        let (handle, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            crate::shell::menus::bind_keys(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts {
                github: Connection::Connected(loupe_github::Viewer {
                    login: "octocat".into(),
                    name: None,
                    avatar_url: String::new(),
                }),
                ..Accounts::default()
            });
            crate::settings::session::connect_for_test(
                vec![("lin_a".into(), hooli), ("lin_b".into(), developer)],
                cx,
            );
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        Workspace::new(
                            Route::LinearTeam {
                                team_id: "back".into(),
                                name: "Back-end".into(),
                                view_id: None,
                            },
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open the window")
        });
        (VisualTestContext::from_window(handle, cx), workspace)
    }

    fn active(cx: &mut VisualTestContext) -> Option<String> {
        cx.update(|_, cx| match &Accounts::get(cx).linear {
            Connection::Connected(viewer) => Some(viewer.organization.id.clone()),
            _ => None,
        })
    }

    #[gpui_kit::test]
    fn o_then_w_opens_the_switcher(cx: &mut TestAppContext) {
        let (mut cx, workspace) = open(cx);
        cx.update(|window, cx| {
            let focus = workspace.read(cx).focus.clone();
            focus.focus(window, cx);
        });
        cx.simulate_keystrokes("o w");
        assert!(
            cx.update(|_, cx| workspace.read(cx).workspace_menu.is_some()),
            "O then W opens the workspace menu"
        );
        cx.simulate_keystrokes("escape");
        assert!(cx.update(|_, cx| workspace.read(cx).workspace_menu.is_none()));
    }

    #[gpui_kit::test]
    fn switching_moves_every_screen_to_the_other_workspace(cx: &mut TestAppContext) {
        let (mut cx, workspace) = open(cx);
        assert_eq!(active(&mut cx).as_deref(), Some("hooli"));

        cx.update(|window, cx| switch("initech", window, cx));

        assert_eq!(active(&mut cx).as_deref(), Some("initech"));
        cx.update(|_, cx| {
            let services = Services::get(cx);
            let developer = services
                .linear_workspace("initech")
                .expect("still connected");
            let linear = services.linear.as_ref().expect("a client");
            assert!(
                Arc::ptr_eq(linear, &developer.client),
                "the sidebar, the views, and My issues read the new workspace"
            );
            assert_eq!(services.linear_workspaces.len(), 2, "Hooli stays connected");
            assert_eq!(
                Settings::get(cx).linear_workspace.as_deref(),
                Some("initech"),
                "the choice survives a restart"
            );
            assert_eq!(
                workspace.read(cx).panes.active().active().current().route,
                Route::LinearMyIssues,
                "a team of the old workspace gives way to My issues"
            );
        });

        // A page outside Linear stays where it is.
        let pr = Route::PullRequest(loupe_github::PrRef::new(
            loupe_github::RepoRef::new("acme", "web"),
            7,
        ));
        cx.update(|window, cx| {
            crate::shell::navigate(pr.clone(), window, cx);
            switch("hooli", window, cx);
        });
        assert_eq!(active(&mut cx).as_deref(), Some("hooli"));
        cx.update(|_, cx| {
            assert_eq!(
                workspace.read(cx).panes.active().active().current().route,
                pr
            );
        });
    }
}
