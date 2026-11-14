//! What the Settings page holds, as data: its sections in order, every
//! setting with the words search matches, and what each section's
//! Restore defaults puts back. Pure, so search and the defaults are tested
//! without a window.

use gpui_kit::assets::IconName;

use crate::settings::shortcuts::SHORTCUTS;
use crate::store::Settings;

/// A section of the Settings page, in page order. The nav lists them in
/// this order under their groups, and the page stacks them the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Appearance,
    GitHub,
    Linear,
    T3,
    Shortcuts,
    About,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::General,
        Section::Appearance,
        Section::GitHub,
        Section::Linear,
        Section::T3,
        Section::Shortcuts,
        Section::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::GitHub => "GitHub",
            Section::Linear => "Linear",
            Section::T3 => "T3 Code",
            Section::Shortcuts => "Keyboard shortcuts",
            Section::About => "About",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Section::General => IconName::SlidersHorizontal,
            Section::Appearance => IconName::Palette,
            Section::GitHub => IconName::Github,
            Section::Linear => IconName::SquareKanban,
            Section::T3 => IconName::Bot,
            Section::Shortcuts => IconName::Keyboard,
            Section::About => IconName::Info,
        }
    }

    /// The nav group the section sits in.
    pub fn group(self) -> &'static str {
        match self {
            Section::General | Section::Appearance => "Preferences",
            Section::GitHub | Section::Linear | Section::T3 => "Accounts",
            Section::Shortcuts | Section::About => "Loupe",
        }
    }

    /// The element id of the section's block on the page.
    pub fn id(self) -> &'static str {
        match self {
            Section::General => "settings-general",
            Section::Appearance => "settings-appearance",
            Section::GitHub => "settings-github",
            Section::Linear => "settings-linear",
            Section::T3 => "settings-t3",
            Section::Shortcuts => "settings-shortcuts",
            Section::About => "settings-about",
        }
    }

    /// Whether the section has preferences Restore defaults can put back.
    /// Accounts and the watched repositories are not preferences: they are
    /// the user's data, and only their own actions change them. T3 Code's
    /// are its review request, review prompts, and risk rules, not its
    /// server.
    pub fn has_defaults(self) -> bool {
        matches!(
            self,
            Section::General | Section::Appearance | Section::GitHub | Section::T3
        )
    }

    /// Puts this section's preferences back to their defaults and leaves
    /// every other field alone.
    pub fn restore_defaults(self, settings: &mut Settings) {
        let defaults = Settings::default();
        match self {
            Section::General => {
                settings.sidebar_collapsed = defaults.sidebar_collapsed;
                settings.group_tabs_by_repo = defaults.group_tabs_by_repo;
                settings.notification_filter = defaults.notification_filter;
            }
            Section::Appearance => {
                settings.zoom = defaults.zoom;
                settings.sidebar_width = defaults.sidebar_width;
                settings.appearance = defaults.appearance;
            }
            Section::GitHub => {
                settings.diff_layout = defaults.diff_layout;
                settings.diff_wrap = defaults.diff_wrap;
                settings.file_tree_position = defaults.file_tree_position;
                settings.comment_order = defaults.comment_order;
                settings.comment_box = defaults.comment_box;
            }
            Section::T3 => settings.t3_review = defaults.t3_review,
            Section::Linear | Section::Shortcuts | Section::About => {}
        }
    }

    /// Whether Restore defaults would change nothing.
    pub fn is_default(self, settings: &Settings) -> bool {
        let mut restored = settings.clone();
        self.restore_defaults(&mut restored);
        restored == *settings
    }
}

/// One setting on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    SidebarIcons,
    GroupTabsByRepo,
    InboxFilter,
    ColorScheme,
    Themes,
    Contrast,
    GlassOpacity,
    Density,
    Zoom,
    SidebarWidth,
    InterfaceFont,
    MonoFont,
    GitHubAccount,
    DiffLayout,
    FileTree,
    LongLines,
    CommentOrder,
    CommentBox,
    Repositories,
    LinearAccount,
    WorkspaceMappings,
    T3Server,
    ReviewPrompt,
    ReviewPrompts,
    RiskCheck,
    RiskPrompt,
    RiskList(crate::t3::RiskList),
    /// The shortcut at this index of [`SHORTCUTS`].
    Shortcut(usize),
    Version,
    Updates,
}

