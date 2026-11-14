//! The sidebar, after Linear's: the workspace row with Search and New issue;
//! Inbox, My issues, and Reviews with their counts; then foldable sections
//! (Workspace, Favorites, Your teams with each team's issues, projects, and
//! views, and Repositories); and the account footer. Expanded, at the width
//! its right edge was dragged to, or collapsed to an icon rail (Cmd+B).
//!
//! The rail is the same sidebar with its texts hidden: the same entries in
//! the same rows, so every icon keeps its y, the step between rows, and the
//! space between groups. A section's heading becomes a short divider in a
//! slot of the heading's height.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{GitHub, NOTIFICATIONS_LIMIT, Notification, Viewer};
use loupe_linear::{Linear, TeamSummary};

use crate::github::notification_filter;
use crate::linear::{self, SavedView};
use crate::route::Route;
use crate::services::{Accounts, Connection, Services};
use crate::shell::actions::{OpenPalette, SwitchWorkspace};
use crate::shell::workspace_menu;
use crate::shell::{OpenOnClick as _, navigate, open, svg_icon};
use crate::store::{self, Favorite, Settings};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

const DEFAULT_WIDTH: f32 = 248.;
/// The narrowest and widest the expanded sidebar can be dragged, in points
/// at 100% zoom.
const MIN_WIDTH: f32 = 180.;
const MAX_WIDTH: f32 = 420.;
/// The workspace's mark, in points at 100% zoom.
const MARK_SIZE: f32 = 20.;
const MARK_RADIUS: f32 = 5.;
/// The rail's width in points at 100% zoom. Its widest content is a 28 px
/// row; the workspace's mark, alone and centered at its top, needs no more.
const RAIL_WIDTH: f32 = 72.;
/// Wide enough that the traffic lights (12 to 64 px) sit inside the rail, so
/// zooming out never narrows it below this.
const COLLAPSED_WIDTH: Pixels = px(72.);
// The mark fits the rail with room on each side.
const _: () = assert!(RAIL_WIDTH >= MARK_SIZE + 16.);
/// How often the Inbox and Reviews counts refresh.
const COUNTS_INTERVAL: Duration = Duration::from_secs(120);

/// The expanded sidebar's width in points at 100% zoom, for a stored or
/// dragged width: the default when there is none, else clamped to the
/// allowed range.
pub(super) fn expanded_width(width: Option<f32>) -> f32 {
    width.map_or(DEFAULT_WIDTH, |width| width.clamp(MIN_WIDTH, MAX_WIDTH))
}

/// The sidebar's width, zoomed: the icon rail when collapsed, else
/// `expanded` points.
pub(super) fn width(collapsed: bool, expanded: f32) -> Pixels {
    if collapsed {
        dp(RAIL_WIDTH).max(COLLAPSED_WIDTH)
    } else {
        dp(expanded)
    }
}

/// What the sidebar shows besides settings, loaded in the background.
#[derive(Clone, Default)]
struct Loaded {
    /// Unread GitHub notifications, counted through the Inbox's filter.
    unread: Option<Arc<Vec<Notification>>>,
    /// Open pull requests waiting on the viewer's review.
    reviews: Option<u32>,
    /// The teams the Linear user is a member of.
    my_teams: Arc<Vec<TeamSummary>>,
    /// Every team, to name the team of a starred view.
    all_teams: Arc<Vec<TeamSummary>>,
    teams_failed: bool,
    remote: Vec<loupe_linear::LinearFavorite>,
}

/// Loads the counts and the teams for the sidebar, and reloads them when a
/// client changes. The workspace owns it and re-renders when it notifies.
pub(super) struct SidebarData {
    github: Option<Arc<GitHub>>,
    linear: Option<Arc<Linear>>,
    loaded: Loaded,
    _counts: Option<Task<()>>,
    _teams: Option<Task<()>>,
    _services: Subscription,
}

