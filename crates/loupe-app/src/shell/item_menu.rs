//! The menu of a row or card that stands for an issue or a pull request, as
//! Linear's: open it (here, in a new tab, side by side), change an issue's
//! status, priority, assignee, labels, and project in submenus, copy its
//! link, identifier, title, or branch, and open it in the browser or in T3
//! Code. Each command has an icon and, where the app binds a key to it, the
//! key at the trailing edge.
//!
//! The same commands are in the command palette, for the row under the
//! pointer or else the page on screen ([`target`]), and their keys act on
//! that same item. Rows report the pointer through [`with_item_menu`];
//! pages report what they show through [`set_page_item`].

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{ContextMenu, ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Icon, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{PrRef, PrSummary};
use loupe_linear::{IssueSummary, Team};

use crate::linear::issue_menu::{self, Field};
use crate::route::Route;
use crate::shell::actions::{
    CopyItemBranch, CopyItemId, CopyItemLink, OpenItemInNewTab, OpenItemSideBySide,
};
use crate::theme::scale::dp;

/// What a row, a card, or a page stands for.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Issue(Box<IssueSummary>),
    PullRequest(PrItem),
    /// A GitHub issue, which Loupe opens only in the browser.
    GitHubIssue {
        title: String,
        url: String,
    },
}

/// A pull request as a row knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PrItem {
    pub pr: PrRef,
    pub title: String,
    pub url: String,
    /// `None` when the row does not carry it; copying it then asks GitHub.
    pub head_ref: Option<String>,
}

impl Item {
    pub fn issue(issue: &IssueSummary) -> Item {
        Item::Issue(Box::new(issue.clone()))
    }

    pub fn pull_request(summary: &PrSummary) -> Item {
        Item::PullRequest(PrItem {
            pr: summary.pr.clone(),
            title: summary.title.clone(),
            url: summary.url.clone(),
            head_ref: Some(summary.head_ref.clone()),
        })
    }

    /// The page Loupe shows it on; `None` for what opens in the browser.
    pub fn route(&self) -> Option<Route> {
        match self {
            Item::Issue(issue) => Some(Route::LinearIssue {
                identifier: issue.identifier.clone(),
            }),
            Item::PullRequest(item) => Some(Route::PullRequest(item.pr.clone())),
            Item::GitHubIssue { .. } => None,
        }
    }

    /// Its name in a heading: `ENG-12`, `owner/repo#3`.
    pub fn reference(&self) -> String {
        match self {
            Item::Issue(issue) => issue.identifier.clone(),
            Item::PullRequest(item) => item.pr.to_string(),
            Item::GitHubIssue { url, .. } => url
                .strip_prefix("https://github.com/")
                .unwrap_or(url)
                .replacen("/issues/", "#", 1),
        }
    }

    pub fn link(&self) -> &str {
        match self {
            Item::Issue(issue) => &issue.url,
            Item::PullRequest(item) => &item.url,
            Item::GitHubIssue { url, .. } => url,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Item::Issue(issue) => &issue.title,
            Item::PullRequest(item) => &item.title,
            Item::GitHubIssue { title, .. } => title,
        }
    }
}

/// One command of the menu and the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    OpenInNewTab,
    OpenSideBySide,
    /// An issue's property: a submenu in the menu, a list in the palette.
    Field(Field),
    CopyLink,
    CopyId,
    CopyTitle,
    CopyBranch,
    /// Open in Linear, or on GitHub.
    OpenInBrowser,
    /// The T3 Code thread linked to a pull request, which holds its branch.
    OpenInT3,
}

/// Where the commands are offered: a row's menu, or for the page on screen,
/// which is already open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Row,
    Page,
}

