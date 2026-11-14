//! Links in prose: a link to a pull request or to an issue of the connected
//! Linear workspace opens in Loupe, any other link in the browser. A
//! right-click opens a menu with "Open in new tab", "Open in browser", and
//! "Copy link". URLs the system hands to Loupe (the `loupe://` scheme, or a
//! link Finicky sends) go through [`open_system_urls`].

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use super::{Workspace, open, open_in_new_tab, with_workspace};
use crate::route::Route;
use crate::services::{Accounts, Connection, Services};

/// A menu the workspace draws over every page at a point: a right-clicked
/// link's, at the pointer, or the workspace switcher, under the sidebar's
/// header.
pub(super) struct FloatingMenu {
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

/// A page for a link, and the Linear workspace to make active before it
/// opens: `Some` for an issue of a connected workspace that is not the
/// active one (an issue's page reads the active workspace).
type Target = (Route, Option<String>);

/// The connected workspaces as `(organization id, slug)`, the active one
/// first, for matching issue links.
fn linear_slugs(cx: &App) -> Vec<(String, String)> {
    let active = match &Accounts::get(cx).linear {
        Connection::Connected(viewer) => Some(viewer.organization.id.clone()),
        _ => None,
    };
    let mut slugs: Vec<(String, String)> = Services::get(cx)
        .linear_workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.id().to_owned(),
                workspace.viewer.organization.url_key.clone(),
            )
        })
        .collect();
    slugs.sort_by_key(|(id, _)| Some(id) != active.as_ref());
    if slugs.is_empty()
        && let Connection::Connected(viewer) = &Accounts::get(cx).linear
    {
        slugs.push((
            viewer.organization.id.clone(),
            viewer.organization.url_key.clone(),
        ));
    }
    slugs
}

/// The page `parse` finds for a link with each workspace's slug in turn,
/// the active one (first in `slugs`) before the others.
fn resolve(
    slugs: &[(String, String)],
    parse: impl Fn(Option<&str>) -> Option<Route>,
) -> Option<Target> {
    if slugs.is_empty() {
        return parse(None).map(|route| (route, None));
    }
    slugs.iter().enumerate().find_map(|(index, (id, slug))| {
        let route = parse(Some(slug))?;
        let switch = (index > 0 && matches!(route, Route::LinearIssue { .. })).then(|| id.clone());
        Some((route, switch))
    })
}

/// The page Loupe shows for `url`, by [`Route::for_link`] with every
/// connected Linear workspace.
fn route_for(url: &str, cx: &App) -> Option<Target> {
    resolve(&linear_slugs(cx), |slug| Route::for_link(url, slug))
}

/// Makes the target's workspace active when it names one, and hands back
/// its page.
fn enter(target: Target, cx: &mut App) -> Route {
    let (route, workspace) = target;
    if let Some(workspace) = workspace {
        crate::settings::session::switch_linear(&workspace, cx);
    }
    route
}

/// Shows each URL the system handed to Loupe (macOS's "open URL" event:
/// `loupe://pr/<owner>/<repo>/<n>`, `loupe://issue/<id>`, or a pull request
/// or Linear issue link) in the tab that already shows it, or in a new
/// active tab, and brings the window forward. Other URLs are dropped: sending
/// them to the browser could hand them straight back to Loupe.
pub fn open_system_urls(urls: Vec<String>, cx: &mut App) {
    let slugs = linear_slugs(cx);
    let targets: Vec<Target> = urls
        .iter()
        .filter_map(|url| resolve(&slugs, |slug| Route::for_system_url(url, slug)))
        .collect();
    let routes: Vec<Route> = targets
        .into_iter()
        .map(|target| enter(target, cx))
        .collect();
    if routes.is_empty() {
        return;
    }
    let Some(handle) = cx
        .active_window()
        .or_else(|| cx.windows().into_iter().next())
    else {
        return;
    };
    handle
        .update(cx, |_, window, cx| {
            for route in routes {
                with_workspace(cx, |workspace, cx| workspace.reveal(route, window, cx));
            }
            window.activate_window();
        })
        .ok();
    cx.activate(true);
}

