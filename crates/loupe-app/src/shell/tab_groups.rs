//! The dialog that names a group of tabs, for "Add to new group…" and
//! "Rename group…".

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;

use crate::theme::scale::dp;

type OnName = Rc<dyn Fn(&str, &mut Window, &mut App)>;

struct NameForm {
    input: Entity<InputState>,
    on_name: OnName,
    confirm: SharedString,
    _subscription: Subscription,
}

impl NameForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        window.close_dialog(cx);
        (self.on_name)(&name, window, cx);
    }
}

impl Render for NameForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let blank = self.input.read(cx).value().trim().is_empty();
        v_flex().gap_3().child(Input::new(&self.input)).child(
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
    let on_name: OnName = Rc::new(on_name);
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
