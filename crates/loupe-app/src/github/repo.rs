//! `Route::Repo`: one repository's pull requests, watch toggle, add repository.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{Page, PrListFilter, PrSummary, RepoRef, RepoSummary};

use super::common::{self, LoadError, Loadable};
use super::pr_row::{AgeFrom, pr_row};
use crate::route::Route;
use crate::shell::navigate;
use crate::store::{Settings, update_settings};
use crate::theme::scale;
use crate::theme::scale::dp;
use crate::ui;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const FILTERS: [(PrListFilter, &str); 4] = [
    (PrListFilter::Open, "Open"),
    (PrListFilter::Closed, "Closed"),
    (PrListFilter::Merged, "Merged"),
    (PrListFilter::All, "All"),
];

fn filter_icon(filter: PrListFilter) -> IconName {
    match filter {
        PrListFilter::Open => IconName::GitPullRequest,
        PrListFilter::Closed => IconName::GitPullRequestClosed,
        PrListFilter::Merged => IconName::GitMerge,
        PrListFilter::All => IconName::Layers,
    }
}

fn search_text(raw: &str) -> Option<String> {
    let text = raw.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub struct RepoView {
    repo: RepoRef,
    filter: PrListFilter,
    search: Entity<InputState>,
    list: Loadable<Page<PrSummary>>,
    more: Option<Result<(), LoadError>>,
    scroll: ScrollHandle,
    _load: Option<Task<()>>,
    _more: Option<Task<()>>,
    _debounce: Option<Task<()>>,
    _search_changes: Subscription,
}

pub fn view(repo: RepoRef, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search pull requests"));
        let changes = cx.subscribe(&search, |view: &mut RepoView, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.debounce_reload(cx);
            }
        });
        let mut view = RepoView {
            repo,
            filter: PrListFilter::Open,
            search,
            list: Loadable::Loading,
            more: None,
            scroll: ScrollHandle::new(),
            _load: None,
            _more: None,
            _debounce: None,
            _search_changes: changes,
        };
        view.reload(cx);
        view
    })
    .into()
}

impl RepoView {
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.list.start_refresh();
        self.more = None;
        self._more = None;
        let repo = self.repo.clone();
        let filter = self.filter;
        let text = search_text(&self.search.read(cx).value());
        self._load = Some(common::run(
            cx,
            move |gh| gh.repo_pull_requests(&repo, filter, text.as_deref(), None),
            |view, result, _| view.list.finish(result),
        ));
        cx.notify();
    }

    fn debounce_reload(&mut self, cx: &mut Context<Self>) {
        self._debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |view, cx| view.reload(cx)).ok();
        }));
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.list.value().and_then(|page| page.next_cursor.clone()) else {
            return;
        };
        self.more = Some(Ok(()));
        let repo = self.repo.clone();
        let filter = self.filter;
        let text = search_text(&self.search.read(cx).value());
        self._more = Some(common::run(
            cx,
            move |gh| gh.repo_pull_requests(&repo, filter, text.as_deref(), Some(&cursor)),
            |view, result, _| match result {
                Ok(next) => {
                    view.more = None;
                    if let Loadable::Loaded(page) = &mut view.list {
                        page.items.extend(next.items);
                        page.next_cursor = next.next_cursor;
                        page.total_count = next.total_count;
                    }
                }
                Err(error) => view.more = Some(Err(error)),
            },
        ));
        cx.notify();
    }

    fn toggle_watch(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        update_settings(cx, move |settings| {
            if let Some(index) = settings.watched_repos.iter().position(|r| r == &repo) {
                settings.watched_repos.remove(index);
            } else {
                settings.watched_repos.push(repo);
            }
        });
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let watching = Settings::get(cx).watched_repos.contains(&self.repo);
        let url = self.repo.url();
        let busy = self.list.is_busy();
        let header = ui::page_header(self.repo.to_string()).icon(IconName::FolderGit2);
        let header = match self.list.value() {
            Some(page) => header.count(page.total_count as usize),
            None => header,
        };
        let actions = Route::Actions {
            repo: self.repo.clone(),
            branch: None,
        };
        header
            .action(
                Button::new("repo-actions")
                    .ghost()
                    .small()
                    .icon(IconName::Workflow)
                    .label("Actions")
                    .tooltip("Workflow runs of this repository")
                    .on_click(move |event, window, cx| {
                        crate::shell::open(actions.clone(), event, window, cx)
                    }),
            )
            .action(
                Button::new("watch")
                    .ghost()
                    .small()
                    .icon(if watching {
                        IconName::EyeOff
                    } else {
                        IconName::Eye
                    })
                    .label(if watching { "Unwatch" } else { "Watch" })
                    .tooltip(if watching {
                        "Remove from the sidebar"
                    } else {
                        "Show in the sidebar"
                    })
                    .on_click(cx.listener(|view, _, _, cx| view.toggle_watch(cx))),
            )
            .action(
                ui::icon_button("add-repo", IconName::Plus, "Add repository…")
                    .on_click(|_, window, cx| open_add_repository(window, cx)),
            )
            .action(
                ui::icon_button("browser", IconName::ExternalLink, "Open in browser")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .action(
                ui::icon_button("refresh", IconName::RefreshCw, "Refresh")
                    .loading(busy)
                    .disabled(busy)
                    .on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
            )
    }

    fn filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = FILTERS
            .iter()
            .position(|(filter, _)| *filter == self.filter)
            .unwrap_or_default();
        let this = cx.entity().downgrade();
        ui::toolbar()
            .child(ui::segmented(
                "repo-filter",
                "Pull request state",
                FILTERS.map(|(filter, label)| (Some(filter_icon(filter)), label)),
                selected,
                move |index, _, cx| {
                    let Some((filter, _)) = FILTERS.get(index).copied() else {
                        return;
                    };
                    this.update(cx, |view, cx| {
                        if view.filter != filter {
                            view.filter = filter;
                            view.reload(cx);
                        }
                    })
                    .ok();
                },
            ))
            .child(div().flex_1())
            .child(
                div().w_64().child(
                    Input::new(&self.search)
                        .small()
                        .prefix(Icon::new(IconName::Search).xsmall())
                        .cleanable(true),
                ),
            )
    }
}