/// The commands for `item`, in the menu's sections. `t3` says a T3 thread
/// is linked to the pull request.
pub fn sections(item: &Item, place: Place, t3: bool) -> Vec<Vec<Command>> {
    use Command::*;
    let opens = match place {
        Place::Row => vec![Open, OpenInNewTab, OpenSideBySide],
        Place::Page => vec![OpenInNewTab, OpenSideBySide],
    };
    match item {
        Item::Issue(issue) => {
            let mut copies = vec![CopyLink, CopyId, CopyTitle];
            if !issue.branch_name.is_empty() {
                copies.push(CopyBranch);
            }
            vec![
                opens,
                issue_menu::Field::ALL
                    .into_iter()
                    .map(Command::Field)
                    .collect(),
                copies,
                vec![OpenInBrowser],
            ]
        }
        Item::PullRequest(_) => {
            let mut away = vec![OpenInBrowser];
            if t3 {
                away.push(OpenInT3);
            }
            vec![opens, vec![CopyLink, CopyBranch, CopyTitle], away]
        }
        Item::GitHubIssue { .. } => vec![vec![OpenInBrowser], vec![CopyLink, CopyTitle]],
    }
}

impl Command {
    /// Its name in the menu.
    pub fn label(self, item: &Item) -> &'static str {
        match self {
            Command::Open => "Open",
            Command::OpenInNewTab => "Open in new tab",
            Command::OpenSideBySide => "Open side by side",
            Command::Field(field) => field.label(),
            Command::CopyLink => "Copy link",
            Command::CopyId => "Copy ID",
            Command::CopyTitle => "Copy title",
            Command::CopyBranch => match item {
                Item::Issue(_) => "Copy branch name",
                _ => "Copy branch",
            },
            Command::OpenInBrowser => match item {
                Item::Issue(_) => "Open in Linear",
                _ => "Open on GitHub",
            },
            Command::OpenInT3 => "Open in T3 Code",
        }
    }

    /// Its name in the palette, where a property names what picking it
    /// does: "Change status…".
    pub fn palette_label(self, item: &Item) -> &'static str {
        match self {
            Command::Field(field) => field.command(),
            other => other.label(item),
        }
    }

    pub fn icon(self, item: &Item) -> IconName {
        match self {
            Command::Open => IconName::ArrowUpRight,
            Command::OpenInNewTab => IconName::AppWindow,
            Command::OpenSideBySide => IconName::Columns2,
            Command::Field(field) => field.icon(),
            Command::CopyLink => IconName::Link,
            Command::CopyId => IconName::Hash,
            Command::CopyTitle => IconName::Type,
            Command::CopyBranch => IconName::GitBranch,
            Command::OpenInBrowser => match item {
                Item::Issue(_) => IconName::ExternalLink,
                _ => IconName::Github,
            },
            Command::OpenInT3 => IconName::SquareTerminal,
        }
    }

    /// The action whose key runs it, for the key hint.
    pub fn action(self) -> Option<Box<dyn Action>> {
        match self {
            Command::OpenInNewTab => Some(Box::new(OpenItemInNewTab)),
            Command::OpenSideBySide => Some(Box::new(OpenItemSideBySide)),
            Command::Field(field) => Some(field.action()),
            Command::CopyLink => Some(Box::new(CopyItemLink)),
            Command::CopyId => Some(Box::new(CopyItemId)),
            Command::CopyBranch => Some(Box::new(CopyItemBranch)),
            Command::Open | Command::CopyTitle | Command::OpenInBrowser | Command::OpenInT3 => None,
        }
    }
}

/// The row under the pointer, the item each page shows, and the T3 threads
/// last found for pull requests.
#[derive(Default)]
struct Targets {
    hovered: Option<(SharedString, Item)>,
    pages: HashMap<EntityId, Item>,
    t3: HashMap<PrRef, String>,
}

impl Global for Targets {}

/// Records what page `view` shows, for the palette and the keys while the
/// pointer is on no row; `None` when it shows nothing yet.
pub fn set_page_item(view: EntityId, item: Option<Item>, cx: &mut App) {
    let current = cx
        .try_global::<Targets>()
        .and_then(|targets| targets.pages.get(&view));
    if current == item.as_ref() {
        return;
    }
    let targets = cx.default_global::<Targets>();
    match item {
        Some(item) => targets.pages.insert(view, item),
        None => targets.pages.remove(&view),
    };
}

