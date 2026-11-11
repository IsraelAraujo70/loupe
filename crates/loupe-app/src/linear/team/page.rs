//! Which view a team screen shows and the edits on top of it, after t3code's
//! `kanbanPageState.ts`: the screen works on a draft of the active view's
//! settings; the draft is "unsaved" while it differs from the saved view, and
//! Save or Discard reconciles them.

use loupe_linear::views::{ViewConfig, ViewsFile};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageState {
    pub view_id: Option<String>,
    pub draft: ViewConfig,
}

impl PageState {
    /// The screen opens on `preferred` when that view is on this team's tabs,
    /// else on the team's Active view, else on the first tab, else on
    /// default settings with no view.
    pub fn open(views: &ViewsFile, team_id: &str, preferred: Option<&str>) -> PageState {
        let on_tabs = |id: &str| views.for_team(team_id).find(|view| view.id == id);
        let active = crate::linear::views_store::active_view_id(team_id);
        let view = preferred
            .and_then(on_tabs)
            .or_else(|| on_tabs(&active))
            .or_else(|| views.for_team(team_id).next());
        match view {
            Some(view) => PageState {
                view_id: Some(view.id.clone()),
                draft: view.config.clone(),
            },
            None => PageState {
                view_id: None,
                draft: ViewConfig::default(),
            },
        }
    }

    pub fn saved<'a>(&self, views: &'a ViewsFile) -> Option<&'a ViewConfig> {
        views.get(self.view_id.as_deref()?).map(|view| &view.config)
    }

    /// The draft differs from the saved view (or there is no saved view to
    /// compare with and the draft is not the default).
    pub fn is_dirty(&self, views: &ViewsFile) -> bool {
        match self.saved(views) {
            Some(saved) => saved != &self.draft,
            None => self.draft != ViewConfig::default(),
        }
    }

    /// Shows view `id` with its saved settings; unsaved edits are dropped.
    pub fn switch(&mut self, views: &ViewsFile, id: &str) {
        if let Some(view) = views.get(id) {
            self.view_id = Some(view.id.clone());
            self.draft = view.config.clone();
        }
    }

    pub fn discard(&mut self, views: &ViewsFile) {
        self.draft = self.saved(views).cloned().unwrap_or_default();
    }

    /// Once a save of `sent` was answered: whether the draft should become
    /// what Linear stored. It should when the draft is still the one that
    /// was sent, or is the view as saved `before` the save (discarded or
    /// opened again meanwhile, with no edit since): shown as it is, that old
    /// view would pass for an unsaved change, and saving it would write the
    /// old view back. A draft edited since is the user's and stays.
    pub fn settles_on_stored(&self, sent: &ViewConfig, before: &ViewConfig) -> bool {
        self.draft == *sent || self.draft == *before
    }

    /// After the active view was deleted: move to the first remaining tab.
    pub fn after_delete(&mut self, views: &ViewsFile, team_id: &str) {
        *self = PageState::open(views, team_id, None);
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use loupe_linear::views::{Layout, ViewConfig, ViewsFile};

    use super::PageState;

    #[test]
    fn edits_are_unsaved_until_saved_or_discarded() {
        let now = Utc::now();
        let mut views = ViewsFile::seeded(now);
        let mut page = PageState::open(&views, "t1", None);
        assert_eq!(page.view_id.as_deref(), Some("v_active"));
        assert!(!page.is_dirty(&views));

        page.draft.layout = Layout::List;
        assert!(page.is_dirty(&views));
        page.discard(&views);
        assert!(!page.is_dirty(&views));

        page.draft.layout = Layout::List;
        let id = page.view_id.clone().expect("a view");
        views.update_config(&id, page.draft.clone(), now);
        assert!(!page.is_dirty(&views));
    }

    #[test]
    fn switching_drops_the_draft_and_unknown_views_are_ignored() {
        let views = ViewsFile::seeded(Utc::now());
        let mut page = PageState::open(&views, "t1", Some("v_done"));
        assert_eq!(page.view_id.as_deref(), Some("v_done"));
        page.draft.layout = Layout::List;
        page.switch(&views, "v_all");
        assert_eq!(page.draft, ViewConfig::default());
        page.switch(&views, "missing");
        assert_eq!(page.view_id.as_deref(), Some("v_all"));
    }

    #[test]
    fn a_team_opens_on_its_active_view_wherever_that_is_in_the_tabs() {
        let now = Utc::now();
        let mut views = ViewsFile::seeded(now);
        let active = crate::linear::views_store::active_view_id("t1");
        views.create("Urgent", Some("t1".into()), ViewConfig::default(), now);
        if let Some(view) = views.views.last_mut() {
            view.id = active.clone();
        }
        assert_eq!(
            PageState::open(&views, "t1", None).view_id,
            Some(active.clone())
        );
        // A view that is no longer on the tabs (one Loupe once seeded)
        // opens the same place.
        assert_eq!(
            PageState::open(&views, "t1", Some("v_gone")).view_id,
            Some(active)
        );
        // Another team's Active view is not this team's.
        assert_eq!(
            PageState::open(&views, "t2", None).view_id.as_deref(),
            Some("v_active")
        );
    }

    #[test]
    fn after_a_save_the_draft_follows_linear_unless_the_user_edited_it() {
        let before = ViewConfig::default();
        let sent = ViewConfig {
            layout: Layout::List,
            ..ViewConfig::default()
        };
        let page = |draft: &ViewConfig| PageState {
            view_id: Some("linear:v1".into()),
            draft: draft.clone(),
        };
        assert!(
            page(&sent).settles_on_stored(&sent, &before),
            "the draft that was sent"
        );
        assert!(
            page(&before).settles_on_stored(&sent, &before),
            "discarded or opened again while the save was on its way"
        );
        let edited = ViewConfig {
            show_empty_columns: false,
            ..sent.clone()
        };
        assert!(
            !page(&edited).settles_on_stored(&sent, &before),
            "edited since: those changes are still unsaved"
        );
    }

    #[test]
    fn deleting_the_active_view_falls_back_to_the_first_tab() {
        let now = Utc::now();
        let mut views = ViewsFile::seeded(now);
        let mut page = PageState::open(&views, "t1", Some("v_backlog"));
        views.delete("v_backlog");
        page.after_delete(&views, "t1");
        assert_eq!(page.view_id.as_deref(), Some("v_active"));
        views.views.clear();
        page.after_delete(&views, "t1");
        assert_eq!(page.view_id, None);
    }
}
