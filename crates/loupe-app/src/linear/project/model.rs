//! What the project's side column edits, without GPUI: the rows of each
//! picker, the change a picked row asks Linear for, and that change applied
//! to the page at once, before Linear answers.

use loupe_linear::{
    Label, ProjectChange, ProjectDetail, ProjectOptions, ProjectStatus, TeamRef, User,
};

/// A property of the project the side column edits with a picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Status,
    Lead,
    Members,
    Teams,
    Labels,
}

impl Field {
    pub const ALL: [Field; 5] = [
        Field::Status,
        Field::Lead,
        Field::Members,
        Field::Teams,
        Field::Labels,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Status => "Status",
            Field::Lead => "Lead",
            Field::Members => "Members",
            Field::Teams => "Teams",
            Field::Labels => "Labels",
        }
    }

    /// The property in a sentence: "Change lead", "Couldn’t change lead".
    pub fn noun(self) -> &'static str {
        match self {
            Field::Status => "status",
            Field::Lead => "lead",
            Field::Members => "members",
            Field::Teams => "teams",
            Field::Labels => "labels",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Field::Status => "project-status",
            Field::Lead => "project-lead",
            Field::Members => "project-members",
            Field::Teams => "project-teams",
            Field::Labels => "project-labels",
        }
    }

    /// A set: picking a row flips it and the picker stays open.
    pub fn is_set(self) -> bool {
        matches!(self, Field::Members | Field::Teams | Field::Labels)
    }
}

/// What a row of a picker shows before its name.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mark {
    Status(ProjectStatus),
    Person(Option<User>),
    /// A label's color, as Linear's hex string.
    Swatch(String),
    Team,
}

/// One row of a picker.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Choice {
    /// The value's id; empty for "No lead".
    pub id: String,
    pub name: String,
    pub mark: Mark,
    /// The project has this value now.
    pub selected: bool,
}

/// The rows of `field`'s picker for the project as shown.
pub(super) fn choices(
    field: Field,
    project: &ProjectDetail,
    options: &ProjectOptions,
) -> Vec<Choice> {
    let person = |user: &User, selected: bool| Choice {
        id: user.id.clone(),
        name: user.name.clone(),
        mark: Mark::Person(Some(user.clone())),
        selected,
    };
    match field {
        Field::Status => options
            .statuses
            .iter()
            .map(|status| Choice {
                id: status.id.clone(),
                name: status.name.clone(),
                mark: Mark::Status(status.clone()),
                selected: status.id == project.status.id,
            })
            .collect(),
        Field::Lead => {
            let lead = project.lead.as_ref().map(|lead| lead.id.as_str());
            std::iter::once(Choice {
                id: String::new(),
                name: "No lead".into(),
                mark: Mark::Person(None),
                selected: lead.is_none(),
            })
            .chain(
                options
                    .users
                    .iter()
                    .map(|user| person(user, lead == Some(user.id.as_str()))),
            )
            .collect()
        }
        Field::Members => options
            .users
            .iter()
            .map(|user| {
                let member = project.members.iter().any(|member| member.id == user.id);
                person(user, member)
            })
            .collect(),
        Field::Teams => options
            .teams
            .iter()
            .map(|team| Choice {
                id: team.id.clone(),
                name: team.name.clone(),
                mark: Mark::Team,
                selected: project.teams.iter().any(|on| on.id == team.id),
            })
            .collect(),
        Field::Labels => options
            .labels
            .iter()
            .map(|label| Choice {
                id: label.id.clone(),
                name: label.name.clone(),
                mark: Mark::Swatch(label.color.clone()),
                selected: project.labels.iter().any(|on| on.id == label.id),
            })
            .collect(),
    }
}

/// `ids` with `id` added, or taken out when it is there.
fn flipped(ids: Vec<String>, id: &str) -> Vec<String> {
    if ids.iter().any(|known| known == id) {
        ids.into_iter().filter(|known| known != id).collect()
    } else {
        let mut ids = ids;
        ids.push(id.to_owned());
        ids
    }
}

