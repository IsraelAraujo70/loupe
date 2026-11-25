//! The commits tab, a stateless list rendered from the loaded detail, and
//! the bar over the Files tab while it shows one of them.

use std::rc::Rc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{Commit, PR_COMMITS_LIMIT, PullRequestDetail};

use crate::github::pr_row::hint;
use crate::github::widgets;
use crate::theme::scale;
use crate::ui::{self, ControlSize as _};

const SHA_LENGTH: usize = 7;

/// Shows one commit's changes in the Files tab, or with `None` the whole
/// pull request's.
pub type OnShow = Rc<dyn Fn(Option<&str>, &mut Window, &mut App)>;

/// Loads the commits after the first page again.
pub type OnRetry = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Where the commits after the ones in the detail stand.
pub enum Rest {
    /// Nothing is loading: the list is whole, or cut at the limit.
    Settled,
    Loading,
    /// They did not load, for this reason.
    Failed(SharedString),
}

/// What the Commits tab says over a list shorter than the pull request:
/// `None` for a whole list.
fn shortfall(shown: usize, total: usize, rest: &Rest) -> Option<String> {
    if shown >= total {
        return None;
    }
    match rest {
        Rest::Loading => None,
        Rest::Failed(reason) => Some(format!(
            "Showing {shown} of {total} commits. The rest did not load: {reason}"
        )),
        Rest::Settled if shown >= PR_COMMITS_LIMIT => Some(format!(
            "Showing the first {shown} of {total} commits. Loupe lists at most {PR_COMMITS_LIMIT}; the rest are on GitHub."
        )),
        Rest::Settled => Some(format!(
            "Showing {shown} of {total} commits. GitHub listed no more; the rest are on GitHub."
        )),
    }
}

/// Where commit `oid` stands in `commits`, and the ones before and after it.
fn neighbors<'a>(
    commits: &'a [Commit],
    oid: &str,
) -> Option<(usize, Option<&'a Commit>, Option<&'a Commit>)> {
    let ix = commits.iter().position(|commit| commit.oid == oid)?;
    let previous = ix.checked_sub(1).map(|ix| &commits[ix]);
    Some((ix, previous, commits.get(ix + 1)))
}

fn short_sha(oid: &str) -> String {
    oid.chars().take(SHA_LENGTH).collect()
}

fn author_name(commit: &Commit) -> &str {
    commit
        .author
        .as_ref()
        .map_or(commit.author_name.as_str(), |actor| actor.login.as_str())
}

