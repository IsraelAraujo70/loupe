//! Every keyboard and menu action Loupe defines. `menus::init` binds keys and
//! builds the app menu; app-level actions are handled there, window-level ones
//! by `Workspace` through `on_action` under the `Workspace` key context.

gpui_kit::actions!(
    loupe,
    [
        /// Cmd+[: back in the active tab's history. Handled by `Workspace`.
        GoBack,
        /// Cmd+]: forward in the active tab's history. Handled by `Workspace`.
        GoForward,
        /// Cmd+T: open a new tab. Handled by `Workspace`.
        NewTab,
        /// Cmd+W: close the active tab. Handled by `Workspace`.
        CloseTab,
        /// Ctrl+Tab: the next tab. Handled by `Workspace`.
        NextTab,
        /// Ctrl+Shift+Tab: the previous tab. Handled by `Workspace`.
        PreviousTab,
        /// Cmd+1 to Cmd+8: the tab at that position. Handled by `Workspace`.
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        /// Cmd+9: the last tab. Handled by `Workspace`.
        ActivateLastTab,
        /// Cmd+Backslash: the active tab in a new pane on the right of its pane.
        /// The tab moves when its pane has others, and is opened again
        /// when it is the pane's only one. Handled by `Workspace`, like the
        /// other three.
        SplitRight,
        /// The active tab in a new pane on the left.
        SplitLeft,
        /// The active tab in a new pane above.
        SplitUp,
        /// Cmd+Shift+Backslash: the active tab in a new pane below.
        SplitDown,
        /// Cmd+Enter in the command palette: open the result in a new tab.
        /// Handled by the palette.
        PaletteOpenInNewTab,
        /// Cmd+B: collapse the sidebar to an icon rail. Handled by `Workspace`.
        ToggleSidebar,
        /// Cmd+K: open the command palette. Handled by `Workspace`.
        OpenPalette,
        /// Cmd+Shift+A, Chrome's key for its tab search: open the command
        /// palette on the open tabs (`tab:`). Handled by `Workspace`, and by
        /// the palette while it is open.
        SearchTabs,
        /// O then W, as in Linear: open the Linear workspace switcher.
        /// Handled by `Workspace`.
        SwitchWorkspace,
        /// Cmd+,: navigate to Settings. Handled by `Workspace`.
        OpenSettings,
        /// /: put the caret in the Settings search. Handled by the Settings
        /// view.
        FocusSettingsSearch,
        /// C, as in Linear: open the new issue modal, for the team on screen
        /// if there is one. Handled by `Workspace`.
        ComposeIssue,
        /// J or Down: the row cursor of the list on screen moves down.
        /// Handled by the list's view.
        SelectNext,
        /// K or Up: the row cursor moves up. Handled by the list's view.
        SelectPrevious,
        /// Enter or O: open the row under the cursor in this tab. Handled by
        /// the list's view.
        OpenSelected,
        /// Cmd+Enter: open the issue or pull request under the pointer (or
        /// on screen) in a new background tab. Handled by `Workspace`, like
        /// the other item actions, which act on the same item.
        OpenItemInNewTab,
        /// Cmd+Shift+Enter: open it in a new pane on the right.
        OpenItemSideBySide,
        /// Cmd+Shift+Comma, as in Linear: copy its link.
        CopyItemLink,
        /// Cmd+Period, as in Linear: copy an issue's identifier.
        CopyItemId,
        /// Cmd+Shift+Period, as in Linear: copy its branch name.
        CopyItemBranch,
        /// Page Down: scroll the current screen down by a page. Handled by
        /// `Workspace`.
        ScrollPageDown,
        /// Page Up: scroll the current screen up by a page. Handled by
        /// `Workspace`.
        ScrollPageUp,
        /// Cmd+= or Cmd++: zoom the whole window one step in. Handled by
        /// `menus`.
        ZoomIn,
        /// Cmd+-: zoom one step out. Handled by `menus`.
        ZoomOut,
        /// Cmd+0: back to 100%. Handled by `menus`.
        ResetZoom,
        /// Loupe > About Loupe. Handled by `menus`.
        About,
        /// Cmd+Q. Handled by `menus`.
        Quit,
        /// Cmd+Shift+W. Handled by `menus`.
        CloseWindow,
        /// Cmd+M. Handled by `menus`.
        Minimize,
    ]
);
