//! The views a Linear screen can show. They come from Linear: its custom
//! views (`remote_views`) and each team's built-in ones (Active, Backlog,
//! All issues). Loupe keeps none of its own.
//!
//! It once did, in `linear-views.json` in the data directory. That file is
//! still read: a view in it is listed as "only in Loupe" until the user
//! saves it to Linear or deletes it, and keeps working until then. Only two
//! kinds are left out, because nothing of them would be missed: a copy of a
//! Linear view, and the three views the app seeded that a team's built-in
//! tabs replace, while nobody changed or starred them. The file is never
//! rewritten except by one of those two actions.

use std::path::PathBuf;

use chrono::Utc;
use gpui_kit::{App, Global, SharedString, Task};
use loupe_linear::filter::{FilterClause, SetField, SetOp};
use loupe_linear::views::{
    self, CompletedWindow, Loaded, View, ViewConfig, ViewsFile, export_linear_filter,
    export_preferences,
};
use loupe_linear::{FavoriteTarget, NewCustomView};

use super::{data, remote_views};
use crate::store::{self, Favorite, Settings};

struct LegacyViews {
    file: ViewsFile,
    /// Why the file on disk could not be used, until the user sees it once.
    warning: Option<SharedString>,
}

impl Global for LegacyViews {}

fn path() -> PathBuf {
    crate::store::data_dir().join("linear-views.json")
}

fn legacy_file(cx: &mut App) -> &mut LegacyViews {
    if !cx.has_global::<LegacyViews>() {
        let empty = ViewsFile {
            version: views::VIEWS_VERSION,
            views: Vec::new(),
        };
        let (file, warning) = match views::load(&path(), Utc::now()) {
            Loaded::Read(file) => (file, None),
            // No file: nothing was ever saved here.
            Loaded::Seeded(_) => (empty, None),
            Loaded::Recovered { reason, .. } => (
                empty,
                Some(format!("The views saved only in Loupe could not be read ({reason}). A copy is in linear-views.json.bak.").into()),
            ),
        };
        cx.set_global(LegacyViews { file, warning });
    }
    cx.global_mut::<LegacyViews>()
}

/// Where a view comes from, by its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A custom view saved in Linear (`linear:<id>`).
    Custom,
    /// One of a team's built-in views (`linear-<kind>:<team id>`).
    BuiltIn,
    /// A view from the file Loupe once kept, not in Linear yet.
    Legacy,
}

pub fn kind(id: &str) -> Kind {
    if id.starts_with("linear:") {
        Kind::Custom
    } else if id.starts_with("linear-") {
        Kind::BuiltIn
    } else {
        Kind::Legacy
    }
}

/// The id of a team's Active view, which its screen opens on.
pub fn active_view_id(team_id: &str) -> String {
    format!("linear-active:{team_id}")
}

/// The views every team has in Linear without anyone saving them, in the
/// order of its tabs.
fn built_in(team_id: &str) -> Vec<View> {
    let now = Utc::now();
    let status_types = |types: &[&str]| {
        vec![FilterClause::Set {
            field: SetField::StatusType,
            op: SetOp::In,
            values: types.iter().map(|kind| (*kind).to_owned()).collect(),
        }]
    };
    let view = |id: String, name: &str, config: ViewConfig| View {
        id,
        team_id: Some(team_id.to_owned()),
        name: name.to_owned(),
        config,
        linear_view_id: None,
        created_at: now,
        updated_at: now,
    };
    vec![
        view(
            active_view_id(team_id),
            "Active",
            ViewConfig {
                filters: status_types(&["unstarted", "started"]),
                ..ViewConfig::default()
            },
        ),
        view(
            format!("linear-backlog:{team_id}"),
            "Backlog",
            ViewConfig {
                filters: status_types(&["triage", "backlog"]),
                ..ViewConfig::default()
            },
        ),
        view(
            format!("linear-all:{team_id}"),
            "All issues",
            ViewConfig {
                completed: CompletedWindow::All,
                ..ViewConfig::default()
            },
        ),
    ]
}

/// The seeded views that a team's built-in tabs stand in for, under the same
/// names. Active and Backlog have the same filter; the built-in All issues
/// also lists issues closed more than a month ago, as its name says.
const REPLACED_SEEDS: [&str; 3] = ["v_active", "v_backlog", "v_all"];

/// The views of `file` that stay listed until the user decides what to do
/// with each. Whether a view is used cannot be known, so only a view with
/// nothing to miss is left out: a copy of a Linear view, and one of
/// [`REPLACED_SEEDS`] as the app seeded it, neither changed nor starred. The
/// other seeded views (My issues, Done) have no counterpart in Linear and
/// stay.
fn kept_legacy(file: &ViewsFile, is_favorite: impl Fn(&str) -> bool) -> Vec<View> {
    file.views
        .iter()
        .filter(|view| view.linear_view_id.is_none())
        .filter(|view| {
            let seeded = ViewsFile::seeded(view.created_at);
            let replaced = REPLACED_SEEDS.contains(&view.id.as_str())
                && seeded.get(&view.id).is_some_and(|seed| {
                    seed.name == view.name
                        && seed.team_id == view.team_id
                        && seed.config == view.config
                });
            !replaced || is_favorite(&view.id)
        })
        .cloned()
        .collect()
}