/// What picking `choice` in `field`'s picker asks Linear for. `None` when it
/// changes nothing, or would take the project's last team away: Linear
/// keeps every project in at least one team.
pub(super) fn change_for(
    field: Field,
    choice: &Choice,
    project: &ProjectDetail,
) -> Option<ProjectChange> {
    let ids = |items: Vec<&str>| items.into_iter().map(str::to_owned).collect::<Vec<_>>();
    match field {
        Field::Status if choice.selected => None,
        Field::Status => Some(ProjectChange {
            status_id: Some(choice.id.clone()),
            ..ProjectChange::default()
        }),
        Field::Lead if choice.selected => None,
        Field::Lead => Some(ProjectChange {
            lead_id: Some((!choice.id.is_empty()).then(|| choice.id.clone())),
            ..ProjectChange::default()
        }),
        Field::Members => {
            let current = ids(project
                .members
                .iter()
                .map(|user| user.id.as_str())
                .collect());
            Some(ProjectChange {
                member_ids: Some(flipped(current, &choice.id)),
                ..ProjectChange::default()
            })
        }
        Field::Teams => {
            let current = ids(project.teams.iter().map(|team| team.id.as_str()).collect());
            let teams = flipped(current, &choice.id);
            (!teams.is_empty()).then(|| ProjectChange {
                team_ids: Some(teams),
                ..ProjectChange::default()
            })
        }
        Field::Labels => {
            let current = ids(project
                .labels
                .iter()
                .map(|label| label.id.as_str())
                .collect());
            Some(ProjectChange {
                label_ids: Some(flipped(current, &choice.id)),
                ..ProjectChange::default()
            })
        }
    }
}

/// The items `ids` names, in that order, from what the project has and the
/// workspace's catalog. An id found in neither is dropped.
fn pick_by_id<T: Clone>(
    ids: &[String],
    known: &[T],
    catalog: &[T],
    id: impl Fn(&T) -> &str,
) -> Vec<T> {
    ids.iter()
        .filter_map(|wanted| {
            known
                .iter()
                .chain(catalog)
                .find(|item| id(item) == wanted)
                .cloned()
        })
        .collect()
}

/// `change` applied to `project` as the page shows it until Linear answers.
pub(super) fn apply(project: &mut ProjectDetail, change: &ProjectChange, options: &ProjectOptions) {
    if let Some(status) = change
        .status_id
        .as_ref()
        .and_then(|id| options.statuses.iter().find(|status| &status.id == id))
    {
        project.status = status.clone();
    }
    if let Some(lead) = &change.lead_id {
        project.lead = lead
            .as_ref()
            .and_then(|id| options.users.iter().find(|user| &user.id == id).cloned());
    }
    if let Some(ids) = &change.member_ids {
        project.members = pick_by_id(ids, &project.members, &options.users, |user: &User| {
            &user.id
        });
    }
    if let Some(ids) = &change.team_ids {
        project.teams = pick_by_id(ids, &project.teams, &options.teams, |team: &TeamRef| {
            &team.id
        });
    }
    if let Some(ids) = &change.label_ids {
        project.labels = pick_by_id(ids, &project.labels, &options.labels, |label: &Label| {
            &label.id
        });
    }
    if let Some(start) = change.start_date {
        project.start_date = start;
    }
    if let Some(target) = change.target_date {
        project.target_date = target;
    }
}

/// The cursor of a picker after a key: Down and J go one row down, Up and K
/// one up, both wrapping. Any other key leaves it.
pub(super) fn step_cursor(cursor: usize, count: usize, key: &str) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match key {
        "down" | "j" => Some((cursor + 1) % count),
        "up" | "k" => Some((cursor + count - 1) % count),
        _ => None,
    }
}

