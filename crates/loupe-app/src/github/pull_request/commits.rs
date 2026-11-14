//! The commits tab: a stateless list rendered from the loaded detail.

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{Commit, PullRequestDetail};

use crate::github::pr_row::hint;
use crate::github::widgets;
use crate::theme::scale;
use crate::theme::scale::dp;
use crate::ui;

const SHA_LENGTH: usize = 7;

fn short_sha(oid: &str) -> String {
    oid.chars().take(SHA_LENGTH).collect()
}

fn author_name(commit: &Commit) -> &str {
    commit
        .author
        .as_ref()
        .map_or(commit.author_name.as_str(), |actor| actor.login.as_str())
}

/// One 36 px row per commit: glyph, headline, author, age, checks, SHA.
pub fn render(detail: &PullRequestDetail, cx: &App) -> AnyElement {
    if detail.commits.is_empty() {
        return ui::empty_state(
            IconName::GitCommitHorizontal,
            "No commits",
            "This pull request has no commits.",
        )
        .into_any_element();
    }
    let theme = cx.theme();
    let (muted, hover) = (theme.muted_foreground, theme.list_hover);
    let mono = theme.mono_font_family.clone();
    let now = Utc::now();
    let repo_url = detail.pr.repo.url();
    v_flex()
        .w_full()
        .py_2()
        .children(detail.commits.iter().enumerate().map(|(ix, commit)| {
            let url = format!("{repo_url}/commit/{}", commit.oid);
            let author = author_name(commit).to_owned();
            let avatar = commit
                .author
                .as_ref()
                .map_or_else(String::new, |actor| actor.avatar_url.clone());
            h_flex()
                .id(("commit", ix))
                .role(Role::Link)
                .aria_label(format!(
                    "Commit {}: {}",
                    short_sha(&commit.oid),
                    commit.headline
                ))
                .h(scale::row_height())
                .px(scale::content_inset())
                .gap_3()
                .items_center()
                .hover(move |row| row.bg(hover))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Open commit in browser")
                        .build(window, cx)
                })
                .on_click(move |_, _, cx| cx.open_url(&url))
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
                        .text_size(scale::body())
                        .font_weight(FontWeight::MEDIUM)
                        .child(commit.headline.clone()),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1p5()
                        .items_center()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(ui::avatar_named(avatar, author.clone(), dp(20.)))
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
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use loupe_github::{Actor, Commit};

    use super::{author_name, short_sha};

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
    fn sha_is_cut_to_seven_characters() {
        assert_eq!(short_sha("0123456789abcdef"), "0123456");
        assert_eq!(short_sha("abc"), "abc");
    }

    #[test]
    fn author_prefers_the_github_login_over_the_git_name() {
        assert_eq!(author_name(&commit(Some("ana"))), "ana");
        assert_eq!(author_name(&commit(None)), "Ana Souza");
    }
}