impl SidebarData {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let mut data = SidebarData {
            github: None,
            linear: None,
            loaded: Loaded::default(),
            _counts: None,
            _teams: None,
            _services: cx.observe_global::<Services>(Self::sync),
        };
        data.sync(cx);
        data
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let services = Services::get(cx);
        let (github, linear) = (services.github.clone(), services.linear.clone());
        if !same_client(&self.github, &github) {
            self.github = github;
            self.refresh_counts(cx);
        }
        if !same_client(&self.linear, &linear) {
            self.linear = linear;
            self.load_teams(cx);
        }
    }

    /// Counts now, then every `COUNTS_INTERVAL` while the client lasts. A
    /// failed fetch keeps the last count.
    fn refresh_counts(&mut self, cx: &mut Context<Self>) {
        self.loaded.unread = None;
        self.loaded.reviews = None;
        self._counts = self.github.clone().map(|github| {
            cx.spawn(async move |this, cx| {
                loop {
                    let client = github.clone();
                    let (unread, reviews) = cx
                        .background_executor()
                        .spawn(async move {
                            let unread = client.notifications(false).map(|mut list| {
                                list.retain(|notification| notification.unread);
                                Arc::new(list)
                            });
                            let reviews = client
                                .inbox()
                                .map(|inbox| inbox.review_requested.total_count);
                            (unread, reviews)
                        })
                        .await;
                    let updated = this.update(cx, |data, cx| {
                        if let Ok(unread) = unread {
                            data.loaded.unread = Some(unread);
                        }
                        if let Ok(reviews) = reviews {
                            data.loaded.reviews = Some(reviews);
                        }
                        cx.notify();
                    });
                    if updated.is_err() {
                        break;
                    }
                    cx.background_executor().timer(COUNTS_INTERVAL).await;
                }
            })
        });
        cx.notify();
    }

    fn load_teams(&mut self, cx: &mut Context<Self>) {
        self.loaded.my_teams = Arc::default();
        self.loaded.all_teams = Arc::default();
        self.loaded.teams_failed = false;
        self._teams = None;
        if self.linear.is_some() {
            let (mine, all) = (linear::my_teams(cx), linear::all_teams(cx));
            self._teams = Some(cx.spawn(async move |this, cx| {
                let (mine, all) = (mine.await, all.await);
                this.update(cx, |data, cx| {
                    data.loaded.teams_failed = mine.is_err();
                    data.loaded.my_teams = mine.unwrap_or_default();
                    data.loaded.all_teams = all.unwrap_or_default();
                    cx.notify();
                })
                .ok();
            }));
        }
        cx.notify();
    }
}

fn same_client<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// What a sidebar row shows in its fixed leading slot.
enum Leading {
    Icon(IconName),
    Colored(IconName, Option<Hsla>),
    Avatar {
        url: SharedString,
        name: SharedString,
    },
}

/// What activating a row does.
#[derive(Debug, PartialEq)]
enum Target {
    Route(Route),
    /// Folds or opens the section with this key.
    Fold {
        section: String,
        open: bool,
    },
    /// Loads the teams again after a failure.
    RetryTeams,
    Url(String),
    Label,
}

struct Row {
    id: SharedString,
    label: SharedString,
    /// The accessible name, when the label alone is ambiguous ("Issues").
    name: Option<SharedString>,
    leading: Leading,
    /// Trailing text, such as a count or a team name.
    meta: Option<SharedString>,
    /// Indented under a team.
    nested: bool,
    target: Target,
}

enum Entry {
    /// A foldable heading; `key` names it in `Settings.collapsed_sections`.
    Section {
        key: &'static str,
        label: &'static str,
        open: bool,
    },
    /// Boxed: a row holds a route, which is far larger than a heading.
    Row(Box<Row>),
}

impl From<Row> for Entry {
    fn from(row: Row) -> Self {
        Entry::Row(Box::new(row))
    }
}

fn row(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: IconName,
    target: Target,
) -> Row {
    Row {
        id: id.into(),
        label: label.into(),
        name: None,
        leading: Leading::Icon(icon),
        meta: None,
        nested: false,
        target,
    }
}

/// A row that opens `route`.
fn link(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: IconName,
    route: Route,
) -> Row {
    row(id, label, icon, Target::Route(route))
}

/// A count as trailing text; zero and unknown show nothing, and a count at
/// `limit` (where fetching stopped) shows as `200+`.
fn count(value: Option<usize>, limit: Option<usize>) -> Option<SharedString> {
    value.filter(|count| *count > 0).map(|count| match limit {
        Some(limit) if count >= limit => format!("{limit}+").into(),
        _ => count.to_string().into(),
    })
}

/// `row` with `count` as its trailing text and in its accessible name, so
/// agents read "Inbox, 12".
fn counted(row: Row, count: Option<SharedString>) -> Row {
    Row {
        name: count
            .as_ref()
            .map(|count| format!("{}, {count}", row.label).into()),
        meta: count,
        ..row
    }
}