/// Opens a link clicked in prose (the `on_link_click` of `ui::markdown`):
/// a pull request or a Linear issue in Loupe, through [`open`], so Cmd and
/// the middle button open it in a new tab; anything else in the browser. A
/// right-click opens the link's menu.
pub fn open_link(url: &SharedString, event: &ClickEvent, window: &mut Window, cx: &mut App) {
    if event.is_right_click() {
        show_menu(url.clone(), event.position(), window, cx);
        return;
    }
    match route_for(url, cx) {
        Some(target) => {
            let route = enter(target, cx);
            open(route, event, window, cx)
        }
        None => cx.open_url(url),
    }
}

fn show_menu(url: SharedString, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let route = route_for(&url, cx);
    let menu = PopupMenu::build(window, cx, move |menu, _, _| {
        let menu = match route {
            Some(target) => menu.item(PopupMenuItem::new("Open in new tab").on_click(
                move |_, window, cx| {
                    let route = enter(target.clone(), cx);
                    open_in_new_tab(route, window, cx)
                },
            )),
            None => menu,
        };
        let browser = url.clone();
        menu.item(
            PopupMenuItem::new("Open in browser").on_click(move |_, _, cx| cx.open_url(&browser)),
        )
        .item(PopupMenuItem::new("Copy link").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(url.to_string()))
        }))
    });
    with_workspace(cx, |workspace, cx| {
        let dismiss = cx.subscribe(
            &menu,
            |workspace: &mut Workspace, _, _: &DismissEvent, cx| {
                workspace.link_menu = None;
                cx.notify();
            },
        );
        workspace.link_menu = Some(FloatingMenu {
            menu: menu.clone(),
            position,
            _dismiss: dismiss,
        });
        cx.notify();
    });
    menu.focus_handle(cx).focus(window, cx);
}

impl FloatingMenu {
    pub(super) fn new(
        menu: Entity<PopupMenu>,
        position: Point<Pixels>,
        dismiss: Subscription,
    ) -> Self {
        FloatingMenu {
            menu,
            position,
            _dismiss: dismiss,
        }
    }

    /// The menu at its point, above the pages and kept inside the window.
    pub(super) fn render(&self) -> AnyElement {
        deferred(
            anchored()
                .position(self.position)
                .snap_to_window_with_margin(px(8.))
                .child(self.menu.clone()),
        )
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::v_flex;
    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, InputEvent as _, InteractiveElement as _,
        IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        WindowBounds, WindowOptions, div, point, px, size,
    };
    use loupe_github::{PrRef, RepoRef};

    use crate::route::Route;
    use crate::services::{Accounts, Connection, Services};
    use crate::shell::Workspace;
    use crate::store::Settings;

    #[test]
    fn an_issue_of_another_connected_workspace_opens_after_switching() {
        let slugs = vec![
            ("hooli-id".to_owned(), "hooli".to_owned()),
            ("globex-id".to_owned(), "globex".to_owned()),
        ];
        let target = |url: &str| super::resolve(&slugs, |slug| Route::for_link(url, slug));
        let issue = |identifier: &str| Route::LinearIssue {
            identifier: identifier.into(),
        };
        assert_eq!(
            target("https://linear.app/hooli/issue/ENG-1/x"),
            Some((issue("ENG-1"), None)),
            "the active workspace's issue opens where it is"
        );
        assert_eq!(
            target("https://linear.app/globex/issue/OPS-86/x"),
            Some((issue("OPS-86"), Some("globex-id".to_owned()))),
            "another workspace's issue switches to it first"
        );
        assert_eq!(target("https://linear.app/elsewhere/issue/ENG-1"), None);
        assert_eq!(
            target(PR),
            Some((
                Route::PullRequest(PrRef::new(RepoRef::new("acme", "web"), 7)),
                None
            )),
            "a pull request never switches"
        );
    }