fn set_hovered(key: &SharedString, item: &Item, hovering: bool, cx: &mut App) {
    let targets = cx.default_global::<Targets>();
    if hovering {
        targets.hovered = Some((key.clone(), item.clone()));
    } else if targets
        .hovered
        .as_ref()
        .is_some_and(|(hovered, _)| hovered == key)
    {
        targets.hovered = None;
    }
}

/// Forgets the row under the pointer: the screen changed under it, and its
/// row may be gone without a word.
pub(super) fn forget_hovered(cx: &mut App) {
    if let Some(targets) = cx.try_global::<Targets>()
        && targets.hovered.is_none()
    {
        return;
    }
    cx.default_global::<Targets>().hovered = None;
}

/// What the palette and the item keys act on: the row under the pointer,
/// else the page `page` (the active pane's) shows. An issue comes as the
/// app last showed it: the row reported the copy it had when the pointer
/// arrived.
pub(super) fn target(page: EntityId, cx: &App) -> Option<(Item, Place)> {
    let targets = cx.try_global::<Targets>()?;
    let (item, place) = match &targets.hovered {
        Some((_, item)) => (item, Place::Row),
        None => (targets.pages.get(&page)?, Place::Page),
    };
    let item = match item {
        Item::Issue(issue) => Item::issue(&issue_menu::freshest(issue, cx)),
        other => other.clone(),
    };
    Some((item, place))
}

/// The deep link of the T3 thread last found for `pr`.
pub fn t3_link(pr: &PrRef, cx: &App) -> Option<String> {
    cx.try_global::<Targets>()?.t3.get(pr).cloned()
}

/// Asks T3 for the thread linked to `pr` and remembers it. The task ends
/// with the link, or `None` when T3 is not connected or no thread is linked.
pub fn find_t3_link(pr: &PrRef, cx: &mut App) -> Task<Option<String>> {
    let Some(lookup) = crate::t3::latest_thread_link(pr, cx) else {
        return Task::ready(None);
    };
    let pr = pr.clone();
    cx.spawn(async move |cx| {
        let link = lookup.await;
        cx.update(|cx| {
            let targets = cx.default_global::<Targets>();
            match &link {
                Some(link) => targets.t3.insert(pr, link.clone()),
                None => targets.t3.remove(&pr),
            };
        });
        link
    })
}

/// Adds `item`'s menu to the row or card that shows it, and reports the
/// pointer over it so the item keys and the palette act on it.
pub fn with_item_menu(element: Stateful<Div>, item: Item) -> ContextMenu<Stateful<Div>> {
    let key = SharedString::from(item.reference());
    let hovered = item.clone();
    element
        // A key press must not end the hover: the item keys read it.
        .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
        .on_hover(move |hovering, _, cx| set_hovered(&key, &hovered, *hovering, cx))
        .context_menu(move |menu, window, cx| open_menu(menu, &item, window, cx))
}

/// Builds the menu with what is known now, and builds it again once the
/// issue's team catalog or the pull request's T3 thread arrives.
fn open_menu(
    menu: PopupMenu,
    item: &Item,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    match item {
        Item::Issue(issue) => {
            let team = issue_menu::cached_team(issue, cx);
            if team.is_none() {
                let load = issue_menu::load_team(issue, cx);
                let item = item.clone();
                cx.spawn_in(window, async move |menu, cx| {
                    let Ok(team) = load.await else {
                        return;
                    };
                    menu.update_in(cx, |menu, window, cx| {
                        menu.rebuild(window, cx, |menu, window, cx| {
                            fill(menu, &item, Some(team), None, window, cx)
                        })
                    })
                    .ok();
                })
                .detach();
            }
            fill(menu, item, team, None, window, cx)
        }
        Item::PullRequest(pull) => {
            let known = t3_link(&pull.pr, cx);
            let lookup = find_t3_link(&pull.pr, cx);
            let item = item.clone();
            let shown = known.clone();
            cx.spawn_in(window, async move |menu, cx| {
                let link = lookup.await;
                if link == shown {
                    return;
                }
                menu.update_in(cx, |menu, window, cx| {
                    menu.rebuild(window, cx, |menu, window, cx| {
                        fill(menu, &item, None, link, window, cx)
                    })
                })
                .ok();
            })
            .detach();
            fill(
                menu,
                &Item::PullRequest(pull.clone()),
                None,
                known,
                window,
                cx,
            )
        }
        Item::GitHubIssue { .. } => fill(menu, item, None, None, window, cx),
    }
}