/// The sidebar's entries, in order: the same expanded and on the rail.
fn entries(settings: &Settings, loaded: &Loaded, views: &[SavedView]) -> Vec<Entry> {
    let mut entries = vec![
        Entry::from(counted(
            link("inbox", "Inbox", IconName::Inbox, Route::Notifications),
            count(
                loaded.unread.as_ref().map(|list| {
                    let terms = notification_filter::from_tokens(&settings.notification_filter);
                    notification_filter::apply(&terms, list).len()
                }),
                Some(NOTIFICATIONS_LIMIT),
            ),
        )),
        Entry::from(link(
            "my-issues",
            "My issues",
            IconName::SquareDashed,
            Route::LinearMyIssues,
        )),
        Entry::from(counted(
            link("reviews", "Reviews", IconName::GitPullRequest, Route::Inbox),
            count(loaded.reviews.map(|reviews| reviews as usize), None),
        )),
    ];
    let section = |entries: &mut Vec<Entry>, key: &'static str, label: &'static str| {
        let open = !settings.is_collapsed(key);
        entries.push(Entry::Section { key, label, open });
        open
    };

    if section(&mut entries, "workspace", "Workspace") {
        entries.push(Entry::from(link(
            "projects",
            "Projects",
            IconName::Box,
            Route::LinearProjects { team_id: None },
        )));
        entries.push(Entry::from(link(
            "views",
            "Views",
            IconName::Layers,
            Route::LinearViews { team_id: None },
        )));
    }

    let mut favorites: Vec<Row> = loaded
        .remote
        .iter()
        .filter_map(|favorite| remote_favorite_row(favorite, loaded, views))
        .collect();
    let local_favorites: Vec<Row> = settings
        .favorites
        .iter()
        .filter_map(|favorite| favorite_row(favorite, loaded, views))
        .collect();
    favorites.extend(local_favorites);
    if !favorites.is_empty() && section(&mut entries, "favorites", "Favorites") {
        entries.extend(favorites.into_iter().map(Entry::from));
    }

    if (!loaded.my_teams.is_empty() || loaded.teams_failed)
        && section(&mut entries, "teams", "Your teams")
    {
        if loaded.teams_failed {
            entries.push(Entry::from(row(
                "teams-retry",
                "Could not load teams. Retry",
                IconName::RotateCw,
                Target::RetryTeams,
            )));
        }
        for team in loaded.my_teams.iter() {
            team_rows(&mut entries, team, settings);
        }
    }

    if !settings.watched_repos.is_empty() && section(&mut entries, "repositories", "Repositories") {
        entries.extend(settings.watched_repos.iter().map(|repo| {
            Entry::from(Row {
                leading: Leading::Avatar {
                    url: format!("https://github.com/{}.png?size=40", repo.owner).into(),
                    name: repo.owner.clone().into(),
                },
                ..link(
                    format!("repo-{repo}"),
                    repo.to_string(),
                    IconName::FolderGit,
                    Route::Repo(repo.clone()),
                )
            })
        }));
    }
    entries
}

/// A team's row, and under it, unless folded, its Issues, Projects, and Views.
fn team_rows(entries: &mut Vec<Entry>, team: &TeamSummary, settings: &Settings) {
    let issues = Route::LinearTeam {
        team_id: team.id.clone(),
        name: team.name.clone(),
        view_id: None,
    };
    let section = format!("team:{}", team.id);
    let open = !settings.is_collapsed(&section);
    entries.push(Entry::from(row(
        format!("team-{}", team.id),
        team.name.clone(),
        IconName::Users,
        Target::Fold { section, open },
    )));
    if !open {
        return;
    }
    let pages = [
        ("issues", "Issues", IconName::Copy, issues),
        (
            "projects",
            "Projects",
            IconName::Box,
            Route::LinearProjects {
                team_id: Some(team.id.clone()),
            },
        ),
        (
            "views",
            "Views",
            IconName::Layers,
            Route::LinearViews {
                team_id: Some(team.id.clone()),
            },
        ),
    ];
    entries.extend(pages.into_iter().map(|(page, label, icon, route)| {
        Entry::from(Row {
            name: Some(format!("{} {label}", team.name).into()),
            nested: true,
            ..link(format!("team-{}-{page}", team.id), label, icon, route)
        })
    }));
}

/// A starred item as a row, or `None` while its team is unknown or after its
/// view was deleted.
fn remote_favorite_row(
    favorite: &loupe_linear::LinearFavorite,
    loaded: &Loaded,
    views: &[SavedView],
) -> Option<Row> {
    use loupe_linear::FavoriteTarget;
    let mut title = favorite.title.clone();
    let target = match &favorite.target {
        FavoriteTarget::CustomView { view_id, team } => {
            let id = format!("linear:{view_id}");
            let saved = views.iter().find(|view| view.id == id)?;
            let team = team
                .as_ref()
                .or_else(|| {
                    saved
                        .team_id
                        .as_ref()
                        .and_then(|id| loaded.all_teams.iter().find(|team| &team.id == id))
                })
                .or_else(|| loaded.all_teams.first());
            let team = team?;
            Target::Route(Route::LinearTeam {
                team_id: team.id.clone(),
                name: team.name.clone(),
                view_id: Some(id),
            })
        }
        FavoriteTarget::PredefinedView {
            kind,
            team: Some(team),
        } if kind == "allIssues" => {
            title = format!("{} {}", title, team.name);
            Target::Route(Route::LinearTeam {
                team_id: team.id.clone(),
                name: team.name.clone(),
                view_id: Some(format!("linear-all:{}", team.id)),
            })
        }
        FavoriteTarget::Issue { identifier } => Target::Route(Route::LinearIssue {
            identifier: identifier.clone(),
        }),
        FavoriteTarget::Other { kind } if kind == "folder" => Target::Label,
        _ => Target::Url(favorite.url.clone()?),
    };
    let icon = if favorite.icon.is_some() {
        linear::view_icon(favorite.icon.as_deref())
    } else {
        match &favorite.target {
            FavoriteTarget::Issue { .. } => IconName::CircleDot,
            FavoriteTarget::PredefinedView { .. } => IconName::Copy,
            FavoriteTarget::Other { kind } if kind == "project" => IconName::Box,
            FavoriteTarget::Other { kind } if kind == "cycle" => IconName::RefreshCw,
            _ => IconName::Layers,
        }
    };
    let mut row = row(
        format!("linear-favorite-{}", favorite.id),
        title,
        icon,
        target,
    );
    row.leading = Leading::Colored(icon, linear::view_color(favorite.color.as_deref()));
    Some(row)
}

