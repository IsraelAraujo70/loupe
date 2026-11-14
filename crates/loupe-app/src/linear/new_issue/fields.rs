//! What the new issue modal holds besides its text, and the rows of each of
//! its property menus. Pure: the view renders these and applies the picks.

use chrono::{Datelike as _, Duration, Months, NaiveDate};
use loupe_linear::{
    Label, NewIssue, Priority, StateRef, StateType, Team, TeamSummary, User, WorkflowState,
};

/// The properties of the issue being written. Ids refer to `team_id`'s
/// catalog.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Fields {
    pub team_id: String,
    pub state_id: Option<String>,
    pub priority: Priority,
    pub assignee_id: Option<String>,
    pub label_ids: Vec<String>,
    pub project_id: Option<String>,
    pub cycle_id: Option<String>,
    pub estimate: Option<u32>,
    pub due_date: Option<NaiveDate>,
}

impl Fields {
    /// A blank issue for `team`, in `state_id` when the team has it (a board
    /// column's "+"), else in its first Todo state.
    pub fn new(team: &Team, state_id: Option<String>) -> Self {
        Fields {
            team_id: team.id.clone(),
            state_id: state_id
                .filter(|id| team.states.iter().any(|state| &state.id == id))
                .or_else(|| default_state(&team.states).map(|state| state.id.clone())),
            priority: Priority::None,
            assignee_id: None,
            label_ids: Vec::new(),
            project_id: None,
            cycle_id: None,
            estimate: None,
            due_date: None,
        }
    }

    /// Moves the issue to `team`: what the team also has stays (its members,
    /// workspace labels, shared projects, an estimate on its scale), the
    /// rest is cleared, and the state becomes the team's default, since
    /// states belong to one team.
    pub fn retarget(&mut self, team: &Team) {
        if self.team_id == team.id {
            return;
        }
        fn keep<'a>(
            id: &Option<String>,
            mut ids: impl Iterator<Item = &'a String>,
        ) -> Option<String> {
            id.as_ref()
                .filter(|id| ids.any(|other| other == *id))
                .cloned()
        }
        self.team_id = team.id.clone();
        self.state_id = default_state(&team.states).map(|state| state.id.clone());
        self.assignee_id = keep(&self.assignee_id, team.members.iter().map(|m| &m.id));
        self.project_id = keep(&self.project_id, team.projects.iter().map(|p| &p.id));
        self.cycle_id = keep(&self.cycle_id, team.cycles.iter().map(|c| &c.id));
        self.label_ids
            .retain(|id| team.labels.iter().any(|label| &label.id == id));
        self.estimate = self
            .estimate
            .filter(|points| team.estimation.points().contains(points));
    }

    /// Applies a picked row. Labels are a set: picking one flips it, and a
    /// label in a group replaces the group's other label, as Linear allows
    /// one label per group.
    pub fn apply(&mut self, pick: &Pick, team: &Team) {
        match pick {
            // The view loads the team's catalog and calls `retarget`.
            Pick::Team(_) => {}
            Pick::State(id) => self.state_id = Some(id.clone()),
            Pick::Priority(priority) => self.priority = *priority,
            Pick::Assignee(id) => self.assignee_id = id.clone(),
            Pick::Label(id) => {
                if let Some(at) = self.label_ids.iter().position(|on| on == id) {
                    self.label_ids.remove(at);
                    return;
                }
                let group = |id: &str| {
                    team.labels
                        .iter()
                        .find(|label| label.id == id)
                        .and_then(|label| label.parent_id.clone())
                };
                if let Some(parent) = group(id) {
                    self.label_ids
                        .retain(|on| group(on).as_ref() != Some(&parent));
                }
                self.label_ids.push(id.clone());
            }
            Pick::Project(id) => self.project_id = id.clone(),
            Pick::Cycle(id) => self.cycle_id = id.clone(),
            Pick::Estimate(points) => self.estimate = *points,
            Pick::DueDate(date) => self.due_date = *date,
        }
    }

    /// The create request, or `None` without a title. Blank descriptions and
    /// "No priority" are left out so Linear applies the team's defaults.
    pub fn request(&self, title: &str, description: &str) -> Option<NewIssue> {
        let title = title.trim();
        if title.is_empty() {
            return None;
        }
        let description = description.trim();
        Some(NewIssue {
            team_id: self.team_id.clone(),
            title: title.to_owned(),
            description: (!description.is_empty()).then(|| description.to_owned()),
            state_id: self.state_id.clone(),
            priority: (self.priority != Priority::None).then_some(self.priority),
            assignee_id: self.assignee_id.clone(),
            label_ids: self.label_ids.clone(),
            project_id: self.project_id.clone(),
            cycle_id: self.cycle_id.clone(),
            estimate: self.estimate,
            due_date: self.due_date,
        })
    }
}

