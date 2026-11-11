//! The segmented control, after Linear's and macOS's: a 30 px track on the
//! raised surface, a 3 px inset, 10 px of side padding in each option, and
//! the selected option a 24 px thumb that slides
//! to the new option. Every size is a whole number of pixels at 100% zoom, so a label
//! sits the same distance from both ends of its thumb. Build it with
//! [`segmented`].

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::scale::dp;
use crate::theme::{scale, tokens};

const SLIDE: Duration = Duration::from_millis(180);
/// Gap between the track's edge and the thumb, border included.
const INSET: f32 = 3.;

type OnSelect = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A segmented control. Each option is an accessible radio button inside a
/// named radio group. Clicking an option focuses the control; then the arrow
/// keys move the selection (wrapping) and Home and End jump to the ends.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    name: SharedString,
    options: Vec<(Option<IconName>, SharedString)>,
    selected: usize,
    on_select: OnSelect,
}

/// Starts a [`SegmentedControl`]. `name` is what a screen reader announces
/// for the group; `on_select` receives the index of the chosen option and is
/// not called for the option already selected.
pub fn segmented(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    options: impl IntoIterator<Item = (Option<IconName>, &'static str)>,
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> SegmentedControl {
    SegmentedControl {
        id: id.into(),
        name: name.into(),
        options: options
            .into_iter()
            .map(|(icon, label)| (icon, label.into()))
            .collect(),
        selected,
        on_select: Rc::new(on_select),
    }
}

/// What the control remembers between frames: its focus, where each option
/// was painted, and the slide in flight.
struct State {
    focus: FocusHandle,
    track: Option<Bounds<Pixels>>,
    options: Vec<Option<Bounds<Pixels>>>,
    shown: usize,
    slide: Option<Slide>,
    slides: usize,
}

/// A thumb moving between two options, in track-relative coordinates.
#[derive(Clone, Copy)]
struct Slide {
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    started: Instant,
    generation: usize,
}

impl State {
    /// Starts a slide when the selection changed since the last frame and
    /// both options have been painted; returns the slide still in flight.
    fn sync(&mut self, selected: usize, count: usize) -> Option<Slide> {
        self.options.resize(count, None);
        if self.shown != selected {
            let relative = |bounds: Bounds<Pixels>, track: Bounds<Pixels>| Bounds {
                origin: bounds.origin - track.origin,
                size: bounds.size,
            };
            let from = self.options.get(self.shown).copied().flatten();
            let to = self.options.get(selected).copied().flatten();
            if let (Some(track), Some(from), Some(to)) = (self.track, from, to) {
                self.slides += 1;
                self.slide = Some(Slide {
                    from: relative(from, track),
                    to: relative(to, track),
                    started: Instant::now(),
                    generation: self.slides,
                });
            }
            self.shown = selected;
        }
        self.slide.filter(|slide| slide.started.elapsed() < SLIDE)
    }
}

/// The option a key moves the selection to, or `None` for any other key.
fn step(key: &str, selected: usize, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match key {
        "left" | "up" => Some((selected + count - 1) % count),
        "right" | "down" => Some((selected + 1) % count),
        "home" => Some(0),
        "end" => Some(count - 1),
        _ => None,
    }
}

/// The look of the selected option: a fill one clear step above the track
/// and a soft shadow. The control's one outline is the track's.
fn thumb<E: Styled>(el: E, radius: Pixels) -> E {
    el.bg(tokens::segmented_thumb())
        .rounded(radius)
        .shadow(vec![BoxShadow {
            color: tokens::segmented_thumb_shadow(),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
            inset: false,
        }])
}

fn lerp(from: Pixels, to: Pixels, t: f32) -> Pixels {
    from + (to - from) * t
}

impl RenderOnce for SegmentedControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let count = self.options.len();
        let selected = self.selected.min(count.saturating_sub(1));
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| State {
            focus: cx.focus_handle(),
            track: None,
            options: Vec::new(),
            shown: selected,
            slide: None,
            slides: 0,
        });
        let (focus, slide) = state.update(cx, |state, _| {
            (state.focus.clone(), state.sync(selected, count))
        });

        let theme = cx.theme();
        let (muted, foreground, ring) = (theme.muted_foreground, theme.foreground, theme.ring);
        let (track, track_border) = (tokens::segmented_track(), theme.border);
        let radius = theme.radius;
        // Concentric with the track: its radius less the gap between them.
        let thumb_radius = radius - dp(INSET);
        let on_select = self.on_select;
        let options = self
            .options
            .into_iter()
            .enumerate()
            .map(|(ix, (icon, label))| {
                let is_selected = ix == selected;
                let on_select = on_select.clone();
                let focus = focus.clone();
                let measure = state.clone();
                h_flex()
                    .id(ix)
                    .relative()
                    .role(Role::RadioButton)
                    .aria_label(label.clone())
                    .aria_toggled(if is_selected {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .aria_selected(is_selected)
                    .aria_position_in_set(ix + 1)
                    .aria_size_of_set(count)
                    .h_full()
                    .px_2p5()
                    .gap_1()
                    .items_center()
                    .justify_center()
                    .rounded(thumb_radius)
                    .text_color(if is_selected { foreground } else { muted })
                    .when(is_selected && slide.is_none(), |el| thumb(el, thumb_radius))
                    .when(!is_selected, |el| {
                        el.hover(|el| el.bg(tokens::glass_hover()).text_color(foreground))
                    })
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                measure.update(cx, |state, _| {
                                    if let Some(slot) = state.options.get_mut(ix) {
                                        *slot = Some(bounds);
                                    }
                                })
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .children(icon.map(|icon| Icon::new(icon).with_size(dp(14.))))
                    .child(label)
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        window.focus(&focus, cx);
                        if !is_selected {
                            on_select(ix, window, cx);
                        }
                    })
            });
        let options: Vec<_> = options.collect();

        let measure = state.clone();
        h_flex()
            .id(self.id)
            .relative()
            .role(Role::RadioGroup)
            .aria_label(self.name)
            .track_focus(&focus)
            .flex_none()
            .h(rems(1.875))
            .p(dp(INSET - 1.))
            .items_center()
            .rounded(radius)
            .border_1()
            .border_color(track_border)
            .bg(track)
            .text_size(scale::body())
            .font_weight(FontWeight::MEDIUM)
            .focus_visible(move |style| style.border_color(ring))
            .on_key_down(move |event, window, cx| {
                if let Some(next) = step(&event.keystroke.key, selected, count) {
                    cx.stop_propagation();
                    if next != selected {
                        on_select(next, window, cx);
                    }
                }
            })
            .child(
                canvas(
                    move |bounds, _, cx| measure.update(cx, |state, _| state.track = Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .when_some(slide, |track, slide| {
                track.child(
                    thumb(div().absolute(), thumb_radius)
                        .top(slide.to.origin.y)
                        .h(slide.to.size.height)
                        .with_animation(
                            ("segmented-slide", slide.generation),
                            Animation::new(SLIDE).with_easing(ease_out_quint()),
                            move |thumb, t| {
                                thumb
                                    .left(lerp(slide.from.origin.x, slide.to.origin.x, t))
                                    .w(lerp(slide.from.size.width, slide.to.size.width, t))
                            },
                        ),
                )
            })
            .children(options)
    }
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn arrows_wrap_and_home_end_jump() {
        assert_eq!(step("right", 0, 3), Some(1));
        assert_eq!(step("right", 2, 3), Some(0));
        assert_eq!(step("left", 0, 3), Some(2));
        assert_eq!(step("up", 1, 3), Some(0));
        assert_eq!(step("down", 1, 3), Some(2));
        assert_eq!(step("home", 2, 3), Some(0));
        assert_eq!(step("end", 0, 3), Some(2));
        assert_eq!(step("a", 0, 3), None);
        assert_eq!(step("right", 0, 0), None);
    }
}