/// The views still saved only in Loupe.
pub fn legacy(cx: &mut App) -> Vec<View> {
    let file = legacy_file(cx).file.clone();
    let settings = Settings::get(cx);
    kept_legacy(&file, |id| {
        settings.is_favorite(&Favorite::View { id: id.to_owned() })
    })
}

/// Every view that is not tied to one team's screen: Linear's custom views
/// and starred built-in ones, then the ones still only in Loupe.
pub fn snapshot(cx: &mut App) -> ViewsFile {
    let mut all = super::remote_views::snapshot(cx);
    all.extend(legacy(cx));
    ViewsFile {
        version: views::VIEWS_VERSION,
        views: all,
    }
}

/// The views `team_id`'s screen shows, in tab order: Linear's custom views
/// for it, its built-in views, then the ones still only in Loupe.
pub fn for_team(cx: &mut App, team_id: &str) -> ViewsFile {
    let built_in = built_in(team_id);
    let all = snapshot(cx);
    let (legacy, custom): (Vec<View>, Vec<View>) = all
        .for_team(team_id)
        // A starred built-in view is listed once, as the team's own tab.
        .filter(|view| built_in.iter().all(|own| own.id != view.id))
        .cloned()
        .partition(|view| kind(&view.id) == Kind::Legacy);
    ViewsFile {
        version: views::VIEWS_VERSION,
        views: custom.into_iter().chain(built_in).chain(legacy).collect(),
    }
}

/// The load warning, once.
pub fn take_warning(cx: &mut App) -> Option<SharedString> {
    legacy_file(cx).warning.take()
}

/// Applies `change` to a copy of the views saved only in Loupe and writes
/// it; only a successful write replaces the views in memory, so a failed
/// save never looks saved. Returns what `change` returned, or why the file
/// could not be written.
pub fn update_legacy<R>(
    cx: &mut App,
    change: impl FnOnce(&mut ViewsFile) -> R,
) -> Result<R, SharedString> {
    let mut next = legacy_file(cx).file.clone();
    let result = change(&mut next);
    views::save(&path(), &next)
        .map_err(|error| SharedString::from(format!("Couldn’t save views: {error}")))?;
    legacy_file(cx).file = next;
    cx.refresh_windows();
    Ok(result)
}

/// A view [`create_in_linear`] made.
pub struct Created {
    /// The view's id here (`linear:<id>`).
    pub id: String,
    /// What Linear refused after it had stored the view, for the user. The
    /// view exists; when it was being moved, Loupe's copy and its star are
    /// still here and the message says so.
    pub warning: Option<SharedString>,
}