fn fill(
    menu: PopupMenu,
    item: &Item,
    team: Option<Arc<Team>>,
    t3: Option<String>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let mut menu = menu.min_w(dp(240.));
    for section in sections(item, Place::Row, t3.is_some()) {
        menu = menu.separator();
        for command in section {
            menu = match (command, item) {
                (Command::Field(field), Item::Issue(issue)) => {
                    let issue = (**issue).clone();
                    let team = team.clone();
                    menu.submenu_with_icon(
                        Some(Icon::new(field.icon())),
                        field.label(),
                        window,
                        cx,
                        move |submenu, _, _| {
                            issue_menu::fill_submenu(submenu, field, &issue, team.clone())
                        },
                    )
                }
                _ => {
                    let item = item.clone();
                    menu.item(
                        PopupMenuItem::new(command.label(&item))
                            .icon(Icon::new(command.icon(&item)))
                            .when_some(command.action(), |row, action| row.action(action))
                            .on_click(move |_, window, cx| run(command, &item, window, cx)),
                    )
                }
            };
        }
    }
    menu
}

/// Runs `command` on `item`. A property opens its list in the palette.
pub fn run(command: Command, item: &Item, window: &mut Window, cx: &mut App) {
    match command {
        Command::Open => match item.route() {
            Some(route) => super::navigate(route, window, cx),
            None => cx.open_url(item.link()),
        },
        Command::OpenInNewTab => {
            if let Some(route) = item.route() {
                super::open_in_new_tab(route, window, cx);
            }
        }
        Command::OpenSideBySide => {
            if let Some(route) = item.route() {
                super::open_side_by_side(route, window, cx);
            }
        }
        Command::Field(field) => {
            if let Item::Issue(issue) = item {
                super::palette::open_picker((**issue).clone(), field, window, cx);
            }
        }
        Command::CopyLink => copy(
            item.link().to_owned(),
            format!("Copied the link to {}", item.reference()),
            window,
            cx,
        ),
        Command::CopyId => {
            if let Item::Issue(issue) = item {
                copy(
                    issue.identifier.clone(),
                    format!("Copied {}", issue.identifier),
                    window,
                    cx,
                );
            }
        }
        Command::CopyTitle => copy(
            item.title().to_owned(),
            format!("Copied the title of {}", item.reference()),
            window,
            cx,
        ),
        Command::CopyBranch => copy_branch(item, window, cx),
        Command::OpenInBrowser => cx.open_url(item.link()),
        Command::OpenInT3 => {
            if let Item::PullRequest(pull) = item {
                open_in_t3(pull.pr.clone(), window, cx);
            }
        }
    }
}

fn copy(text: String, note: String, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    window.push_notification(Notification::success(note), cx);
}

fn copy_branch(item: &Item, window: &mut Window, cx: &mut App) {
    match item {
        Item::Issue(issue) if !issue.branch_name.is_empty() => copy(
            issue.branch_name.clone(),
            format!("Copied {}", issue.branch_name),
            window,
            cx,
        ),
        Item::PullRequest(PrItem {
            head_ref: Some(branch),
            ..
        }) => copy(branch.clone(), format!("Copied {branch}"), window, cx),
        Item::PullRequest(pull) => {
            let read = crate::github::head_branch(pull.pr.clone(), cx);
            window
                .spawn(cx, async move |cx| {
                    let result = read.await;
                    cx.update(|window, cx| match result {
                        Ok(branch) => {
                            let note = format!("Copied {branch}");
                            copy(branch, note, window, cx);
                        }
                        Err(message) => window.push_notification(
                            Notification::error(message).title("Could not copy the branch"),
                            cx,
                        ),
                    })
                    .ok();
                })
                .detach();
        }
        _ => {}
    }
}