/// A setting with the text the page shows for it and the words search
/// also matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub item: Item,
    pub section: Section,
    /// The card it shares with its neighbors of the same group; a group
    /// other than "" has its name as a label over the card.
    pub group: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub keywords: &'static str,
}

const fn entry(
    item: Item,
    section: Section,
    group: &'static str,
    title: &'static str,
    description: &'static str,
    keywords: &'static str,
) -> Entry {
    Entry {
        item,
        section,
        group,
        title,
        description,
        keywords,
    }
}

/// Every setting, in page order.
pub fn entries() -> Vec<Entry> {
    use Item::*;
    use Section as S;
    let mut entries = vec![
        entry(
            SidebarIcons,
            S::General,
            "",
            "Sidebar as icons",
            "Shrink the sidebar to a rail of icons. Cmd+B switches it.",
            "collapse rail compact",
        ),
        entry(
            GroupTabsByRepo,
            S::General,
            "",
            "Group tabs by repository automatically",
            "A new pull request or repository tab joins a group named after its repository. Turning this off keeps the groups.",
            "tab groups auto repo",
        ),
        entry(
            InboxFilter,
            S::General,
            "",
            "Inbox filter",
            "The org:, repo:, and words the Inbox shows. Type them in the Inbox.",
            "notifications org repo",
        ),
        entry(
            ColorScheme,
            S::Appearance,
            "Color scheme",
            "Color scheme",
            "Light, dark, or the system's.",
            "theme light dark system mode appearance",
        ),
        entry(
            Themes,
            S::Appearance,
            "Themes",
            "Themes",
            "Each theme has a light and a dark palette.",
            "theme colors palette create add import duplicate edit export delete",
        ),
        entry(
            Contrast,
            S::Appearance,
            "Interface",
            "Contrast",
            "The strength of borders and washes; above 100% muted text gets brighter too.",
            "colors borders accessibility",
        ),
        entry(
            GlassOpacity,
            S::Appearance,
            "Interface",
            "Glass opacity",
            "Higher values make the frosted window more solid.",
            "transparency blur translucent vibrancy",
        ),
        entry(
            Density,
            S::Appearance,
            "Interface",
            "Density",
            "The height of list and sidebar rows.",
            "compact comfortable spacing rows",
        ),
        entry(
            Zoom,
            S::Appearance,
            "Interface",
            "Zoom",
            "The size of everything in the window. Cmd+= and Cmd+- step it.",
            "font size scale text bigger smaller",
        ),
        entry(
            SidebarWidth,
            S::Appearance,
            "Interface",
            "Sidebar width",
            "Drag the sidebar's edge to change it.",
            "resize",
        ),
        entry(
            InterfaceFont,
            S::Appearance,
            "Typography",
            "Interface font",
            "Everything outside code. The size is the size of list rows and values.",
            "typeface family text size",
        ),
        entry(
            MonoFont,
            S::Appearance,
            "Typography",
            "Monospace font",
            "Diffs, code blocks, logs, branches, and SHAs.",
            "typeface family code size mono",
        ),
        entry(
            GitHubAccount,
            S::GitHub,
            "",
            "GitHub account",
            "The account Loupe reads pull requests and notifications with.",
            "sign out login token user",
        ),
        entry(
            DiffLayout,
            S::GitHub,
            "Pull requests",
            "Diff layout",
            "How the Files tab shows changes.",
            "unified split side by side",
        ),
        entry(
            FileTree,
            S::GitHub,
            "Pull requests",
            "File tree position",
            "Which side of the diff the tree of changed files is on.",
            "left right files panel",
        ),
        entry(
            LongLines,
            S::GitHub,
            "Pull requests",
            "Long lines",
            "Whether a diff scrolls long lines sideways or wraps them.",
            "wrap scroll diff",
        ),
        entry(
            CommentOrder,
            S::GitHub,
            "Pull requests",
            "Conversation order",
            "The order of a pull request's comments.",
            "oldest newest sort comments",
        ),
        entry(
            CommentBox,
            S::GitHub,
            "Pull requests",
            "Comment box",
            "Where the new-comment box sits in a conversation.",
            "top bottom end compose",
        ),
        entry(
            Repositories,
            S::GitHub,
            "Repositories",
            "Watched repositories",
            "The repositories the sidebar lists. Watch one from its page.",
            "repos unwatch sidebar",
        ),
        entry(
            LinearAccount,
            S::Linear,
            "",
            "Linear workspaces",
            "The workspaces Loupe reads issues from, one personal API key each.",
            "api key disconnect workspace account switch add",
        ),
        entry(
            WorkspaceMappings,
            S::Linear,
            "Pull requests",
            "Workspace by repository",
            "Which workspace a pull request's issues come from. Others use the active one.",
            "map mapping github org organization repo repository owner",
        ),
        entry(
            T3Server,
            S::T3,
            "",
            "T3 Code server",
            "The server Loupe reads threads from and starts reviews on.",
            "url token operate threads review",
        ),
        entry(
            ReviewPrompt,
            S::T3,
            "Review",
            "Review request",
            "What a review thread starts with. The dialog shows it before Send, and an edit \
             there is for that one review.",
            "prompt template variables restore default",
        ),
        entry(
            ReviewPrompts,
            S::T3,
            "Review prompts",
            "Review prompts",
            "Each is a tab of the review dialog, with its own text, model, and effort.",
            "prompt reviewer model effort provider add edit reorder delete default security",
        ),
        entry(
            RiskCheck,
            S::T3,
            "Risky pull requests",
            "Detect risky pull requests",
            "A risky pull request starts on the prompt chosen for risk.",
            "risk security reviewer",
        ),
        entry(
            RiskPrompt,
            S::T3,
            "Risky pull requests",
            "Prompt for risky pull requests",
            "The review prompt the dialog starts on for a risky pull request.",
            "risk security reviewer prompt",
        ),
        entry(
            RiskList(crate::t3::RiskList::Labels),
            S::T3,
            "Risky pull requests",
            "Risk labels",
            "A label that contains one of these marks the pull request. Separate them with \
             commas.",
            "risk labels",
        ),
        entry(
            RiskList(crate::t3::RiskList::Words),
            S::T3,
            "Risky pull requests",
            "Risky path words",
            "A changed file with a path word that starts with one of these.",
            "risk paths files words",
        ),
        entry(
            RiskList(crate::t3::RiskList::Exceptions),
            S::T3,
            "Risky pull requests",
            "Not risky",
            "Path words that start like a risky one and are not, such as author.",
            "risk exceptions ignore",
        ),
    ];
    entries.extend(SHORTCUTS.iter().enumerate().map(|(index, shortcut)| {
        entry(
            Shortcut(index),
            S::Shortcuts,
            shortcut.group,
            shortcut.label,
            "",
            "keys keybinding hotkey",
        )
    }));
    entries.extend([
        entry(
            Version,
            S::About,
            "",
            "Version",
            "Review GitHub pull requests next to your Linear issues.",
            "loupe release",
        ),
        entry(
            Updates,
            S::About,
            "",
            "Updates",
            "Loupe checks for a new release every hour.",
            "update upgrade install restart release",
        ),
    ]);
    entries
}