/// Saves `config` in Linear as a new custom view named `name`: of
/// `team_id`, or a workspace view with `None`; private to the key's owner.
/// The search text is not part of a view. Fails only when Linear has no
/// such view; once the view is stored, a refused display setting or
/// favorite is a warning.
///
/// `moved` is the view only in Loupe that the new one replaces. Its star
/// goes to Linear's favorites, and Loupe's copy and star are deleted only
/// once Linear holds all of it: the view, its display settings, and its
/// star. After anything less they stay, and moving the view again finishes
/// the job.
///
/// A move makes no new view when this session already lists a private one
/// with the same name, team, and filter, whether an earlier try made it or
/// the user made it in Linear: that view is the one kept, and it takes
/// these display settings. With no such view listed (the list is loading,
/// or the filter differs) a new one is made. A favorite the view already
/// has is not added again.
pub fn create_in_linear(
    name: &str,
    config: &ViewConfig,
    team_id: Option<String>,
    moved: Option<String>,
    cx: &mut App,
) -> Task<Result<Created, SharedString>> {
    let Some(client) = data::client(cx) else {
        return Task::ready(Err("Linear is not connected.".into()));
    };
    let filters: Vec<FilterClause> = config
        .filters
        .iter()
        .filter(|clause| !matches!(clause, FilterClause::Text { .. }))
        .cloned()
        .collect();
    let filter_data = match export_linear_filter(&filters) {
        Ok(filter) => filter,
        Err(reason) => {
            return Task::ready(Err(format!(
                "{reason}. Remove it to save the view in Linear."
            )
            .into()));
        }
    };
    let new = NewCustomView {
        name: name.trim().to_owned(),
        team_id,
        shared: false,
        filter_data,
    };
    let preferences = export_preferences(config, None, &serde_json::Value::Null);
    // Moving the same view again (Loupe's copy could not be removed the
    // first time) finds the view Linear already has for it.
    let existing = moved
        .as_ref()
        .and_then(|_| remote_views::same_view(&new, cx));
    let starred = moved
        .as_ref()
        .is_some_and(|id| Settings::get(cx).is_favorite(&Favorite::View { id: id.clone() }));
    let saved = crate::services::background(cx, move || {
        let reused = existing.is_some();
        let created = match existing {
            Some(existing) => existing,
            None => client.create_custom_view(&new)?,
        };
        let mut refused = Vec::new();
        let stored = match client.save_view_preferences(&created.id, &preferences) {
            Ok(stored) => stored,
            // A view taken from this session's list was not stored by Linear
            // just now and may be gone there: nothing is known to exist.
            Err(error) if reused => return Err(error),
            Err(error) => {
                refused.push(("its display settings", error));
                created
            }
        };
        if starred {
            // An earlier try may have left the star: Linear says.
            let has_star = client.favorites().map(|favorites| {
                favorites.iter().any(|favorite| {
                    matches!(
                        &favorite.target,
                        FavoriteTarget::CustomView { view_id, .. } if *view_id == stored.id
                    )
                })
            });
            let star = match has_star {
                Ok(true) => Ok(()),
                Ok(false) => client.favorite_custom_view(&stored.id),
                Err(error) => Err(error),
            };
            if let Err(error) = star {
                refused.push(("its star", error));
            }
        }
        Ok::<_, loupe_linear::Error>((stored, refused))
    });
    cx.spawn(async move |cx| {
        let (stored, refused) = saved.await.map_err(|error| {
            SharedString::from(format!(
                "Couldn’t save the view in Linear: {}",
                data::error_text(&error)
            ))
        })?;
        let warning = refused.first().map(|(_, error)| {
            let what: Vec<&str> = refused.iter().map(|(what, _)| *what).collect();
            let kept = if moved.is_some() {
                " Loupe kept its copy: save it to Linear again to finish."
            } else {
                ""
            };
            SharedString::from(format!(
                "The view is saved in Linear, but not {}: {}{kept}",
                what.join(" and "),
                data::error_text(error)
            ))
        });
        cx.update(|cx| {
            let id = format!("linear:{}", stored.id);
            remote_views::upsert(stored, cx);
            // Loupe's copy and its star go only once Linear holds all of
            // what they were.
            let Some(old) = moved.filter(|_| refused.is_empty()) else {
                return Ok(Created { id, warning });
            };
            update_legacy(cx, |views| views.delete(&old))?;
            store::update_settings(cx, |settings| {
                let favorite = Favorite::View { id: old };
                if settings.is_favorite(&favorite) {
                    settings.toggle_favorite(favorite);
                }
            });
            if starred {
                // Linear lists the new favorite on its next read.
                remote_views::retry(cx);
            }
            Ok(Created { id, warning })
        })
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use loupe_linear::views::{Layout, ViewConfig, ViewsFile};

    use super::{Kind, built_in, kept_legacy, kind};

    fn names(views: &[loupe_linear::views::View]) -> Vec<&str> {
        views.iter().map(|view| view.name.as_str()).collect()
    }

    #[test]
    fn only_the_seeded_views_the_built_in_tabs_replace_are_left_out() {
        let now = Utc::now();
        let mut file = ViewsFile::seeded(now);
        // Active, Backlog, and All issues are a team's built-in tabs now.
        // My issues and Done have no counterpart in Linear: the user may
        // open them every day without ever starring or changing them.
        assert_eq!(names(&kept_legacy(&file, |_| false)), ["My issues", "Done"]);

        // A replaced one the user starred is the user's too.
        assert_eq!(
            names(&kept_legacy(&file, |id| id == "v_active")),
            ["Active", "My issues", "Done"]
        );

        // Changed, renamed, or made here: listed as well.
        let list = ViewConfig {
            layout: Layout::List,
            ..ViewConfig::default()
        };
        file.update_config("v_all", list, now);
        file.rename("v_backlog", "Later", now);
        file.create("Urgent", Some("t1".into()), ViewConfig::default(), now);
        assert_eq!(
            names(&kept_legacy(&file, |_| false)),
            ["Later", "All issues", "My issues", "Done", "Urgent"]
        );
    }

    #[test]
    fn a_copy_of_a_linear_view_is_not_kept() {
        let now = Utc::now();
        let mut file = ViewsFile::seeded(now);
        let id = file.create("From Linear", None, ViewConfig::default(), now);
        if let Some(view) = file.views.iter_mut().find(|view| view.id == id) {
            view.linear_view_id = Some("abc".into());
        }
        assert!(
            kept_legacy(&file, |_| true)
                .iter()
                .all(|view| view.id != id)
        );
    }

    #[test]
    fn every_team_has_linears_three_views() {
        let views = built_in("t1");
        assert_eq!(names(&views), ["Active", "Backlog", "All issues"]);
        assert!(views.iter().all(|view| kind(&view.id) == Kind::BuiltIn));
        assert!(
            views
                .iter()
                .all(|view| view.team_id.as_deref() == Some("t1"))
        );
        assert_eq!(kind("linear:0c9f"), Kind::Custom);
        assert_eq!(kind("v_mine"), Kind::Legacy);
    }
}
