//! A file's header in the diff: the path, shortened as VS Code does to fit,
//! with the size and Viewed pinned to the trailing edge of the part of the
//! header on screen.
//!
//! The diff component draws its headers inside the rows that scroll
//! sideways, so while long lines scroll a header is as wide as the longest
//! line and its far end is out of sight. The component keeps the sideways
//! offset to itself; the header reads what is on screen from the clip it is
//! painted in instead, frame by frame.

use std::rc::Rc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::github::widgets;

/// What stands for the folders a shortened path leaves out.
const ELLIPSIS: &str = "…";

/// `path` shortened to at most `max` wide, as `width` measures text.
///
/// The file name is kept whole while it fits: the folders go first, from
/// the middle out. The first folder (the area of the repository) and the
/// ones nearest the file stay longest, so `apps/web/src/billing/service/a.ts`
/// becomes `apps/…/service/a.ts`, then `…/service/a.ts`, `…/a.ts`, and `a.ts`.
/// A name that does not fit alone loses the end of its stem and keeps its
/// extension (`billing.serv….ts`), as the files tree does. The result is
/// never wider than `max`; it is empty when not even `…` fits.
pub fn abbreviate_path(path: &str, max: f32, width: impl Fn(&str) -> f32) -> String {
    if width(path) <= max {
        return path.to_owned();
    }
    let (folders, name) = match path.rsplit_once('/') {
        Some((folders, name)) => (folders.split('/').collect::<Vec<_>>(), name),
        None => (Vec::new(), path),
    };
    for kept in (0..folders.len()).rev() {
        let label = fold(&folders, kept, name);
        if width(&label) <= max {
            return label;
        }
    }
    if width(name) <= max {
        return name.to_owned();
    }
    cut_name(name, max, &width)
}

/// `name` under `kept` of `folders`, with `…` for the rest: the first
/// folder and the last `kept - 1` once two or more are kept, else the last.
fn fold(folders: &[&str], kept: usize, name: &str) -> String {
    let head = if kept >= 2 { 1 } else { 0 };
    let tail = kept - head;
    let mut parts = folders[..head].to_vec();
    parts.push(ELLIPSIS);
    parts.extend_from_slice(&folders[folders.len() - tail..]);
    parts.push(name);
    parts.join("/")
}

/// The longest start of `name` that fits with `…` and the extension after
/// it, or without the extension when even one character and it do not fit.
fn cut_name(name: &str, max: f32, width: &impl Fn(&str) -> f32) -> String {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    for (body, tail) in [(stem, extension), (name, "")] {
        let ends: Vec<usize> = body.char_indices().map(|(ix, _)| ix).skip(1).collect();
        let label = |count: usize| format!("{}{ELLIPSIS}{tail}", &body[..ends[count - 1]]);
        let fits = |count: usize| width(&label(count)) <= max;
        if ends.is_empty() || !fits(1) {
            continue;
        }
        // The widest count that fits: text grows with every character.
        let (mut low, mut high) = (1, ends.len());
        while low < high {
            let mid = (low + high).div_ceil(2);
            if fits(mid) {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        return label(low);
    }
    if width(ELLIPSIS) <= max {
        ELLIPSIS.to_owned()
    } else {
        String::new()
    }
}

/// What a click on Viewed does, with the new state.
pub type OnViewed = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Everything a file's header shows besides the component's collapse
/// control.
pub struct HeaderContent {
    /// The path as the header names it: `old → new` for a rename.
    pub title: SharedString,
    /// The path that is shortened when the title does not fit.
    pub path: SharedString,
    pub additions: u32,
    pub deletions: u32,
    /// "Added file", "Deleted file", "Renamed", or "Copied".
    pub status: Option<&'static str>,
    pub viewed: bool,
    /// Room kept clear at the trailing edge of the screen, where the panel
    /// fades while long lines scroll.
    pub fade: Pixels,
    pub on_viewed: OnViewed,
}

/// A file's header: the path at the leading edge of what is on screen and
/// the size, the status, and Viewed at its trailing edge, 12 px apart.
pub fn file_header(content: HeaderContent) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            // The clip of the rows that scroll sideways is the part of the
            // diff on screen.
            let mask = window.content_mask().bounds;
            // Scrolled past its start, the path keeps 8 px off the panel's edge.
            let left = bounds.left().max(mask.left() + window.rem_size() * 0.5);
            let right = bounds.right().min(mask.right());
            let fade = (content.fade - (mask.right() - right)).max(px(0.));
            let end = right - fade;

            let mut band = trailing_band(&content, cx);
            let band_size = band.layout_as_root(AvailableSpace::min_size(), window, cx);
            let gap = window.rem_size() * 0.75;
            let room = (end - left - band_size.width - gap).max(px(0.));

            let style = window.text_style();
            let font = Font {
                weight: FontWeight::MEDIUM,
                ..style.font()
            };
            let font_size = style.font_size.to_pixels(window.rem_size());
            let measure = |text: &str| -> f32 {
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(text.to_owned().into(), font_size, &[run], None)
                    .width
                    .into()
            };
            let room_f = f32::from(room);
            let label: SharedString = if measure(&content.title) <= room_f {
                content.title.clone()
            } else {
                abbreviate_path(&content.path, room_f, measure).into()
            };
            let mut title = path_label(label, content.title.clone());
            let title_size = title.layout_as_root(
                size(AvailableSpace::Definite(room), AvailableSpace::MinContent),
                window,
                cx,
            );
            let middle = |height: Pixels| bounds.top() + (bounds.size.height - height) / 2.;
            title.prepaint_at(point(left, middle(title_size.height)), window, cx);
            band.prepaint_at(
                point(end - band_size.width, middle(band_size.height)),
                window,
                cx,
            );
            (title, band)
        },
        |_, (mut title, mut band), window, cx| {
            title.paint(window, cx);
            band.paint(window, cx);
        },
    )
    .flex_1()
    .min_w_0()
    // The height of the component's own header content (the Viewed
    // checkbox), so the rows under it keep their place.
    .h(rems(1.375))
}

