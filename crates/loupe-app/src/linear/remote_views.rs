//! Linear's custom views and favorites as this session knows them: loaded
//! once per client, and kept in step with what the app itself saves in
//! Linear ([`upsert`], [`remove`]).
use super::data;
use chrono::Utc;
use gpui_kit::{App, Global, SharedString, Task};
use loupe_linear::views::{View, linear_preferences};
use loupe_linear::{Linear, LinearCustomView, LinearFavorite};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct RemoteViews {
    owner: Option<Arc<Linear>>,
    pub views: Vec<LinearCustomView>,
    pub favorites: Vec<LinearFavorite>,
    pub error: Option<SharedString>,
    pub loading: bool,
    task: Option<Task<()>>,
}
impl Global for RemoteViews {}

pub(super) fn ensure(cx: &mut App) {
    let client = data::client(cx);
    if cx.has_global::<RemoteViews>()
        && data::same_client(&cx.global::<RemoteViews>().owner, &client)
    {
        return;
    }
    cx.set_global(RemoteViews {
        owner: client.clone(),
        ..Default::default()
    });
    let Some(client) = client else { return };
    cx.global_mut::<RemoteViews>().loading = true;
    let worker = client.clone();
    let task = crate::services::background(cx, move || {
        Ok::<_, loupe_linear::Error>((worker.custom_views()?, worker.favorites()?))
    });
    let task = cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            if !data::same_client(&data::client(cx), &Some(client)) {
                return;
            }
            let state = cx.global_mut::<RemoteViews>();
            state.loading = false;
            match result {
                Ok((views, favorites)) => {
                    state.views = views;
                    state.favorites = favorites;
                }
                Err(error) => state.error = Some(data::error_text(&error)),
            }
            cx.refresh_windows();
        });
    });
    cx.global_mut::<RemoteViews>().task = Some(task);
}

pub(super) fn retry(cx: &mut App) {
    cx.global_mut::<RemoteViews>().owner = None;
    ensure(cx);
}

/// Records a view Linear just stored: a new one goes last, a known one is
/// replaced in place.
pub(super) fn upsert(view: LinearCustomView, cx: &mut App) {
    ensure(cx);
    let views = &mut cx.global_mut::<RemoteViews>().views;
    match views.iter_mut().find(|known| known.id == view.id) {
        Some(known) => *known = view,
        None => views.push(view),
    }
    cx.refresh_windows();
}

/// Whether `known` is the key owner's own copy of `new`: private, with the
/// same name, team, and filter. A shared view may be someone else's, who can
/// change or delete it, so it never stands in for a view the user is saving;
/// a private view the key can see is its owner's.
fn is_own_copy(known: &LinearCustomView, new: &loupe_linear::NewCustomView) -> bool {
    !known.shared
        && known.name == new.name
        && known.team_id == new.team_id
        && known.filter_data == new.filter_data
}

/// The private view Linear already has with this name, team, and filter,
/// when the views are loaded.
pub(super) fn same_view(
    new: &loupe_linear::NewCustomView,
    cx: &mut App,
) -> Option<LinearCustomView> {
    ensure(cx);
    cx.global::<RemoteViews>()
        .views
        .iter()
        .find(|known| is_own_copy(known, new))
        .cloned()
}

/// Forgets a view Linear just deleted.
pub(super) fn remove(id: &str, cx: &mut App) {
    ensure(cx);
    cx.global_mut::<RemoteViews>()
        .views
        .retain(|known| known.id != id);
    cx.refresh_windows();
}

pub(super) fn snapshot(cx: &mut App) -> Vec<View> {
    ensure(cx);
    let now = Utc::now();
    let mut views: Vec<View> = cx
        .global::<RemoteViews>()
        .views
        .iter()
        .map(|view| View {
            id: format!("linear:{}", view.id),
            name: view.name.clone(),
            team_id: view.team_id.clone(),
            config: linear_preferences(&view.preferences).0,
            linear_view_id: Some(view.id.clone()),
            created_at: now,
            updated_at: now,
        })
        .collect();
    for favorite in &cx.global::<RemoteViews>().favorites {
        if let loupe_linear::FavoriteTarget::PredefinedView {
            kind,
            team: Some(team),
        } = &favorite.target
            && kind == "allIssues"
        {
            views.push(View {
                id: format!("linear-all:{}", team.id),
                name: format!("{} {}", favorite.title, team.name),
                team_id: Some(team.id.clone()),
                config: loupe_linear::views::ViewConfig {
                    completed: loupe_linear::views::CompletedWindow::All,
                    ..Default::default()
                },
                linear_view_id: None,
                created_at: now,
                updated_at: now,
            });
        }
    }
    let favorites = &cx.global::<RemoteViews>().favorites;
    views.sort_by_key(|view| {
        favorites
            .iter()
            .position(|favorite| {
                super::favorite_view_id(favorite).as_deref() == Some(view.id.as_str())
            })
            .unwrap_or(usize::MAX)
    });
    views
}

pub(super) fn metadata(id: &str, cx: &mut App) -> Option<LinearCustomView> {
    ensure(cx);
    let id = id.strip_prefix("linear:")?;
    cx.global::<RemoteViews>()
        .views
        .iter()
        .find(|view| view.id == id)
        .cloned()
}

#[cfg(test)]
mod tests {
    use loupe_linear::{LinearCustomView, NewCustomView};
    use serde_json::{Value, json};

    use super::is_own_copy;

    fn mine() -> NewCustomView {
        NewCustomView {
            name: "My issues".into(),
            team_id: Some("t1".into()),
            shared: false,
            filter_data: json!({ "and": [{ "assignee": { "isMe": { "eq": true } } }] }),
        }
    }

    fn in_linear(new: &NewCustomView, shared: bool) -> LinearCustomView {
        LinearCustomView {
            id: "v1".into(),
            name: new.name.clone(),
            icon: None,
            color: None,
            shared,
            owner_name: "Someone".into(),
            team_id: new.team_id.clone(),
            filter_data: new.filter_data.clone(),
            preferences: Value::Null,
            own_preferences: Value::Null,
        }
    }

    #[test]
    fn a_view_being_moved_again_finds_only_its_own_private_copy() {
        let new = mine();
        assert!(
            is_own_copy(&in_linear(&new, false), &new),
            "the view an earlier try made is reused"
        );
        assert!(
            !is_own_copy(&in_linear(&new, true), &new),
            "a shared view with the same name, team, and filter may be someone else's"
        );

        let other_team = LinearCustomView {
            team_id: Some("t2".into()),
            ..in_linear(&new, false)
        };
        let workspace = LinearCustomView {
            team_id: None,
            ..in_linear(&new, false)
        };
        let other_filter = LinearCustomView {
            filter_data: json!({}),
            ..in_linear(&new, false)
        };
        let other_name = LinearCustomView {
            name: "Mine".into(),
            ..in_linear(&new, false)
        };
        for different in [other_team, workspace, other_filter, other_name] {
            assert!(!is_own_copy(&different, &new), "{different:?}");
        }
    }
}
