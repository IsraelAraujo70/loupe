//! The Keyboard shortcuts section: what each shortcut does, in Loupe's
//! words, with its keys read from the keymap `shell::menus::init` binds, so
//! the page never shows a key the app does not answer to.

use gpui_kit::{Action, App, AsKeystroke as _, Keystroke};

use crate::shell::actions::{
    ActivateLastTab, ActivateTab1, CloseTab, CloseWindow, ComposeIssue, CopyItemBranch, CopyItemId,
    CopyItemLink, FocusSettingsSearch, GoBack, GoForward, Minimize, NewTab, NextTab,
    OpenItemInNewTab, OpenItemSideBySide, OpenPalette, OpenSelected, OpenSettings,
    PaletteOpenInNewTab, PreviousTab, Quit, ResetZoom, ScrollPageDown, ScrollPageUp, SearchTabs,
    SelectNext, SelectPrevious, SwitchWorkspace, ToggleSidebar, ZoomIn, ZoomOut,
};

/// One row of the section: its group (the card it is in), what it does,
/// and the action whose bindings are its keys.
pub struct Shortcut {
    pub group: &'static str,
    pub label: &'static str,
    action: fn() -> Box<dyn Action>,
}

const fn shortcut(
    group: &'static str,
    label: &'static str,
    action: fn() -> Box<dyn Action>,
) -> Shortcut {
    Shortcut {
        group,
        label,
        action,
    }
}

pub const SHORTCUTS: &[Shortcut] = &[
    shortcut("Go to", "Search and go to", || Box::new(OpenPalette)),
    shortcut("Go to", "Open the result in a new tab", || {
        Box::new(PaletteOpenInNewTab)
    }),
    shortcut("Go to", "Back", || Box::new(GoBack)),
    shortcut("Go to", "Forward", || Box::new(GoForward)),
    shortcut("Go to", "Settings", || Box::new(OpenSettings)),
    shortcut("Go to", "Switch workspace", || Box::new(SwitchWorkspace)),
    shortcut("Go to", "Search settings", || Box::new(FocusSettingsSearch)),
    shortcut("Tabs", "New tab", || Box::new(NewTab)),
    shortcut("Tabs", "Close tab", || Box::new(CloseTab)),
    shortcut("Tabs", "Next tab", || Box::new(NextTab)),
    shortcut("Tabs", "Previous tab", || Box::new(PreviousTab)),
    shortcut("Tabs", "First tab (the next keys go up to the 8th)", || {
        Box::new(ActivateTab1)
    }),
    shortcut("Tabs", "Last tab", || Box::new(ActivateLastTab)),
    shortcut("Tabs", "Search the open tabs", || Box::new(SearchTabs)),
    shortcut("Lists", "Next row", || Box::new(SelectNext)),
    shortcut("Lists", "Previous row", || Box::new(SelectPrevious)),
    shortcut("Lists", "Open the row", || Box::new(OpenSelected)),
    shortcut("Lists", "Select every pull request in the section", || {
        Box::new(crate::github::SelectSectionRows)
    }),
    shortcut("Lists", "Clear the selection", || {
        Box::new(crate::github::ClearReviewsSelection)
    }),
    shortcut("Lists", "Filter Reviews", || {
        Box::new(crate::github::OpenReviewsFilter)
    }),
    shortcut("Lists", "Filter a team's issues", || {
        Box::new(crate::linear::OpenFilter)
    }),
    shortcut(
        "Lists",
        "Open the row under the pointer in a new tab",
        || Box::new(OpenItemInNewTab),
    ),
    shortcut(
        "Lists",
        "Open the row under the pointer side by side",
        || Box::new(OpenItemSideBySide),
    ),
    shortcut(
        "Lists",
        "Copy the link of the row under the pointer",
        || Box::new(CopyItemLink),
    ),
    shortcut(
        "Lists",
        "Copy the ID of the issue under the pointer",
        || Box::new(CopyItemId),
    ),
    shortcut(
        "Lists",
        "Copy the branch of the row under the pointer",
        || Box::new(CopyItemBranch),
    ),
    shortcut("Lists", "Scroll down a page", || Box::new(ScrollPageDown)),
    shortcut("Lists", "Scroll up a page", || Box::new(ScrollPageUp)),
    shortcut("Issue", "New issue", || Box::new(ComposeIssue)),
    shortcut("Issue", "Change the state", || {
        Box::new(crate::linear::ChangeState)
    }),
    shortcut("Issue", "Change the priority", || {
        Box::new(crate::linear::ChangePriority)
    }),
    shortcut("Issue", "Change the assignee", || {
        Box::new(crate::linear::ChangeAssignee)
    }),
    shortcut("Issue", "Change the labels", || {
        Box::new(crate::linear::ChangeLabels)
    }),
    shortcut("Issue", "Move to a project", || {
        Box::new(crate::linear::SetProject)
    }),
    shortcut("Issue", "Write a comment", || {
        Box::new(crate::linear::WriteComment)
    }),
    shortcut("Window", "Show the sidebar as icons", || {
        Box::new(ToggleSidebar)
    }),
    shortcut("Window", "Zoom in", || Box::new(ZoomIn)),
    shortcut("Window", "Zoom out", || Box::new(ZoomOut)),
    shortcut("Window", "Actual size", || Box::new(ResetZoom)),
    shortcut("Window", "Minimize", || Box::new(Minimize)),
    shortcut("Window", "Close window", || Box::new(CloseWindow)),
    shortcut("Window", "Quit Loupe", || Box::new(Quit)),
];

impl Shortcut {
    /// The keys of every binding of the action, each binding's keystrokes
    /// in order (O then W), without repeats, in the order they were bound
    /// (the first is the one the menus show).
    pub fn keys(&self, cx: &App) -> Vec<Vec<Keystroke>> {
        let action = (self.action)();
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let mut keys: Vec<Vec<Keystroke>> = Vec::new();
        for binding in keymap.bindings_for_action(action.as_ref()) {
            let strokes: Vec<Keystroke> = binding
                .keystrokes()
                .iter()
                .map(|stroke| stroke.as_keystroke().clone())
                .collect();
            if !strokes.is_empty() && !keys.contains(&strokes) {
                keys.push(strokes);
            }
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Keystroke, TestAppContext};

    use super::SHORTCUTS;

    #[gpui_kit::test]
    fn every_shortcut_shows_the_keys_the_app_binds(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::shell::menus::bind_keys(cx);
            for shortcut in SHORTCUTS {
                assert!(
                    !shortcut.keys(cx).is_empty(),
                    "{:?} has no key bound",
                    shortcut.label
                );
            }
            let next_row = SHORTCUTS
                .iter()
                .find(|shortcut| shortcut.label == "Next row")
                .expect("the row");
            assert_eq!(
                next_row.keys(cx),
                [
                    vec![Keystroke::parse("j").expect("j")],
                    vec![Keystroke::parse("down").expect("down")]
                ],
                "every key of an action, in the order bound"
            );
            let switch = SHORTCUTS
                .iter()
                .find(|shortcut| shortcut.label == "Switch workspace")
                .expect("the row");
            assert_eq!(
                switch.keys(cx),
                [vec![
                    Keystroke::parse("o").expect("o"),
                    Keystroke::parse("w").expect("w")
                ]],
                "a sequence keeps every key"
            );
        });
    }
}