/// The shortened path, with the whole one in its tooltip and Copy path in
/// its menu.
fn path_label(label: SharedString, whole: SharedString) -> AnyElement {
    let tooltip = whole.clone();
    div()
        .id("header-path")
        .debug_selector(|| "header-path".into())
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(label)
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .context_menu(move |menu, _, _| {
            let whole = whole.clone();
            menu.item(
                PopupMenuItem::new("Copy path").on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(whole.to_string()));
                    window.push_notification(Notification::success(format!("Copied {whole}")), cx);
                }),
            )
        })
        .into_any_element()
}

fn trailing_band(content: &HeaderContent, cx: &App) -> AnyElement {
    let on_viewed = content.on_viewed.clone();
    h_flex()
        .gap_3()
        .items_center()
        .child(widgets::diff_stat(content.additions, content.deletions))
        .when_some(content.status, |band, status| {
            band.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
        })
        .child(
            div().debug_selector(|| "header-viewed".into()).child(
                Checkbox::new(SharedString::from(format!(
                    "header-viewed-{}",
                    content.path
                )))
                .label("Viewed")
                .checked(content.viewed)
                .on_change(move |checked, window, cx| on_viewed(*checked, window, cx)),
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::abbreviate_path;

    /// One unit per character, so a width is a character count.
    fn chars(text: &str) -> f32 {
        text.chars().count() as f32
    }

    const PATH: &str = "apps/web-api/src/modules/billing/application/service/billing.service.payment-reminder.spec.ts";
    const NAME: &str = "billing.service.payment-reminder.spec.ts";

    #[test]
    fn a_path_that_fits_is_left_whole() {
        assert_eq!(abbreviate_path(PATH, chars(PATH), chars), PATH);
        assert_eq!(abbreviate_path("src/main.rs", 40., chars), "src/main.rs");
        assert_eq!(abbreviate_path("README.md", 9., chars), "README.md");
    }

    #[test]
    fn the_middle_folders_go_first_and_the_name_stays_whole() {
        assert_eq!(
            abbreviate_path(PATH, chars(PATH) - 1., chars),
            "apps/…/src/modules/billing/application/service/billing.service.payment-reminder.spec.ts"
        );
        assert_eq!(
            abbreviate_path(PATH, 60., chars),
            "apps/…/service/billing.service.payment-reminder.spec.ts"
        );
        assert_eq!(
            abbreviate_path(PATH, 50., chars),
            "…/service/billing.service.payment-reminder.spec.ts"
        );
        assert_eq!(
            abbreviate_path(PATH, 42., chars),
            "…/billing.service.payment-reminder.spec.ts"
        );
        assert_eq!(abbreviate_path(PATH, chars(NAME), chars), NAME);
    }

    #[test]
    fn a_name_that_does_not_fit_keeps_its_extension() {
        assert_eq!(abbreviate_path(PATH, 21., chars), "billing.service.p….ts");
        assert_eq!(abbreviate_path(PATH, 20., chars), "billing.service.….ts");
        assert_eq!(abbreviate_path(PATH, 5., chars), "b….ts");
        assert_eq!(abbreviate_path(PATH, 4., chars), "bil…");
        assert_eq!(abbreviate_path(PATH, 1., chars), "…");
        assert_eq!(abbreviate_path(PATH, 0., chars), "");
        assert_eq!(abbreviate_path("Makefile", 5., chars), "Make…");
    }

    /// For every width: never wider than it, the name whole whenever it
    /// fits, and a folder kept only while every folder between it and the
    /// file is kept too, except the first.
    #[test]
    fn every_width_keeps_the_rule() {
        let folders: Vec<&str> = PATH.rsplit_once('/').unwrap().0.split('/').collect();
        for max in 0..=chars(PATH) as usize + 2 {
            let max = max as f32;
            let label = abbreviate_path(PATH, max, chars);
            assert!(chars(&label) <= max, "{label:?} is wider than {max}");
            if chars(NAME) <= max {
                assert!(
                    label == PATH || label.ends_with(&format!("/{NAME}")) || label == NAME,
                    "the name is cut at {max}: {label:?}"
                );
            }
            if label == PATH || !label.contains('/') {
                continue;
            }
            let shown: Vec<&str> = label.rsplit_once('/').unwrap().0.split('/').collect();
            let (head, tail) = match shown.iter().position(|part| *part == "…") {
                Some(ix) => (&shown[..ix], &shown[ix + 1..]),
                None => panic!("a shortened path marks what it leaves out: {label:?}"),
            };
            assert!(head.len() <= 1, "only the first folder leads: {label:?}");
            assert!(
                head.is_empty() || head[0] == folders[0],
                "the leading folder is the first: {label:?}"
            );
            assert_eq!(
                tail,
                &folders[folders.len() - tail.len()..],
                "the folders kept are the ones nearest the file: {label:?}"
            );
        }
    }
}