/// Whether a starred team or issue belongs to the active workspace: one of
/// its teams matches. Until the teams load, every star shows.
fn in_workspace(loaded: &Loaded, matches: impl Fn(&TeamSummary) -> bool) -> bool {
    loaded.all_teams.is_empty() || loaded.all_teams.iter().any(matches)
}

fn favorite_row(favorite: &Favorite, loaded: &Loaded, views: &[SavedView]) -> Option<Row> {
    match favorite {
        Favorite::View { id } => {
            let view = views.iter().find(|view| &view.id == id)?;
            let team = match &view.team_id {
                Some(team_id) => loaded
                    .my_teams
                    .iter()
                    .chain(loaded.all_teams.iter())
                    .find(|team| &team.id == team_id)?,
                None => loaded.my_teams.first().or(loaded.all_teams.first())?,
            };
            let route = Route::LinearTeam {
                team_id: team.id.clone(),
                name: team.name.clone(),
                view_id: Some(view.id.clone()),
            };
            // "My issues view", apart from the fixed "My issues" row.
            Some(Row {
                name: Some(format!("{} view", view.name).into()),
                ..link(
                    format!("favorite-view-{id}"),
                    view.name.clone(),
                    IconName::Layers,
                    route,
                )
            })
        }
        Favorite::Team { team_id, name } => {
            if !in_workspace(loaded, |team| &team.id == team_id) {
                return None;
            }
            let route = Route::LinearTeam {
                team_id: team_id.clone(),
                name: name.clone(),
                view_id: None,
            };
            Some(Row {
                name: Some(format!("All issues {name}").into()),
                meta: Some(name.clone().into()),
                ..link(
                    format!("favorite-team-{team_id}"),
                    "All issues",
                    IconName::Copy,
                    route,
                )
            })
        }
        Favorite::Issue { identifier, title } => {
            let key = identifier.split('-').next().unwrap_or_default();
            if !in_workspace(loaded, |team| team.key.eq_ignore_ascii_case(key)) {
                return None;
            }
            let route = Route::LinearIssue {
                identifier: identifier.clone(),
            };
            Some(Row {
                name: Some(format!("{identifier} {title}").into()),
                meta: Some(identifier.clone().into()),
                ..link(
                    format!("favorite-issue-{identifier}"),
                    title.clone(),
                    IconName::CircleDot,
                    route,
                )
            })
        }
    }
}

/// Whether a row's route is the page on screen. A team matches by id and
/// view, whatever name the route carries.
fn same_page(row: &Route, current: &Route) -> bool {
    match (row, current) {
        (
            Route::LinearTeam {
                team_id: a,
                view_id: view_a,
                ..
            },
            Route::LinearTeam {
                team_id: b,
                view_id: view_b,
                ..
            },
        ) => a == b && view_a == view_b,
        _ => row == current,
    }
}

/// Colors the sidebar reads, copied out of the theme once per render.
#[derive(Clone, Copy)]
struct Palette {
    foreground: Hsla,
    muted: Hsla,
    faint: Hsla,
    hover: Hsla,
    selected: Hsla,
    mark: Hsla,
    mark_text: Hsla,
}