/// The state a new issue starts in: the team's first "todo" state, else its
/// first state.
pub(super) fn default_state(states: &[WorkflowState]) -> Option<&WorkflowState> {
    states
        .iter()
        .find(|state| state.kind == StateType::Unstarted)
        .or_else(|| states.first())
}

pub(super) fn state_ref(state: &WorkflowState) -> StateRef {
    StateRef {
        id: state.id.clone(),
        name: state.name.clone(),
        kind: state.kind,
        color: state.color.clone(),
    }
}

/// A property of the modal's breadcrumb or its row of chips, in the order
/// the row shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Property {
    Team,
    State,
    Priority,
    Assignee,
    Labels,
    Project,
    Cycle,
    Estimate,
    DueDate,
}

impl Property {
    /// The chips of the row, after the breadcrumb's team.
    pub const CHIPS: [Property; 8] = [
        Property::State,
        Property::Priority,
        Property::Assignee,
        Property::Labels,
        Property::Project,
        Property::Cycle,
        Property::Estimate,
        Property::DueDate,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Property::Team => "Team",
            Property::State => "Status",
            Property::Priority => "Priority",
            Property::Assignee => "Assignee",
            Property::Labels => "Labels",
            Property::Project => "Project",
            Property::Cycle => "Cycle",
            Property::Estimate => "Estimate",
            Property::DueDate => "Due date",
        }
    }

    /// The key that opens the menu while no text field has the focus:
    /// Linear's own for the properties it binds.
    pub fn key(self) -> &'static str {
        match self {
            Property::Team => "T",
            Property::State => "S",
            Property::Priority => "P",
            Property::Assignee => "A",
            Property::Labels => "L",
            Property::Project => "Shift P",
            Property::Cycle => "Shift C",
            Property::Estimate => "Shift E",
            Property::DueDate => "Shift D",
        }
    }

    /// What the menu's search field asks for.
    pub fn placeholder(self) -> &'static str {
        match self {
            Property::Team => "Move to team\u{2026}",
            Property::State => "Change status\u{2026}",
            Property::Priority => "Set priority\u{2026}",
            Property::Assignee => "Assign to\u{2026}",
            Property::Labels => "Add labels\u{2026}",
            Property::Project => "Add to project\u{2026}",
            Property::Cycle => "Add to cycle\u{2026}",
            Property::Estimate => "Set estimate\u{2026}",
            Property::DueDate => "Due date, or type YYYY-MM-DD\u{2026}",
        }
    }

    /// Picking a row keeps the menu open: labels are a set.
    pub fn keeps_open(self) -> bool {
        self == Property::Labels
    }

    /// Whether `team` has anything to choose for this chip. A team without
    /// estimates or cycles shows no chip for them.
    pub fn applies_to(self, team: &Team) -> bool {
        match self {
            Property::Cycle => !team.cycles.is_empty(),
            Property::Estimate => !team.estimation.points().is_empty(),
            _ => true,
        }
    }
}

/// What a row shows before its name.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mark {
    Team,
    State(StateRef),
    Priority(Priority),
    Person(Option<User>),
    /// A label's or a project's color, as Linear's hex string.
    Swatch(Option<String>),
    Project,
    Cycle,
    Estimate,
    Date,
    /// "No project", "No cycle": the property's own icon, faint.
    None(Property),
}