impl Item {
    /// Whether the setting draws a block of its own (a card, a grid of
    /// cards) instead of a row in its group's card.
    pub fn is_block(self) -> bool {
        matches!(
            self,
            Item::GitHubAccount
                | Item::LinearAccount
                | Item::T3Server
                | Item::ReviewPrompt
                | Item::ReviewPrompts
                | Item::ColorScheme
                | Item::Themes
        )
    }
}

impl Entry {
    /// Whether every word of `query` starts a word, in any case, of the
    /// setting's title, description, group, section, or keywords.
    pub fn matches(&self, query: &str) -> bool {
        let haystack = [
            self.section.title(),
            self.group,
            self.title,
            self.description,
            self.keywords,
        ]
        .join(" ")
        .to_lowercase();
        let words: Vec<&str> = split_words(&haystack).collect();
        split_words(&query.to_lowercase())
            .all(|typed| words.iter().any(|word| word.starts_with(typed)))
    }
}

fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

/// The sections that have a setting matching `query`, in page order, each
/// with its matching settings. An empty query keeps every setting.
pub fn search(query: &str) -> Vec<(Section, Vec<Entry>)> {
    let entries = entries();
    Section::ALL
        .into_iter()
        .filter_map(|section| {
            let found: Vec<Entry> = entries
                .iter()
                .filter(|entry| entry.section == section && entry.matches(query))
                .copied()
                .collect();
            (!found.is_empty()).then_some((section, found))
        })
        .collect()
}

