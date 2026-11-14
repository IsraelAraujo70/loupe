//! Attachments: the pull requests that Linear's GitHub integration linked,
//! and every other attachment as a plain link.

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_github::PrRef;
use loupe_linear::{Attachment, PullRequestLink};

use crate::linear::widgets;
use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::theme::scale::dp;

fn section(title: &'static str, rows: Vec<Stateful<Div>>, cx: &App) -> AnyElement {
    v_flex()
        .gap_2()
        .child(crate::ui::section_label(title, cx))
        .child(v_flex().gap_1().children(rows))
        .into_any_element()
}

fn row(attachment: &Attachment, cx: &App) -> Stateful<Div> {
    let hover = cx.theme().list_hover;
    h_flex()
        .id(SharedString::from(format!("attachment-{}", attachment.id)))
        .role(Role::Link)
        .aria_label(attachment.title.clone())
        .w_full()
        .h(dp(widgets::ROW_HEIGHT))
        .gap_2()
        .items_center()
        .px_2()
        .rounded(cx.theme().radius)
        .hover(move |style| style.bg(hover))
}

/// The "Pull requests" section, or `None` when no attachment is a pull
/// request. It sits right under the description: which pull request closes
/// the issue, its title, and where it stands, on one line. Clicking one
/// opens it in Loupe.
pub(super) fn pull_requests(attachments: &[Attachment], cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let (border, surface) = (theme.border, crate::theme::tokens::surface());
    let rows: Vec<Stateful<Div>> = attachments
        .iter()
        .filter_map(|attachment| {
            let link: &PullRequestLink = attachment.pull_request.as_ref()?;
            let url = attachment.url.clone();
            // The repository and number; the owner is in the row's
            // accessible name through the title.
            let slug = format!("{}#{}", link.repo, link.number);
            Some(
                row(attachment, cx)
                    .h(dp(36.))
                    .px_3()
                    .gap_3()
                    .border_1()
                    .border_color(border)
                    .bg(surface)
                    .child(
                        Icon::new(IconName::Github)
                            .size(dp(16.))
                            .text_color(widgets::pull_request_color(link.state, cx)),
                    )
                    .child(widgets::identifier(slug, cx).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::theme::scale::body())
                            .child(attachment.title.clone()),
                    )
                    .child(widgets::pull_request_tag(link.state, cx))
                    .on_open(move |event, window, cx| match PrRef::parse(&url) {
                        Some(pr) => crate::shell::open(Route::PullRequest(pr), event, window, cx),
                        None => cx.open_url(&url),
                    }),
            )
        })
        .collect();
    (!rows.is_empty()).then(|| section("Pull requests", rows, cx))
}

/// The linked pull requests as header buttons, two at most: the mark in the
/// state's color, then `repo#number`. A click opens the pull request in
/// Loupe. With more than two, the rest are under the description.
pub(super) fn pull_request_chips(attachments: &[Attachment], cx: &App) -> Vec<AnyElement> {
    attachments
        .iter()
        .filter_map(|attachment| {
            let link = attachment.pull_request.as_ref()?;
            let url = attachment.url.clone();
            let slug = format!("{}#{}", link.repo, link.number);
            let state = widgets::pull_request_label(link.state);
            let open =
                move |event: &ClickEvent, window: &mut Window, cx: &mut App| match PrRef::parse(
                    &url,
                ) {
                    Some(pr) => crate::shell::open(Route::PullRequest(pr), event, window, cx),
                    None => cx.open_url(&url),
                };
            Some(
                Button::new(SharedString::from(format!("header-pr-{}", attachment.id)))
                    .outline()
                    .small()
                    .accessibility_label(format!("Open pull request {slug}, {state}"))
                    .tooltip(format!("{} · {state}", attachment.title))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .text_size(crate::theme::scale::meta())
                            .child(
                                Icon::new(IconName::Github)
                                    .size(dp(14.))
                                    .text_color(widgets::pull_request_color(link.state, cx)),
                            )
                            .child(slug),
                    )
                    .on_click(open.clone())
                    .on_middle_click(open)
                    .into_any_element(),
            )
        })
        .take(2)
        .collect()
}

/// The "Links" section for attachments that are not pull requests, or `None`
/// when there are none. Clicking one opens it through `shell::open_link`:
/// a pull request or a Linear issue in Loupe, anything else in the browser.
pub(super) fn other_attachments(attachments: &[Attachment], cx: &App) -> Option<AnyElement> {
    let muted = cx.theme().muted_foreground;
    let rows: Vec<Stateful<Div>> = attachments
        .iter()
        .filter(|attachment| attachment.pull_request.is_none())
        .map(|attachment| {
            let url = SharedString::from(attachment.url.clone());
            let title = if attachment.title.is_empty() {
                attachment.url.clone()
            } else {
                attachment.title.clone()
            };
            row(attachment, cx)
                .child(
                    Icon::new(IconName::ExternalLink)
                        .size(dp(14.))
                        .text_color(muted),
                )
                .child(div().min_w_0().truncate().text_sm().child(title))
                .children(attachment.subtitle.as_ref().map(|subtitle| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted)
                        .child(subtitle.clone())
                }))
                .on_open(move |event, window, cx| crate::shell::open_link(&url, event, window, cx))
        })
        .collect();
    (!rows.is_empty()).then(|| section("Links", rows, cx))
}