/// A share from 0 to 1 as the whole percent Linear shows.
pub(super) fn percent(progress: f32) -> u32 {
    (progress.clamp(0., 1.) * 100.).round() as u32
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use loupe_linear::{
        Label, ProjectChange, ProjectDetail, ProjectOptions, ProjectStatus, ProjectStatusKind,
        TeamRef, User,
    };

    use super::{Field, apply, change_for, choices, percent};

    fn user(id: &str) -> User {
        User {
            id: id.into(),
            name: format!("User {id}"),
            display_name: id.into(),
            avatar_url: None,
        }
    }

    fn status(id: &str, kind: ProjectStatusKind) -> ProjectStatus {
        ProjectStatus {
            id: id.into(),
            name: id.into(),
            kind,
            color: "#bec2c8".into(),
        }
    }

    fn team(id: &str) -> TeamRef {
        TeamRef {
            id: id.into(),
            key: id.to_uppercase(),
            name: id.into(),
        }
    }

    fn label(id: &str) -> Label {
        Label {
            id: id.into(),
            name: id.into(),
            color: "#5e6ad2".into(),
            parent_id: None,
        }
    }

    fn options() -> ProjectOptions {
        ProjectOptions {
            statuses: vec![
                status("backlog", ProjectStatusKind::Backlog),
                status("started", ProjectStatusKind::Started),
            ],
            labels: vec![label("vendor-a"), label("identity-check")],
            users: vec![user("u1"), user("u2"), user("u3")],
            teams: vec![team("back"), team("front")],
        }
    }

    fn project() -> ProjectDetail {
        ProjectDetail {
            id: "p1".into(),
            name: "Integrations".into(),
            icon: None,
            color: "#4cb782".into(),
            summary: String::new(),
            content: None,
            status: status("started", ProjectStatusKind::Started),
            progress: 0.375,
            lead: Some(user("u2")),
            start_date: None,
            target_date: NaiveDate::from_ymd_opt(2026, 11, 20),
            url: "https://linear.app/acme/project/p1".into(),
            lead_team_id: "back".into(),
            teams: vec![team("back")],
            members: vec![user("u1"), user("u2")],
            labels: vec![label("vendor-a")],
            milestones: Vec::new(),
            updates: Vec::new(),
            resources: Vec::new(),
        }
    }

    fn pick(field: Field, id: &str) -> Option<ProjectChange> {
        let project = project();
        let rows = choices(field, &project, &options());
        let choice = rows.iter().find(|choice| choice.id == id).expect("a row");
        change_for(field, choice, &project)
    }

    #[test]
    fn each_picker_marks_what_the_project_has() {
        let project = project();
        let selected = |field| -> Vec<String> {
            choices(field, &project, &options())
                .into_iter()
                .filter(|choice| choice.selected)
                .map(|choice| choice.id)
                .collect()
        };
        assert_eq!(selected(Field::Status), ["started"]);
        assert_eq!(selected(Field::Lead), ["u2"]);
        assert_eq!(selected(Field::Members), ["u1", "u2"]);
        assert_eq!(selected(Field::Teams), ["back"]);
        assert_eq!(selected(Field::Labels), ["vendor-a"]);
        // "No lead" comes first and is how the lead is cleared.
        let lead = choices(Field::Lead, &project, &options());
        assert_eq!(lead[0].name, "No lead");
    }

    #[test]
    fn a_single_value_is_sent_only_when_it_changes() {
        assert_eq!(pick(Field::Status, "started"), None);
        assert_eq!(
            pick(Field::Status, "backlog"),
            Some(ProjectChange {
                status_id: Some("backlog".into()),
                ..ProjectChange::default()
            })
        );
        assert_eq!(pick(Field::Lead, "u2"), None);
        assert_eq!(
            pick(Field::Lead, "").and_then(|change| change.lead_id),
            Some(None)
        );
    }

    #[test]
    fn a_set_flips_the_picked_row_and_keeps_the_rest() {
        assert_eq!(
            pick(Field::Members, "u3").and_then(|change| change.member_ids),
            Some(vec!["u1".into(), "u2".into(), "u3".into()])
        );
        assert_eq!(
            pick(Field::Members, "u1").and_then(|change| change.member_ids),
            Some(vec!["u2".into()])
        );
        assert_eq!(
            pick(Field::Labels, "identity-check").and_then(|change| change.label_ids),
            Some(vec!["vendor-a".into(), "identity-check".into()])
        );
        assert_eq!(
            pick(Field::Teams, "front").and_then(|change| change.team_ids),
            Some(vec!["back".into(), "front".into()])
        );
    }

    #[test]
    fn the_last_team_cannot_be_taken_away() {
        assert_eq!(pick(Field::Teams, "back"), None);
    }

    #[test]
    fn a_change_shows_at_once_with_the_catalogs_values() {
        let mut shown = project();
        let change = ProjectChange {
            status_id: Some("backlog".into()),
            lead_id: Some(Some("u3".into())),
            member_ids: Some(vec!["u2".into(), "u3".into()]),
            label_ids: Some(Vec::new()),
            start_date: Some(NaiveDate::from_ymd_opt(2026, 11, 6)),
            target_date: Some(None),
            ..ProjectChange::default()
        };
        apply(&mut shown, &change, &options());
        assert_eq!(shown.status.id, "backlog");
        assert_eq!(shown.lead.map(|lead| lead.id).as_deref(), Some("u3"));
        let members: Vec<_> = shown.members.iter().map(|user| user.id.as_str()).collect();
        assert_eq!(members, ["u2", "u3"]);
        assert!(shown.labels.is_empty());
        assert_eq!(shown.start_date, NaiveDate::from_ymd_opt(2026, 11, 6));
        assert_eq!(shown.target_date, None);
        // Fields the change leaves alone stay as they were.
        assert_eq!(shown.teams.len(), 1);
    }

    #[test]
    fn progress_is_the_whole_percent_linear_shows() {
        assert_eq!(percent(0.375), 38);
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(1.2), 100);
    }
}