/// One 36 px row per commit: glyph, headline, author, age, checks, SHA, and
/// Open on GitHub. A click on the row shows the commit's changes. Skeleton
/// rows follow while the rest of the commits load; a list that stays
/// shorter than the pull request says over it how far it goes and why.
pub fn render(
    detail: &PullRequestDetail,
    rest: Rest,
    on_show: OnShow,
    on_retry: OnRetry,
    cx: &App,
) -> AnyElement {
    if detail.commits.is_empty() && detail.commit_count() == 0 {
        return ui::empty_state(
            IconName::GitCommitHorizontal,
            "No commits",
            "This pull request has no commits.",
        )
        .into_any_element();
    }
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let mono = theme.mono_font_family.clone();
    let now = Utc::now();
    let repo_url = detail.pr.repo.url();
    let shown = detail.commits.len();
    let total = detail.commit_count();
    let loading = matches!(rest, Rest::Loading) && shown < total;
    let notice = shortfall(shown, total, &rest).map(|text| match rest {
        Rest::Failed(_) => {
            ui::load_error(text, move |event, window, cx| on_retry(event, window, cx))
                .into_any_element()
        }
        Rest::Settled | Rest::Loading => {
            let url = format!("{}/commits", detail.url);
            h_flex()
                .id("commits-cut")
                .role(Role::Note)
                .aria_label(SharedString::from(text.clone()))
                .w_full()
                .px(scale::content_inset())
                .py_2()
                .gap_2()
                .items_center()
                .text_size(scale::meta())
                .text_color(muted)
                .child(Icon::new(IconName::Info).small().text_color(muted))
                .child(div().flex_1().min_w_0().child(text))
                .child(
                    Button::new("commits-on-github")
                        .ghost()
                        .control()
                        .icon(IconName::ExternalLink)
                        .label("Open on GitHub")
                        .tooltip("See every commit in the browser")
                        .accessibility_label("Open the commits on GitHub")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
                .into_any_element()
        }
    });
    let list = ui::list_container()
        .children(detail.commits.iter().enumerate().map(|(ix, commit)| {
            let url = format!("{repo_url}/commit/{}", commit.oid);
            let oid = commit.oid.clone();
            let on_show = on_show.clone();
            let author = author_name(commit).to_owned();
            let avatar = commit
                .author
                .as_ref()
                .map_or_else(String::new, |actor| actor.avatar_url.clone());
            ui::list_row(("commit", ix), cx)
                .role(Role::Link)
                .aria_label(format!(
                    "Commit {}: {}",
                    short_sha(&commit.oid),
                    commit.headline
                ))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Show this commit's changes")
                        .build(window, cx)
                })
                .on_click(move |_, window, cx| on_show(Some(&oid), window, cx))
                .child(
                    Icon::new(IconName::GitCommitHorizontal)
                        .small()
                        .text_color(muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(commit.headline.clone()),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1p5()
                        .items_center()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(ui::avatar_named(avatar, author.clone(), scale::avatar_md()))
                        .child(author),
                )
                .child(h_flex().flex_none().w_4().justify_center().when_some(
                    commit.checks,
                    |lane, state| {
                        lane.child(hint(
                            SharedString::from(format!("commit-checks-{ix}")),
                            widgets::checks_label(state),
                            widgets::checks_icon(state),
                        ))
                    },
                ))
                .child(
                    div()
                        .flex_none()
                        .w_16()
                        .font_family(mono.clone())
                        .text_size(scale::code())
                        .text_color(muted)
                        .child(short_sha(&commit.oid)),
                )
                .child(
                    div()
                        .flex_none()
                        .w_8()
                        .text_right()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(ui::relative_time(commit.committed_at, now)),
                )
                .child(
                    ui::icon_button(
                        ("commit-on-github", ix),
                        IconName::ExternalLink,
                        "Open commit on GitHub",
                    )
                    .on_click(move |_, _, cx| {
                        // The row shows the commit in Loupe on a click.
                        cx.stop_propagation();
                        cx.open_url(&url);
                    }),
                )
        }))
        .when(loading, |list| {
            list.child(
                div()
                    .id("commits-loading")
                    .role(Role::Status)
                    .aria_label(format!("Loading {} more commits", total - shown))
                    .child(ui::skeleton_rows(3)),
            )
        });
    v_flex()
        .w_full()
        .children(notice)
        .child(list)
        .into_any_element()
}