    const PR: &str = "https://github.com/acme/web/pull/7";
    const ISSUE: &str = "https://linear.app/acme/issue/ENG-1/fix-the-thing";
    const OTHER_ISSUE: &str = "https://linear.app/elsewhere/issue/ENG-1";
    const SITE: &str = "https://example.com/docs";

    /// One paragraph per link, each in its own line of prose, over the
    /// workspace that the links navigate and that draws the link menu.
    struct Prose {
        workspace: Entity<Workspace>,
    }

    impl Render for Prose {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let line = |selector: &'static str, url: &str| {
                div()
                    .h(px(40.))
                    .debug_selector(move || selector.into())
                    .child(crate::ui::markdown(
                        selector,
                        format!("[a link that goes somewhere]({url}) and some words"),
                    ))
            };
            v_flex()
                .size_full()
                .child(line("pr", PR))
                .child(line("issue", ISSUE))
                .child(line("other-issue", OTHER_ISSUE))
                .child(line("site", SITE))
                .child(div().flex_1().child(self.workspace.clone()))
        }
    }

    fn open_prose(cx: &mut TestAppContext) -> VisualTestContext {
        let (handle, _) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts {
                github: Connection::Connected(loupe_github::Viewer {
                    login: "octocat".into(),
                    name: None,
                    avatar_url: String::new(),
                }),
                linear: Connection::Connected(loupe_linear::Viewer {
                    user: loupe_linear::User {
                        id: "me".into(),
                        name: "Me".into(),
                        display_name: "me".into(),
                        avatar_url: None,
                    },
                    organization: loupe_linear::Organization {
                        id: "org".into(),
                        name: "Acme".into(),
                        url_key: "acme".into(),
                        logo_url: None,
                    },
                    email: String::new(),
                }),
                ..Accounts::default()
            });
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1100.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let workspace = cx.new(|cx| Workspace::new(Route::Inbox, window, cx));
                    cx.new(|_| Prose { workspace })
                },
            )
            .expect("open the window")
        });
        VisualTestContext::from_window(handle, cx)
    }

    /// Clicks the link at the start of the line `selector`, with a frame
    /// between each step so hit testing sees the pointer.
    fn click(
        cx: &mut VisualTestContext,
        selector: &'static str,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"));
        let position = bounds.origin + point(px(40.), px(10.));
        let events = [
            MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }
            .to_platform_input(),
            MouseDownEvent {
                position,
                button,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            MouseUpEvent {
                position,
                button,
                modifiers,
                click_count: 1,
            }
            .to_platform_input(),
        ];
        for event in events {
            cx.update(|window, cx| {
                window.dispatch_event(event, cx);
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        cx.run_until_parked();
    }

    fn tabs(cx: &mut VisualTestContext) -> (Vec<Route>, usize) {
        cx.update(|_, cx| Settings::get(cx).single_pane_tabs())
    }

    fn pull_request() -> Route {
        Route::PullRequest(PrRef::new(RepoRef::new("acme", "web"), 7))
    }

    fn issue() -> Route {
        Route::LinearIssue {
            identifier: "ENG-1".into(),
        }
    }

    #[gpui_kit::test]
    fn a_system_url_shows_its_page_in_one_tab(cx: &mut TestAppContext) {
        let cx = &mut open_prose(cx);
        let open = |cx: &mut VisualTestContext, url: &str| {
            // As the platform does: from the app, not from inside a window.
            cx.cx
                .update(|cx| super::open_system_urls(vec![url.to_string()], cx));
            cx.run_until_parked();
        };

        open(cx, "loupe://pr/acme/web/7");
        assert_eq!(
            tabs(cx),
            (vec![Route::Inbox, pull_request()], 1),
            "a new tab, shown at once"
        );

        open(cx, "loupe://issue/ENG-1");
        assert_eq!(tabs(cx), (vec![Route::Inbox, pull_request(), issue()], 2));
        open(cx, PR);
        let (routes, active) = tabs(cx);
        assert_eq!(
            (routes.clone(), active),
            (vec![Route::Inbox, pull_request(), issue()], 1),
            "the pull request already has a tab: it comes forward, with no second one"
        );

        open(cx, SITE);
        open(cx, OTHER_ISSUE);
        assert_eq!(tabs(cx), (routes, active), "other URLs change nothing");
        assert_eq!(cx.opened_url(), None, "and do not go to the browser");
    }

    fn menu_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|_, cx| {
            cx.try_global::<super::super::WorkspaceHandle>()
                .and_then(|handle| handle.0.upgrade())
                .is_some_and(|workspace| workspace.read(cx).link_menu.is_some())
        })
    }

    #[gpui_kit::test]
    fn a_pull_request_or_issue_link_opens_in_loupe(cx: &mut TestAppContext) {
        let cx = &mut open_prose(cx);
        let none = Modifiers::none();

        click(cx, "pr", MouseButton::Left, none);
        assert_eq!(
            tabs(cx),
            (vec![pull_request()], 0),
            "the pull request opens in this tab"
        );
        assert_eq!(cx.opened_url(), None, "and not in the browser");

        click(cx, "issue", MouseButton::Left, none);
        assert_eq!(
            tabs(cx),
            (vec![issue()], 0),
            "the Linear issue opens in this tab"
        );

        click(cx, "pr", MouseButton::Left, Modifiers::command());
        click(cx, "pr", MouseButton::Middle, none);
        assert_eq!(
            tabs(cx),
            (vec![issue(), pull_request(), pull_request()], 0),
            "a Cmd-click and a middle click open it in a new tab"
        );
        assert_eq!(cx.opened_url(), None);
    }

    #[gpui_kit::test]
    fn other_links_open_in_the_browser(cx: &mut TestAppContext) {
        let cx = &mut open_prose(cx);
        let start = tabs(cx);

        click(cx, "site", MouseButton::Left, Modifiers::none());
        assert_eq!(cx.opened_url().as_deref(), Some(SITE));

        click(cx, "other-issue", MouseButton::Left, Modifiers::none());
        assert_eq!(
            cx.opened_url().as_deref(),
            Some(OTHER_ISSUE),
            "an issue of another Linear workspace stays in the browser"
        );
        assert_eq!(tabs(cx), start, "no page changed");
    }

    #[gpui_kit::test]
    fn the_menu_of_a_link_opens_it_in_a_new_tab_or_the_browser(cx: &mut TestAppContext) {
        let cx = &mut open_prose(cx);
        let start = tabs(cx);

        click(cx, "pr", MouseButton::Right, Modifiers::none());
        assert!(menu_open(cx), "a right-click opens the link's menu");
        assert_eq!(tabs(cx), start, "and opens nothing yet");
        assert_eq!(cx.opened_url(), None);

        // The menu has the focus: its rows are Open in new tab, Open in
        // browser, Copy link.
        cx.simulate_keystrokes("down down enter");
        assert_eq!(cx.opened_url().as_deref(), Some(PR), "Open in browser");
        assert!(!menu_open(cx), "picking a row closes the menu");
        assert_eq!(tabs(cx), start);

        click(cx, "pr", MouseButton::Right, Modifiers::none());
        cx.simulate_keystrokes("down enter");
        let (routes, active) = tabs(cx);
        assert_eq!(routes.last(), Some(&pull_request()), "Open in new tab");
        assert_eq!(active, start.1, "in the background");

        click(cx, "site", MouseButton::Right, Modifiers::none());
        cx.simulate_keystrokes("down down enter");
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(
            copied.as_deref(),
            Some(SITE),
            "a link Loupe cannot open has no Open in new tab: its second row is Copy link"
        );
    }
}
