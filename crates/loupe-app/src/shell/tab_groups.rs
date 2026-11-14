//! The dialog that names a group of tabs, for "Rename group…", and the one
//! that names a new group and picks its color, for "Add to new group…" and
//! Reviews' "Open in group".

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::store::TabGroupColor;
use crate::theme::scale::dp;
use crate::theme::tokens;

type OnName = Rc<dyn Fn(&str, TabGroupColor, &mut Window, &mut App)>;

struct NameForm {
    input: Entity<InputState>,
    on_name: OnName,
    confirm: SharedString,
    /// The color picked, for a new group; `None` asks only for the name.
    color: Option<TabGroupColor>,
    _subscription: Subscription,
}

impl NameForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        window.close_dialog(cx);
        (self.on_name)(&name, self.color.unwrap_or_default(), window, cx);
    }

    /// One 24 px swatch per color, the picked one with a check.
    fn swatches(&self, picked: TabGroupColor, cx: &mut Context<Self>) -> impl IntoElement {
        let ring = cx.theme().foreground;
        let count = TabGroupColor::ALL.len();
        h_flex()
            .id("group-colors")
            .gap_2()
            .role(Role::RadioGroup)
            .aria_label("Color")
            .children(
                TabGroupColor::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(ix, color)| {
                        let selected = color == picked;
                        div()
                            .id(("group-color", ix))
                            .debug_selector(move || {
                                format!("group-color-{}", color.label().to_lowercase())
                            })
                            .role(Role::RadioButton)
                            .aria_label(color.label())
                            .aria_toggled(if selected {
                                accesskit::Toggled::True
                            } else {
                                accesskit::Toggled::False
                            })
                            .aria_position_in_set(ix + 1)
                            .aria_size_of_set(count)
                            .tooltip(move |window, cx| {
                                Tooltip::new(color.label()).build(window, cx)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .size_6()
                            .rounded_full()
                            .bg(tokens::tab_group(color))
                            .border_2()
                            .border_color(if selected { ring } else { transparent_black() })
                            .hover(move |style| style.border_color(ring))
                            .when(selected, |swatch| {
                                swatch.child(
                                    svg()
                                        .path(IconName::Check.path())
                                        .size_3p5()
                                        .text_color(tokens::on_accent()),
                                )
                            })
                            .on_click(cx.listener(move |form, _, _, cx| {
                                form.color = Some(color);
                                cx.notify();
                            }))
                    }),
            )
    }
}

impl Render for NameForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let blank = self.input.read(cx).value().trim().is_empty();
        let swatches = self.color.map(|picked| self.swatches(picked, cx));
        v_flex()
            .gap_3()
            .child(Input::new(&self.input))
            .children(swatches)
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("group-name-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("group-name-confirm")
                            .primary()
                            .small()
                            .label(self.confirm.clone())
                            .disabled(blank)
                            .on_click(cx.listener(|form, _, window, cx| form.submit(window, cx))),
                    ),
            )
    }
}

/// Asks for a group's name, starting from `initial`, and hands a name that
/// is not blank to `on_name` once confirmed with the button or Enter.
pub(super) fn ask_name(
    title: &'static str,
    confirm: &'static str,
    initial: String,
    on_name: impl Fn(&str, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let on_name: OnName = Rc::new(move |name, _, window, cx| on_name(name, window, cx));
    open(title, confirm, initial, None, on_name, window, cx);
}

/// Asks for a new group's name and its color, starting from `color`, and
/// hands them to `on_pick` once confirmed with the button or Enter.
pub(super) fn ask_name_and_color(
    title: &'static str,
    confirm: &'static str,
    color: TabGroupColor,
    on_pick: impl Fn(&str, TabGroupColor, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    open(
        title,
        confirm,
        String::new(),
        Some(color),
        Rc::new(on_pick),
        window,
        cx,
    );
}

fn open(
    title: &'static str,
    confirm: &'static str,
    initial: String,
    color: Option<TabGroupColor>,
    on_name: OnName,
    window: &mut Window,
    cx: &mut App,
) {
    let form = cx.new(|cx| {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Group name")
                .default_value(initial)
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |form: &mut NameForm, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    form.submit(window, cx);
                }
                cx.notify();
            },
        );
        NameForm {
            input,
            on_name,
            confirm: confirm.into(),
            color,
            _subscription: subscription,
        }
    });
    let input = form.read(cx).input.clone();
    let focus_once = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus_once.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        dialog.title(title).w(dp(360.)).child(form.clone())
    });
}
