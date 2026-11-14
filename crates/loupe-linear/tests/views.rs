//! Persistence of Loupe's saved views: missing, corrupt, newer, and round
//! trip.

use std::path::PathBuf;

use chrono::Utc;
use loupe_linear::filter::{FilterClause, SetField, SetOp};
use loupe_linear::views::{Layout, Loaded, VIEWS_VERSION, ViewConfig, ViewsFile, load, save};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("views-tests");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("json.bak"));
    path
}

#[test]
fn a_missing_file_starts_with_the_seeded_views() {
    let path = scratch("missing.json");
    match load(&path, Utc::now()) {
        Loaded::Seeded(file) => {
            let names: Vec<&str> = file.views.iter().map(|v| v.name.as_str()).collect();
            assert_eq!(
                names,
                ["Active", "Backlog", "All issues", "My issues", "Done"]
            );
        }
        other => panic!("expected seeded views, got {other:?}"),
    }
}

#[test]
fn saved_views_load_back_unchanged() {
    let path = scratch("round-trip.json");
    let now = Utc::now();
    let mut file = ViewsFile::seeded(now);
    let id = file.create(
        "  Urgent bugs  ",
        Some("team-1".into()),
        ViewConfig {
            filters: vec![FilterClause::Set {
                field: SetField::Priority,
                op: SetOp::In,
                values: vec!["1".into()],
            }],
            layout: Layout::List,
            ..ViewConfig::default()
        },
        now,
    );
    assert!(file.rename(&id, "Urgent", now));
    assert!(file.move_by(&id, "team-1", -1));
    save(&path, &file).expect("save");
    match load(&path, now) {
        Loaded::Read(back) => {
            assert_eq!(back, file);
            assert_eq!(back.views[4].name, "Urgent");
            assert_eq!(back.version, VIEWS_VERSION);
        }
        other => panic!("expected the saved file, got {other:?}"),
    }
}

#[test]
fn a_corrupt_file_is_kept_as_a_backup_and_seeds_are_used() {
    let path = scratch("corrupt.json");
    std::fs::write(&path, b"{ not json").expect("write corrupt file");
    match load(&path, Utc::now()) {
        Loaded::Recovered { file, reason } => {
            assert_eq!(file.views.len(), 5);
            assert!(!reason.is_empty());
        }
        other => panic!("expected recovery, got {other:?}"),
    }
    let backup = std::fs::read(path.with_extension("json.bak")).expect("backup");
    assert_eq!(backup, b"{ not json");
}

#[test]
fn a_newer_file_is_not_overwritten_silently() {
    let path = scratch("newer.json");
    std::fs::write(&path, br#"{ "version": 99, "views": [] }"#).expect("write newer file");
    assert!(matches!(load(&path, Utc::now()), Loaded::Recovered { .. }));
    assert!(path.with_extension("json.bak").exists());
}

#[test]
fn unknown_and_missing_config_fields_take_defaults() {
    let path = scratch("partial.json");
    std::fs::write(
        &path,
        br#"{ "version": 1, "views": [ { "id": "v1", "name": "Mine",
            "createdAt": "2026-11-10T12:00:00Z", "updatedAt": "2026-11-10T12:00:00Z",
            "config": { "layout": "list", "futureSetting": true } } ] }"#,
    )
    .expect("write partial file");
    let file = load(&path, Utc::now()).into_file();
    assert_eq!(file.views[0].config.layout, Layout::List);
    assert_eq!(file.views[0].config.filters, Vec::new());
    assert_eq!(file.views[0].team_id, None);
}

#[test]
fn moving_and_deleting_views() {
    let now = Utc::now();
    let mut file = ViewsFile::seeded(now);
    let other_team = file.create("Theirs", Some("team-2".into()), ViewConfig::default(), now);
    let mine = file.create("Mine", Some("team-1".into()), ViewConfig::default(), now);
    // "Theirs" is not on team-1's tabs, so moving "Mine" earlier skips it.
    assert!(file.move_by(&mine, "team-1", -1));
    let tabs: Vec<&str> = file.for_team("team-1").map(|v| v.name.as_str()).collect();
    assert_eq!(
        tabs,
        [
            "Active",
            "Backlog",
            "All issues",
            "My issues",
            "Mine",
            "Done"
        ]
    );
    assert!(!file.move_by(&mine, "team-1", 5));
    assert!(file.delete(&other_team));
    assert!(!file.delete(&other_team));
    assert!(file.for_team("team-2").all(|v| v.team_id.is_none()));
}

#[test]
fn linear_display_settings_are_applied_without_local_defaults_hiding_closed_issues() {
    use loupe_linear::views::{
        CompletedWindow, DisplayProperty, Grouping, Ordering, linear_preferences,
    };
    let (config, warnings) = linear_preferences(&serde_json::json!({
        "layout":"list", "issueGrouping":"none", "viewOrdering":"createdAt",
        "showCompletedIssues":"all", "showEmptyGroups":true, "fieldDateCreated":true, "fieldDateUpdated":false
    }));
    assert!(warnings.is_empty());
    assert_eq!(config.layout, Layout::List);
    assert_eq!(config.group_by, Grouping::None);
    assert_eq!(config.order_by, Ordering::Created);
    assert_eq!(config.completed, CompletedWindow::All);
    assert!(config.show_empty_columns);
    assert!(config.shows(DisplayProperty::Created));
    assert!(!config.shows(DisplayProperty::Updated));
    let (_, warnings) =
        linear_preferences(&serde_json::json!({"issueGrouping":"unsupported-group"}));
    assert!(warnings[0].contains("unsupported-group"));
}

#[test]
fn linear_explicit_order_direction_is_preserved() {
    use loupe_linear::views::{SortDirection, linear_preferences};
    let (config, warnings) = linear_preferences(
        &serde_json::json!({"viewOrdering":"createdAt", "viewOrderingDirection":"ascending"}),
    );
    assert!(warnings.is_empty());
    assert_eq!(config.sort_direction, Some(SortDirection::Ascending));
}