impl Render for RepoView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.list {
            Loadable::Failed(error) => {
                common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .into_any_element()
            }
            Loadable::Loading => div().p_2().child(ui::skeleton_rows(10)).into_any_element(),
            Loadable::Loaded(page) | Loadable::Refreshing(page) if page.items.is_empty() => {
                ui::empty_state(
                    IconName::GitPullRequest,
                    "No pull requests",
                    "Nothing matches this filter.",
                )
                .into_any_element()
            }
            Loadable::Loaded(page) | Loadable::Refreshing(page) => {
                let page = page.clone();
                let loading_more = matches!(self.more, Some(Ok(())));
                let age = AgeFrom::Updated;
                v_flex()
                    .w_full()
                    .p_2()
                    .children(
                        page.items
                            .iter()
                            .map(|summary| pr_row(summary, false, true, false, age, cx)),
                    )
                    .when(page.next_cursor.is_some(), |column| {
                        column.child(
                            h_flex().py_3().justify_center().child(
                                Button::new("more")
                                    .outline()
                                    .small()
                                    .icon(IconName::ChevronDown)
                                    .label(format!(
                                        "Load more ({} of {})",
                                        page.items.len(),
                                        page.total_count
                                    ))
                                    .loading(loading_more)
                                    .disabled(loading_more)
                                    .on_click(cx.listener(|view, _, _, cx| view.load_more(cx))),
                            ),
                        )
                    })
                    .children(match &self.more {
                        Some(Err(error)) => Some(common::load_error(
                            error,
                            cx.listener(|view, _, _, cx| view.load_more(cx)),
                        )),
                        _ => None,
                    })
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .child(self.header(cx))
            .child(self.filters(cx))
            .child(
                div()
                    .id("repo")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
    }
}

/// Opens the "add repository" dialog: searches the viewer's repositories and
/// adds the chosen one to `Settings::watched_repos`.
pub fn open_add_repository(window: &mut Window, cx: &mut App) {
    let picker = cx.new(|cx| AddRepository::new(window, cx));
    let focus = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            picker.update(cx, |picker, cx| {
                picker.query.update(cx, |query, cx| query.focus(window, cx))
            });
        }
        dialog
            .title("Add repository")
            .w(dp(560.))
            .child(picker.clone())
    });
}

/// Repositories to offer for `query`: the viewer's repositories whose name
/// contains it, then search results not already listed, at most 30.
fn candidates(viewer: &[RepoSummary], searched: &[RepoSummary], query: &str) -> Vec<RepoSummary> {
    let needle = query.trim().to_lowercase();
    let mut out: Vec<RepoSummary> = viewer
        .iter()
        .filter(|repo| needle.is_empty() || repo.repo.to_string().to_lowercase().contains(&needle))
        .cloned()
        .collect();
    for repo in searched {
        if !out.iter().any(|known| known.repo == repo.repo) {
            out.push(repo.clone());
        }
    }
    out.truncate(30);
    out
}

struct AddRepository {
    query: Entity<InputState>,
    viewer: Loadable<Vec<RepoSummary>>,
    searched: Vec<RepoSummary>,
    _viewer: Option<Task<()>>,
    _search: Option<Task<()>>,
    _changes: Subscription,
}