/// Renders the sidebar for the current route and signed-in user.
pub(super) fn render(
    current: &Route,
    viewer: &Viewer,
    data: &Entity<SidebarData>,
    collapsed: bool,
    expanded: f32,
    cx: &mut App,
) -> impl IntoElement {
    let mut loaded = data.read(cx).loaded.clone();
    loaded.remote = linear::remote_favorites(cx);
    let views = linear::saved_views(cx);
    let organization: Option<SharedString> = match &Accounts::get(cx).linear {
        Connection::Connected(viewer) => Some(viewer.organization.name.clone().into()),
        _ => None,
    };
    let logo = linear::workspace_logo(cx);
    let current_team = match current {
        Route::LinearTeam { team_id, .. } => Some(team_id.clone()),
        _ => None,
    };
    let theme = cx.theme();
    let colors = Palette {
        foreground: theme.sidebar_foreground,
        muted: theme.muted_foreground,
        faint: tokens::text_faint(),
        hover: tokens::glass_hover(),
        selected: theme.sidebar_accent,
        mark: theme.primary,
        mark_text: theme.primary_foreground,
    };

    let rows = entries(Settings::get(cx), &loaded, &views)
        .into_iter()
        .map(|entry| match entry {
            Entry::Section { key, label, open } => {
                section_header(key, label, open, collapsed, colors).into_any_element()
            }
            Entry::Row(row) => nav_row(*row, current, collapsed, colors, data).into_any_element(),
        });

    v_flex()
        .w(width(collapsed, expanded))
        .h_full()
        .flex_none()
        .overflow_hidden()
        .text_color(colors.foreground)
        .child(workspace_row(
            organization,
            logo,
            current_team,
            collapsed,
            colors,
        ))
        .child(
            v_flex()
                .id("sidebar-nav")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_2()
                .pb_2()
                .when(collapsed, |nav| nav.items_center())
                .children(rows),
        )
        .child(footer(
            viewer,
            collapsed,
            *current == Route::Settings,
            colors,
        ))
}

/// The Linear workspace's name with Search and New issue, as in Linear's
/// sidebar header.
fn workspace_row(
    organization: Option<SharedString>,
    logo: Option<Arc<Image>>,
    current_team: Option<String>,
    collapsed: bool,
    colors: Palette,
) -> impl IntoElement {
    let connected = organization.is_some();
    let name = organization.unwrap_or_else(|| "Loupe".into());
    let initial: SharedString = name
        .chars()
        .next()
        .unwrap_or('L')
        .to_uppercase()
        .collect::<String>()
        .into();
    // The workspace's own logo; its initial on the accent stands in only
    // when it has none (or Linear is not connected).
    let mark = div()
        .id("sidebar-workspace-mark")
        .flex_none()
        .size(dp(MARK_SIZE))
        .rounded(dp(MARK_RADIUS));
    let mark = match logo {
        // The image carries the radius itself: a parent clips its children
        // to its box, not to its rounded corners.
        Some(logo) => mark.child(
            img(logo)
                .size_full()
                .rounded(dp(MARK_RADIUS))
                .object_fit(ObjectFit::Cover),
        ),
        None => mark
            .bg(colors.mark)
            .text_color(colors.mark_text)
            .text_size(scale::label())
            .font_weight(FontWeight::BOLD)
            .flex()
            .items_center()
            .justify_center()
            .child(initial),
    };
    if collapsed {
        // The same 44 px row with the mark alone, centered over the icons
        // below, at the y it has with the sidebar open. The name is its
        // tooltip; Search and New issue come back with the sidebar.
        let tooltip = name.clone();
        return h_flex()
            .id("sidebar-workspace")
            .role(Role::Heading)
            .aria_label(name)
            .relative()
            .flex_none()
            .h(dp(44.))
            .items_center()
            .justify_center()
            .child(workspace_menu::track_header())
            .child(
                mark.role(Role::Button)
                    .aria_label("Switch workspace")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(SwitchWorkspace), cx))
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx)),
            )
            .into_any_element();
    }
    let search = Button::new("sidebar-search")
        .ghost()
        .small()
        .icon(IconName::Search)
        .tooltip("Search (⌘K)")
        .accessibility_label("Search")
        .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenPalette), cx));
    let new_issue = Button::new("sidebar-new-issue")
        .ghost()
        .small()
        .icon(IconName::SquarePen)
        .tooltip("New issue (C)")
        .accessibility_label("New issue")
        .disabled(!connected)
        .on_click(move |_, window, cx| linear::compose_issue(current_team.clone(), window, cx));
    // The mark keeps its x: the row's 12 px and the switcher's 4 px.
    h_flex()
        .flex_none()
        .h(dp(44.))
        .pl_3()
        .pr_2()
        .gap_1()
        .items_center()
        .child(
            h_flex()
                .id("sidebar-workspace-switcher")
                .debug_selector(|| "sidebar-workspace-switcher".into())
                .role(Role::Button)
                .aria_label("Switch workspace")
                .relative()
                .flex_1()
                .min_w_0()
                .h(scale::nav_row_height())
                .px_1()
                .gap_1()
                .items_center()
                .rounded(dp(6.))
                .hover(|style| style.bg(colors.hover))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(SwitchWorkspace), cx))
                .child(workspace_menu::track_header())
                .child(
                    h_flex()
                        .id("sidebar-workspace")
                        .role(Role::Heading)
                        .aria_label(name.clone())
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .child(mark)
                        .child(
                            div()
                                .truncate()
                                .text_size(scale::body())
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name),
                        ),
                )
                .child(svg_icon(IconName::ChevronDown, dp(12.), colors.faint)),
        )
        .child(search)
        .child(new_issue)
        .into_any_element()
}