/// What picking a row sets.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Pick {
    Team(String),
    State(String),
    Priority(Priority),
    Assignee(Option<String>),
    Label(String),
    Project(Option<String>),
    Cycle(Option<String>),
    Estimate(Option<u32>),
    DueDate(Option<NaiveDate>),
}

/// One row of a property menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Choice {
    /// Stable within the menu, for the row's element id.
    pub id: String,
    pub name: String,
    /// Faint text after the name: a team's key, a label's group, a date.
    pub detail: Option<String>,
    pub mark: Mark,
    pub selected: bool,
    pub pick: Pick,
}

impl Choice {
    fn new(id: String, name: String, mark: Mark, selected: bool, pick: Pick) -> Self {
        Choice {
            id,
            name,
            detail: None,
            mark,
            selected,
            pick,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.name.to_lowercase().contains(&query)
            || self
                .detail
                .as_ref()
                .is_some_and(|detail| detail.to_lowercase().contains(&query))
    }
}

pub(super) fn cycle_name(number: u32, name: Option<&str>) -> String {
    name.map_or_else(|| format!("Cycle {number}"), str::to_owned)
}

pub(super) fn date_text(date: NaiveDate, today: NaiveDate) -> String {
    if date.year() == today.year() {
        date.format("%b %-d").to_string()
    } else {
        date.format("%b %-d, %Y").to_string()
    }
}

/// A date typed in the due date menu: `2026-12-31` or `12/31/2026`.
fn parse_date(query: &str) -> Option<NaiveDate> {
    let query = query.trim();
    NaiveDate::parse_from_str(query, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(query, "%m/%d/%Y"))
        .ok()
}

/// The rows of `property`'s menu for the issue as `fields` holds it, those
/// whose name or detail contains `query`. `teams` are the teams the user can
/// move the issue to; `today` dates the due date presets.
pub(super) fn choices(
    property: Property,
    team: &Team,
    teams: &[TeamSummary],
    fields: &Fields,
    query: &str,
    today: NaiveDate,
) -> Vec<Choice> {
    let all = match property {
        Property::Team => teams
            .iter()
            .map(|other| {
                Choice::new(
                    format!("team-{}", other.id),
                    other.name.clone(),
                    Mark::Team,
                    other.id == fields.team_id,
                    Pick::Team(other.id.clone()),
                )
                .detail(other.key.clone())
            })
            .collect(),
        Property::State => team
            .states
            .iter()
            .map(|state| {
                Choice::new(
                    format!("state-{}", state.id),
                    state.name.clone(),
                    Mark::State(state_ref(state)),
                    fields.state_id.as_ref() == Some(&state.id),
                    Pick::State(state.id.clone()),
                )
            })
            .collect(),
        Property::Priority => Priority::ALL
            .into_iter()
            .map(|priority| {
                Choice::new(
                    format!("priority-{}", priority.to_wire()),
                    priority.label().to_owned(),
                    Mark::Priority(priority),
                    priority == fields.priority,
                    Pick::Priority(priority),
                )
            })
            .collect(),
        Property::Assignee => std::iter::once(Choice::new(
            "assignee-none".into(),
            "No assignee".into(),
            Mark::Person(None),
            fields.assignee_id.is_none(),
            Pick::Assignee(None),
        ))
        .chain(team.members.iter().map(|member| {
            Choice::new(
                format!("assignee-{}", member.id),
                member.name.clone(),
                Mark::Person(Some(member.clone())),
                fields.assignee_id.as_ref() == Some(&member.id),
                Pick::Assignee(Some(member.id.clone())),
            )
        }))
        .collect(),
        Property::Labels => label_choices(team, &fields.label_ids),
        Property::Project => std::iter::once(Choice::new(
            "project-none".into(),
            "No project".into(),
            Mark::None(Property::Project),
            fields.project_id.is_none(),
            Pick::Project(None),
        ))
        .chain(team.projects.iter().map(|project| {
            Choice::new(
                format!("project-{}", project.id),
                project.name.clone(),
                match &project.color {
                    Some(color) => Mark::Swatch(Some(color.clone())),
                    None => Mark::Project,
                },
                fields.project_id.as_ref() == Some(&project.id),
                Pick::Project(Some(project.id.clone())),
            )
        }))
        .collect(),
        Property::Cycle => std::iter::once(Choice::new(
            "cycle-none".into(),
            "No cycle".into(),
            Mark::None(Property::Cycle),
            fields.cycle_id.is_none(),
            Pick::Cycle(None),
        ))
        .chain(team.cycles.iter().map(|cycle| {
            Choice::new(
                format!("cycle-{}", cycle.id),
                cycle_name(cycle.number, cycle.name.as_deref()),
                Mark::Cycle,
                fields.cycle_id.as_ref() == Some(&cycle.id),
                Pick::Cycle(Some(cycle.id.clone())),
            )
            .detail(format!(
                "{} \u{2013} {}",
                date_text(cycle.starts_at.date_naive(), today),
                date_text(cycle.ends_at.date_naive(), today)
            ))
        }))
        .collect(),
        Property::Estimate => std::iter::once(Choice::new(
            "estimate-none".into(),
            "No estimate".into(),
            Mark::None(Property::Estimate),
            fields.estimate.is_none(),
            Pick::Estimate(None),
        ))
        .chain(team.estimation.points().into_iter().map(|points| {
            Choice::new(
                format!("estimate-{points}"),
                team.estimation.label(points),
                Mark::Estimate,
                fields.estimate == Some(points),
                Pick::Estimate(Some(points)),
            )
        }))
        .collect(),
        Property::DueDate => due_date_choices(fields.due_date, query, today),
    };
    all.into_iter()
        .filter(|choice| property == Property::DueDate || choice.matches(query))
        .collect()
}

/// A team's labels a new issue can take: every label but the groups (those
/// Linear marks as groups, and those with labels under them), named with
/// their group.
fn label_choices(team: &Team, on: &[String]) -> Vec<Choice> {
    let labels = &team.labels;
    let is_group = |label: &Label| {
        team.label_groups.contains(&label.id)
            || labels
                .iter()
                .any(|other| other.parent_id.as_ref() == Some(&label.id))
    };
    labels
        .iter()
        .filter(|label| !is_group(label))
        .map(|label| {
            let choice = Choice::new(
                format!("label-{}", label.id),
                label.name.clone(),
                Mark::Swatch(Some(label.color.clone())),
                on.contains(&label.id),
                Pick::Label(label.id.clone()),
            );
            let group = label
                .parent_id
                .as_ref()
                .and_then(|parent| labels.iter().find(|other| &other.id == parent));
            match group {
                Some(group) => choice.detail(group.name.clone()),
                None => choice,
            }
        })
        .collect()
}

/// The due date menu: a date typed in full first, then the presets whose
/// name matches, then "No due date" while one is set.
fn due_date_choices(current: Option<NaiveDate>, query: &str, today: NaiveDate) -> Vec<Choice> {
    let at = |date: NaiveDate, name: String| {
        Choice::new(
            format!("due-{date}"),
            name,
            Mark::Date,
            current == Some(date),
            Pick::DueDate(Some(date)),
        )
        .detail(date_text(date, today))
    };
    let typed = parse_date(query).map(|date| {
        let mut choice = at(date, date_text(date, today));
        choice.detail = None;
        choice.id = "due-typed".into();
        choice
    });
    let presets = [
        ("Today", today),
        ("Tomorrow", today + Duration::days(1)),
        ("In one week", today + Duration::weeks(1)),
        ("In two weeks", today + Duration::weeks(2)),
        (
            "In one month",
            today
                .checked_add_months(Months::new(1))
                .unwrap_or(today + Duration::days(30)),
        ),
    ];
    let typed_a_date = typed.is_some();
    let presets = presets
        .into_iter()
        .map(|(name, date)| at(date, name.to_owned()))
        .filter(|choice| typed_a_date || choice.matches(query));
    let clear = current.map(|_| {
        Choice::new(
            "due-none".into(),
            "No due date".into(),
            Mark::None(Property::DueDate),
            false,
            Pick::DueDate(None),
        )
    });
    typed
        .into_iter()
        .chain(presets)
        .chain(clear.filter(|choice| choice.matches(query)))
        .collect()
}

/// The row a menu's cursor starts on: the current value, else the first.
pub(super) fn first_cursor(choices: &[Choice]) -> usize {
    choices
        .iter()
        .position(|choice| choice.selected)
        .unwrap_or(0)
}

/// The cursor after Up or Down, wrapping. Any other key leaves it.
pub(super) fn step_cursor(cursor: usize, count: usize, key: &str) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match key {
        "down" => Some((cursor + 1) % count),
        "up" => Some((cursor + count - 1) % count),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate};
    use loupe_linear::{
        Cycle, EstimateScale, Estimation, Label, NewIssue, Priority, Project, StateType, Team,
        TeamSummary, User, WorkflowState,
    };