/// How far below the top of the page a section's heading may sit and still
/// count as the section being read: its heading and first row.
pub const READING_SLACK: f32 = 48.;

/// The index of the section being read, given where each section starts
/// (`tops`, in page order) and the page's top edge: the last section whose
/// top is within [`READING_SLACK`] of that edge, or the last section once
/// the page is scrolled to its end, where a short final section can never
/// reach the top.
pub fn reading_index(tops: &[f32], page_top: f32, at_end: bool) -> usize {
    if at_end {
        return tops.len().saturating_sub(1);
    }
    tops.iter()
        .rposition(|top| *top <= page_top + READING_SLACK)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Item, Section, entries, reading_index, search};
    use crate::store::{CommentBoxPlace, CommentOrder, DiffLayout, FileTreePosition, Settings};
    use crate::theme::Zoom;
    use loupe_github::RepoRef;

    fn changed() -> Settings {
        Settings {
            sidebar_collapsed: true,
            group_tabs_by_repo: true,
            notification_filter: vec!["org:acme".into()],
            zoom: Zoom::from(125),
            sidebar_width: Some(300.),
            appearance: crate::theme::Appearance {
                theme: "grove".into(),
                color_scheme: crate::theme::ColorScheme::Light,
                contrast: 130,
                ..crate::theme::Appearance::default()
            },
            diff_layout: DiffLayout::Split,
            diff_wrap: true,
            file_tree_position: FileTreePosition::Right,
            comment_order: CommentOrder::NewestFirst,
            comment_box: Some(CommentBoxPlace::Bottom),
            watched_repos: vec![RepoRef::new("acme", "app")],
            t3_server_url: Some("https://t3.example".into()),
            t3_review: crate::t3::ReviewSettings {
                prompt: Some("Review #{number}".into()),
                risk_check: false,
                ..crate::t3::ReviewSettings::default()
            },
            ..Settings::default()
        }
    }

    #[test]
    fn restoring_a_section_resets_only_its_own_preferences() {
        let mut settings = changed();
        Section::GitHub.restore_defaults(&mut settings);
        assert_eq!(settings.diff_layout, DiffLayout::Unified);
        assert!(!settings.diff_wrap);
        assert_eq!(settings.file_tree_position, FileTreePosition::Left);
        assert_eq!(settings.comment_order, CommentOrder::OldestFirst);
        assert_eq!(settings.comment_box, None);
        assert_eq!(
            settings.watched_repos,
            [RepoRef::new("acme", "app")],
            "watched repositories are data, not a default"
        );
        assert_eq!(settings.zoom, Zoom::from(125), "Appearance is untouched");
        assert!(settings.sidebar_collapsed, "General is untouched");
        assert!(settings.group_tabs_by_repo, "General is untouched");
        assert_eq!(
            settings.t3_server_url.as_deref(),
            Some("https://t3.example")
        );

        Section::Appearance.restore_defaults(&mut settings);
        assert_eq!(settings.appearance, crate::theme::Appearance::default());
        assert_eq!(settings.zoom, Zoom::ACTUAL_SIZE);
        assert_eq!(settings.sidebar_width, None);
        assert!(settings.sidebar_collapsed);

        Section::General.restore_defaults(&mut settings);
        assert!(!settings.sidebar_collapsed);
        assert!(!settings.group_tabs_by_repo);
        assert!(settings.notification_filter.is_empty());
        assert_eq!(settings.watched_repos.len(), 1);
        assert!(!settings.t3_review.risk_check, "T3 Code is untouched");

        Section::T3.restore_defaults(&mut settings);
        assert_eq!(settings.t3_review, crate::t3::ReviewSettings::default());
        assert_eq!(
            settings.t3_server_url.as_deref(),
            Some("https://t3.example"),
            "the server is an account, not a default"
        );
    }

    #[test]
    fn a_section_is_default_until_one_of_its_preferences_changes() {
        let defaults = Settings::default();
        for section in Section::ALL {
            assert!(section.is_default(&defaults), "{section:?}");
        }
        let changed = changed();
        for section in Section::ALL {
            assert_eq!(
                section.is_default(&changed),
                !section.has_defaults(),
                "{section:?}"
            );
        }
        let only_repos = Settings {
            watched_repos: vec![RepoRef::new("acme", "app")],
            ..Settings::default()
        };
        assert!(Section::GitHub.is_default(&only_repos));
    }

    #[test]
    fn an_empty_query_keeps_every_section_and_setting() {
        let found = search("  ");
        assert_eq!(
            found
                .iter()
                .map(|(section, _)| *section)
                .collect::<Vec<_>>(),
            Section::ALL
        );
        let count: usize = found.iter().map(|(_, entries)| entries.len()).sum();
        assert_eq!(count, entries().len());
    }

    #[test]
    fn search_matches_every_word_in_any_case_across_sections() {
        let found = search("TREE");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, Section::GitHub);
        assert_eq!(
            found[0]
                .1
                .iter()
                .map(|entry| entry.item)
                .collect::<Vec<_>>(),
            [Item::FileTree]
        );

        // Words match the description and keywords, and all must match.
        let found = search("wrap diff");
        assert_eq!(
            found
                .iter()
                .flat_map(|(_, entries)| entries.iter().map(|entry| entry.item))
                .collect::<Vec<_>>(),
            [Item::LongLines]
        );

        // A section's name finds its settings.
        let found = search("t3");
        assert_eq!(found[0].0, Section::T3);

        assert!(search("no such setting").is_empty());
        assert!(
            search("ree").is_empty(),
            "a word matches from its start, not inside \"tree\""
        );
    }

    #[test]
    fn a_block_is_alone_in_its_group() {
        // The page wraps a run of settings of one group in a card, so a
        // block that shares its run would be a card inside a card.
        let entries = entries();
        for pair in entries.windows(2) {
            let same_run = pair[0].section == pair[1].section && pair[0].group == pair[1].group;
            assert!(
                !(same_run && (pair[0].item.is_block() || pair[1].item.is_block())),
                "{:?} and {:?} share a card",
                pair[0].item,
                pair[1].item
            );
        }
    }

    #[test]
    fn search_finds_shortcuts_by_their_label() {
        let found = search("zoom out");
        let sections: Vec<Section> = found.iter().map(|(section, _)| *section).collect();
        assert_eq!(sections, [Section::Shortcuts]);
        assert!(matches!(found[0].1[0].item, Item::Shortcut(_)));
    }

    #[test]
    fn the_section_being_read_is_the_last_one_that_reached_the_top() {
        let tops = [0., 300., 900., 1400.];
        assert_eq!(reading_index(&tops, 0., false), 0);
        assert_eq!(reading_index(&tops, 280., false), 1, "within the slack");
        assert_eq!(reading_index(&tops, 899., false), 2);
        assert_eq!(reading_index(&tops, 1000., true), 3, "the end of the page");
        assert_eq!(reading_index(&[], 0., false), 0);
    }
}