/// A section's heading: its label and a chevron. On the rail, a short
/// divider in a slot of the same height and with the same space above, so
/// the rows under it keep their place; it still folds the section, and its
/// tooltip names it.
fn section_header(
    key: &'static str,
    label: &'static str,
    open: bool,
    collapsed: bool,
    colors: Palette,
) -> impl IntoElement {
    let header = h_flex()
        .id(SharedString::from(format!("section-{key}")))
        .role(Role::Button)
        .aria_label(format!(
            "{} {label}",
            if open { "Collapse" } else { "Expand" }
        ))
        .mt_4()
        .h(scale::nav_row_height())
        .px_2()
        .gap_1()
        .items_center()
        .rounded(dp(6.))
        .on_click(move |_, _, cx| {
            store::update_settings(cx, |settings| settings.toggle_collapsed(key));
        });
    if collapsed {
        return header
            .w(scale::nav_row_height())
            .justify_center()
            .hover(|style| style.bg(colors.hover))
            .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
            .child(
                div()
                    .w_4()
                    .h(px(1.))
                    .bg(colors.faint.opacity(if open { 0.6 } else { 0.3 })),
            );
    }
    header
        // A section label, like `ui::section_label`, in the sidebar's own
        // faint color so it stays under the rows it heads.
        .text_size(scale::body())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.faint)
        .hover(|style| style.text_color(colors.muted))
        .child(label)
        .child(svg_icon(
            if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            },
            dp(12.),
            colors.faint,
        ))
}

fn nav_row(
    row: Row,
    current: &Route,
    collapsed: bool,
    colors: Palette,
    data: &Entity<SidebarData>,
) -> impl IntoElement {
    let is_selected = matches!(&row.target, Target::Route(route) if same_page(route, current));
    let fold = match &row.target {
        Target::Fold { open, .. } => Some(*open),
        _ => None,
    };
    let name = match (&row.name, fold) {
        (Some(name), _) => name.clone(),
        (None, Some(open)) => {
            format!("{} {}", if open { "Collapse" } else { "Expand" }, row.label).into()
        }
        (None, None) => row.label.clone(),
    };
    let label = row.label;
    let target = row.target;
    let hint = name.clone();
    let route = match &target {
        Target::Route(route) => Some(route.clone()),
        _ => None,
    };
    let data = data.downgrade();
    let selector = format!("sidebar-{}", row.id);
    h_flex()
        .id(row.id)
        .debug_selector(|| selector)
        .role(if matches!(target, Target::Label) {
            Role::Heading
        } else if fold.is_some() {
            Role::Button
        } else {
            Role::Link
        })
        .aria_label(name)
        .h(scale::nav_row_height())
        .px_2()
        .when(row.nested && !collapsed, |item| item.pl(dp(32.)))
        .gap_2()
        .rounded(dp(6.))
        .text_size(scale::body())
        // On the rail the row is its icon alone, in the same 28 px slot;
        // its tooltip says what the label and the count said.
        .when(collapsed, |item| {
            item.w(scale::nav_row_height())
                .justify_center()
                .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
        })
        // The fill and the foreground color mark the selection; the weight
        // stays, so selecting a row never moves its label.
        .when(is_selected, |item| {
            item.bg(colors.selected).text_color(colors.foreground)
        })
        .when(!is_selected, |item| {
            item.text_color(colors.muted)
                .hover(|style| style.bg(colors.hover).text_color(colors.foreground))
        })
        .on_click(move |event, window, cx| match &target {
            Target::Route(route) => open(route.clone(), event, window, cx),
            Target::Fold { section, .. } => {
                let section = section.clone();
                store::update_settings(cx, move |settings| settings.toggle_collapsed(&section));
            }
            Target::Label => {}
            Target::Url(url) => cx.open_url(url),
            Target::RetryTeams => {
                data.update(cx, |data, cx| data.load_teams(cx)).ok();
            }
        })
        .when_some(route, |item, route| {
            item.on_middle_click(move |event, window, cx| open(route.clone(), event, window, cx))
        })
        // The open page's icon takes the accent, so the selection is not
        // one more grey fill beside the open tab and the open view.
        .child(leading(
            row.leading,
            if is_selected {
                tokens::accent()
            } else {
                colors.faint
            },
        ))
        .when(!collapsed, |item| {
            item.child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .items_center()
                    .child(div().min_w_0().truncate().child(label))
                    .when_some(fold, |label, open| {
                        label.child(svg_icon(
                            if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                            dp(12.),
                            colors.faint,
                        ))
                    }),
            )
            .when_some(row.meta, |item, meta| {
                item.child(
                    div()
                        .flex_none()
                        .max_w(dp(96.))
                        .truncate()
                        .text_size(scale::meta())
                        .text_color(colors.faint)
                        .child(meta),
                )
            })
        })
}