/// The bar over the Files tab while it shows commit `oid`: back to all
/// changes, which commit it is, the commits before and after it, and Open
/// on GitHub.
pub fn commit_bar(
    detail: &PullRequestDetail,
    oid: &str,
    on_show: OnShow,
    cx: &App,
) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let place = neighbors(&detail.commits, oid);
    let commit = place.map(|(ix, ..)| &detail.commits[ix]);
    let step = |id: &'static str, icon: IconName, name: &'static str, to: Option<&Commit>| {
        let on_show = on_show.clone();
        let to = to.map(|commit| commit.oid.clone());
        ui::icon_button(id, icon, name)
            .disabled(to.is_none())
            .on_click(move |_, window, cx| {
                if let Some(oid) = &to {
                    on_show(Some(oid), window, cx);
                }
            })
    };
    let summary = match (commit, place) {
        (Some(commit), Some((ix, ..))) => format!(
            "Commit {}: {}, {} of {}",
            short_sha(oid),
            commit.headline,
            ix + 1,
            detail.commit_count()
        ),
        _ => format!("Commit {}", short_sha(oid)),
    };
    let url = format!("{}/commit/{oid}", detail.pr.repo.url());
    let all = on_show.clone();
    ui::toolbar()
        .child(
            Button::new("commit-all-changes")
                .ghost()
                .control()
                .icon(IconName::ArrowLeft)
                .label("All changes")
                .tooltip("Show every change of the pull request")
                .accessibility_label("Show all changes")
                .on_click(move |_, window, cx| all(None, window, cx)),
        )
        .child(ui::vertical_divider(cx))
        .child(
            h_flex()
                .id("commit-summary")
                .role(Role::Group)
                .aria_label(summary.clone())
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(IconName::GitCommitHorizontal)
                        .small()
                        .text_color(muted),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(scale::code())
                        .text_color(muted)
                        .child(short_sha(oid)),
                )
                .children(commit.map(|commit| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .child(commit.headline.clone())
                }))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(summary.clone()).build(window, cx)
                }),
        )
        .children(place.map(|(ix, ..)| {
            div()
                .flex_none()
                .text_size(scale::meta())
                .text_color(muted)
                .child(format!("{} of {}", ix + 1, detail.commit_count()))
        }))
        .child(step(
            "commit-previous",
            IconName::ChevronLeft,
            "Previous commit",
            place.and_then(|(_, previous, _)| previous),
        ))
        .child(step(
            "commit-next",
            IconName::ChevronRight,
            "Next commit",
            place.and_then(|(_, _, next)| next),
        ))
        .child(
            Button::new("commit-open-on-github")
                .ghost()
                .control()
                .icon(IconName::ExternalLink)
                .label("Open on GitHub")
                .tooltip("Open this commit in the browser")
                .accessibility_label("Open commit on GitHub")
                .on_click(move |_, _, cx| cx.open_url(&url)),
        )
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use loupe_github::{Actor, Commit};

    use super::{Rest, author_name, neighbors, short_sha, shortfall};

    fn commit(author: Option<&str>) -> Commit {
        Commit {
            oid: "0123456789abcdef".into(),
            headline: "Fix it".into(),
            body: String::new(),
            author_name: "Ana Souza".into(),
            author: author.map(|login| Actor {
                login: login.into(),
                avatar_url: String::new(),
            }),
            committed_at: DateTime::from_timestamp(0, 0).unwrap(),
            checks: None,
        }
    }

    #[test]
    fn a_short_list_says_how_far_it_goes_and_why() {
        assert_eq!(shortfall(769, 769, &Rest::Settled), None);
        assert_eq!(
            shortfall(100, 769, &Rest::Loading),
            None,
            "skeleton rows stand for the commits on their way"
        );
        assert_eq!(
            shortfall(100, 769, &Rest::Failed("HTTP 502".into())).as_deref(),
            Some("Showing 100 of 769 commits. The rest did not load: HTTP 502")
        );
        assert_eq!(
            shortfall(760, 769, &Rest::Settled).as_deref(),
            Some("Showing 760 of 769 commits. GitHub listed no more; the rest are on GitHub.")
        );
        assert_eq!(
            shortfall(3000, 5000, &Rest::Settled).as_deref(),
            Some(
                "Showing the first 3000 of 5000 commits. Loupe lists at most 3000; the rest are on GitHub."
            )
        );
    }

    #[test]
    fn sha_is_cut_to_seven_characters() {
        assert_eq!(short_sha("0123456789abcdef"), "0123456");
        assert_eq!(short_sha("abc"), "abc");
    }

    #[test]
    fn a_commit_knows_its_place_and_the_ones_beside_it() {
        let commits: Vec<Commit> = ["a1", "b2", "c3"]
            .into_iter()
            .map(|oid| Commit {
                oid: oid.into(),
                ..commit(None)
            })
            .collect();
        let oid = |commit: Option<&Commit>| commit.map(|commit| commit.oid.clone());

        let (ix, previous, next) = neighbors(&commits, "a1").expect("the first is listed");
        assert_eq!((ix, oid(previous), oid(next)), (0, None, Some("b2".into())));

        let (ix, previous, next) = neighbors(&commits, "b2").expect("the middle is listed");
        assert_eq!(
            (ix, oid(previous), oid(next)),
            (1, Some("a1".into()), Some("c3".into()))
        );

        let (ix, previous, next) = neighbors(&commits, "c3").expect("the last is listed");
        assert_eq!((ix, oid(previous), oid(next)), (2, Some("b2".into()), None));

        assert!(
            neighbors(&commits, "gone").is_none(),
            "a commit a force push dropped has no place"
        );
    }

    #[test]
    fn author_prefers_the_github_login_over_the_git_name() {
        assert_eq!(author_name(&commit(Some("ana"))), "ana");
        assert_eq!(author_name(&commit(None)), "Ana Souza");
    }
}