    use super::{Fields, Pick, Property, choices, default_state, first_cursor, step_cursor};

    fn state(id: &str, kind: StateType) -> WorkflowState {
        WorkflowState {
            id: id.into(),
            name: id.into(),
            kind,
            position: 0.0,
            color: "#999999".into(),
        }
    }

    fn user(id: &str, name: &str) -> User {
        User {
            id: id.into(),
            name: name.into(),
            display_name: name.to_lowercase(),
            avatar_url: None,
        }
    }

    fn label(id: &str, name: &str, parent: Option<&str>) -> Label {
        Label {
            id: id.into(),
            name: name.into(),
            color: "#ff0000".into(),
            parent_id: parent.map(Into::into),
        }
    }

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            color: None,
        }
    }

    fn team(id: &str) -> Team {
        Team {
            id: id.into(),
            key: id.to_uppercase(),
            name: id.into(),
            states: vec![
                state(&format!("{id}-backlog"), StateType::Backlog),
                state(&format!("{id}-todo"), StateType::Unstarted),
                state(&format!("{id}-done"), StateType::Completed),
            ],
            members: vec![user("u-ana", "Ana"), user("u-bia", "Bia")],
            labels: vec![
                label("l-bug", "Bug", None),
                label("l-area", "Area", None),
                label("l-api", "API", Some("l-area")),
                label("l-web", "Web", Some("l-area")),
                label("l-empty", "Empty group", None),
            ],
            projects: vec![project("p-atlas", "Atlas")],
            cycles: vec![Cycle {
                id: format!("{id}-c1"),
                number: 7,
                name: None,
                starts_at: DateTime::parse_from_rfc3339("2026-11-09T00:00:00Z")
                    .unwrap()
                    .to_utc(),
                ends_at: DateTime::parse_from_rfc3339("2026-11-23T00:00:00Z")
                    .unwrap()
                    .to_utc(),
            }],
            label_groups: vec!["l-area".into(), "l-empty".into()],
            estimation: Estimation {
                scale: EstimateScale::Fibonacci,
                allow_zero: false,
                extended: false,
            },
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 11).unwrap()
    }

    fn names(property: Property, team: &Team, fields: &Fields, query: &str) -> Vec<String> {
        choices(property, team, &[], fields, query, today())
            .into_iter()
            .map(|choice| choice.name)
            .collect()
    }

    #[test]
    fn new_issues_start_in_the_first_todo_state_or_the_columns() {
        let eng = team("eng");
        assert_eq!(
            Fields::new(&eng, None).state_id.as_deref(),
            Some("eng-todo")
        );
        assert_eq!(
            Fields::new(&eng, Some("eng-done".into()))
                .state_id
                .as_deref(),
            Some("eng-done")
        );
        // A state of another team is not this team's column.
        assert_eq!(
            Fields::new(&eng, Some("ops-done".into()))
                .state_id
                .as_deref(),
            Some("eng-todo")
        );
        let started = [state("started", StateType::Started)];
        assert_eq!(
            default_state(&started).map(|s| s.id.as_str()),
            Some("started")
        );
        assert!(default_state(&[]).is_none());
    }

    #[test]
    fn the_request_carries_every_property_that_was_set() {
        let eng = team("eng");
        let mut fields = Fields::new(&eng, None);
        assert_eq!(fields.request("   ", "body"), None);
        assert_eq!(
            fields.request(" Fix crash ", "  "),
            Some(NewIssue {
                team_id: "eng".into(),
                title: "Fix crash".into(),
                state_id: Some("eng-todo".into()),
                ..NewIssue::default()
            })
        );

        let due = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        for pick in [
            Pick::Priority(Priority::High),
            Pick::Assignee(Some("u-bia".into())),
            Pick::Label("l-bug".into()),
            Pick::Project(Some("p-atlas".into())),
            Pick::Cycle(Some("eng-c1".into())),
            Pick::Estimate(Some(5)),
            Pick::DueDate(Some(due)),
        ] {
            fields.apply(&pick, &eng);
        }
        assert_eq!(
            fields.request("Fix crash", "Steps"),
            Some(NewIssue {
                team_id: "eng".into(),
                title: "Fix crash".into(),
                description: Some("Steps".into()),
                state_id: Some("eng-todo".into()),
                priority: Some(Priority::High),
                assignee_id: Some("u-bia".into()),
                label_ids: vec!["l-bug".into()],
                project_id: Some("p-atlas".into()),
                cycle_id: Some("eng-c1".into()),
                estimate: Some(5),
                due_date: Some(due),
            })
        );
    }

    #[test]
    fn labels_toggle_and_a_group_holds_one() {
        let eng = team("eng");
        let mut fields = Fields::new(&eng, None);
        fields.apply(&Pick::Label("l-bug".into()), &eng);
        fields.apply(&Pick::Label("l-api".into()), &eng);
        assert_eq!(fields.label_ids, ["l-bug", "l-api"]);
        fields.apply(&Pick::Label("l-web".into()), &eng);
        assert_eq!(fields.label_ids, ["l-bug", "l-web"]);
        fields.apply(&Pick::Label("l-bug".into()), &eng);
        assert_eq!(fields.label_ids, ["l-web"]);
    }

    #[test]
    fn the_label_menu_hides_groups_and_names_the_group() {
        let eng = team("eng");
        let fields = Fields::new(&eng, None);
        let rows = choices(Property::Labels, &eng, &[], &fields, "", today());
        let shown: Vec<_> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.detail.as_deref()))
            .collect();
        assert_eq!(
            shown,
            [("Bug", None), ("API", Some("Area")), ("Web", Some("Area"))]
        );
        // The group's name finds its labels.
        assert_eq!(
            names(Property::Labels, &eng, &fields, "area"),
            ["API", "Web"]
        );
    }

    #[test]
    fn moving_to_another_team_keeps_only_what_it_has() {
        let eng = team("eng");
        let mut ops = team("ops");
        ops.members.retain(|member| member.id == "u-ana");
        ops.labels.retain(|label| label.id == "l-bug");
        ops.projects.clear();
        ops.estimation.scale = EstimateScale::Exponential;

        let mut fields = Fields::new(&eng, Some("eng-done".into()));
        for pick in [
            Pick::Assignee(Some("u-bia".into())),
            Pick::Label("l-bug".into()),
            Pick::Label("l-api".into()),
            Pick::Project(Some("p-atlas".into())),
            Pick::Cycle(Some("eng-c1".into())),
            Pick::Estimate(Some(2)),
            Pick::Priority(Priority::Urgent),
        ] {
            fields.apply(&pick, &eng);
        }
        fields.retarget(&ops);
        assert_eq!(fields.team_id, "ops");
        assert_eq!(fields.state_id.as_deref(), Some("ops-todo"));
        assert_eq!(fields.assignee_id, None);
        assert_eq!(fields.label_ids, ["l-bug"]);
        assert_eq!(fields.project_id, None);
        assert_eq!(fields.cycle_id, None);
        assert_eq!(
            fields.estimate,
            Some(2),
            "2 is on the exponential scale too"
        );
        assert_eq!(fields.priority, Priority::Urgent);

        fields.apply(&Pick::Estimate(Some(4)), &ops);
        fields.apply(&Pick::Assignee(Some("u-ana".into())), &ops);
        fields.retarget(&eng);
        assert_eq!(fields.estimate, None, "4 is not a Fibonacci point");
        assert_eq!(fields.assignee_id.as_deref(), Some("u-ana"));
    }

    #[test]
    fn typing_narrows_a_menu_and_the_cursor_starts_on_the_value() {
        let eng = team("eng");
        let mut fields = Fields::new(&eng, None);
        assert_eq!(
            names(Property::Assignee, &eng, &fields, ""),
            ["No assignee", "Ana", "Bia"]
        );
        assert_eq!(names(Property::Assignee, &eng, &fields, " BI"), ["Bia"]);
        fields.apply(&Pick::Assignee(Some("u-bia".into())), &eng);
        let rows = choices(Property::Assignee, &eng, &[], &fields, "", today());
        assert_eq!(first_cursor(&rows), 2);

        let teams = [
            TeamSummary {
                id: "eng".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            TeamSummary {
                id: "ops".into(),
                key: "OPS".into(),
                name: "Operations".into(),
            },
        ];
        let rows = choices(Property::Team, &eng, &teams, &fields, "ops", today());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pick, Pick::Team("ops".into()));
    }

    #[test]
    fn estimates_follow_the_teams_scale() {
        let mut eng = team("eng");
        let fields = Fields::new(&eng, None);
        assert_eq!(
            names(Property::Estimate, &eng, &fields, ""),
            [
                "No estimate",
                "1 point",
                "2 points",
                "3 points",
                "5 points",
                "8 points"
            ]
        );
        eng.estimation = Estimation {
            scale: EstimateScale::TShirt,
            allow_zero: true,
            extended: true,
        };
        assert_eq!(
            names(Property::Estimate, &eng, &fields, ""),
            [
                "No estimate",
                "0 points",
                "XS",
                "S",
                "M",
                "L",
                "XL",
                "XXL",
                "XXXL"
            ]
        );
        eng.estimation.scale = EstimateScale::NotUsed;
        assert!(!Property::Estimate.applies_to(&eng));
        eng.cycles.clear();
        assert!(!Property::Cycle.applies_to(&eng));
    }

    #[test]
    fn due_dates_come_from_presets_or_a_typed_date() {
        let eng = team("eng");
        let mut fields = Fields::new(&eng, None);
        let rows = choices(Property::DueDate, &eng, &[], &fields, "", today());
        let shown: Vec<_> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.detail.as_deref()))
            .collect();
        assert_eq!(
            shown,
            [
                ("Today", Some("Nov 11")),
                ("Tomorrow", Some("Nov 12")),
                ("In one week", Some("Nov 18")),
                ("In two weeks", Some("Nov 25")),
                ("In one month", Some("Dec 11")),
            ]
        );
        assert_eq!(names(Property::DueDate, &eng, &fields, "tom"), ["Tomorrow"]);

        let typed = choices(Property::DueDate, &eng, &[], &fields, "2027-01-15", today());
        assert_eq!(typed[0].name, "Jan 15, 2027");
        assert_eq!(
            typed[0].pick,
            Pick::DueDate(NaiveDate::from_ymd_opt(2027, 1, 15))
        );

        fields.apply(&typed[0].pick, &eng);
        let rows = names(Property::DueDate, &eng, &fields, "");
        assert_eq!(rows.last().map(String::as_str), Some("No due date"));
    }

    #[test]
    fn the_cursor_wraps_and_ignores_other_keys() {
        assert_eq!(step_cursor(0, 3, "up"), Some(2));
        assert_eq!(step_cursor(2, 3, "down"), Some(0));
        assert_eq!(step_cursor(1, 3, "j"), None);
        assert_eq!(step_cursor(0, 0, "down"), None);
    }
}