/// A 16 px slot, so labels line up whatever leads them.
fn leading(leading: Leading, icon: Hsla) -> impl IntoElement {
    let slot = div()
        .size_4()
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match leading {
        Leading::Icon(name) => slot.child(svg_icon(name, dp(16.), icon)),
        Leading::Colored(name, color) => slot.child(svg_icon(name, dp(16.), color.unwrap_or(icon))),
        Leading::Avatar { url, name } => slot.child(ui::avatar_named(url, name, dp(16.))),
    }
}

fn footer(
    viewer: &Viewer,
    collapsed: bool,
    settings_selected: bool,
    colors: Palette,
) -> impl IntoElement {
    let name: SharedString = viewer
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| viewer.login.clone())
        .into();
    let login: SharedString = format!("@{}", viewer.login).into();
    let settings = Button::new("sidebar-settings")
        .ghost()
        .small()
        .icon(IconName::Settings)
        .selected(settings_selected)
        .tooltip("Settings (⌘,)")
        .accessibility_label("Settings")
        .on_click(|_, window, cx| navigate(Route::Settings, window, cx));
    let avatar = ui::avatar_named(viewer.avatar_url.clone(), name.clone(), dp(24.));
    // One row in both states: the avatar, the name over the login, the
    // settings button. On the rail the two lines are still laid out, in a
    // column with no width, so the row keeps its height and the avatar and
    // the button keep their y.
    let account = v_flex()
        .min_w_0()
        .when(!collapsed, |account| account.flex_1().mx_2())
        .when(collapsed, |account| {
            account.flex_none().w_0().mx_1().overflow_hidden()
        })
        .child(
            div()
                .truncate()
                .text_size(scale::body())
                .font_weight(FontWeight::MEDIUM)
                .child(name),
        )
        .child(
            div()
                .truncate()
                .text_size(scale::meta())
                .text_color(colors.faint)
                .child(login),
        );
    div()
        .flex_none()
        .when(!collapsed, |footer| footer.pl_4().pr_2())
        .when(collapsed, |footer| footer.px_2())
        .py_2()
        .border_t_1()
        .border_color(tokens::border_subtle())
        .child(
            h_flex()
                .when(collapsed, |row| row.justify_center())
                .child(div().flex_none().child(avatar))
                .child(account)
                .child(settings),
        )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::DateTime;
    use loupe_github::{Notification, NotificationSubject, RepoRef};
    use loupe_linear::TeamSummary;

    use super::{
        COLLAPSED_WIDTH, DEFAULT_WIDTH, Entry, Loaded, Target, entries, expanded_width, same_page,
        width,
    };
    use crate::linear::SavedView;
    use crate::route::Route;
    use crate::store::{Favorite, Settings};

    #[test]
    fn the_rail_is_never_narrower_than_the_traffic_lights() {
        // Whatever the zoom, the traffic lights stay inside it.
        assert!(width(true, DEFAULT_WIDTH) >= COLLAPSED_WIDTH);
    }

    #[test]
    fn expanded_width_defaults_and_clamps_to_the_drag_range() {
        assert_eq!(expanded_width(None), 248.);
        assert_eq!(expanded_width(Some(300.)), 300.);
        assert_eq!(expanded_width(Some(40.)), 180.);
        assert_eq!(expanded_width(Some(2000.)), 420.);
    }

    fn team(id: &str, name: &str) -> TeamSummary {
        TeamSummary {
            id: id.into(),
            key: id.to_uppercase(),
            name: name.into(),
        }
    }

    fn unread(count: usize, repo: &str) -> Vec<Notification> {
        let repo = RepoRef::parse(repo).expect("repo");
        (0..count)
            .map(|ix| Notification {
                id: format!("{repo}-{ix}"),
                repo: repo.clone(),
                title: format!("Notification {ix}"),
                subject: NotificationSubject::Other {
                    kind: "Issue".into(),
                },
                reason: "mention".into(),
                unread: true,
                updated_at: DateTime::from_timestamp(0, 0).expect("timestamp"),
            })
            .collect()
    }

    fn loaded() -> Loaded {
        Loaded {
            unread: Some(Arc::new(unread(73, "acme/api"))),
            reviews: Some(0),
            my_teams: Arc::new(vec![team("be", "Back-end")]),
            all_teams: Arc::new(vec![team("be", "Back-end"), team("fe", "Front-end")]),
            teams_failed: false,
            remote: Vec::new(),
        }
    }

    /// Each entry as a short line: `## Section`, or a row's accessible name
    /// with its count, `  ` in front when nested.
    fn outline(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Section { label, open, .. } => {
                    format!("## {label}{}", if *open { "" } else { " (folded)" })
                }
                Entry::Row(row) => {
                    let name = row.name.clone().unwrap_or_else(|| row.label.clone());
                    let meta = row
                        .meta
                        .as_ref()
                        .map(|meta| format!(" [{meta}]"))
                        .unwrap_or_default();
                    format!("{}{name}{meta}", if row.nested { "  " } else { "" })
                }
            })
            .collect()
    }

    #[test]
    fn lays_out_like_linear_with_counts_favorites_and_team_pages() {
        let settings = Settings {
            favorites: vec![
                Favorite::View { id: "v-fe".into() },
                Favorite::View {
                    id: "deleted".into(),
                },
                Favorite::Team {
                    team_id: "be".into(),
                    name: "Back-end".into(),
                },
                Favorite::Issue {
                    identifier: "BE-293".into(),
                    title: "[Loupe test]".into(),
                },
                // Starred in another workspace: none of this one's teams.
                Favorite::Team {
                    team_id: "ops".into(),
                    name: "Operations".into(),
                },
                Favorite::Issue {
                    identifier: "OPS-7".into(),
                    title: "Elsewhere".into(),
                },
            ],
            ..Settings::default()
        };
        let views = vec![SavedView {
            id: "v-fe".into(),
            name: "Meus issues".into(),
            team_id: Some("fe".into()),
        }];
        let built = entries(&settings, &loaded(), &views);
        assert_eq!(
            outline(&built),
            [
                "Inbox, 73 [73]",
                "My issues",
                "Reviews",
                "## Workspace",
                "Projects",
                "Views",
                "## Favorites",
                "Meus issues view",
                "All issues Back-end [Back-end]",
                "BE-293 [Loupe test] [BE-293]",
                "## Your teams",
                "Back-end",
                "  Back-end Issues",
                "  Back-end Projects",
                "  Back-end Views",
            ]
        );
        // A starred view of a team the user is not in still opens on that team.
        let Entry::Row(view_row) = &built[7] else {
            panic!("a row")
        };
        assert_eq!(
            view_row.target,
            Target::Route(Route::LinearTeam {
                team_id: "fe".into(),
                name: "Front-end".into(),
                view_id: Some("v-fe".into()),
            })
        );
    }

    #[test]
    fn an_unread_count_at_the_fetch_limit_reads_as_more() {
        let loaded = Loaded {
            unread: Some(Arc::new(unread(
                loupe_github::NOTIFICATIONS_LIMIT,
                "acme/api",
            ))),
            ..loaded()
        };
        let lines = outline(&entries(&Settings::default(), &loaded, &[]));
        assert_eq!(
            lines[0],
            format!(
                "Inbox, {limit}+ [{limit}+]",
                limit = loupe_github::NOTIFICATIONS_LIMIT
            )
        );
    }

    #[test]
    fn the_inbox_count_follows_the_inbox_filter() {
        let mut list = unread(3, "IsraelAraujo70/loupe");
        list.extend(unread(126, "hooli-inc/hooli-api"));
        let loaded = Loaded {
            unread: Some(Arc::new(list)),
            ..loaded()
        };
        let settings = Settings {
            notification_filter: vec!["org:IsraelAraujo70".into()],
            ..Settings::default()
        };
        assert_eq!(
            outline(&entries(&settings, &loaded, &[]))[0],
            "Inbox, 3 [3]"
        );
        assert_eq!(
            outline(&entries(&Settings::default(), &loaded, &[]))[0],
            "Inbox, 129 [129]"
        );
    }

    #[test]
    fn folded_sections_and_teams_hide_their_rows() {
        let mut settings = Settings::default();
        settings.toggle_collapsed("workspace");
        settings.toggle_collapsed("team:be");
        let lines = outline(&entries(&settings, &loaded(), &[]));
        assert_eq!(
            lines,
            [
                "Inbox, 73 [73]",
                "My issues",
                "Reviews",
                "## Workspace (folded)",
                "## Your teams",
                "Back-end",
            ]
        );
    }

    #[test]
    fn a_team_row_matches_its_page_whatever_name_the_route_carries() {
        let team = |name: &str, view: Option<&str>| Route::LinearTeam {
            team_id: "be".into(),
            name: name.into(),
            view_id: view.map(Into::into),
        };
        assert!(same_page(&team("Back-end", None), &team("Backend", None)));
        assert!(!same_page(
            &team("Back-end", None),
            &team("Back-end", Some("v1"))
        ));
    }
}