fn open_in_t3(pr: PrRef, window: &mut Window, cx: &mut App) {
    if let Some(link) = t3_link(&pr, cx) {
        cx.open_url(&link);
        return;
    }
    let lookup = find_t3_link(&pr, cx);
    window
        .spawn(cx, async move |cx| {
            let link = lookup.await;
            cx.update(|window, cx| match link {
                Some(link) => cx.open_url(&link),
                None => window.push_notification(
                    Notification::info("No T3 Code thread is linked to this pull request."),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use loupe_github::{PrRef, RepoRef};

    use super::{Command, Item, Place, PrItem, sections};

    fn offers(item: &Item, place: Place, t3: bool, command: Command) -> bool {
        sections(item, place, t3).concat().contains(&command)
    }
    use crate::linear::issue_menu::Field;

    fn pull(head_ref: Option<&str>) -> Item {
        Item::PullRequest(PrItem {
            pr: PrRef::new(RepoRef::new("acme", "api"), 7),
            title: "Fix the login".into(),
            url: "https://github.com/acme/api/pull/7".into(),
            head_ref: head_ref.map(str::to_owned),
        })
    }

    fn labels(item: &Item, place: Place, t3: bool) -> Vec<Vec<&'static str>> {
        sections(item, place, t3)
            .into_iter()
            .map(|section| {
                section
                    .into_iter()
                    .map(|command| command.label(item))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_pull_requests_menu_opens_copies_and_leaves_the_app() {
        assert_eq!(
            labels(&pull(Some("fix/login")), Place::Row, false),
            [
                vec!["Open", "Open in new tab", "Open side by side"],
                vec!["Copy link", "Copy branch", "Copy title"],
                vec!["Open on GitHub"],
            ]
        );
        assert!(
            offers(&pull(None), Place::Row, true, Command::OpenInT3),
            "with a linked thread, the menu opens it in T3 Code"
        );
        assert!(
            !offers(&pull(None), Place::Row, false, Command::OpenInT3),
            "without one, there is no T3 row"
        );
        assert!(
            !offers(&pull(None), Place::Row, false, Command::CopyId),
            "a pull request has no Linear identifier"
        );
    }

    #[test]
    fn the_page_on_screen_is_not_opened_again() {
        let item = pull(None);
        assert!(offers(&item, Place::Row, false, Command::Open));
        assert!(!offers(&item, Place::Page, false, Command::Open));
        assert!(offers(&item, Place::Page, false, Command::OpenSideBySide));
    }

    #[test]
    fn a_github_issue_opens_only_in_the_browser() {
        let item = Item::GitHubIssue {
            title: "Crash".into(),
            url: "https://github.com/acme/api/issues/3".into(),
        };
        assert_eq!(item.reference(), "acme/api#3");
        assert_eq!(item.route(), None);
        assert_eq!(
            labels(&item, Place::Row, false),
            [vec!["Open on GitHub"], vec!["Copy link", "Copy title"]]
        );
    }

    #[test]
    fn every_command_but_open_title_and_browser_has_a_key() {
        let fields = Field::ALL.map(Command::Field);
        for command in [
            Command::OpenInNewTab,
            Command::OpenSideBySide,
            Command::CopyLink,
            Command::CopyId,
            Command::CopyBranch,
        ]
        .into_iter()
        .chain(fields)
        {
            assert!(command.action().is_some(), "{command:?} has no key");
        }
    }
    mod window {
        use gpui_kit::{
            AppContext as _, Context, InteractiveElement as _, IntoElement, KeyContext,
            MouseMoveEvent, ParentElement as _, Pixels, Point, Render,
            StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext,
            Window, div, point, px,
        };
        use loupe_github::{PrRef, RepoRef};

        use super::super::{Command, Item, Place, PrItem, target, with_item_menu};
        use crate::linear::issue_menu::Field;
        use crate::store::Settings;

        fn pull() -> Item {
            Item::PullRequest(PrItem {
                pr: PrRef::new(RepoRef::new("acme", "api"), 7),
                title: "Fix the login".into(),
                url: "https://github.com/acme/api/pull/7".into(),
                head_ref: None,
            })
        }

        /// A page with no row, so the pointer is on none.
        struct NoRows;

        impl Render for NoRows {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full()
            }
        }

        /// One row with the item menu over empty space.
        struct Rows;

        impl Render for Rows {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(with_item_menu(
                    div()
                        .id("row")
                        .debug_selector(|| "row".into())
                        .w(px(300.))
                        .h(px(32.))
                        .on_click(|_, _, _| {}),
                    pull(),
                ))
            }
        }

        fn move_to(cx: &mut VisualTestContext, position: Point<Pixels>) {
            cx.simulate_event(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Default::default(),
            });
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }

        /// The row under the pointer is what the palette and the keys act
        /// on, and stops being it when the pointer leaves, even after a key
        /// press.
        #[gpui_kit::test]
        fn the_row_under_the_pointer_is_the_target(cx: &mut TestAppContext) {
            let (handle, rows) = cx.update(|cx| {
                gpui_kit::init(cx);
                crate::theme::init(cx);
                cx.set_global(Settings::default());
                gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Rows))
                    .expect("open a window")
            });
            let page = rows.entity_id();
            let cx = &mut VisualTestContext::from_window(handle, cx);
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            let row = cx.debug_bounds("row").expect("the row");
            move_to(cx, row.center());
            assert_eq!(
                cx.update(|_, cx| target(page, cx)),
                Some((pull(), Place::Row))
            );
            cx.simulate_keystrokes("x");
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            assert_eq!(
                cx.update(|_, cx| target(page, cx)),
                Some((pull(), Place::Row)),
                "a key press does not end the hover"
            );
            move_to(cx, point(row.center().x, row.bottom() + px(100.)));
            assert_eq!(cx.update(|_, cx| target(page, cx)), None);
        }

        /// Every key the menu shows is one the window answers to wherever
        /// rows are: in the workspace, while no text field has the focus.
        #[gpui_kit::test]
        fn every_key_of_the_menu_is_bound_where_rows_are(cx: &mut TestAppContext) {
            let (handle, _) = cx.update(|cx| {
                gpui_kit::init(cx);
                crate::theme::init(cx);
                cx.set_global(Settings::default());
                crate::shell::menus::bind_keys(cx);
                gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Rows))
                    .expect("open a window")
            });
            let cx = &mut VisualTestContext::from_window(handle, cx);
            let workspace = KeyContext::parse("Workspace").expect("context");
            let commands = [
                Command::OpenInNewTab,
                Command::OpenSideBySide,
                Command::CopyLink,
                Command::CopyId,
                Command::CopyBranch,
            ]
            .into_iter()
            .chain(Field::ALL.map(Command::Field));
            for command in commands {
                let action = command.action().expect("a key");
                let bound = cx.update(|window, _| {
                    window
                        .highest_precedence_binding_for_action_in_context(
                            action.as_ref(),
                            workspace.clone(),
                        )
                        .is_some()
                });
                assert!(bound, "{command:?} shows a key nothing binds");
            }
        }

        /// The keys and the palette act on the issue as the app last showed
        /// it, not on the copy the page or the row reported earlier.
        #[gpui_kit::test]
        fn the_target_issue_is_the_newest_copy(cx: &mut TestAppContext) {
            use crate::linear::issue_menu::tests::issue;

            let (_, rows) = cx.update(|cx| {
                gpui_kit::init(cx);
                crate::theme::init(cx);
                cx.set_global(Settings::default());
                gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| NoRows))
                    .expect("open a window")
            });
            let page = rows.entity_id();
            cx.update(|cx| {
                super::super::set_page_item(page, Some(Item::issue(&issue())), cx);
                let mut picked = issue();
                picked.state.id = "done".into();
                crate::linear::issue_menu::tests::publish_shown(picked, cx);
                let Some((Item::Issue(target), Place::Page)) = target(page, cx) else {
                    panic!("the page's issue");
                };
                assert_eq!(target.state.id, "done");
            });
        }
    }
}