impl AddRepository {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter your repositories or search owner/name")
        });
        let changes = cx.subscribe(&query, |picker: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                picker.search(cx);
                cx.notify();
            }
        });
        let viewer = common::run(
            cx,
            |gh| gh.viewer_repositories(),
            |picker: &mut Self, result, _| picker.viewer.finish(result),
        );
        Self {
            query,
            viewer: Loadable::Loading,
            searched: Vec::new(),
            _viewer: Some(viewer),
            _search: None,
            _changes: changes,
        }
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let text = self.query.read(cx).value().trim().to_owned();
        if text.chars().count() < 3 {
            self.searched.clear();
            self._search = None;
            return;
        }
        let Some(github) = common::client(cx) else {
            return;
        };
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let query = format!("{text} in:name");
            let found = cx
                .background_executor()
                .spawn(async move { github.search_repositories(&query) })
                .await;
            this.update(cx, |picker, cx| {
                picker.searched = found.unwrap_or_default();
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for AddRepository {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let watched = Settings::get(cx).watched_repos.clone();
        let query = self.query.read(cx).value().to_string();
        let list = match &self.viewer {
            Loadable::Failed(error) => ui::error_banner(error.message(), |_, window, cx| {
                window.close_dialog(cx);
                open_add_repository(window, cx);
            })
            .into_any_element(),
            Loadable::Loading => ui::skeleton_rows(5).into_any_element(),
            Loadable::Loaded(viewer) | Loadable::Refreshing(viewer) => {
                let items = candidates(viewer, &self.searched, &query);
                if items.is_empty() {
                    div()
                        .p_3()
                        .child(ui::empty_line(
                            IconName::FolderSearch,
                            "No repositories match",
                            cx,
                        ))
                        .into_any_element()
                } else {
                    v_flex()
                        .children(items.into_iter().map(|summary| {
                            let is_watched = watched.contains(&summary.repo);
                            let repo = summary.repo.clone();
                            h_flex()
                                .id(SharedString::from(format!("pick-{}", summary.repo)))
                                .role(Role::Button)
                                .aria_label(format!("Watch {}", summary.repo))
                                .px_2()
                                .py_1p5()
                                .gap_3()
                                .items_center()
                                .rounded(cx.theme().radius)
                                .hover(|row| row.bg(cx.theme().list_hover))
                                .on_click(move |_, window, cx| {
                                    let added = repo.clone();
                                    update_settings(cx, move |settings| {
                                        if !settings.watched_repos.contains(&added) {
                                            settings.watched_repos.push(added);
                                        }
                                    });
                                    window.close_dialog(cx);
                                    navigate(Route::Repo(repo.clone()), window, cx);
                                })
                                .child(
                                    Icon::new(IconName::FolderGit2)
                                        .small()
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(scale::body())
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(summary.repo.to_string()),
                                        )
                                        .children(summary.description.clone().map(|text| {
                                            div()
                                                .text_xs()
                                                .truncate()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(text)
                                        })),
                                )
                                .when(summary.is_private, |row| {
                                    row.child(
                                        Icon::new(IconName::Lock)
                                            .xsmall()
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                })
                                .when(is_watched, |row| {
                                    row.child(crate::ui::chip().small().child("Watching"))
                                })
                        }))
                        .into_any_element()
                }
            }
        };
        v_flex()
            .gap_3()
            .child(Input::new(&self.query).prefix(Icon::new(IconName::Search).small()))
            .child(
                div()
                    .id("repo-candidates")
                    .max_h(dp(380.))
                    .overflow_y_scroll()
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use loupe_github::{RepoRef, RepoSummary};

    use super::{candidates, search_text};

    fn repo(slug: &str) -> RepoSummary {
        RepoSummary {
            repo: RepoRef::parse(slug).expect("slug"),
            description: None,
            is_private: false,
            pushed_at: None,
        }
    }

    #[test]
    fn candidates_filter_viewer_repos_then_add_new_search_results() {
        let viewer = [repo("acme/web"), repo("acme/api"), repo("me/dotfiles")];
        let searched = [repo("acme/web"), repo("other/webkit")];
        let names: Vec<String> = candidates(&viewer, &searched, " WEB ")
            .into_iter()
            .map(|r| r.repo.to_string())
            .collect();
        assert_eq!(names, ["acme/web", "other/webkit"]);
        assert_eq!(candidates(&viewer, &[], "").len(), 3);
    }

    #[test]
    fn blank_search_is_none() {
        assert_eq!(search_text("  "), None);
        assert_eq!(search_text(" fix "), Some("fix".into()));
    }
}
