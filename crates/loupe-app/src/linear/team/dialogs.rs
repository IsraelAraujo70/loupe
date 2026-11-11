//! Small dialogs for saved views: name a view (rename, save as new) and
//! confirm deleting one.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::*;

use super::TeamView;
use crate::theme::scale::dp;

type OnName = Rc<dyn Fn(&mut TeamView, &str, &mut Context<TeamView>)>;

struct NameForm {
    input: Entity<InputState>,
    parent: WeakEntity<TeamView>,
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
                    Button::new("name-cancel")
                        .ghost()
                        .small()
                        .label("Cancel")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    Button::new("name-confirm")
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
    parent: WeakEntity<TeamView>,
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
        dialog.title(title).w(dp(400.)).child(form.clone())
    });
}

/// "Rename view…".
pub(super) fn rename(
    parent: WeakEntity<TeamView>,
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

/// "Save as new view…": the current filters and display settings as a new
/// view in Linear, under a new name.
pub(super) fn save_as(parent: WeakEntity<TeamView>, window: &mut Window, cx: &mut App) {
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

/// "Delete view…", behind a confirmation that names the view and says
/// where it is deleted from.
pub(super) fn confirm_delete(
    parent: WeakEntity<TeamView>,
    id: String,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    let title: SharedString = format!("Delete “{name}”?").into();
    let consequence = match crate::linear::views_store::kind(&id) {
        crate::linear::views_store::Kind::Legacy => {
            "The view is saved only in Loupe and is removed from it. Linear is not changed."
        }
        _ => "The view is deleted in Linear, for everyone who can see it.",
    };
    window.open_dialog(cx, move |dialog, _, cx| {
        let (parent, id) = (parent.clone(), id.clone());
        dialog.title(title.clone()).w(dp(400.)).child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(consequence),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("delete-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("delete-confirm")
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
