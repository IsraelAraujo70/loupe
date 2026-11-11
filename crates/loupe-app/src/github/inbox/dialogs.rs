//! Dialogs for saved Reviews views: name a view (save as new, rename) and
//! confirm deleting one.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::*;

use super::InboxView;

type OnName = Rc<dyn Fn(&mut InboxView, &str, &mut Context<InboxView>)>;

struct NameForm {
    input: Entity<InputState>,
    parent: WeakEntity<InboxView>,
    on_name: OnName,
    confirm: SharedString,
    _subscription: Subscription,
}

impl NameForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().to_string();
        if name.trim().is_empty() {
            return;
        }
        let on_name = self.on_name.clone();
        self.parent
            .update(cx, |view, cx| on_name(view, &name, cx))
            .ok();
        window.close_dialog(cx);
    }
}

impl Render for NameForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let blank = self.input.read(cx).value().trim().is_empty();
        v_flex().gap_3().child(Input::new(&self.input)).child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("reviews-name-cancel")
                        .ghost()
                        .small()
                        .label("Cancel")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    Button::new("reviews-name-confirm")
                        .primary()
                        .small()
                        .label(self.confirm.clone())
                        .disabled(blank)
                        .on_click(cx.listener(|form, _, window, cx| form.submit(window, cx))),
                ),
        )
    }
}

fn open_name_dialog(
    parent: WeakEntity<InboxView>,
    title: &'static str,
    confirm: &'static str,
    initial: String,
    on_name: OnName,
    window: &mut Window,
    cx: &mut App,
) {
    let form = cx.new(|cx| {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("View name")
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
            parent,
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
        dialog.title(title).w(px(400.)).child(form.clone())
    });
}

/// "Save as new view…": the current filters under a new name.
pub(super) fn save_as(parent: WeakEntity<InboxView>, window: &mut Window, cx: &mut App) {
    open_name_dialog(
        parent,
        "Save as new view",
        "Save view",
        String::new(),
        Rc::new(|view, name, cx| view.save_as_new(name, cx)),
        window,
        cx,
    );
}

/// "Rename view…".
pub(super) fn rename(
    parent: WeakEntity<InboxView>,
    id: String,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    open_name_dialog(
        parent,
        "Rename view",
        "Rename",
        name,
        Rc::new(move |view, name, cx| view.rename_view(&id, name, cx)),
        window,
        cx,
    );
}

/// "Delete view…", behind a confirmation that names the view.
pub(super) fn confirm_delete(
    parent: WeakEntity<InboxView>,
    id: String,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    let title: SharedString = format!("Delete “{name}”?").into();
    window.open_dialog(cx, move |dialog, _, cx| {
        let (parent, id) = (parent.clone(), id.clone());
        dialog.title(title.clone()).w(px(400.)).child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("The view is removed from Loupe. GitHub is not changed."),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("reviews-delete-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("reviews-delete-confirm")
                                .danger()
                                .small()
                                .label("Delete")
                                .on_click(move |_, window, cx| {
                                    parent.update(cx, |view, cx| view.delete_view(&id, cx)).ok();
                                    window.close_dialog(cx);
                                }),
                        ),
                ),
        )
    });
}
