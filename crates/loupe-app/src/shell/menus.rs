//! Key bindings, the native app menu, and app-level actions.

use gpui_kit::component::input::{Copy as CopyText, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::*;

use crate::route::Route;
use crate::services::{Accounts, Connection};
use crate::shell::actions::{
    About, ActivateLastTab, ActivateTab1, ActivateTab2, ActivateTab3, ActivateTab4, ActivateTab5,
    ActivateTab6, ActivateTab7, ActivateTab8, CloseTab, CloseWindow, ComposeIssue, CopyItemBranch,
    CopyItemId, CopyItemLink, FocusSettingsSearch, GoBack, GoForward, Minimize, NewTab, NextTab,
    OpenItemInNewTab, OpenItemSideBySide, OpenPalette, OpenSelected, OpenSettings,
    PaletteOpenInNewTab, PreviousTab, Quit, ResetZoom, ScrollPageDown, ScrollPageUp, SearchTabs,
    SelectNext, SelectPrevious, SplitDown, SplitLeft, SplitRight, SplitUp, SwitchWorkspace,
    ToggleSidebar, ZoomIn, ZoomOut,
};
use crate::shell::navigate;
use crate::store::{Settings, update_settings};
use crate::theme::{self, Zoom};

/// Where the list keys (J, K, the arrows, Enter, O) act: a screen with a
/// row cursor, while no text field has the focus.
const LIST_KEYS: &str = "InboxView && !Input";

/// Where an issue's single-letter keys act: its screen, while no text field
/// has the focus.
const ISSUE_KEYS: &str = "IssueView && !Input";

/// Where the new issue modal's property keys act: the modal, while no text
/// field (its title, its description, a menu's search) has the focus.
const NEW_ISSUE_KEYS: &str = "NewIssue && !Input";

/// Where the keys of an issue's or a pull request's menu act on the row
/// under the pointer, or else on the page on screen: anywhere in the
/// window while no text field has the focus. A dialog (the palette, New
/// issue) is outside `Workspace`, so its own keys keep theirs.
const ITEM_KEYS: &str = "Workspace && !Input";

/// Where Settings' own key acts: its screen, while no text field has the
/// focus.
const SETTINGS_KEYS: &str = "SettingsView && !Input";

const ABOUT_DESCRIPTION: &str = "Review GitHub pull requests next to your Linear issues.";

/// Binds every shortcut in `actions`, sets the menu bar (Loupe: About,
/// Settings Cmd+,, Quit Cmd+Q; Edit: undo/redo/cut/copy/paste/select all as
/// OS actions; View: Zoom In Cmd+=, Zoom Out Cmd+-, Actual Size Cmd+0;
/// Window: tabs, the four splits, Close Window Cmd+Shift+W, Minimize Cmd+M),
/// handles `About`, `Quit`, `CloseWindow`, `Minimize` and the zoom actions,
/// and quits when the last window closes.
///
/// Window-level shortcuts are bound without a key context so they work
/// wherever focus is inside the window. A focused text input still wins for
/// keys it binds itself (`cmd-[` outdents there); every other key reaches
/// `Workspace`, and typing is untouched.
pub fn init(cx: &mut App) {
    bind_keys(cx);
    set_menus(cx);
    handle_app_actions(cx);
}

/// Binds every shortcut. Settings > Keyboard shortcuts reads its keys from
/// these bindings.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-[", GoBack, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-k", OpenPalette, None),
        KeyBinding::new("cmd-shift-a", SearchTabs, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("pagedown", ScrollPageDown, Some("Workspace")),
        KeyBinding::new("pageup", ScrollPageUp, Some("Workspace")),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-w", CloseWindow, None),
        KeyBinding::new("cmd-]", GoForward, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("cmd-1", ActivateTab1, None),
        KeyBinding::new("cmd-2", ActivateTab2, None),
        KeyBinding::new("cmd-3", ActivateTab3, None),
        KeyBinding::new("cmd-4", ActivateTab4, None),
        KeyBinding::new("cmd-5", ActivateTab5, None),
        KeyBinding::new("cmd-6", ActivateTab6, None),
        KeyBinding::new("cmd-7", ActivateTab7, None),
        KeyBinding::new("cmd-8", ActivateTab8, None),
        KeyBinding::new("cmd-9", ActivateLastTab, None),
        // Zed's and VS Code's key for splitting to the right.
        KeyBinding::new("cmd-\\", SplitRight, None),
        KeyBinding::new("cmd-shift-\\", SplitDown, None),
        // The palette's Cmd+Enter opens the result in a new tab.
        KeyBinding::new("cmd-enter", PaletteOpenInNewTab, Some("Command")),
        KeyBinding::new("cmd-m", Minimize, None),
        // The first binding is the one the View menu shows.
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ResetZoom, None),
        // Text fields keep the letter: GPUI prefers typing when one is focused.
        KeyBinding::new("f", crate::linear::OpenFilter, Some("TeamView")),
        KeyBinding::new("f", crate::github::OpenReviewsFilter, Some("InboxView")),
        // An issue's properties, on Linear's own keys, and its comment box.
        KeyBinding::new("s", crate::linear::ChangeState, Some(ISSUE_KEYS)),
        KeyBinding::new("p", crate::linear::ChangePriority, Some(ISSUE_KEYS)),
        KeyBinding::new("a", crate::linear::ChangeAssignee, Some(ISSUE_KEYS)),
        KeyBinding::new("l", crate::linear::ChangeLabels, Some(ISSUE_KEYS)),
        KeyBinding::new("c", crate::linear::WriteComment, Some(ISSUE_KEYS)),
        // Linear's key for a new issue, wherever no text field has the
        // focus; an issue's own C (comment) is the more specific binding.
        KeyBinding::new("c", ComposeIssue, Some("Workspace && !Input")),
        // Linear's O W. Not on a list, where O alone opens the row.
        KeyBinding::new(
            "o w",
            SwitchWorkspace,
            Some("Workspace && !Input && !InboxView"),
        ),
        // The new issue modal's menus, on the same keys and on Linear's
        // Shift keys for the properties an issue's page does not edit.
        KeyBinding::new("s", crate::linear::ChangeState, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("p", crate::linear::ChangePriority, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("a", crate::linear::ChangeAssignee, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("l", crate::linear::ChangeLabels, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("shift-p", crate::linear::SetProject, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("shift-c", crate::linear::SetCycle, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("shift-e", crate::linear::SetEstimate, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("shift-d", crate::linear::SetDueDate, Some(NEW_ISSUE_KEYS)),
        KeyBinding::new("t", crate::linear::MoveToTeam, Some(NEW_ISSUE_KEYS)),
        // The row cursor of a list. Not while a text field has the focus:
        // there the same keys type and move the caret.
        KeyBinding::new("j", SelectNext, Some(LIST_KEYS)),
        KeyBinding::new("down", SelectNext, Some(LIST_KEYS)),
        KeyBinding::new("k", SelectPrevious, Some(LIST_KEYS)),
        KeyBinding::new("up", SelectPrevious, Some(LIST_KEYS)),
        KeyBinding::new("enter", OpenSelected, Some(LIST_KEYS)),
        KeyBinding::new("o", OpenSelected, Some(LIST_KEYS)),
        KeyBinding::new("cmd-a", crate::github::SelectTabRows, Some(LIST_KEYS)),
        KeyBinding::new("right", crate::github::OpenStack, Some(LIST_KEYS)),
        KeyBinding::new("left", crate::github::CloseStack, Some(LIST_KEYS)),
        KeyBinding::new(
            "escape",
            crate::github::ClearReviewsSelection,
            Some(LIST_KEYS),
        ),
        KeyBinding::new("/", FocusSettingsSearch, Some(SETTINGS_KEYS)),
        // The context menu of an issue or a pull request, on Linear's keys
        // where Linear has one. An issue's page binds S, P, A, and L itself.
        KeyBinding::new("cmd-enter", OpenItemInNewTab, Some(ITEM_KEYS)),
        KeyBinding::new("cmd-shift-enter", OpenItemSideBySide, Some(ITEM_KEYS)),
        // Linux reports Shift with punctuation as the shifted character, so
        // each Shift key is bound both ways; the last is the one shown.
        KeyBinding::new("cmd-<", CopyItemLink, Some(ITEM_KEYS)),
        KeyBinding::new("cmd-shift-,", CopyItemLink, Some(ITEM_KEYS)),
        KeyBinding::new("cmd-.", CopyItemId, Some(ITEM_KEYS)),
        KeyBinding::new("cmd->", CopyItemBranch, Some(ITEM_KEYS)),
        KeyBinding::new("cmd-shift-.", CopyItemBranch, Some(ITEM_KEYS)),
        KeyBinding::new("s", crate::linear::ChangeState, Some(ITEM_KEYS)),
        KeyBinding::new("p", crate::linear::ChangePriority, Some(ITEM_KEYS)),
        KeyBinding::new("a", crate::linear::ChangeAssignee, Some(ITEM_KEYS)),
        KeyBinding::new("l", crate::linear::ChangeLabels, Some(ITEM_KEYS)),
        KeyBinding::new("shift-p", crate::linear::SetProject, Some(ITEM_KEYS)),
    ]);
}

fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu::new("Loupe").items([
            MenuItem::action("About Loupe", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit Loupe", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", CopyText, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Actual Size", ResetZoom),
        ]),
        Menu::new("Window").items([
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Previous Tab", PreviousTab),
            MenuItem::action("Search Tabs…", SearchTabs),
            MenuItem::separator(),
            MenuItem::action("Split Right", SplitRight),
            MenuItem::action("Split Left", SplitLeft),
            MenuItem::action("Split Up", SplitUp),
            MenuItem::action("Split Down", SplitDown),
            MenuItem::separator(),
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Close Window", CloseWindow),
        ]),
    ]);
}

fn handle_app_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ZoomIn, cx| set_zoom(cx, Zoom::zoomed_in));
    cx.on_action(|_: &ZoomOut, cx| set_zoom(cx, Zoom::zoomed_out));
    cx.on_action(|_: &ResetZoom, cx| set_zoom(cx, |_| Zoom::ACTUAL_SIZE));
    // App-level, so the menu item stays enabled when no view in the window
    // holds focus or Loupe is not the active app.
    cx.on_action(|_: &OpenSettings, cx| {
        if matches!(Accounts::get(cx).github, Connection::Connected(_)) {
            with_active_window(cx, |window, cx| navigate(Route::Settings, window, cx));
        }
    });
    cx.on_action(|_: &CloseWindow, cx| {
        with_active_window(cx, |window, _| window.remove_window());
    });
    cx.on_action(|_: &Minimize, cx| {
        with_active_window(cx, |window, _| window.minimize_window());
    });
    cx.on_action(|_: &About, cx| {
        with_active_window(cx, |window, cx| {
            window.open_dialog(cx, |dialog, _, cx| {
                dialog.title("Loupe").child(
                    v_flex()
                        .gap_1()
                        .child(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(ABOUT_DESCRIPTION),
                        ),
                )
            });
        });
    });

    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// Saves the zoom `change` makes and applies it to every window.
fn set_zoom(cx: &mut App, change: impl FnOnce(Zoom) -> Zoom) {
    let zoom = change(Settings::get(cx).zoom);
    update_settings(cx, |settings| settings.zoom = zoom);
    theme::apply_zoom(zoom, cx);
}

/// Runs `f` in the frontmost window, or in the first open window when Loupe
/// is not the active app (menu actions still arrive then).
///
/// Deferred: a key binding (Cmd+, where no native menu takes the key first,
/// as on Linux) dispatches its action while that window is being updated,
/// and updating it again from here would fail.
fn with_active_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let Some(handle) = cx
            .active_window()
            .or_else(|| cx.windows().into_iter().next())
        else {
            return;
        };
        handle.update(cx, |_, window, cx| f(window, cx)).ok();
    });
}
